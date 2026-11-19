// SPDX-License-Identifier: GPL-2.0-only
/* MT7932 firmware MLME and opaque, board-specific calibration cache. */
#include <linux/firmware.h>
#include <linux/etherdevice.h>
#include <linux/unaligned.h>
#include "mt7921.h"
#include "mt7932_offload.h"
#include "mt7932_mcu.h"

/* Observed ABI offsets. Padding and the unclassified security buffer remain zero. */
struct mt7932_connect_command {
	__le32 auth_suite, pairwise_cipher, operation_deadline;
	__le32 reserved0, flags, operation;
	__le32 ssid_len;
	u8 ssid[IEEE80211_MAX_SSID_LEN];
	__le32 reserved1, password_len, password_format, reserved2;
	u8 password[64];
	u8 reserved3[308];
	__le32 pmf;
	__le32 reserved4, security_buffer_len;
	u8 security_buffer[32], reserved4b[32];
	__le32 allowed_auth;
	u8 reserved5[24];
	__le32 compatibility_policy;
	u8 reserved5b[252];
	__le32 candidate_flags;
	u8 bssid[ETH_ALEN], reserved6[6];
	u8 band, channel, reserved7[2];
	u8 candidate_policy[8];
	__le32 reserved8;
	u8 candidate_tail_policy[4];
	u8 reserved9[8];
	__le32 candidates;
} __packed;

static_assert(sizeof(struct mt7932_connect_command) == 856);
static_assert(sizeof(struct mt7932_connect_params) == 106);
static_assert(offsetof(struct mt7932_connect_command, ssid_len) == 24);
static_assert(offsetof(struct mt7932_connect_command, ssid) == 28);
static_assert(offsetof(struct mt7932_connect_command, password_len) == 64);
static_assert(offsetof(struct mt7932_connect_command, password) == 76);
static_assert(offsetof(struct mt7932_connect_command, pmf) == 448);
static_assert(offsetof(struct mt7932_connect_command, security_buffer_len) == 456);
static_assert(offsetof(struct mt7932_connect_command, security_buffer) == 460);
static_assert(offsetof(struct mt7932_connect_command, allowed_auth) == 524);
static_assert(offsetof(struct mt7932_connect_command, bssid) == 812);
static_assert(offsetof(struct mt7932_connect_command, band) == 824);
static_assert(offsetof(struct mt7932_connect_command, candidates) == 852);

