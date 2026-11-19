// SPDX-License-Identifier: GPL-2.0-only
/* Private T8140 microphone bring-up. AFK transport follows the existing Asahi
 * AOP driver; Neo command layout is checked against native runtime captures.
 * ALSA capture is exposed by the companion neo_pcm.c RX0 driver. */
#include <linux/completion.h>
#include <linux/atomic.h>
#include <linux/debugfs.h>
#include <linux/fs.h>
#include <linux/dma-mapping.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/of_platform.h>
#include <linux/clocksource.h>
#include <clocksource/arm_arch_timer.h>
#include <linux/platform_device.h>
#include <linux/pm_domain.h>
#include <linux/slab.h>
#include <linux/sizes.h>
#include <linux/soc/apple/rtkit.h>
#include <linux/unaligned.h>
#include "neo_secure.h"
#include "neo_mic.h"

#define NEO_EP_FIRST 0x20
#define NEO_EP_COUNT 16
/* Software allocation ceiling; T8142 advertises a 0x88000-byte AFK buffer. */
#define NEO_AF_LIMIT SZ_1M
#define NEO_CALL_TIMEOUT msecs_to_jiffies(5000)
#define NEO_HPAI 0x68706169
#define NEO_AUDIO_UNSUPPORTED 0xe00002c7
#define NEO_AUDIO_NOT_FOUND 0xe00002f0

struct neo_ring {
	u32 offset, size, capacity, block;
	bool valid;
};

struct neo_ep {
	void *buffer;
	dma_addr_t iova;
	size_t size;
	struct neo_ring tx, rx;
};

struct neo_mic {
	struct device *dev;
	struct apple_rtkit *rtkit;
	struct neo_secure *secure;
	void __iomem *asc;
	struct neo_ep ep[NEO_EP_COUNT];
	struct completion announced, replied;
	struct mutex call_lock;
	struct mutex power_lock;
	struct generic_pm_domain speaker_domain;
	struct generic_pm_domain *domains[1];
	struct genpd_onecell_data domain_data;
	bool speaker_warm_held;
	u8 audio_ep;
	u32 audio_channel;
	u16 seq, pending_tag;
	atomic_t power_cookie;
	bool ready;
	u32 retcode;
	u8 reply[256];
	size_t reply_size;
	bool crashed;
	struct dentry *debugfs;
};

static int neo_send(struct neo_mic *mic, u8 ep, u64 value)
{
	return apple_rtkit_send_message(mic->rtkit, ep, value, NULL, false);
}

static u32 neo_rb_read(struct neo_ep *ep, u32 off)
{
	return le32_to_cpu(READ_ONCE(*(__le32 *)(ep->buffer + off)));
}

static void neo_rb_write(struct neo_ep *ep, u32 off, u32 value)
{
	WRITE_ONCE(*(__le32 *)(ep->buffer + off), cpu_to_le32(value));
}

static int neo_ring_init(struct neo_mic *mic, struct neo_ep *ep,
			struct neo_ring *ring, u64 value)
{
	u32 offset = ((value >> 32) & 0xffff) * 64;
	u32 size = ((value >> 16) & 0xffff) * 64;
	u32 capacity, block;

	if (!ep->buffer || size < 0x100 || offset > ep->size || size > ep->size - offset)
		return -EINVAL;
	dma_rmb();
	capacity = neo_rb_read(ep, offset);
	if (!capacity || capacity >= size || (size - capacity) % 3)
		return -EINVAL;
	block = (size - capacity) / 3;
	if (block < 64 || block % 64 || capacity % block)
		return -EINVAL;
	*ring = (struct neo_ring){offset, size, capacity, block, true};
	dev_info(mic->dev, "AFK ring offset=%x size=%x capacity=%x block=%x\n",
		 offset, size, capacity, block);
	return 0;
}

