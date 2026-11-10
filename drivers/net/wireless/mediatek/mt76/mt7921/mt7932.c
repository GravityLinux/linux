// SPDX-License-Identifier: GPL-2.0-only
/* MT7932 CONNAC2 boot transport, based on documented hardware behavior. */
#include <linux/firmware.h>
#include <linux/debugfs.h>
#include <linux/ktime.h>
#include <linux/pci.h>
#include <linux/unaligned.h>
#include "mt7921.h"
#include "mcu.h"
#include "mt7932_offload.h"
#include "mt7932_mcu.h"
#include "mt7932_fullmac.h"
#include "mt7932_regs.h"
#include "../mt76_connac2_mac.h"

static u8 mt7932_diag_data[1024 * 1024];
static unsigned int mt7932_diag_used = 4;
static DEFINE_SPINLOCK(mt7932_diag_lock);
static struct debugfs_blob_wrapper mt7932_diag_blob = {
	.data = mt7932_diag_data,
	.size = sizeof(mt7932_diag_data),
};

/* Root-only protocol bring-up transport, limited to reversible runtime settings. */
static ssize_t mt7932_runtime_write(struct file *file, const char __user *user,
				   size_t count, loff_t *ppos)
{
	struct mt792x_dev *dev = file->private_data;
	u8 *request;
	u16 cmd;
	int ret;

	if (count < 2 || count > 1538)
		return -EINVAL;
	request = memdup_user(user, count);
	if (IS_ERR(request))
		return PTR_ERR(request);
	cmd = get_unaligned_le16(request);
	switch (cmd) {
	case MT7932_CMD_CUSTOMER_CONFIG: /* Runtime key/value configuration. */
	case MT7932_CMD_QUEUE_MANAGEMENT: /* Queue management. */
	case MT7932_CMD_RUNTIME_93: /* Payload semantics unresolved. */
	case MT7932_CMD_PROBE_PRIVACY: /* Receive filtering. */
	case MT7932_CMD_RTS: /* RTS protection. */
	case MT7932_CMD_BEACON_PARSE: /* Beacon IE parse result. */
	case MT7932_CMD_SME_RNR: /* Reduced-neighbor report. */
	case MT7932_CMD_SME_DHCP_RELEASE: /* DHCP channel privilege release. */
		mt792x_mutex_acquire(dev);
		ret = mt76_mcu_send_msg(&dev->mt76, cmd, request + 2,
				      count - 2, false);
		mt792x_mutex_release(dev);
		break;
	default:
		ret = -EOPNOTSUPP;
	}
	memzero_explicit(request, count);
	kfree(request);
	return ret ? ret : count;
}

static const struct file_operations mt7932_runtime_fops = {
	.owner = THIS_MODULE,
	.open = simple_open,
	.write = mt7932_runtime_write,
	.llseek = noop_llseek,
};

/* Exercise the real recovery path without deliberately crashing firmware. */
static int mt7932_recovery_set(void *data, u64 value)
{
	struct mt792x_dev *dev = data;
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);
	int ret = 0;

	if (value != 1)
		return -EINVAL;
	if (!fm || !dev->hw_init_done)
		return -ENODEV;
	wiphy_lock(fm->wiphy);
	if (fm->removing)
		ret = -ENODEV;
	else
		mt792x_reset(&dev->mt76);
	wiphy_unlock(fm->wiphy);
	return ret;
}
DEFINE_DEBUGFS_ATTRIBUTE(mt7932_recovery_fops, NULL, mt7932_recovery_set, "%llu\n");

void mt7932_diag_init(struct mt792x_dev *dev, struct dentry *dir)
{
	debugfs_create_blob("neo_firmware_events", 0400, dir, &mt7932_diag_blob);
	debugfs_create_file("neo_recover", 0200, dir, dev, &mt7932_recovery_fops);
	debugfs_create_file("neo_runtime_command", 0200, dir, dev,
			    &mt7932_runtime_fops);
}

void mt7932_diag_rx(enum mt76_rxq_id q, struct sk_buff *skb)
{
	unsigned long flags;
	u8 *p;

	/* Opaque firmware diagnostics only; never expose normal data-ring traffic. */
	if (q != MT_RXQ_MCU && q != MT_RXQ_MCU_WA && q != MT_RXQ_BAND2_WA)
		return;
	if (skb->len > 2352)
		return;
	spin_lock_irqsave(&mt7932_diag_lock, flags);
	if (skb->len + 12 <= sizeof(mt7932_diag_data) - mt7932_diag_used) {
		p = mt7932_diag_data + mt7932_diag_used;
		put_unaligned_le16(q, p);
		put_unaligned_le16(skb->len, p + 2);
		put_unaligned_le64(ktime_get_boottime_ns(), p + 4);
		if (!skb_copy_bits(skb, 0, p + 12, skb->len)) {
			mt7932_diag_used += 12 + skb->len;
			put_unaligned_le32(mt7932_diag_used, mt7932_diag_data);
		}
	}
	spin_unlock_irqrestore(&mt7932_diag_lock, flags);
}

static unsigned int mt7932_mlme_config;
module_param(mt7932_mlme_config, uint, 0600);
MODULE_PARM_DESC(mt7932_mlme_config, "MT7932 experimental MLME configuration word");
static unsigned int mt7932_host_bss_index = 1;
module_param(mt7932_host_bss_index, uint, 0600);
MODULE_PARM_DESC(mt7932_host_bss_index, "MT7932 host SME BSS index (0=AIS, 1=host-managed client)");

struct mt7932_shared_memory {
	void *cpu[3];
	dma_addr_t dma[3];
};

static void mt7932_shared_release(struct device *dev, void *res)
{
	struct mt7932_shared_memory *mem = res;
	int i;

	for (i = 0; i < ARRAY_SIZE(mem->cpu); i++)
		if (mem->cpu[i])
			dma_free_coherent(dev, SZ_16K, mem->cpu[i], mem->dma[i]);
}