int mt7932_offload_connect(struct mt792x_dev *dev,
			   const struct mt7932_connect_params *p)
{
	struct mt7932_connect_command *cmd;
	struct mt7932_ais_mac mac = {};
	__le32 profile = cpu_to_le32(MT7932_SME_NATIVE_PROFILE);
	struct mt7932_basic_config basic = MT7932_BASIC_CONFIG_INITIALIZER(0);
	int ret;

	if (!p->ssid_len || p->ssid_len > sizeof(p->ssid) ||
	    !p->password_len || p->password_len > sizeof(p->password) ||
	    !is_valid_ether_addr(p->bssid) || p->band < MT7932_BAND_2GHZ || p->band > MT7932_BAND_6GHZ ||
	    !p->channel || (p->band == MT7932_BAND_2GHZ && p->channel > 14) ||
	    (p->band == MT7932_BAND_6GHZ && (p->channel > 233 || (p->channel - 1) % 4)))
		return -EINVAL;
	cmd = kzalloc(sizeof(*cmd), GFP_KERNEL);
	if (!cmd)
		return -ENOMEM;
	/* SAE password offload, CCMP, mandatory PMF; no host-supplied PMK. */
	cmd->auth_suite = cpu_to_le32(MT7932_SME_AUTH_SAE);
	cmd->pairwise_cipher = cpu_to_le32(MT7932_SME_CIPHER_CCMP);
	cmd->operation_deadline = cpu_to_le32(MT7932_SME_NATIVE_DEADLINE);
	cmd->flags = cpu_to_le32(MT7932_SME_CONNECT_FLAGS);
	cmd->operation = cpu_to_le32(MT7932_SME_NATIVE_OPERATION);
	cmd->ssid_len = cpu_to_le32(p->ssid_len);
	memcpy(cmd->ssid, p->ssid, p->ssid_len);
	cmd->password_len = cpu_to_le32(p->password_len);
	/* This field is 10 for the observed password encoding; it is not
	 * gen4m's AUTH_MODE enum (SAE is 11 in that separate namespace).
	 */
	cmd->password_format = cpu_to_le32(MT7932_SME_PASSWORD_FORMAT);
	memcpy(cmd->password, p->password, p->password_len);
	cmd->pmf = cpu_to_le32(MT7932_SME_PMF_REQUIRED);
	/* No cached security value for fresh SAE. Its role in the captured
	 * request is unclassified. An absent buffer, zero buffer and newly
	 * derived PSK are being checked independently of radio controls.
	 */
	/* Observed firmware authentication policy. Bit 2's meaning is pending
	 * independent verification; this is not the public gen4m auth enum.
	 */
	cmd->allowed_auth = cpu_to_le32(MT7932_SME_NATIVE_AUTH_POLICY);
	/* Compatibility fields under investigation. These contain public
	 * policy/PHY information, not any network identity or secret. Keep
	 * them explicit so individual fields can be tested independently.
	 */
	cmd->compatibility_policy = cpu_to_le32(MT7932_SME_NATIVE_COMPATIBILITY);
	cmd->candidate_policy[0] = 1;
	cmd->candidate_policy[1] = 2;
	cmd->candidate_policy[2] = 4;
	cmd->candidate_policy[4] = 6;
	cmd->candidate_policy[5] = 7;
	cmd->candidate_tail_policy[0] = 4;
	cmd->candidate_tail_policy[1] = 16;
	cmd->candidate_tail_policy[2] = 18;
	cmd->candidate_tail_policy[3] = 20;
	memcpy(cmd->bssid, p->bssid, ETH_ALEN);
	cmd->band = p->band;
	cmd->channel = p->channel;
	cmd->candidates = cpu_to_le32(1);

	mt792x_mutex_acquire(dev);
	ret = mt7932_start(&dev->phy);
	if (!ret)
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_BASIC_CONFIG,
				       &basic, sizeof(basic), false);
	if (!ret)
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SME_CONFIG,
				       &profile, sizeof(profile), false);
	ether_addr_copy(mac.mac, dev->mphy.macaddr);
	if (!ret)
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_AIS_MAC,
				       &mac, sizeof(mac), false);
	if (!ret) {
		mt7932_fullmac_connecting(dev, p->bssid);
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SME_CONNECT, cmd, sizeof(*cmd), false);
	}
	mt792x_mutex_release(dev);
	kfree_sensitive(cmd);
	return ret;
}

/* EA/42 is emitted by the native CoreWLAN disassociate operation.
 * Preserve the observed policy words until their individual semantics are
 * established. Construct the peer field from the current connection.
 */
struct mt7932_disconnect_command {
	__le32 policy;
	__le32 reserved;
	__le16 peer_tag;
	__le16 peer_len;
	struct_group(peer,
		u8 bss_idx;
		u8 bssid[ETH_ALEN];
		u8 padding;
	);
	__le32 trailer;
} __packed;

static_assert(sizeof(struct mt7932_disconnect_command) == 24);
static_assert(offsetof(struct mt7932_disconnect_command, bssid) == 13);

int mt7932_offload_disconnect(struct mt792x_dev *dev, const u8 *bssid)
{
	struct mt7932_disconnect_command cmd = {
		.policy = cpu_to_le32(MT7932_DISCONNECT_POLICY),
		.peer_tag = cpu_to_le16(MT7932_DISCONNECT_PEER_TAG),
		.peer_len = cpu_to_le16(sizeof(cmd.peer)),
		.trailer = cpu_to_le32(MT7932_DISCONNECT_TRAILER),
	};
	int ret;

	if (!is_valid_ether_addr(bssid))
		return -EINVAL;
	ether_addr_copy(cmd.bssid, bssid);
	mt792x_mutex_acquire(dev);
	ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SME_DISCONNECT,
			       &cmd, sizeof(cmd), false);
	mt792x_mutex_release(dev);
	return ret;
}

/* On disk: MT7932P1, MAC[6], LE16 record count, LE32 total size, records.
 * Each record is calibration metadata followed by opaque RF data. There are
 * no MCU descriptors, sequence numbers, commands or connection credentials.
 */
