// SPDX-License-Identifier: GPL-2.0-only
/* Native MT7932 Ethernet transport. cfg80211 control is under bring-up. */
#include <linux/etherdevice.h>
#include <linux/unaligned.h>
#include "mt7921.h"
#include "mt7932_offload.h"
#include "../mt76_connac2_mac.h"

#include "mt7932_fullmac.h"
#include "mt7932_mcu.h"

#define MT7932_EVENT_QUEUE_MAX 64
#define MT7932_TX_RESERVE 8

static void mt7932_fullmac_event_work(struct wiphy *wiphy, struct wiphy_work *work)
{
	struct mt7932_fullmac *fm = container_of(work, struct mt7932_fullmac, event_work);
	struct sk_buff *skb;

	if (READ_ONCE(fm->removing)) {
		skb_queue_purge(&fm->events);
		return;
	}
	if (xchg(&fm->reset_pending, false)) {
		mt7932_cfg80211_scan_done(fm, true);
		mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
		skb_queue_purge(&fm->events);
	}
	while ((skb = skb_dequeue(&fm->events))) {
		const struct mt76_connac2_mcu_rxd *rxd = (void *)skb->data;
		const struct mt7932_mlme_event *event = (void *)rxd->tlv;
		const u8 *p = rxd->tlv;
		size_t plen = skb->len - sizeof(*rxd);
		u8 eid = rxd->eid;
		u32 type, len;
		int ret = 0;

		if (fm->dev->fw_assert) {
			mt7932_cfg80211_scan_done(fm, true);
			mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
			goto next;
		}
		if (eid == MCU_EVENT_SCAN_DONE) {
			if (plen)
				mt7932_cfg80211_scan_complete(fm, p[0]);
			goto next;
		}
		if (eid == MT7932_EVENT_PRECAL_REQUEST) {
			ret = mt7932_precal_request(fm->dev, fm->cal, p, plen);
			goto result;
		}
		if ((eid != MT7932_EVENT_MLME && eid != MT7932_EVENT_MLME_ALT) ||
		    plen < sizeof(*event) || le32_to_cpu(event->unknown_0))
			goto next;
		type = le32_to_cpu(event->type);
		len = le32_to_cpu(event->len);
		if (len > plen - sizeof(*event))
			goto next;
		p = event->data;
		switch (type) {
		case MT7932_MLME_CONNECTING: {
			const struct mt7932_mlme_connecting *connecting = (void *)p;
			enum nl80211_band band;

			if (!fm->connecting || len < sizeof(*connecting) ||
			    !ether_addr_equal(connecting->bssid, fm->bssid) ||
			    connecting->band < MT7932_BAND_2GHZ ||
			    connecting->band > MT7932_BAND_6GHZ)
				break;
			fm->cal_band = connecting->band;
			band = connecting->band == MT7932_BAND_2GHZ ? NL80211_BAND_2GHZ :
			       connecting->band == MT7932_BAND_5GHZ ? NL80211_BAND_5GHZ :
								     NL80211_BAND_6GHZ;
			fm->cal_center_khz = ieee80211_channel_to_frequency(
				connecting->center_channel, band) * 1000;
			fm->active = true;
			ret = mt7932_precal_preload(fm->dev, fm->cal, fm->cal_band);
			break;
		}
		case MT7932_MLME_ASSOCIATED: {
			const struct mt7932_mlme_associated *associated = (void *)p;

			if (!fm->connecting || len < sizeof(*associated) ||
			    !ether_addr_equal(associated->bssid, fm->bssid))
				break;
			if (le32_to_cpu(associated->status) ||
			    associated->peer_wcid >= MT7932_PEER_WCID_LIMIT) {
				mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
				break;
			}
			fm->peer_wcid = associated->peer_wcid;
			break;
		}
		case MT7932_MLME_AUTHORIZED: {
			const struct mt7932_mlme_authorized *authorized = (void *)p;

			if (!fm->connecting || len < sizeof(*authorized) ||
			    fm->peer_wcid >= MT7932_PEER_WCID_LIMIT ||
			    !ether_addr_equal(authorized->bssid, fm->bssid))
				break;
			if (le32_to_cpu(authorized->status)) {
				mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
				break;
			}
			ret = mt7932_precal_apply(fm->dev, fm->cal,
						 fm->cal_band, fm->cal_center_khz);
			if (!ret) {
				fm->cal_applied = true;
				mt7932_cfg80211_connected(fm);
			}
			break;
		}
		case MT7932_MLME_DISCONNECTED:
			mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
			break;
		}
result:
		if (ret) {
			wiphy_warn(fm->wiphy, "firmware event %#x failed: %d\n", eid, ret);
			mt7932_cfg80211_disconnected(fm, WLAN_REASON_UNSPECIFIED);
		}
next:
		dev_kfree_skb(skb);
	}
}