static int mt7932_publish_shared_memory(struct mt792x_dev *dev)
{
	struct mt7932_shared_memory *mem;
	struct device *device = dev->mt76.dev;
	int i;

	mem = devres_find(device, mt7932_shared_release, NULL, NULL);
	if (!mem) {
		mem = devres_alloc(mt7932_shared_release, sizeof(*mem), GFP_KERNEL);
		if (!mem)
			return -ENOMEM;
		for (i = 0; i < ARRAY_SIZE(mem->cpu); i++) {
			mem->cpu[i] = dma_alloc_coherent(device, SZ_16K,
						       &mem->dma[i], GFP_KERNEL);
			if (!mem->cpu[i] || upper_32_bits(mem->dma[i])) {
				mt7932_shared_release(device, mem);
				devres_free(mem);
				return -ENOMEM;
			}
		}
		devres_add(device, mem);
	}
	for (i = 0; i < ARRAY_SIZE(mem->cpu); i++)
		memset(mem->cpu[i], 0, SZ_16K);
	/* Firmware expects a 0x300-byte status block and 0x320-byte descriptors. */
	dma_wmb();
	mt76_wr(dev, MT7932_HOST_DMA_CONTROL0, MT7932_HOST_DMA_CONTROL0_INIT);
	mt76_wr(dev, MT7932_HOST_DMA_CONTROL1, MT7932_HOST_DMA_CONTROL1_INIT);
	mt76_wr(dev, MT7932_HOST_DMA_ADDR0, lower_32_bits(mem->dma[0]));
	mt76_wr(dev, MT7932_HOST_DMA_ADDR1, lower_32_bits(mem->dma[1]));
	mt76_wr(dev, MT7932_HOST_DMA_ADDR2, lower_32_bits(mem->dma[2]));
	dev_info(device, "MT7932 shared host DMA buffers published\n");
	return 0;
}

u8 mt7932_host_bss(void)
{
	return min_t(unsigned int, mt7932_host_bss_index, 1);
}

/* Same two manufacturer privacy records qualified in m1n1 044d00d8.
 * Index39 is inclusive (40 bitmap bytes); extension58 occupies the LAST byte.
 * These are IE permissions, not RF-power or timing calibration settings.
 * Repeat before MLME/scan activation after every firmware initialization.
 */
static int mt7932_probe_privacy(struct mt76_dev *mdev)
{
	static const u8 ordinary[] = {
		WLAN_EID_SSID, WLAN_EID_SUPP_RATES, WLAN_EID_DS_PARAMS,
		WLAN_EID_HT_CAPABILITY, WLAN_EID_EXT_SUPP_RATES,
		WLAN_EID_BSS_COEX_2040, WLAN_EID_INTERWORKING,
		WLAN_EID_ADVERTISEMENT_PROTOCOL, WLAN_EID_EXT_CAPABILITY,
		WLAN_EID_VHT_CAPABILITY, WLAN_EID_VENDOR_SPECIFIC, WLAN_EID_EXTENSION,
	};
	static const u8 extended[] = {
		WLAN_EID_EXT_HE_CAPABILITY, WLAN_EID_EXT_SHORT_SSID_LIST,
	};
	struct mt7932_probe_privacy bitmap = {
		.last_bitmap_byte = sizeof(bitmap.bitmap) - 1,
	};
	struct mt7932_probe_privacy policy = {
		.subcommand = 1,
		.unknown_policy0 = 20,
		.unknown_policy1 = 20,
	};
	struct sk_buff *reply;
	int ret, i;

	for (i = 0; i < ARRAY_SIZE(ordinary); i++)
		bitmap.bitmap[ordinary[i] / BITS_PER_BYTE] |=
			BIT(ordinary[i] % BITS_PER_BYTE);
	for (i = 0; i < ARRAY_SIZE(extended); i++)
		bitmap.bitmap[MT7932_PRIVACY_ORDINARY_BYTES +
			      extended[i] / BITS_PER_BYTE] |=
			BIT(extended[i] % BITS_PER_BYTE);
	ret = mt76_mcu_send_msg(mdev, MT7932_CMD_PROBE_PRIVACY,
			       &bitmap, sizeof(bitmap), false);
	if (ret)
		return ret;
	ret = mt76_mcu_send_msg(mdev, MT7932_CMD_PROBE_PRIVACY,
			       &policy, sizeof(policy), false);
	if (ret)
		return ret;
	/* Consume a synchronous capability reply before discovery starts. */
	ret = mt76_mcu_send_and_get_msg(mdev, MCU_CE_CMD(GET_NIC_CAPAB),
				      NULL, 0, true, &reply);
	if (!ret) {
		dev_kfree_skb(reply);
		dev_info(mdev->dev, "MT7932 probe-request privacy initialized\n");
	}
	return ret;
}

static int mt7932_basic_config(struct mt76_dev *mdev)
{
	struct mt7932_basic_config basic = MT7932_BASIC_CONFIG_INITIALIZER(1);
	u8 mlme[4];
	/* Queue-management startup records observed on this 7932 firmware ABI. */
	u8 qm[3][32] = {
		{ 0x10, 0x11, 2, 0, 3, 0, 7, 0, 0x2f, 0, 2 },
		{ 0xe0, 0x6e, 2, 0, 0x0f, 0, 0xff, 3, 0, 0, 3 },
		{ 0x0f, 0, 1 },
	};
	int ret = mt76_mcu_send_msg(mdev, MT7932_CMD_BASIC_CONFIG,
				     &basic, sizeof(basic), false);
	int i;

	if (ret)
		return ret;
	ret = mt7932_probe_privacy(mdev);
	if (ret)
		return ret;
	/* Test host SME operation; the offload configuration bits are undocumented. */
	put_unaligned_le32(mt7932_mlme_config, mlme);
	ret = mt76_mcu_send_msg(mdev, MT7932_CMD_SME_CONFIG,
			       mlme, sizeof(mlme), false);
	if (ret)
		return ret;
	for (i = 0; i < ARRAY_SIZE(qm); i++) {
		ret = mt76_mcu_send_msg(mdev, MT7932_CMD_QUEUE_MANAGEMENT,
			       qm[i], sizeof(qm[i]), false);
		if (ret)
			return ret;
	}
	return 0;
}

int mt7932_start(struct mt792x_phy *phy)
{
	struct mt76_dev *mdev = phy->mt76->dev;
	u8 dbdc[36] = { [4] = 2, [6] = 36 };
	u8 power[4] = {};
	int ret;

	ret = mt76_mcu_send_msg(mdev, MCU_CMD(NIC_POWER_CTRL),
			       power, sizeof(power), false);
	if (ret)
		return ret;
	ret = mt76_mcu_send_msg(mdev, MT7932_CMD_DBDC, dbdc, sizeof(dbdc), false);
	if (ret)
		return ret;
	ret = mt76_connac_mcu_set_channel_domain(phy->mt76);
	if (ret)
		return ret;
	set_bit(MT76_STATE_RUNNING, &phy->mt76->state);
	return 0;
}