#define MT7932_PRECAL_MAGIC "MT7932P1"
#define MT7932_PRECAL_MAX_SIZE (400 * SZ_1K)
#define MT7932_PRECAL_MAX_RECORDS 128
#define MT7932_PRECAL_GROUP_CLASS 1
#define MT7932_PRECAL_CHANNEL_CLASS 2
#define MT7932_PRECAL_CHANNEL_INFO_CLASS 3
#define MT7932_PRECAL_GROUP_BYTES 1080
#define MT7932_PRECAL_CHANNEL_BYTES 604
#define MT7932_PRECAL_CHANNEL_INFO_BYTES 16
#define MT7932_PRECAL_GROUP_MASK GENMASK(15, 0)
#define MT7932_PRECAL_PRELOAD_MASK GENMASK(31, 16)
#define MT7932_PRECAL_FREQUENCY_MASK GENMASK(23, 0)
/* Explicit coverage restriction, not a general band-to-calibration map. */
#define MT7932_PRECAL_QUALIFIED_6GHZ_GROUP 28

struct mt7932_precal_file_header {
	u8 magic[8];
	u8 mac[ETH_ALEN];
	__le16 count;
	__le32 size;
} __packed;
static_assert(sizeof(struct mt7932_precal_file_header) == 20);

struct mt7932_precal_key {
	__le32 selector;
	__le32 frequency;
	__le32 control;
} __packed;

struct mt7932_precal_request {
	__le32 unknown_0;
	struct mt7932_precal_key key;
} __packed;
static_assert(sizeof(struct mt7932_precal_request) == 16);

struct mt7932_cal_record {
	u8 class, operation;
	__le16 id;
	__le32 len;
	union {
		struct mt7932_precal_key key;
		struct { __le32 selector, frequency, control; };
	};
	u8 data[];
} __packed;
static_assert(sizeof(struct mt7932_cal_record) == 20);

struct mt7932_cal_cache {
	unsigned int count;
	u8 data[];
};

int mt7932_precal_load(struct mt792x_dev *dev, struct mt7932_cal_cache **out)
{
	struct mt7932_cal_cache *cache;
	const struct mt7932_precal_file_header *header;
	const struct firmware *fw;
	char name[80];
	size_t pos;
	u16 count;
	int i, ret;

	*out = NULL;
	snprintf(name, sizeof(name), "mediatek/mt7932/precal-%pm.bin", dev->mphy.macaddr);
	ret = request_firmware(&fw, name, dev->mt76.dev);
	if (ret)
		return ret;
	ret = -EINVAL;
	header = (const void *)fw->data;
	if (fw->size < sizeof(*header) || fw->size > MT7932_PRECAL_MAX_SIZE ||
	    memcmp(header->magic, MT7932_PRECAL_MAGIC, sizeof(header->magic)) ||
	    !ether_addr_equal(header->mac, dev->mphy.macaddr) ||
	    le32_to_cpu(header->size) != fw->size)
		goto out;
	count = le16_to_cpu(header->count);
	if (!count || count > MT7932_PRECAL_MAX_RECORDS)
		goto out;
	for (i = 0, pos = sizeof(*header); i < count; i++) {
		const struct mt7932_cal_record *r;
		u32 len;

		if (fw->size - pos < sizeof(*r))
			goto out;
		r = (const void *)(fw->data + pos);
		len = le32_to_cpu(r->len);
		if (r->operation || r->class < MT7932_PRECAL_GROUP_CLASS ||
		    r->class > MT7932_PRECAL_CHANNEL_INFO_CLASS ||
		    len != (r->class == MT7932_PRECAL_GROUP_CLASS ? MT7932_PRECAL_GROUP_BYTES :
			    r->class == MT7932_PRECAL_CHANNEL_CLASS ? MT7932_PRECAL_CHANNEL_BYTES :
								    MT7932_PRECAL_CHANNEL_INFO_BYTES) ||
		    len > fw->size - pos - sizeof(*r))
			goto out;
		pos += sizeof(*r) + len;
	}
	if (pos != fw->size)
		goto out;
	cache = devm_kzalloc(dev->mt76.dev,
			     sizeof(*cache) + fw->size - sizeof(*header), GFP_KERNEL);
	if (!cache) {
		ret = -ENOMEM;
		goto out;
	}
	cache->count = count;
	memcpy(cache->data, fw->data + sizeof(*header), fw->size - sizeof(*header));
	*out = cache;
	dev_info(dev->mt76.dev, "MT7932 board precal cache loaded (%u records)\n", count);
	ret = 0;
out:
	release_firmware(fw);
	return ret;
}