static void neo_packet(struct neo_mic *mic, u8 ep, const u8 *packet, size_t size)
{
	u8 category;
	u16 subtype, tag;
	const u8 *body;
	size_t length;

	/* QE channel/type (8 bytes), EPIC header and subheader (40 bytes). */
	if (size < 48)
		return;
	category = packet[29];
	subtype = get_unaligned_le16(packet + 30);
	tag = get_unaligned_le16(packet + 32);
	body = packet + 48;
	length = size - 48;
	if (category == 0 && subtype == 0xc0 && length >= 36) {
		dev_info(mic->dev, "Service ep=%x ch=%x name=%.*s\n", ep,
			 get_unaligned_le32(body + 32), 20, body);
		if (!memcmp(body, "aop-audio\0", 10)) {
			mic->audio_ep = ep;
			mic->audio_channel = get_unaligned_le32(body + 32);
			complete(&mic->announced);
		}
	} else if (category == 0x20 && ep == mic->audio_ep &&
		   tag == READ_ONCE(mic->pending_tag) && length >= 4) {
		mic->retcode = get_unaligned_le32(body);
		mic->reply_size = min(length - 4, sizeof(mic->reply));
		memcpy(mic->reply, body + 4, mic->reply_size);
		complete(&mic->replied);
	}
}

static int neo_drain(struct neo_mic *mic, u8 number, struct neo_ep *ep)
{
	struct neo_ring *ring = &ep->rx;
	u32 rptr, wptr, offset, magic, length;
	unsigned int count;

	if (!ring->valid)
		return -ENXIO;
	dma_rmb();
	rptr = neo_rb_read(ep, ring->offset + ring->block);
	wptr = neo_rb_read(ep, ring->offset + 2 * ring->block);
	for (count = 0; rptr != wptr && count <= ring->capacity / ring->block; count++) {
		if (rptr >= ring->capacity || wptr >= ring->capacity ||
		    rptr + 16 > ring->capacity)
			return -EINVAL;
		offset = ring->offset + 3 * ring->block + rptr;
		magic = neo_rb_read(ep, offset);
		length = neo_rb_read(ep, offset + 4);
		if ((magic != 0x20504f49 && magic != 0x20504f41) || length > ring->capacity - 16)
			return -EINVAL;
		if (length > ring->capacity - rptr - 16) {
			rptr = 0;
			offset = ring->offset + 3 * ring->block;
			magic = neo_rb_read(ep, offset);
			length = neo_rb_read(ep, offset + 4);
			if ((magic != 0x20504f49 && magic != 0x20504f41) || length > ring->capacity - 16)
				return -EINVAL;
		}
		neo_packet(mic, number, ep->buffer + offset + 8, length + 8);
		rptr = ALIGN(rptr + 16 + length, ring->block) % ring->capacity;
		dma_wmb();
		neo_rb_write(ep, ring->offset + ring->block, rptr);
		dma_rmb();
		wptr = neo_rb_read(ep, ring->offset + 2 * ring->block);
	}
	return rptr == wptr ? 0 : -EOVERFLOW;
}

static void neo_recv(void *cookie, u8 number, u64 value)
{
	struct neo_mic *mic = cookie;
	struct neo_ep *ep;
	u16 opcode = value >> 48;
	int error = 0;

	if (number < NEO_EP_FIRST || number >= NEO_EP_FIRST + NEO_EP_COUNT)
		return;
	ep = &mic->ep[number - NEO_EP_FIRST];
	switch (opcode) {
	case 0x80:
		error = neo_send(mic, number, 0xa0ULL << 48);
		break;
	case 0x89:
		if (ep->buffer) { error = -EBUSY; break; }
		ep->size = ((value >> 16) & 0xffff) * 64;
		if (!ep->size || ep->size > NEO_AF_LIMIT) { error = -EINVAL; break; }
		ep->buffer = dma_alloc_coherent(mic->dev, ep->size, &ep->iova, GFP_KERNEL);
		if (!ep->buffer) { error = -ENOMEM; break; }
		dev_info(mic->dev, "AFK allocation ep=%x size=%zx iova=%pad\n", number, ep->size, &ep->iova);
		error = neo_send(mic, number, (0xa1ULL << 48) | ep->iova);
		break;
	case 0x8a:
	case 0x8b:
		error = neo_ring_init(mic, ep, opcode == 0x8a ? &ep->tx : &ep->rx, value);
		if (!error && ep->tx.valid && ep->rx.valid)
			error = neo_send(mic, number, 0xa3ULL << 48);
		break;
	case 0x85:
		error = neo_drain(mic, number, ep);
		break;
	case 0x86:
	case 0x8c:
	case 0xc1:
		break;
	default:
		dev_warn(mic->dev, "Unknown AFK ep=%x message=%llx\n", number, value);
	}
	if (error)
		dev_err(mic->dev, "AFK ep=%x opcode=%x error=%d\n", number, opcode, error);
}