static int mt7932_fill_rlm(struct mt792x_phy *phy,
			 const struct cfg80211_chan_def *ch, u8 bss_idx,
			 struct mt7932_rlm *rlm)
{
	if (!ch->chan)
		return -EINVAL;
	rlm->bss_idx = bss_idx;
	rlm->band = ch->chan->band == NL80211_BAND_2GHZ ? MT7932_BAND_2GHZ :
		    ch->chan->band == NL80211_BAND_5GHZ ? MT7932_BAND_5GHZ :
						       MT7932_BAND_6GHZ;
	rlm->primary_channel = ch->chan->hw_value;
	if (ch->width != NL80211_CHAN_WIDTH_20 &&
	    ch->width != NL80211_CHAN_WIDTH_20_NOHT)
		rlm->secondary_channel_offset =
			ch->center_freq1 > ch->chan->center_freq ?
			IEEE80211_HT_PARAM_CHA_SEC_ABOVE :
			IEEE80211_HT_PARAM_CHA_SEC_BELOW;
	rlm->short_slot_time = 1;
	switch (ch->width) {
	case NL80211_CHAN_WIDTH_80:
		rlm->vht_width = IEEE80211_VHT_CHANWIDTH_80MHZ;
		break;
	case NL80211_CHAN_WIDTH_160:
		rlm->vht_width = IEEE80211_VHT_CHANWIDTH_160MHZ;
		break;
	case NL80211_CHAN_WIDTH_80P80:
		rlm->vht_width = IEEE80211_VHT_CHANWIDTH_80P80MHZ;
		break;
	default:
		rlm->vht_width = IEEE80211_VHT_CHANWIDTH_USE_HT;
		break;
	}
	if (rlm->vht_width)
		rlm->center_channel1 = ieee80211_frequency_to_channel(ch->center_freq1);
	if (ch->center_freq2)
		rlm->center_channel2 = ieee80211_frequency_to_channel(ch->center_freq2);
	rlm->tx_nss = rlm->rx_nss = hweight8(phy->mt76->antenna_mask);
	return 0;
}

int mt7932_set_channel(struct mt792x_phy *phy, u8 bss_idx)
{
	struct mt7932_rlm rlm = {};
	int ret = mt7932_fill_rlm(phy, &phy->mt76->chandef, bss_idx, &rlm);

	if (ret)
		return ret;
	return mt76_mcu_send_msg(phy->mt76->dev, MT7932_CMD_SET_RLM,
			       &rlm, sizeof(rlm), false);
}

int mt7932_add_interface(struct mt792x_dev *dev, struct ieee80211_vif *vif,
			struct mt792x_vif *mvif, bool enable)
{
	struct mt7932_activate_bss activate = {};
	struct mt7932_ais_mac mac = {};
	int ret;

	if (vif->type != NL80211_IFTYPE_STATION ||
	    mvif->bss_conf.mt76.idx > MT7932_DISCOVERY_BSS)
		return -EOPNOTSUPP;
	/* AIS binds its address directly; host-owned roles use MUAR. */
	if (enable && mvif->bss_conf.mt76.idx == MT7932_AIS_BSS) {
		mac.bss_idx = mvif->bss_conf.mt76.idx;
		ether_addr_copy(mac.mac, vif->addr);
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_AIS_MAC,
				       &mac, sizeof(mac), false);
		if (ret)
			return ret;
	}
	activate.bss_idx = mvif->bss_conf.mt76.idx;
	activate.active = enable;
	/* The P2P client role delegates management frames to the host SME. */
	activate.network_type = mvif->bss_conf.mt76.idx ? 1 : 0;
	activate.omac_idx = mvif->bss_conf.mt76.omac_idx;
	ether_addr_copy(activate.mac, vif->addr);
	/* Scan-only contexts have no group key or allocated firmware BMC row. */
	activate.bmc_wcid = MT7932_NO_WCID;
	activate.unknown_11 = 1;
	return mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_ACTIVATE_BSS,
			       &activate, sizeof(activate), false);
}

int mt7932_bss_update(struct mt792x_dev *dev, struct ieee80211_vif *vif)
{
	struct mt792x_vif *mvif = (void *)vif->drv_priv;
	struct ieee80211_bss_conf *info = &vif->bss_conf;
	const struct cfg80211_chan_def *ch;
	u8 bss[116] = {};
	int ret;

	ch = mvif->bss_conf.mt76.ctx ? &mvif->bss_conf.mt76.ctx->def :
		&dev->mphy.chandef;
	if (!ch->chan)
		return -EINVAL;
	bss[0] = mvif->bss_conf.mt76.idx;
	/* Legacy ENUM_PARAM_MEDIA_STATE: connected=0, disconnected=1. */
	bss[1] = !vif->cfg.assoc;
	bss[3] = vif->cfg.ssid_len;
	memcpy(bss + 4, vif->cfg.ssid, bss[3]);
	if (info->bssid)
		memcpy(bss + 0x24, info->bssid, ETH_ALEN);
	bss[0x2a] = info->qos;
	bss[0x2b] = 1;
	put_unaligned_le16(ch->chan->band == NL80211_BAND_2GHZ ? 0xfff : 0xff0,
			   bss + 0x2c);
	put_unaligned_le16(info->basic_rates <<
			   (ch->chan->band == NL80211_BAND_2GHZ ? 0 : 4), bss + 0x2e);
	bss[0x30] = mvif->wep_sta ? mvif->wep_sta->deflink.wcid.idx + 2 : 0xfe;
	bss[0x34] = ch->chan->band == NL80211_BAND_2GHZ ? 1 : 2;
	bss[0x37] = 0x7f;
	bss[0x3a] = 7;
	ret = mt7932_fill_rlm(&dev->phy, ch, bss[0], (void *)(bss + 0x44));
	if (ret)
		return ret;
	bss[0x5b] = mvif->bss_conf.mt76.wmm_idx;
	bss[0x5d] = hweight8(dev->mphy.antenna_mask);
	memcpy(bss + 0x60, &info->he_oper.params, 3);
	bss[0x63] = info->he_bss_color.color;
	put_unaligned_le16(info->he_oper.nss_set, bss + 0x64);
	dev_info(dev->mt76.dev, "MT7932 BSS bss=%u media_state=%u ap=%u bmc=%u band=%u channel=%u\n",
		 bss[0], bss[1], bss[0x30], bss[0x3a], bss[0x45], bss[0x46]);
	ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SET_RLM, bss + 0x44, 22, false);
	if (ret)
		return ret;
	return mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SET_BSS, bss, sizeof(bss), false);
}

int mt7932_sta_update(struct mt792x_dev *dev, struct ieee80211_sta *sta,
		     struct ieee80211_vif *vif, bool enable,
		     enum mt76_sta_info_state state)
{
	struct mt792x_vif *mvif = (void *)vif->drv_priv;
	struct mt792x_sta *msta;
	struct ieee80211_link_sta *link;
	u8 body[200] = {};
	int idx, band, shift;