static int mt7932_precal_send(struct mt792x_dev *dev,
			    const struct mt7932_cal_record *r)
{
	struct mt7932_cal_record *req;
	u32 len = le32_to_cpu(r->len);
	int ret;

	req = kzalloc(sizeof(*req) + len, GFP_KERNEL);
	if (!req)
		return -ENOMEM;
	req->class = r->class;
	req->id = r->id;
	req->len = cpu_to_le32(len);
	req->selector = r->selector;
	req->frequency = r->frequency;
	req->control = r->control;
	memcpy(req->data, r->data, len);
	/* Completion is an unsolicited D9, never a sequence-number reply. */
	ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_PRECAL,
			       req, sizeof(*req) + len, false);
	kfree(req);
	return ret;
}

int mt7932_precal_preload(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			   u8 band)
{
	const struct mt7932_cal_record *r;
	size_t pos = 0;
	int i, sent = 0, ret = 0;

	if (!cache)
		return -ENOENT;
	mt792x_mutex_acquire(dev);
	for (i = 0; i < cache->count; i++) {
		u32 selector;

		r = (const void *)(cache->data + pos);
		pos += sizeof(*r) + le32_to_cpu(r->len);
		selector = le32_to_cpu(r->selector);
		if (band != MT7932_BAND_6GHZ || r->class != MT7932_PRECAL_GROUP_CLASS ||
		    FIELD_GET(MT7932_PRECAL_GROUP_MASK, selector) !=
		    MT7932_PRECAL_QUALIFIED_6GHZ_GROUP ||
		    !(selector & MT7932_PRECAL_PRELOAD_MASK))
			continue;
		ret = mt7932_precal_send(dev, r);
		if (ret)
			break;
		sent++;
	}
	mt792x_mutex_release(dev);
	dev_info(dev->mt76.dev, "MT7932 group precal preload band=%u records=%d result=%d\n",
		 band, sent, ret);
	return ret ?: (sent ? 0 : -ENOENT);
}

int mt7932_precal_apply(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			 u8 band, u32 center_khz)
{
	const struct mt7932_cal_record *r;
	size_t pos = 0;
	int i, sent = 0, ret = 0;

	if (!cache)
		return -ENOENT;
	mt792x_mutex_acquire(dev);
	for (i = 0; i < cache->count; i++) {
		u32 frequency, group;

		r = (const void *)(cache->data + pos);
		pos += sizeof(*r) + le32_to_cpu(r->len);
		frequency = le32_to_cpu(r->frequency) & MT7932_PRECAL_FREQUENCY_MASK;
		group = FIELD_GET(MT7932_PRECAL_GROUP_MASK, le32_to_cpu(r->selector));
		/* Preload-marked group records were submitted before connect. */
		if (r->class == MT7932_PRECAL_GROUP_CLASS &&
		    (le32_to_cpu(r->selector) & MT7932_PRECAL_PRELOAD_MASK))
			continue;
		/* This bank covers the observed 6 GHz group 28 only. Never apply
		 * another channel's DPD data to an unsupported frequency.
		 */
		if (band != MT7932_BAND_6GHZ ||
		    (r->class == MT7932_PRECAL_GROUP_CLASS ?
		     group != MT7932_PRECAL_QUALIFIED_6GHZ_GROUP : frequency != center_khz))
			continue;
		ret = mt7932_precal_send(dev, r);
		if (ret)
			break;
		sent++;
	}
	mt792x_mutex_release(dev);
	dev_info(dev->mt76.dev, "MT7932 precal apply band=%u center=%u kHz records=%d result=%d\n",
		 band, center_khz, sent, ret);
	return ret ?: (sent ? 0 : -ENOENT);
}

int mt7932_precal_request(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			   const u8 *request, size_t len)
{
	const struct mt7932_cal_record *r;
	size_t pos = 0;
	int i, ret = -ENOENT;

	if (!cache || len < sizeof(struct mt7932_precal_request))
		return -EINVAL;
	/* The D7 key split is still undocumented. Match all three words
	 * exactly, rather than guessing a frequency or returning a wrong block.
	 */
	mt792x_mutex_acquire(dev);
	for (i = 0; i < cache->count; i++) {
		r = (const void *)(cache->data + pos);
		pos += sizeof(*r) + le32_to_cpu(r->len);
		if (memcmp(&r->key,
			   &((const struct mt7932_precal_request *)request)->key,
			   sizeof(r->key)))
			continue;
		ret = mt7932_precal_send(dev, r);
		if (ret)
			break;
	}
	mt792x_mutex_release(dev);
	return ret;
}