void mt7932_fullmac_connecting(struct mt792x_dev *dev, const u8 *bssid)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (fm) {
		netif_carrier_off(fm->netdev);
		WRITE_ONCE(fm->peer_wcid, MT7932_NO_WCID);
		WRITE_ONCE(fm->cal_applied, false);
		WRITE_ONCE(fm->cal_band, 0);
		WRITE_ONCE(fm->cal_center_khz, 0);
		ether_addr_copy(fm->bssid, bssid);
		/* Discovery management frames must reach the existing RX path. */
		WRITE_ONCE(fm->active, false);
	}
}

void mt7932_fullmac_reset(struct mt792x_dev *dev)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (!fm || READ_ONCE(fm->removing))
		return;
	netif_carrier_off(fm->netdev);
	WRITE_ONCE(fm->active, false);
	WRITE_ONCE(fm->peer_wcid, MT7932_NO_WCID);
	WRITE_ONCE(fm->cal_applied, false);
	WRITE_ONCE(fm->scan_context_active, false);
	WRITE_ONCE(fm->cal_band, 0);
	WRITE_ONCE(fm->cal_center_khz, 0);
	WRITE_ONCE(fm->reset_pending, true);
	wiphy_work_queue(fm->wiphy, &fm->event_work);
}

void mt7932_fullmac_reset_prepare(struct mt792x_dev *dev)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (!fm || READ_ONCE(fm->removing))
		return;
	netif_tx_disable(fm->netdev);
	mt7932_fullmac_reset(dev);
}

/* Called with the MCU mutex held after firmware reload. Restore both domain
 * and the existing conservative SAR/power programming before any new scan or
 * transmission. The firmware lost RLM state even if Linux kept its chandef.
 */
int mt7932_fullmac_reinit(struct mt792x_dev *dev)
{
	int ret;

	ret = mt76_connac_mcu_set_channel_domain(&dev->mphy);
	if (!ret)
		ret = mt7921_set_tx_sar_pwr(dev->mphy.hw, NULL);
	if (!ret && dev->mphy.chandef.chan)
		ret = mt7932_set_channel(&dev->phy, MT7932_AIS_BSS);
	return ret;
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_reinit);

void mt7932_fullmac_reset_done(struct mt792x_dev *dev, int error)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (!fm || READ_ONCE(fm->removing))
		return;
	if (error)
		dev->fw_assert = true;
	else if (netif_running(fm->netdev))
		netif_wake_queue(fm->netdev);
	dev_info(dev->mt76.dev, "MT7932 firmware recovery completed: %d\n", error);
}

int mt7932_fullmac_preload(struct mt792x_dev *dev, u8 band)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	return fm ? mt7932_precal_preload(dev, fm->cal, band) : -ENODEV;
}

static int mt7932_native_open(struct net_device *netdev)
{
	netif_start_queue(netdev);
	return 0;
}

static int mt7932_native_stop(struct net_device *netdev)
{
	netif_stop_queue(netdev);
	return 0;
}

static netdev_tx_t mt7932_native_tx(struct sk_buff *skb,
				 struct net_device *netdev)
{
	struct mt7932_fullmac *fm = *(void **)netdev_priv(netdev);
	struct mt76_phy *phy = &fm->dev->mphy;
	struct mt76_dev *mdev = &fm->dev->mt76;
	struct mt76_queue *q = phy->q_tx[MT_TXQ_BE];
	unsigned int bytes = skb->len;
	int ret;