	if (!sta)
		return mt7932_bss_update(dev, vif);
	msta = (void *)sta->drv_priv;
	link = &sta->deflink;
	idx = msta->deflink.wcid.idx;
	if (idx > 12)
		return -ENOSPC;
	if (!enable) {
		u8 remove[5] = { 0, idx + 2, mvif->bss_conf.mt76.idx };

		return mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_REMOVE_STA, remove, sizeof(remove), false);
	}
	band = mvif->bss_conf.mt76.ctx ? mvif->bss_conf.mt76.ctx->def.chan->band :
		NL80211_BAND_2GHZ;
	shift = band == NL80211_BAND_2GHZ ? 0 : 4;
	body[0] = idx + 2;
	body[1] = mvif->bss_conf.mt76.idx ? 0x42 : 0x41;
	memcpy(body + 2, sta->addr, ETH_ALEN);
	put_unaligned_le16(sta->aid, body + 8);
	put_unaligned_le16(dev->mphy.hw->conf.listen_interval, body + 10);
	body[0xc] = mvif->bss_conf.mt76.idx;
	body[0xd] = band == NL80211_BAND_2GHZ ? 7 : 8;
	put_unaligned_le16(link->supp_rates[band] << shift, body + 0xe);
	put_unaligned_le16(vif->bss_conf.basic_rates << shift, body + 0x10);
	body[0x12] = sta->wme;
	body[0x14] = state == MT76_STA_INFO_STATE_ASSOC ? 2 : 0;
	body[0x17] = 1;
	if (link->ht_cap.ht_supported) {
		body[0xd] |= BIT(4);
		body[0x15] = link->ht_cap.mcs.rx_mask[0];
		memcpy(body + 0x18, link->ht_cap.mcs.rx_mask, 10);
		memcpy(body + 0x22, &link->ht_cap.mcs.rx_highest, 2);
		memcpy(body + 0x24, &link->ht_cap.mcs.tx_params, 1);
		put_unaligned_le16(link->ht_cap.cap, body + 0x28);
		body[0x30] = link->ht_cap.ampdu_factor | link->ht_cap.ampdu_density << 2;
	}
	body[0x32] = to_rcpi(-ewma_rssi_read(&mvif->bss_conf.rssi));
	body[0x33] = body[0x14] == 2;
	body[0x36] = idx;
	body[0x37] = 0xff;
	if (link->vht_cap.vht_supported) {
		body[0xd] |= BIT(5);
		put_unaligned_le32(link->vht_cap.cap, body + 0x38);
		memcpy(body + 0x3c, &link->vht_cap.vht_mcs, 8);
	}
	body[0x44] = 3;
	put_unaligned_le16(0xffff, body + 0x60);
	body[0x7d] = 1;
	if (link->he_cap.has_he) {
		body[0xd] |= BIT(6);
		memcpy(body + 0x88, link->he_cap.he_cap_elem.mac_cap_info, 6);
		memcpy(body + 0x8e, link->he_cap.he_cap_elem.phy_cap_info, 11);
		memcpy(body + 0x9c, &link->he_cap.he_mcs_nss_supp, 12);
		memcpy(body + 0xa8, &link->he_6ghz_capa.capa, 2);
	}
	dev_info(dev->mt76.dev, "MT7932 STA sta=%u wcid=%u bss=%u state=%u\n",
		 body[0], body[0x36], body[0xc], body[0x14]);
	return mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_SET_STA, body, sizeof(body), body[0x33]);
}

int mt7932_set_key(struct mt792x_dev *dev, struct ieee80211_vif *vif,
		   struct ieee80211_sta *sta, struct ieee80211_key_conf *key,
		   enum set_key_cmd cmd)
{
	struct mt792x_vif *mvif = (void *)vif->drv_priv;
	struct mt792x_sta *msta = sta ? (void *)sta->drv_priv : &mvif->sta;
	struct mt76_wcid *wcid = &msta->deflink.wcid;
	u8 body[64] = {};
	int ret;

	if (key->cipher != WLAN_CIPHER_SUITE_CCMP || key->keylen != 16)
		return -EOPNOTSUPP;
	body[0] = cmd == SET_KEY;
	body[1] = !!(key->flags & IEEE80211_KEY_FLAG_PAIRWISE);
	body[2] = body[1];
	if (sta)
		memcpy(body + 4, sta->addr, ETH_ALEN);
	else
		eth_broadcast_addr(body + 4);
	body[10] = mvif->bss_conf.mt76.idx;
	body[11] = 4;
	body[12] = key->keyidx;
	body[13] = key->keylen;
	body[14] = wcid->idx;
	memcpy(body + 16, key->key, key->keylen);
	mt792x_mutex_acquire(dev);
	ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_ADD_REMOVE_KEY, body, sizeof(body), false);
	if (!ret) {
		wcid->hw_key_idx = cmd == SET_KEY ? key->keyidx : -1;
		mt76_wcid_key_setup(&dev->mt76, wcid, cmd == SET_KEY ? key : NULL);
	}
	mt792x_mutex_release(dev);
	memzero_explicit(body, sizeof(body));
	return ret;
}