static void neo_crashed(void *cookie, const void *log, size_t length)
{
	struct neo_mic *mic = cookie;
	mic->crashed = true;
	mic->retcode = ~0U;
	complete_all(&mic->announced);
	complete_all(&mic->replied);
	dev_err(mic->dev, "AOP firmware crashed (%zu-byte log)\n", length);
}

static const struct apple_rtkit_ops neo_rtkit_ops = {
	.quirks = APPLE_RTKIT_QUIRK_OSLOG_NO_SIZE,
	.recv_message = neo_recv,
	.crashed = neo_crashed,
};

static void neo_halt(void *cookie)
{
	struct neo_mic *mic = cookie;
	/* Stop the coprocessor before RTKit drains its worker and before the
	 * coherent rings are released. */
	writel(readl(mic->asc + 0x44) & ~BIT(4), mic->asc + 0x44);
}

static void neo_free_rings(void *cookie)
{
	struct neo_mic *mic = cookie;
	int i;
	for (i = 0; i < NEO_EP_COUNT; i++)
		if (mic->ep[i].buffer)
			dma_free_coherent(mic->dev, mic->ep[i].size,
					  mic->ep[i].buffer, mic->ep[i].iova);
}

static int neo_call_full(struct neo_mic *mic, const u8 *body, size_t length,
			u32 inline_size, u8 *reply, size_t *reply_size, u32 *status)
{
	struct neo_ep *ep = &mic->ep[mic->audio_ep - NEO_EP_FIRST];
	struct neo_ring *ring = &ep->tx;
	u8 packet[256] = {0};
	u32 rptr, wptr, next, needed;
	int error;

	if (!ring->valid || length > sizeof(packet) - 56 || mic->crashed)
		return -EINVAL;
	mutex_lock(&mic->call_lock);
	put_unaligned_le32(0x20504f49, packet);
	put_unaligned_le32(40 + length, packet + 4);
	put_unaligned_le32(mic->audio_channel, packet + 8);
	packet[16] = 2;
	put_unaligned_le16(mic->seq++, packet + 17);
	put_unaligned_le32(length, packet + 32);
	packet[36] = 2;
	packet[37] = 0x10;
	put_unaligned_le16(0x20, packet + 38);
	mic->pending_tag = 1;
	put_unaligned_le16(mic->pending_tag, packet + 40);
	put_unaligned_le32(inline_size, packet + 52);
	memcpy(packet + 56, body, length);
	dma_rmb();
	rptr = neo_rb_read(ep, ring->offset + ring->block);
	wptr = neo_rb_read(ep, ring->offset + 2 * ring->block);
	needed = ALIGN(56 + length, ring->block);
	if (rptr >= ring->capacity || wptr >= ring->capacity || needed >= ring->capacity) {
		error = -EINVAL; goto out;
	}
	if (56 + length > ring->capacity - wptr) {
		/* AFK leaves a wrap header at the old cursor, then writes at zero. */
		if (rptr <= needed || ring->capacity - wptr < 16) { error = -ENOSPC; goto out; }
		memcpy(ep->buffer + ring->offset + 3 * ring->block + wptr, packet, 16);
		wptr = 0;
	} else if (wptr < rptr && needed >= rptr - wptr) {
		error = -ENOSPC; goto out;
	}
	next = (wptr + needed) % ring->capacity;
	if (next == rptr) { error = -ENOSPC; goto out; }
	reinit_completion(&mic->replied);
	mic->retcode = ~0U;
	mic->reply_size = 0;
	memcpy(ep->buffer + ring->offset + 3 * ring->block + wptr, packet, 56 + length);
	dma_wmb();
	neo_rb_write(ep, ring->offset + 2 * ring->block, next);
	error = neo_send(mic, mic->audio_ep, (0xa2ULL << 48) | next);
	if (!error && !wait_for_completion_timeout(&mic->replied, NEO_CALL_TIMEOUT))
		error = -ETIMEDOUT;
	if (!error && mic->retcode) {
		if (mic->retcode == NEO_AUDIO_UNSUPPORTED)
			error = -EOPNOTSUPP;
		else if (mic->retcode == NEO_AUDIO_NOT_FOUND)
			error = -ENOENT;
		else
			error = -EIO;
		dev_info(mic->dev, "Audio call returned %08x\n", mic->retcode);
	}
	if (status)
		*status = mic->retcode;
	if (reply && reply_size) {
		*reply_size = min(*reply_size, mic->reply_size);
		memcpy(reply, mic->reply, *reply_size);
	}
out:
	mutex_unlock(&mic->call_lock);
	return error;
}