	if (!netif_carrier_ok(netdev) || skb_linearize(skb)) {
		netdev->stats.tx_dropped++;
		dev_kfree_skb_any(skb);
		return NETDEV_TX_OK;
	}
	memset(skb->cb, 0, sizeof(skb->cb));
	IEEE80211_SKB_CB(skb)->flags = IEEE80211_TX_CTL_HW_80211_ENCAP;
	skb_set_queue_mapping(skb, 0);
	spin_lock_bh(&q->lock);
	if (q->queued >= q->ndesc - MT7932_TX_RESERVE ||
	    mdev->token_count >= mdev->token_size - MT7932_TX_RESERVE) {
		spin_unlock_bh(&q->lock);
		return NETDEV_TX_BUSY;
	}
	ret = mdev->queue_ops->tx_queue_skb(phy, q, MT_TXQ_BE, skb, NULL, NULL);
	if (ret >= 0)
		mdev->queue_ops->kick(mdev, q);
	spin_unlock_bh(&q->lock);
	if (ret >= 0) {
		netdev->stats.tx_packets++;
		netdev->stats.tx_bytes += bytes;
		WRITE_ONCE(fm->last_activity, jiffies);
	} else {
		netdev->stats.tx_errors++;
	}
	return NETDEV_TX_OK;
}

static const struct net_device_ops mt7932_native_ops = {
	.ndo_open = mt7932_native_open,
	.ndo_stop = mt7932_native_stop,
	.ndo_start_xmit = mt7932_native_tx,
	.ndo_validate_addr = eth_validate_addr,
};

int mt7932_fullmac_init(struct mt792x_dev *dev)
{
	struct mt7932_fullmac *fm;
	struct net_device *netdev;
	int ret;

	fm = kzalloc(sizeof(*fm), GFP_KERNEL);
	if (!fm)
		return -ENOMEM;
	netdev = alloc_etherdev(sizeof(fm));
	if (!netdev) {
		kfree(fm);
		return -ENOMEM;
	}
	fm->dev = dev;
	fm->netdev = netdev;
	wiphy_work_init(&fm->event_work, mt7932_fullmac_event_work);
	skb_queue_head_init(&fm->events);
	ret = mt7932_precal_load(dev, &fm->cal);
	if (ret) {
		free_netdev(netdev);
		kfree(fm);
		return ret;
	}
	/* Assigned from the firmware association event before authorizing TX. */
	fm->peer_wcid = MT7932_NO_WCID;
	*(void **)netdev_priv(netdev) = fm;
	strscpy(netdev->name, "wlan%d", IFNAMSIZ);
	netdev->netdev_ops = &mt7932_native_ops;
	SET_NETDEV_DEV(netdev, dev->mt76.dev);
	eth_hw_addr_set(netdev, dev->mphy.macaddr);
	netif_carrier_off(netdev);
	WRITE_ONCE(dev->fullmac, fm);
	ret = mt7932_cfg80211_register(fm);
	if (ret) {
		WRITE_ONCE(dev->fullmac, NULL);
		free_netdev(netdev);
		kfree(fm);
		return ret;
	}
	dev_info(dev->mt76.dev, "MT7932 cfg80211 station interface %s\n",
		 netdev->name);
	return 0;
}

bool mt7932_fullmac_is_tx(struct mt792x_dev *dev, struct sk_buff *skb)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	return fm && skb && skb->dev == fm->netdev;
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_is_tx);

void mt7932_fullmac_txwi(struct mt792x_dev *dev, __le32 *txwi,
			struct sk_buff *skb)
{
	struct mt76_wcid wcid = { .idx = dev->fullmac->peer_wcid };

	mt76_connac2_mac_write_txwi(&dev->mt76, txwi, skb, &wcid,
				  NULL, 0, MT_TXQ_BE, 0);
	/* Firmware installed the pairwise key; hardware supplies PN and sequence. */
	txwi[3] |= cpu_to_le32(MT_TXD3_PROTECT_FRAME);
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_txwi);

bool mt7932_fullmac_tx_free(struct mt792x_dev *dev, struct mt76_txwi_cache *t)
{
	if (!mt7932_fullmac_is_tx(dev, t->skb))
		return false;
	mt76_connac_txp_skb_unmap(&dev->mt76, t);
	dev_consume_skb_any(t->skb);
	t->skb = NULL;
	mt76_put_txwi(&dev->mt76, t);
	return true;
}

void mt7932_fullmac_quiesce(struct mt792x_dev *dev)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (!fm)
		return;
	wiphy_lock(fm->wiphy);
	WRITE_ONCE(fm->removing, true);
	netif_tx_disable(fm->netdev);
	wiphy_work_cancel(fm->wiphy, &fm->event_work);
	wiphy_delayed_work_cancel(fm->wiphy, &fm->scan_timeout);
	wiphy_delayed_work_cancel(fm->wiphy, &fm->connect_timeout);
	mt7932_cfg80211_scan_done(fm, true);
	mt7932_cfg80211_disconnected(fm, WLAN_REASON_DEAUTH_LEAVING);
	skb_queue_purge(&fm->events);
	wiphy_unlock(fm->wiphy);
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_quiesce);