int mt7932_load_calibration(struct mt792x_dev *dev)
{
	const struct mt7932_cal_file_header *header;
	const struct mt7932_cal_profile_reply *profile;
	const struct firmware *fw;
	struct sk_buff *reply;
	struct mt7932_cal_profile_query query = {
		.selector = MT7932_CAL_PROFILE_SELECTOR,
	};
	char name[80];
	size_t pos;
	u32 count, i;
	int ret;

	ret = mt7932_basic_config(&dev->mt76);
	if (ret)
		return ret;
	snprintf(name, sizeof(name), "mediatek/mt7932/calibration-%pm.bin",
		 dev->mphy.macaddr);
	ret = request_firmware(&fw, name, dev->mt76.dev);
	if (ret)
		return ret;
	ret = -EINVAL;
	header = (const void *)fw->data;
	if (fw->size < sizeof(*header) || fw->size > MT7932_CAL_MAX_FILE_SIZE ||
	    memcmp(header->magic, MT7932_CAL_MAGIC, sizeof(header->magic)) ||
	    !ether_addr_equal(header->mac, dev->mphy.macaddr))
		goto out;
	count = le32_to_cpu(header->count);
	if (!count || count > MT7932_CAL_MAX_RECORDS)
		goto out;
	/* Validate the entire replay before provisioning this individual board. */
	for (pos = sizeof(*header), i = 0; i < count; i++) {
		const struct mt7932_cal_file_record *record;
		u32 size;

		if (pos > fw->size || fw->size - pos < sizeof(*record))
			goto out;
		record = (const void *)(fw->data + pos);
		size = le32_to_cpu(record->len);
		pos += sizeof(*record);
		if (!size || size > MT7932_CAL_MAX_PAYLOAD || size > fw->size - pos ||
		    record->set_query > 1 || record->wait_response > 1)
			goto out;
		if (record->cid == FIELD_GET(__MCU_CMD_FIELD_ID, MT7932_CMD_CAL_EXT_21)) {
			const struct mt7932_cal_ext_fragment *fragment = (const void *)record->data;

			if (record->ext_cid != FIELD_GET(__MCU_CMD_FIELD_EXT_ID,
							 MT7932_CMD_CAL_EXT_21) ||
			    size < sizeof(*fragment) ||
			    size > sizeof(*fragment) + MT7932_CAL_EXT_MAX_DATA ||
			    fragment->type != MT7932_CAL_EXT_FRAGMENT ||
			    le16_to_cpu(fragment->len) != size - sizeof(*fragment))
				goto out;
		} else if ((record->cid != MT7932_CMD_PRECAL &&
			    record->cid != MT7932_CMD_CAL_D7) || record->ext_cid) {
			goto out;
		}
		pos += size;
	}
	if (pos != fw->size)
		goto out;
	ret = mt76_mcu_send_and_get_msg(&dev->mt76,
			MT7932_CMD_CAL_PROFILE | __MCU_CMD_FIELD_QUERY,
			&query, sizeof(query), true, &reply);
	if (ret)
		goto out;
	profile = (const void *)reply->data;
	if (reply->len < sizeof(*profile) || profile->cid != MT7932_CMD_CAL_PROFILE) {
		dev_err(dev->mt76.dev, "MT7932 calibration-type query invalid (len %u)\n",
			 reply->len);
		ret = -EINVAL;
	} else {
		dev_info(dev->mt76.dev, "MT7932 calibration profile: %*ph\n",
			 (int)sizeof(*profile), reply->data);
	}
	dev_kfree_skb(reply);
	if (ret)
		goto out;
	for (pos = sizeof(*header), i = 0; i < count; i++) {
		const struct mt7932_cal_file_record *record = (const void *)(fw->data + pos);
		u32 size = le32_to_cpu(record->len);
		int cmd = FIELD_PREP(__MCU_CMD_FIELD_ID, record->cid) |
			  FIELD_PREP(__MCU_CMD_FIELD_EXT_ID, record->ext_cid);

		if (!record->set_query)
			cmd |= __MCU_CMD_FIELD_QUERY;
		ret = mt76_mcu_send_msg(&dev->mt76, cmd, record->data, size,
				      !record->set_query || record->wait_response);
		if (ret)
			goto out;
		pos += sizeof(*record) + size;
	}
	dev_info(dev->mt76.dev, "MT7932 board calibration uploaded (%u records)\n", count);
out:
	release_firmware(fw);
	return ret;
}