static int neo_call(struct neo_mic *mic, const u8 *body, size_t length, u32 inline_size)
{
	return neo_call_full(mic, body, length, inline_size, NULL, NULL, NULL);
}

/* Development-only, read-only firmware properties. Each open owns one
 * transaction and a bounded reply. This is not a stable userspace ABI.
 */
static ssize_t neo_query_write(struct file *file, const char __user *input,
			       size_t count, loff_t *position)
{
	struct neo_mic *mic = file_inode(file)->i_private;
	u8 body[52] = {0}, reply[256];
	size_t reply_size = sizeof(reply);
	u32 device, property, inline_size, status = ~0U;
	char *transaction;
	int error;

	if (!READ_ONCE(mic->ready) || READ_ONCE(mic->crashed))
		return -ENODEV;
	if (!count || count >= 64)
		return -EINVAL;
	transaction = simple_transaction_get(file, input, count);
	if (IS_ERR(transaction))
		return PTR_ERR(transaction);
	if (sscanf(transaction, "%x %u %u", &device, &property, &inline_size) != 3 ||
	    inline_size > sizeof(reply))
		return -EINVAL;
	put_unaligned_le32(~0U, body + 4);
	put_unaligned_le32(0xc3000004, body + 8);
	put_unaligned_le32(0x30, body + 32);
	put_unaligned_le32(device, body + 40);
	put_unaligned_le32(property, body + 44);
	put_unaligned_le32(1, body + 48);
	error = neo_call_full(mic, body, sizeof(body), inline_size,
			      reply, &reply_size, &status);
	if (error && status == ~0U)
		return error;
	put_unaligned_le32(status, transaction);
	put_unaligned_le32(reply_size, transaction + 4);
	memcpy(transaction + 8, reply, reply_size);
	simple_transaction_set(file, 8 + reply_size);
	return count;
}

static const struct file_operations neo_query_fops = {
	.owner = THIS_MODULE,
	.write = neo_query_write,
	.read = simple_transaction_read,
	.release = simple_transaction_release,
};

static void neo_remove_debugfs(void *cookie)
{
	struct neo_mic *mic = cookie;
	debugfs_remove_recursive(mic->debugfs);
}

static int neo_query_boot_status(struct neo_mic *mic)
{
	u8 body[52] = {0};
	int error;

	put_unaligned_le32(~0U, body + 4);
	put_unaligned_le32(0xc3000004, body + 8);
	put_unaligned_le32(0x30, body + 32);
	put_unaligned_le32(NEO_HPAI, body + 40);
	put_unaligned_le32(401, body + 44);
	put_unaligned_le32(1, body + 48);
	error = neo_call(mic, body, sizeof(body), 8);
	if (!error) {
		if (mic->reply_size < 8 || get_unaligned_le32(mic->reply) != 4)
			return -EPROTO;
		dev_info(mic->dev, "HPAI property401=%08x; audio control transport verified\n",
			 get_unaligned_le32(mic->reply + 4));
	}
	return error;
}