/* Caller has stopped native TX and quiesced DMA/NAPI. The common token
 * teardown unmaps each TXP exactly once. Only detach Ethernet SKBs here,
 * so it cannot send their completions to an unregistered mac80211 device.
 */
void mt7932_fullmac_tx_purge(struct mt792x_dev *dev)
{
	struct mt76_txwi_cache *t;
	int id;

	spin_lock_bh(&dev->mt76.token_lock);
	idr_for_each_entry(&dev->mt76.token, t, id) {
		if (!mt7932_fullmac_is_tx(dev, t->skb))
			continue;
		dev_kfree_skb_any(t->skb);
		t->skb = NULL;
	}
	spin_unlock_bh(&dev->mt76.token_lock);
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_tx_purge);

void mt7932_fullmac_remove(struct mt792x_dev *dev)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);

	if (!fm)
		return;
	mt7932_fullmac_quiesce(dev);
	mt7932_cfg80211_unregister(fm);
	mt7932_fullmac_tx_purge(dev);
	mt76_wcid_cleanup(&dev->mt76, &dev->mt76.global_wcid);
	WRITE_ONCE(dev->fullmac, NULL);
	synchronize_net();
	free_netdev(fm->netdev);
	kfree(fm);
}
EXPORT_SYMBOL_GPL(mt7932_fullmac_remove);

void mt7932_fullmac_event(struct mt792x_dev *dev, struct sk_buff *skb)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);
	struct sk_buff *copy;

	if (!fm || !fm->wiphy || READ_ONCE(fm->removing) ||
	    skb->len < sizeof(struct mt76_connac2_mcu_rxd) ||
	    skb_queue_len(&fm->events) >= MT7932_EVENT_QUEUE_MAX)
		return;
	copy = skb_copy(skb, GFP_ATOMIC);
	if (!copy)
		return;
	skb_queue_tail(&fm->events, copy);
	wiphy_work_queue(fm->wiphy, &fm->event_work);
}

bool mt7932_fullmac_rx(struct mt792x_dev *dev, struct sk_buff *skb)
{
	struct mt7932_fullmac *fm = READ_ONCE(dev->fullmac);
	struct mt76_rx_status *status = (void *)skb->cb;
	unsigned int bytes;

	/* Firmware-owned Ethernet goes to the station netdev. Management RX is
	 * reported through cfg80211, including scans while associated.
	 */
	if (!fm)
		return false;
	if (!(status->flag & RX_FLAG_8023)) {
		struct ieee80211_mgmt *mgmt = (void *)skb->data;
		struct ieee80211_channel *channel;
		struct cfg80211_bss *bss;

		if (skb->len >= offsetof(struct ieee80211_mgmt, u.beacon.variable) &&
		    (ieee80211_is_beacon(mgmt->frame_control) ||
		     ieee80211_is_probe_resp(mgmt->frame_control)) &&
		    !(status->flag & (RX_FLAG_ONLY_MONITOR | RX_FLAG_FAILED_FCS_CRC))) {
			channel = ieee80211_get_channel(fm->wiphy, status->freq);
			if (channel) {
				bss = cfg80211_inform_bss_frame(fm->wiphy, channel,
					mgmt, skb->len, status->signal * 100, GFP_ATOMIC);
				cfg80211_put_bss(fm->wiphy, bss);
			}
		}
		dev_kfree_skb(skb);
		return true;
	}
	if (!READ_ONCE(fm->connected) || skb->len < ETH_HLEN ||
	    !netif_running(fm->netdev) ||
	    status->flag & (RX_FLAG_ONLY_MONITOR | RX_FLAG_FAILED_FCS_CRC)) {
		dev_kfree_skb(skb);
		return true;
	}
	WRITE_ONCE(fm->last_activity, jiffies);
	if (status->chains && status->signal < 0 && status->signal > S8_MIN) {
		WRITE_ONCE(fm->last_signal, status->signal);
		WRITE_ONCE(fm->signal_valid, true);
	}
	bytes = skb->len;
	skb->dev = fm->netdev;
	memset(skb->cb, 0, sizeof(skb->cb));
	skb->ip_summed = CHECKSUM_NONE;
	skb->protocol = eth_type_trans(skb, fm->netdev);
	fm->netdev->stats.rx_packets++;
	fm->netdev->stats.rx_bytes += bytes;
	netif_rx(skb);
	return true;
}