int mt7932_mcu_send_message(struct mt76_dev *mdev, struct sk_buff *skb,
			  int cmd, int *wait_seq)
{
	struct mt792x_dev *dev = container_of(mdev, struct mt792x_dev, mt76);
	struct mt76_connac2_mcu_txd *hdr;
	u8 seq = ++mdev->mcu.msg_seq;
	bool running = test_bit(MT76_STATE_MCU_RUNNING, &dev->mphy.state);

	if (!seq)
		seq = ++mdev->mcu.msg_seq;
	if (wait_seq)
		*wait_seq = seq;
	mdev->mcu.timeout = 6 * HZ;
	if (cmd == MCU_CMD(FW_SCATTER))
		return mt76_tx_queue_skb_raw(dev, mdev->q_mcu[MT_MCUQ_FWDL], skb, 0);
	/* MT7932 uses legacy flat commands; do not send CONNAC UNI TLVs. */
	if (cmd & __MCU_CMD_FIELD_UNI) {
		dev_err(mdev->dev, "MT7932 unsupported UNI command %#x\n", cmd);
		dev_kfree_skb(skb);
		return -EOPNOTSUPP;
	}
	if (running && cmd == MCU_CE_CMD(SET_CHAN_DOMAIN)) {
		struct mt7932_channel_domain_input *input = (void *)skb->data;
		struct mt7932_channel_domain *domain = (void *)skb->data;
		struct mt7932_domain_channel *channels;
		unsigned int count, i;

		if (skb->len < sizeof(*input)) {
			dev_kfree_skb(skb);
			return -EINVAL;
		}
		count = input->n_2ch + input->n_5ch + input->n_6ch;
		if (skb->len != sizeof(*domain) + sizeof(*channels) * count) {
			dev_kfree_skb(skb);
			return -EINVAL;
		}
		if (domain->alpha2[0] == '0' && domain->alpha2[1] == '0') {
			domain->alpha2[0] = 'X';
			domain->alpha2[1] = 'Z';
		}
		domain->unknown_policy4 = cpu_to_le16(1);
		domain->unknown_policy6 = dev->mphy.cap.has_6ghz ? 2 : 0;
		domain->flags = (mdev->region ? MT7932_DOMAIN_HAS_DFS_REGION |
				FIELD_PREP(MT7932_DOMAIN_DFS_REGION, mdev->region) : 0) |
				(input->n_6ch ? MT7932_DOMAIN_HAS_6GHZ : 0);
		domain->unknown_fcc_policy = mdev->region == NL80211_DFS_FCC ? 0x0f : 0;
		channels = (void *)(domain + 1);
		for (i = 0; i < count; i++) {
			u32 flags = le32_to_cpu(channels[i].flags);

			flags = (flags & IEEE80211_CHAN_NO_IR ? MT7932_CHANNEL_PASSIVE : 0) |
				(flags & IEEE80211_CHAN_RADAR ? MT7932_CHANNEL_DFS : 0);
			channels[i].flags = cpu_to_le32(flags);
		}
	}

	if (running && cmd == MCU_CE_CMD(START_HW_SCAN)) {
		struct mt76_connac_hw_scan_req *req = (void *)skb->data;
		struct mt7932_scan_request *native;
		struct mt76_connac_hw_scan_req *common;
		struct sk_buff *wire;

		if (skb->len != sizeof(*req)) {
			dev_kfree_skb(skb);
			return -EINVAL;
		}
		wire = mt76_mcu_msg_alloc(mdev, NULL, sizeof(*native));
		if (!wire) {
			dev_kfree_skb(skb);
			return -ENOMEM;
		}
		native = skb_put_zero(wire, sizeof(*native));
		common = (void *)native->common;
		memcpy(native->common, req, sizeof(native->common));
		memcpy(native->ext_ssids, req->ext_ssids, sizeof(native->ext_ssids));
		ether_addr_copy(native->bssid, req->bssid);
		ether_addr_copy(native->random_mac, req->random_mac);
		/* No split scan or DBDC policy on this single-radio setup. */
		common->scan_func &= ~SCAN_FUNC_SPLIT_SCAN;
		common->scan_func |= MT7932_SCAN_DISABLE_DBDC;
		common->probe_req_num = 0;
		common->version = 1;
		/* Receive beacons while the active-probe ABI is brought up. */
		common->scan_type = 0;
		if (req->ssid_type_ext && common->ssids_num)
			common->ssid_type = MT7932_SCAN_SPECIFIED_SSIDS;
		/* Firmware generates rate and PHY capability IEs itself. Keep the
		 * caller's additional IEs without duplicating those capabilities.
		 */
		{
			u8 *ies = common->ies;
			unsigned int len = le16_to_cpu(common->ies_len);
			unsigned int pos = 0, out = 0;

			if (len > sizeof(common->ies))
				goto invalid_scan;
			while (pos < len) {
				unsigned int size;
				u8 id;
				bool generated;

				if (len - pos < 2)
					goto invalid_scan;
				id = ies[pos];
				size = 2 + ies[pos + 1];
				if (size > len - pos)
					goto invalid_scan;
				generated = id == WLAN_EID_SSID ||
					id == WLAN_EID_SUPP_RATES ||
					id == WLAN_EID_EXT_SUPP_RATES ||
					id == WLAN_EID_HT_CAPABILITY ||
					id == WLAN_EID_VHT_CAPABILITY ||
					id == WLAN_EID_EXT_CAPABILITY ||
					id == WLAN_EID_SUPPORTED_REGULATORY_CLASSES ||
					(id == WLAN_EID_EXTENSION && size > 2 &&
					 (ies[pos + 2] == WLAN_EID_EXT_HE_CAPABILITY ||
					  ies[pos + 2] == WLAN_EID_EXT_HE_6GHZ_CAPA));
				if (!generated) {
					memmove(ies + out, ies + pos, size);
					out += size;
				}
				pos += size;
			}
			memset(ies + out, 0, sizeof(common->ies) - out);
			common->ies_len = cpu_to_le16(out);
		}
		goto scan_valid;
invalid_scan:
		dev_kfree_skb(wire);
		dev_kfree_skb(skb);
		return -EINVAL;
scan_valid:
		dev_info(mdev->dev, "MT7932 scan seq=%u bss=%u type=%u mask=%#x channels=%u+%u band=%u\n",
			 common->seq_num, common->bss_idx, common->scan_type,
			 common->scan_func, common->channels_num,
			 common->ext_channels_num, common->channel_type);
		common->channel_dwell_time = cpu_to_le16(MT7932_SCAN_DWELL_MS);
		common->timeout_value = cpu_to_le16(MT7932_SCAN_FW_TIMEOUT_MS);
		common->channel_min_dwell_time = cpu_to_le16(MT7932_SCAN_MIN_DWELL_MS);
		dev_kfree_skb(skb);
		skb = wire;
	}
	if (running && cmd == MCU_CE_CMD(SET_RATE_TX_POWER)) {
		struct mt76_connac_tx_power_limit_tlv *req = (void *)skb->data;
		struct mt76_connac_sku_tlv *rates;
		struct mt76_connac_tx_power_limit_tlv *header;
		struct mt7932_power_channel *channels;
		struct sk_buff *wire;
		int i, j, len;

		if (skb->len < sizeof(*req) || req->n_chan > MT7932_POWER_MAX_CHANNELS ||
		    skb->len != sizeof(*req) + req->n_chan * sizeof(*rates)) {
			dev_kfree_skb(skb);
			return -EINVAL;
		}
		len = sizeof(*header) + sizeof(*channels) * req->n_chan;
		wire = mt76_mcu_msg_alloc(mdev, NULL, len);
		if (!wire) {
			dev_kfree_skb(skb);
			return -ENOMEM;
		}
		header = skb_put_zero(wire, len);
		memcpy(header, req, sizeof(*header));
		header->ver = MT7932_POWER_WIRE_VERSION;
		header->pad0 = MT7932_POWER_WIRE_FORMAT;
		header->len = cpu_to_le16(len);
		channels = (void *)(header + 1);
		rates = (void *)(skb->data + sizeof(*req));
		for (i = 0; i < req->n_chan; i++) {
			s8 limit = MT7932_POWER_CEILING_HALF_DBM;

			for (j = 0; j < ARRAY_SIZE(rates[i].pwr_limit); j++)
				limit = min(limit, rates[i].pwr_limit[j]);
			channels[i].channel = rates[i].channel;
			memset(channels[i].rate_limit, limit, sizeof(channels[i].rate_limit));
		}
		dev_kfree_skb(skb);
		skb = wire;
	}
	hdr = (void *)skb_push(skb, sizeof(*hdr));
	memset(hdr, 0, sizeof(*hdr));
	hdr->txd[0] = cpu_to_le32(FIELD_PREP(MT_TXD0_TX_BYTES, skb->len) |
				FIELD_PREP(MT_TXD0_PKT_FMT, MT_TX_TYPE_CMD));
	hdr->txd[1] = cpu_to_le32(FIELD_PREP(MT_TXD1_HDR_FORMAT, MT_HDR_FORMAT_CMD));
	hdr->len = cpu_to_le16(skb->len - sizeof(hdr->txd));
	hdr->cid = FIELD_GET(__MCU_CMD_FIELD_ID, cmd);
	hdr->pkt_type = MCU_PKT_ID;
	hdr->seq = seq;
	if (running) {
		hdr->ext_cid = FIELD_GET(__MCU_CMD_FIELD_EXT_ID, cmd);
		hdr->set_query = !(cmd & __MCU_CMD_FIELD_QUERY) &&
				 hdr->cid != MCU_CE_CMD_GET_NIC_CAPAB;
		hdr->ext_cid_ack = !!wait_seq;
	}
	return mt76_tx_queue_skb_raw(dev, mdev->q_mcu[MT_MCUQ_WM], skb, 0);
}
EXPORT_SYMBOL_GPL(mt7932_mcu_send_message);