bool neo_audio_healthy(struct neo_mic *mic)
{
	return READ_ONCE(mic->ready) && !READ_ONCE(mic->crashed);
}

int neo_audio_set(struct neo_mic *mic, u32 device, u32 property,
		  const void *payload, size_t length)
{
	u8 body[200] = {0};
	if (length > sizeof(body) - 52)
		return -EINVAL;
	put_unaligned_le32(~0U, body + 4);
	put_unaligned_le32(0xc3000005, body + 8);
	put_unaligned_le32(0x30 + ALIGN(length, 4), body + 32);
	put_unaligned_le32(device, body + 40);
	put_unaligned_le32(property, body + 44);
	put_unaligned_le32(length, body + 48);
	memcpy(body + 52, payload, length);
	return neo_call(mic, body, 52 + ALIGN(length, 4), 0);
}

static int neo_audio_power_locked(struct neo_mic *mic, u32 device, u32 state, u32 active)
{
	u8 body[100] = {0};
	u64 ticks = mul_u64_u32_div(arch_timer_read_counter(), 24000000,
				  arch_timer_get_rate());
	int error;

	put_unaligned_le32(~0U, body + 4);
	put_unaligned_le32(0xc3000005, body + 8);
	put_unaligned_le32(0x60, body + 32);
	put_unaligned_le32(device, body + 40);
	put_unaligned_le32(202, body + 44);
	put_unaligned_le32(48, body + 48);
	put_unaligned_le32(device, body + 52);
	put_unaligned_le32(atomic_inc_return(&mic->power_cookie), body + 56);
	put_unaligned_le64(ticks, body + 60);
	put_unaligned_le32(state, body + 72);
	put_unaligned_le32(active, body + 76);
	error = neo_call(mic, body, sizeof(body), 0);
	dev_info(mic->dev, "Audio device=%08x power state=%08x active=%u result=%d\n",
		 device, state, active, error);
	return error;
}

int neo_audio_power(struct neo_mic *mic, u32 device, u32 state, u32 active)
{
	int error;

	mutex_lock(&mic->power_lock);
	/* A DMA supplier may still own the AP register aperture after PCM stops.
	 * Keep it warm until that supplier releases its generic power domain.
	 */
	if (device == NEO_SPEAKER_DEVICE && state == NEO_MIC_OFF && mic->speaker_warm_held) {
		state = NEO_MIC_WARM;
		active = 0;
	}
	error = neo_audio_power_locked(mic, device, state, active);
	mutex_unlock(&mic->power_lock);
	return error;
}

static int neo_speaker_domain_on(struct generic_pm_domain *domain)
{
	struct neo_mic *mic = container_of(domain, struct neo_mic, speaker_domain);
	int error;

	mutex_lock(&mic->power_lock);
	error = neo_audio_power_locked(mic, NEO_SPEAKER_DEVICE, NEO_MIC_WARM, 0);
	if (!error)
		mic->speaker_warm_held = true;
	mutex_unlock(&mic->power_lock);
	return error;
}

static int neo_speaker_domain_off(struct generic_pm_domain *domain)
{
	struct neo_mic *mic = container_of(domain, struct neo_mic, speaker_domain);
	int error;

	mutex_lock(&mic->power_lock);
	error = neo_audio_power_locked(mic, NEO_SPEAKER_DEVICE, NEO_MIC_OFF, 0);
	if (!error)
		mic->speaker_warm_held = false;
	mutex_unlock(&mic->power_lock);
	return error;
}

static void neo_remove_power_domain(void *cookie)
{
	struct neo_mic *mic = cookie;

	of_genpd_del_provider(mic->dev->of_node);
	if (!pm_genpd_remove(&mic->speaker_domain) && mic->speaker_warm_held)
		neo_speaker_domain_off(&mic->speaker_domain);
}

