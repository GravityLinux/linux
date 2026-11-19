/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __MT7932_FULLMAC_H
#define __MT7932_FULLMAC_H

struct mt7932_fullmac {
	struct mt792x_dev *dev;
	struct net_device *netdev;
	struct wiphy *wiphy;
	struct wireless_dev wdev;
	struct cfg80211_scan_request *scan_request;
	unsigned int scan_next_channel;
	struct wiphy_delayed_work scan_timeout;
	struct wiphy_delayed_work connect_timeout;
	bool connecting, connected, disconnecting, active, scan_context_active;
	bool reset_pending, removing;
	bool signal_valid;
	s8 last_signal;
	unsigned long connected_at, last_activity;
	struct rtnl_link_stats64 station_start_stats;
	u16 disconnect_reason;
	u8 scan_seq, peer_wcid, cal_band;
	u32 cal_center_khz;
	bool cal_applied;
	u8 bssid[ETH_ALEN];
	struct mt7932_cal_cache *cal;
	struct sk_buff_head events;
	struct wiphy_work event_work;
};

int mt7932_cfg80211_register(struct mt7932_fullmac *fm);
void mt7932_cfg80211_unregister(struct mt7932_fullmac *fm);
void mt7932_cfg80211_scan_done(struct mt7932_fullmac *fm, bool aborted);
void mt7932_cfg80211_scan_complete(struct mt7932_fullmac *fm, u8 sequence);
void mt7932_cfg80211_connected(struct mt7932_fullmac *fm);
void mt7932_cfg80211_disconnected(struct mt7932_fullmac *fm, u16 reason);
#endif