int mt7932_mcu_parse_response(struct mt76_dev *mdev, int cmd,
			    struct sk_buff *skb, int seq)
{
	struct mt792x_dev *dev = container_of(mdev, struct mt792x_dev, mt76);
	const struct mt76_connac2_mcu_rxd *rxd;
	u8 cid = FIELD_GET(__MCU_CMD_FIELD_ID, cmd);
	u16 type;

	if (!skb) {
		dev_err(mdev->dev, "MT7932 command %#x seq %u timed out\n", cmd, seq);
		return -ETIMEDOUT;
	}
	if (cid == MCU_CMD_PATCH_SEM_CONTROL || cid == MCU_CE_CMD_GET_NIC_CAPAB)
		dev_info(mdev->dev, "MT7932 CID %#x reply len=%u: %*ph\n", cid,
			 skb->len, min_t(unsigned int, skb->len, 48), skb->data);
	if (skb->len < MT7932_BOOT_HEADER_SIZE)
		return -EINVAL;
	rxd = (const void *)skb->data;
	if (rxd->seq != seq)
		return -EAGAIN;
	type = le16_to_cpu(rxd->pkt_type_id);
	/* The Neo ROM emits packet type7 in its eight-byte event header. */
	if (type == MT7932_BOOT_PACKET_TYPE || (type == MT7932_ROM_PACKET_TYPE &&
	    !test_bit(MT76_STATE_MCU_RUNNING, &dev->mphy.state))) {
		u8 expected_eid = cid == MCU_CMD_PATCH_SEM_CONTROL ? MCU_EVENT_MT_PATCH_SEM :
				  cid == MCU_CMD_INIT_ACCESS_REG ? MCU_EVENT_ACCESS_REG :
								  MCU_EVENT_GENERIC;
		u8 status;

		/* Preserve the existing four-byte minimum boot reply payload. */
		if (skb->len < MT7932_BOOT_HEADER_SIZE + sizeof(__le32))
			return -EINVAL;
		if (rxd->eid != expected_eid)
			return -EAGAIN;
		status = skb->data[MT7932_BOOT_HEADER_SIZE];
		skb_pull(skb, MT7932_BOOT_HEADER_SIZE);
		if (cid == MCU_CMD_PATCH_SEM_CONTROL) {
			dev_info(mdev->dev, "MT7932 patch semaphore status %u\n", status);
			return status;
		}
		if (cid == MCU_CMD_INIT_ACCESS_REG)
			return 0;
		if (status != MT7932_BOOT_SUCCESS)
			dev_err(mdev->dev, "MT7932 boot CID %#x status %u\n", cid, status);
		return status == MT7932_BOOT_SUCCESS ? 0 : -EIO;
	}
	if (type != MT7932_RUNTIME_PACKET_TYPE || skb->len < sizeof(*rxd))
		return -EINVAL;
	if (cmd == MCU_CE_QUERY(REG_READ)) {
		const struct mt7932_register_result *result = (const void *)rxd->tlv;

		if (skb->len < sizeof(*rxd) + sizeof(*result))
			return -EINVAL;
		return (int)le32_to_cpu(result->value);
	}
	/* A generic result includes the CID; reject unrelated command replies. */
	if (rxd->eid == MT7932_EVENT_COMMAND_RESULT) {
		const struct mt7932_command_result *result = (const void *)rxd->tlv;

		if (skb->len < sizeof(*rxd) + sizeof(*result))
			return -EINVAL;
		if (result->cid != cid)
			return -EAGAIN;
		if (result->status)
			return -EIO;
	}
	skb_pull(skb, sizeof(*rxd));
	return 0;
}
EXPORT_SYMBOL_GPL(mt7932_mcu_parse_response);

static int mt7932_scatter(struct mt792x_dev *dev, const u8 *data, size_t len)
{
	struct mt76_queue *q = dev->mt76.q_mcu[MT_MCUQ_FWDL];
	unsigned long expires;

	while (len) {
		size_t size = min_t(size_t, len, MT7932_FW_SCATTER_CHUNK);
		int ret;

		expires = jiffies + 6 * HZ;
		while (READ_ONCE(q->queued) + 1 >= q->ndesc - 1) {
			mt76_queue_tx_cleanup(dev, q, false);
			if (time_after(jiffies, expires)) {
				dev_err(dev->mt76.dev, "MT7932 firmware download ring stalled\n");
				return -ETIMEDOUT;
			}
			usleep_range(100, 200);
		}
		ret = mt76_mcu_send_msg(&dev->mt76, MCU_CMD(FW_SCATTER),
				       data, size, false);
		if (ret)
			return ret;
		mt76_queue_tx_cleanup(dev, q, false);
		data += size;
		len -= size;
	}
	/* Finish consuming this target before issuing the next boot command. */
	expires = jiffies + 6 * HZ;
	while (READ_ONCE(q->queued)) {
		mt76_queue_tx_cleanup(dev, q, false);
		if (time_after(jiffies, expires))
			return -ETIMEDOUT;
		usleep_range(100, 200);
	}
	return 0;
}

static int mt7932_download(struct mt792x_dev *dev, u8 cid, u32 addr,
			  const u8 *data, u32 len)
{
	struct { __le32 addr, len, mode; } req = {
		cpu_to_le32(addr), cpu_to_le32(len), cpu_to_le32(DL_MODE_NEED_RSP),
	};
	int ret = mt76_mcu_send_msg(&dev->mt76, cid, &req,
				   sizeof(req), true);

	if (ret)
		return ret;
	return mt7932_scatter(dev, data, len);
}

static int mt7932_secure_command(struct mt792x_dev *dev, u8 cid,
				 const void *data, size_t len)
{
	int ret;

	mt76_wr(dev, MT7932_PCIE_REMAP(0), MT7932_PCIE_REMAP_BOOT);
	udelay(MT7932_PCIE_REMAP_SETTLE_US);
	if (!mt76_poll_msec(dev, MT7932_SECURE_READY, MT7932_SECURE_READY_BIT,
			    MT7932_SECURE_READY_BIT, 5000)) {
		ret = -ETIMEDOUT;
		goto restore;
	}
	ret = mt76_mcu_send_msg(&dev->mt76, cid, data, len, true);
	mt76_wr(dev, MT7932_SECURE_COMPLETE, MT7932_SECURE_COMPLETE_BIT);
restore:
	mt76_wr(dev, MT7932_PCIE_REMAP(0), MT7932_PCIE_REMAP_RUNTIME);
	return ret;
}