static int neo_register_power_domain(struct neo_mic *mic)
{
	u32 cells;
	int error;

	if (!of_find_property(mic->dev->of_node, "#power-domain-cells", NULL))
		return 0;
	error = of_property_read_u32(mic->dev->of_node, "#power-domain-cells", &cells);
	if (error || cells != 1)
		return -EINVAL;
	mic->speaker_domain.name = devm_kasprintf(mic->dev, GFP_KERNEL, "%s-speaker-dma",
						dev_name(mic->dev));
	if (!mic->speaker_domain.name)
		return -ENOMEM;
	mic->speaker_domain.power_on = neo_speaker_domain_on;
	mic->speaker_domain.power_off = neo_speaker_domain_off;
	/* ADMAC currently has no runtime PM. Keep the register aperture warm
	 * while it is bound, including IRQ handling; amplifier power is separate.
	 * Remove this constraint once ADMAC runtime PM is implemented and tested.
	 */
	mic->speaker_domain.flags = GENPD_FLAG_ALWAYS_ON;
	error = neo_speaker_domain_on(&mic->speaker_domain);
	if (error)
		return error;
	error = pm_genpd_init(&mic->speaker_domain, NULL, false);
	if (error) {
		neo_speaker_domain_off(&mic->speaker_domain);
		return error;
	}
	mic->domains[0] = &mic->speaker_domain;
	mic->domain_data.domains = mic->domains;
	mic->domain_data.num_domains = ARRAY_SIZE(mic->domains);
	error = of_genpd_add_provider_onecell(mic->dev->of_node, &mic->domain_data);
	if (error) {
		pm_genpd_remove(&mic->speaker_domain);
		neo_speaker_domain_off(&mic->speaker_domain);
		return error;
	}
	return devm_add_action_or_reset(mic->dev, neo_remove_power_domain, mic);
}

int neo_mic_power(struct neo_mic *mic, u32 state, u32 active)
{
	return neo_audio_power(mic, NEO_HPAI, state, active);
}

struct neo_mic *neo_mic_get(struct device *dev)
{
	struct device_node *node = of_parse_phandle(dev->of_node, "apple,aop", 0);
	struct platform_device *pdev;
	struct neo_mic *mic;
	if (!node) return ERR_PTR(-ENODEV);
	pdev = of_find_device_by_node(node);
	of_node_put(node);
	if (!pdev) return ERR_PTR(-EPROBE_DEFER);
	mic = platform_get_drvdata(pdev);
	if (!mic || !mic->ready) mic = ERR_PTR(-EPROBE_DEFER);
	else if (!device_link_add(dev, &pdev->dev, DL_FLAG_AUTOREMOVE_CONSUMER))
		mic = ERR_PTR(-ENOMEM);
	put_device(&pdev->dev);
	return mic;
}

static int neo_mic_channels(struct neo_mic *mic)
{
	u8 body[64] = {0};
	put_unaligned_le32(~0U, body + 4);
	put_unaligned_le32(0xc3000005, body + 8);
	put_unaligned_le32(0x3c, body + 32);
	put_unaligned_le32(0x6c706169, body + 40); /* lpai */
	put_unaligned_le32(300, body + 44);
	put_unaligned_le32(12, body + 48);
	put_unaligned_le32(3, body + 52);
	put_unaligned_le32(3, body + 56);
	put_unaligned_le32(3, body + 60);
	return neo_call(mic, body, sizeof(body), 0);
}

static int neo_mic_probe(struct platform_device *pdev)
{
	struct neo_mic *mic;
	struct device_node *pmp;
	int error, i;

	/* PMP owns SoC power policy on systems which declare this supplier. */
	pmp = of_parse_phandle(pdev->dev.of_node, "apple,pmp", 0);
	if (pmp) {
		struct platform_device *supplier = of_find_device_by_node(pmp);
		struct device_link *link;

		of_node_put(pmp);
		if (!supplier)
			return -EPROBE_DEFER;
		if (!device_is_bound(&supplier->dev)) {
			put_device(&supplier->dev);
			return -EPROBE_DEFER;
		}
		link = device_link_add(&pdev->dev, &supplier->dev,
				       DL_FLAG_AUTOREMOVE_CONSUMER);
		put_device(&supplier->dev);
		if (!link)
			return -ENOMEM;
	}

	mic = devm_kzalloc(&pdev->dev, sizeof(*mic), GFP_KERNEL);
	if (!mic) return -ENOMEM;
	mic->dev = &pdev->dev;
	mic->secure = neo_secure_get(mic->dev);
	if (IS_ERR(mic->secure)) return PTR_ERR(mic->secure);
	init_completion(&mic->announced);
	init_completion(&mic->replied);
	mutex_init(&mic->call_lock);
	mutex_init(&mic->power_lock);
	platform_set_drvdata(pdev, mic);
	error = dma_set_mask_and_coherent(mic->dev, DMA_BIT_MASK(42));
	if (error) return error;
	mic->asc = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(mic->asc)) return PTR_ERR(mic->asc);
	error = devm_add_action_or_reset(mic->dev, neo_free_rings, mic);
	if (error) return error;
	mic->rtkit = devm_apple_rtkit_init(mic->dev, mic, "mbox", 0, &neo_rtkit_ops);
	if (IS_ERR(mic->rtkit)) return PTR_ERR(mic->rtkit);
	error = devm_add_action_or_reset(mic->dev, neo_halt, mic);
	if (error) return error;
	dev_info(mic->dev, "Starting resident Neo AOP firmware\n");
	writel(readl(mic->asc + 0x44) | BIT(4), mic->asc + 0x44);
	error = apple_rtkit_boot(mic->rtkit);
	if (error) return dev_err_probe(mic->dev, error, "RTKit boot failed\n");
	error = neo_secure_wait(mic->secure);
	if (error) return dev_err_probe(mic->dev, error, "Second-host boot failed\n");
	for (i = 0; i < NEO_EP_COUNT; i++) {
		u8 ep = NEO_EP_FIRST + i;
		if (!apple_rtkit_has_endpoint(mic->rtkit, ep)) continue;
		error = apple_rtkit_start_ep(mic->rtkit, ep);
		/* Neo firmware initiates AFK with version 8 after endpoint start.
		 * The older host-initiated INIT transaction is absent natively. */
		if (error) return error;
	}
	if (!wait_for_completion_timeout(&mic->announced, NEO_CALL_TIMEOUT) || mic->crashed)
		return dev_err_probe(mic->dev, -ETIMEDOUT, "Audio service absent\n");
	error = neo_query_boot_status(mic);
	if (error) return dev_err_probe(mic->dev, error, "Microphone state query failed\n");
	error = neo_mic_channels(mic);
	if (error == -EOPNOTSUPP)
		dev_info(mic->dev, "LPAI channel configuration is unsupported; retaining HPAI transport\n");
	else if (error)
		return dev_err_probe(mic->dev, error, "Microphone channel setup failed\n");
	error = neo_register_power_domain(mic);
	if (error)
		return dev_err_probe(mic->dev, error, "Audio power domain registration failed\n");
	mic->ready = true;
	mic->debugfs = debugfs_create_dir(dev_name(mic->dev), NULL);
	debugfs_create_file("query", 0600, mic->debugfs, mic, &neo_query_fops);
	error = devm_add_action_or_reset(mic->dev, neo_remove_debugfs, mic);
	if (error)
		return error;
	return 0;
}

static const struct of_device_id neo_mic_of_match[] = {
	{ .compatible = "apple,t8142-aop-audio" },
	{ .compatible = "apple,t8140-neo-mic" },
	{}
};
MODULE_DEVICE_TABLE(of, neo_mic_of_match);

static struct platform_driver neo_mic_driver = {
	.probe = neo_mic_probe,
	.driver = { .name = "neo-aop-mic", .of_match_table = neo_mic_of_match },
};
module_platform_driver(neo_mic_driver);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Private Neo AOP microphone control");