static int mt7932_load_patch(struct mt792x_dev *dev)
{
	const struct mt76_connac2_patch_hdr *hdr;
	const struct firmware *fw;
	u8 sem[4] = { MT7932_PATCH_SEM_REQUEST };
	u32 n, i, wifi_sections = 0;
	int ret, tries;

	for (tries = 0; tries < 50; tries++) {
		ret = mt76_mcu_send_msg(&dev->mt76, MCU_CMD(PATCH_SEM_CONTROL),
					sem, sizeof(sem), true);
		if (ret != MT7932_PATCH_SEM_BUSY)
			break;
		msleep(100);
	}
	if (ret == MT7932_PATCH_ALREADY_LOADED)
		return 0;
	if (ret != MT7932_PATCH_SEM_ACQUIRED)
		dev_err(dev->mt76.dev, "MT7932 patch semaphore request failed: %d\n", ret);
	if (ret != MT7932_PATCH_SEM_ACQUIRED)
		return ret < 0 ? ret : -EIO;
	ret = request_firmware(&fw, MT7932_ROM_PATCH, dev->mt76.dev);
	if (ret)
		return ret;
	ret = -EINVAL;
	if (fw->size < sizeof(*hdr))
		goto out;
	hdr = (const void *)fw->data;
	n = be32_to_cpu(hdr->desc.n_region);
	if (!n || n > (fw->size - sizeof(*hdr)) / sizeof(struct mt76_connac2_patch_sec))
		goto out;
	/* Reject malformed sections before sending any of the patch. */
	for (i = 0; i < n; i++) {
		const struct mt76_connac2_patch_sec *sec =
			(const void *)(fw->data + sizeof(*hdr) + i * sizeof(*sec));
		u32 off = be32_to_cpu(sec->offs), len = be32_to_cpu(sec->info.len);

		if (get_unaligned_be32(&sec->info.rsv[0]) != MT7932_PATCH_WIFI_SECTION)
			continue;
		wifi_sections++;
		if (!len || off < sizeof(*hdr) + n * sizeof(*sec) ||
		    off > fw->size || len > fw->size - off ||
		    len > be32_to_cpu(sec->size))
			goto out;
		if (be32_to_cpu(sec->info.sec_key_idx) != PATCH_SEC_NOT_SUPPORT) {
			ret = -EOPNOTSUPP;
			goto out;
		}
	}
	if (!wifi_sections)
		goto out;
	for (i = 0; i < n; i++) {
		const struct mt76_connac2_patch_sec *sec =
			(const void *)(fw->data + sizeof(*hdr) + i * sizeof(*sec));
		u32 off = be32_to_cpu(sec->offs), len = be32_to_cpu(sec->info.len);

		if (get_unaligned_be32(&sec->info.rsv[0]) != MT7932_PATCH_WIFI_SECTION)
			continue;
		ret = mt7932_download(dev, MCU_CMD_PATCH_START_REQ,
				      be32_to_cpu(sec->info.addr),
				      fw->data + off, len);
		if (ret)
			goto out;
	}
	{
		u8 finish[4] = {};
		ret = mt7932_secure_command(dev, MCU_CMD_PATCH_FINISH_REQ,
					    finish, sizeof(finish));
	}
out:
	release_firmware(fw);
	return ret;
}

static int mt7932_load_ram(struct mt792x_dev *dev)
{
	const struct mt76_connac2_fw_trailer *trailer;
	const struct mt76_connac2_fw_region *regions;
	const struct firmware *fw;
	struct { __le32 option, addr; } start = {};
	size_t data_size, off = 0;
	int ret, i;

	ret = request_firmware(&fw, MT7932_FIRMWARE_WM, dev->mt76.dev);
	if (ret)
		return ret;
	ret = -EINVAL;
	if (fw->size < sizeof(*trailer))
		goto out;
	trailer = (const void *)(fw->data + fw->size - sizeof(*trailer));
	if (!trailer->n_region || trailer->n_region > MT7932_FW_MAX_REGIONS ||
	    fw->size < sizeof(*trailer) + trailer->n_region * sizeof(*regions))
		goto out;
	data_size = fw->size - sizeof(*trailer) - trailer->n_region * sizeof(*regions);
	regions = (const void *)(fw->data + data_size);
	/* Validate every region before issuing the first download command. */
	for (i = 0; i < trailer->n_region; i++) {
		u32 len = le32_to_cpu(regions[i].len);
		if (off > data_size || len > data_size - off)
			goto out;
		if (regions[i].feature_set & FW_FEATURE_SET_ENCRYPT) {
			ret = -EOPNOTSUPP;
			goto out;
		}
		off += len;
	}
	off = 0;
	for (i = 0; i < trailer->n_region; i++) {
		u32 len = le32_to_cpu(regions[i].len);
		if (!(regions[i].feature_set & FW_FEATURE_NON_DL)) {
			ret = mt7932_download(dev, MCU_CMD_TARGET_ADDRESS_LEN_REQ,
					      le32_to_cpu(regions[i].addr),
					      fw->data + off, len);
			if (ret)
				goto out;
		}
		if (regions[i].feature_set & FW_FEATURE_OVERRIDE_ADDR) {
			start.option = cpu_to_le32(FW_START_OVERRIDE);
			start.addr = regions[i].addr;
		}
		off += len;
	}
	ret = mt7932_secure_command(dev, MCU_CMD_FW_START_REQ, &start, sizeof(start));
out:
	release_firmware(fw);
	return ret;
}

int mt7932_load_firmware(struct mt792x_dev *dev)
{
	struct pci_dev *pdev = to_pci_dev(dev->mt76.dev);
	u32 state;
	int ret, i;

	mt7932_fullmac_reset(dev);
	clear_bit(MT76_STATE_MCU_RUNNING, &dev->mphy.state);
	ret = mt7932_publish_shared_memory(dev);
	if (ret)
		return ret;
	ret = mt7932_load_patch(dev);
	if (ret)
		return ret;
	ret = mt7932_load_ram(dev);
	if (ret)
		return ret;
	for (i = 0; i < 1000; i++) {
		ret = pci_read_config_dword(pdev, MT7932_PCIE_FW_STATE, &state);
		if (ret || state == ~0U)
			return -EIO;
		if (FIELD_GET(MT7932_PCIE_FW_STATE_MASK, state) ==
		    MT7932_PCIE_FW_STATE_READY) {
			set_bit(MT76_STATE_MCU_RUNNING, &dev->mphy.state);
			dev_info(dev->mt76.dev, "MT7932 firmware ready\n");
			return mt76_mcu_send_msg(&dev->mt76, __MCU_CMD_FIELD_CE, NULL, 0, false);
		}
		msleep(5);
	}
	return -ETIMEDOUT;
}
MODULE_FIRMWARE(MT7932_FIRMWARE_WM);
MODULE_FIRMWARE(MT7932_ROM_PATCH);
