// SPDX-License-Identifier: GPL-2.0-only
/* MT7932 firmware SME frontend using the standard cfg80211 station API. */
#include <linux/etherdevice.h>
#include <linux/unaligned.h>
#include "mt7921.h"
#include "regd.h"
#include "mt7932_offload.h"
#include "mt7932_fullmac.h"
#include "mt7932_mcu.h"

#define MT7932_SCAN_MAX_SSIDS 4
#define MT7932_SCAN_TIMEOUT (20 * HZ)
#define MT7932_CONNECT_TIMEOUT (45 * HZ)
#define MT7932_DISCONNECT_TIMEOUT (5 * HZ)

static struct mt7932_fullmac *mt7932_wiphy_priv(struct wiphy *wiphy)
{
	return *(struct mt7932_fullmac **)wiphy_priv(wiphy);
}

void mt7932_cfg80211_scan_done(struct mt7932_fullmac *fm, bool aborted)
{
	struct cfg80211_scan_info info = { .aborted = aborted };
	struct cfg80211_scan_request *request = fm->scan_request;

	lockdep_assert_wiphy(fm->wiphy);
	if (!request)
		return;
	fm->scan_request = NULL;
	fm->scan_next_channel = 0;
	wiphy_delayed_work_cancel(fm->wiphy, &fm->scan_timeout);
	cfg80211_scan_done(request, &info);
}

static void mt7932_cancel_scan(struct mt7932_fullmac *fm)
{
	struct {
		u8 seq_num;
		u8 is_ext_channel;
		u8 reserved[2];
	} __packed cmd = { .seq_num = fm->scan_seq & MT7932_SCAN_SEQUENCE_MASK };
	int ret;

	if (!fm->scan_request)
		return;
	mt792x_mutex_acquire(fm->dev);
	ret = mt76_mcu_send_msg(&fm->dev->mt76, MCU_CE_CMD(CANCEL_HW_SCAN),
			       &cmd, sizeof(cmd), false);
	mt792x_mutex_release(fm->dev);
	/* Same ordered cancel protocol used by mt76_connac_mcu_cancel_hw_scan.
	 * Finish once; late completions cannot finish a newer scan sequence.
	 */
	mt7932_cfg80211_scan_done(fm, true);
	if (ret)
		mt792x_reset(&fm->dev->mt76);
}

static void mt7932_scan_timeout(struct wiphy *wiphy, struct wiphy_work *work)
{
	mt7932_cancel_scan(mt7932_wiphy_priv(wiphy));
}

static void mt7932_cfg_abort_scan(struct wiphy *wiphy, struct wireless_dev *wdev)
{
	mt7932_cancel_scan(mt7932_wiphy_priv(wiphy));
}

void mt7932_cfg80211_connected(struct mt7932_fullmac *fm)
{
	lockdep_assert_wiphy(fm->wiphy);
	if (!fm->connecting || fm->disconnecting)
		return;
	fm->connecting = false;
	fm->connected = true;
	fm->connected_at = jiffies;
	WRITE_ONCE(fm->last_activity, jiffies);
	WRITE_ONCE(fm->signal_valid, false);
	dev_get_stats(fm->netdev, &fm->station_start_stats);
	wiphy_delayed_work_cancel(fm->wiphy, &fm->connect_timeout);
	cfg80211_connect_result(fm->netdev, fm->bssid, NULL, 0, NULL, 0,
				WLAN_STATUS_SUCCESS, GFP_KERNEL);
	cfg80211_port_authorized(fm->netdev, fm->bssid, NULL, 0, GFP_KERNEL);
	netif_carrier_on(fm->netdev);
}

void mt7932_cfg80211_disconnected(struct mt7932_fullmac *fm, u16 reason)
{
	lockdep_assert_wiphy(fm->wiphy);
	netif_carrier_off(fm->netdev);
	fm->active = false;
	fm->peer_wcid = MT7932_NO_WCID;
	wiphy_delayed_work_cancel(fm->wiphy, &fm->connect_timeout);
	if (fm->connecting)
		cfg80211_connect_result(fm->netdev, fm->bssid, NULL, 0, NULL, 0,
					WLAN_STATUS_UNSPECIFIED_FAILURE, GFP_KERNEL);
	else if (fm->connected)
		cfg80211_disconnected(fm->netdev,
			fm->disconnecting ? fm->disconnect_reason : reason,
			NULL, 0, fm->disconnecting, GFP_KERNEL);
	fm->connecting = false;
	fm->connected = false;
	fm->disconnecting = false;
}

static void mt7932_connect_timeout(struct wiphy *wiphy, struct wiphy_work *work)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);

	if (fm->disconnecting) {
		/* A missing firmware completion requires actual recovery, not a
		 * fabricated success notification while the radio stays joined.
		 */
		mt792x_reset(&fm->dev->mt76);
		return;
	}
	if (fm->connecting) {
		cfg80211_connect_timeout(fm->netdev, fm->bssid, NULL, 0,
			GFP_KERNEL, NL80211_TIMEOUT_UNSPECIFIED);
		fm->connecting = false;
		fm->disconnecting = true;
		fm->disconnect_reason = WLAN_REASON_UNSPECIFIED;
		if (mt7932_offload_disconnect(fm->dev, fm->bssid))
			mt792x_reset(&fm->dev->mt76);
		else
			wiphy_delayed_work_queue(wiphy, &fm->connect_timeout,
						MT7932_DISCONNECT_TIMEOUT);
	}
}

/* Discovery uses the firmware's host-owned context separately from AIS.
 * The old mac80211 add_interface callback supplied this initialization.
 */
static int mt7932_prepare_discovery(struct mt7932_fullmac *fm)
{
	struct mt792x_dev *dev = fm->dev;
	struct mt7932_activate_bss activate = {
		.bss_idx = MT7932_DISCOVERY_BSS,
		.active = 1,
		.network_type = 1, /* Host-managed P2P client role. */
		.omac_idx = MT7932_DISCOVERY_BSS,
		.bmc_wcid = MT7932_NO_WCID,
		.unknown_11 = 1,
	};
	int ret;

	if (fm->scan_context_active)
		return 0;
	ether_addr_copy(activate.mac, dev->mphy.macaddr);
	ret = mt7932_start(&dev->phy);
	if (!ret)
		ret = mt76_mcu_send_msg(&dev->mt76, MT7932_CMD_ACTIVATE_BSS,
				       &activate, sizeof(activate), false);
	if (!ret && !dev->mphy.chandef.chan) {
		struct ieee80211_supported_band *band;

		band = fm->wiphy->bands[NL80211_BAND_2GHZ];
		if (!band || !band->n_channels)
			return -ENODEV;
		cfg80211_chandef_create(&dev->mphy.chandef,
				       &band->channels[0], NL80211_CHAN_NO_HT);
		ret = mt7932_set_channel(&dev->phy, MT7932_AIS_BSS);
	}
	if (!ret)
		ret = mt7921_mcu_set_rxfilter(dev, 0, 0, 0);
	if (!ret)
		fm->scan_context_active = true;
	return ret;
}

static int mt7932_scan_batch(struct mt7932_fullmac *fm)
{
	struct cfg80211_scan_request *request = fm->scan_request;
	struct mt76_connac_hw_scan_req *cmd;
	unsigned int first = fm->scan_next_channel;
	unsigned int count, i;
	int ret;

	cmd = kzalloc(sizeof(*cmd), GFP_KERNEL);
	if (!cmd)
		return -ENOMEM;
	count = min_t(unsigned int, request->n_channels - first,
		      ARRAY_SIZE(cmd->channels) + ARRAY_SIZE(cmd->ext_channels));
	cmd->seq_num = ++fm->scan_seq & MT7932_SCAN_SEQUENCE_MASK;
	cmd->bss_idx = MT7932_DISCOVERY_BSS;
	cmd->version = 1;
	cmd->scan_type = request->n_ssids ? 1 : 0;
	cmd->ssid_type = request->n_ssids ? BIT(2) : BIT(0);
	cmd->ssids_num = request->n_ssids;
	for (i = 0; i < request->n_ssids; i++) {
		cmd->ssids[i].ssid_len = cpu_to_le32(request->ssids[i].ssid_len);
		memcpy(cmd->ssids[i].ssid, request->ssids[i].ssid,
		       request->ssids[i].ssid_len);
	}
	cmd->channel_type = 4; /* Specified channels, including each batch. */
	cmd->channels_num = min_t(unsigned int, count, ARRAY_SIZE(cmd->channels));
	cmd->ext_channels_num = count - cmd->channels_num;
	for (i = 0; i < count; i++) {
		struct ieee80211_channel *ch = request->channels[first + i];
		struct mt76_connac_mcu_scan_channel *c = i < ARRAY_SIZE(cmd->channels) ?
			&cmd->channels[i] : &cmd->ext_channels[i - ARRAY_SIZE(cmd->channels)];

		c->band = ch->band == NL80211_BAND_2GHZ ? MT7932_BAND_2GHZ :
			  ch->band == NL80211_BAND_5GHZ ? MT7932_BAND_5GHZ :
							MT7932_BAND_6GHZ;
		c->channel_num = ch->hw_value;
	}
	cmd->channel_dwell_time = cpu_to_le16(MT7932_SCAN_DWELL_MS);
	cmd->channel_min_dwell_time = cpu_to_le16(MT7932_SCAN_MIN_DWELL_MS);
	cmd->timeout_value = cpu_to_le16(MT7932_SCAN_FW_TIMEOUT_MS);
	cmd->ies_len = cpu_to_le16(request->ie_len);
	if (request->ie_len)
		memcpy(cmd->ies, request->ie, request->ie_len);
	ether_addr_copy(cmd->bssid, request->bssid);
	mt792x_mutex_acquire(fm->dev);
	ret = mt7932_prepare_discovery(fm);
	if (!ret)
		ret = mt76_mcu_send_msg(&fm->dev->mt76,
			MCU_CE_CMD(START_HW_SCAN), cmd, sizeof(*cmd), false);
	mt792x_mutex_release(fm->dev);
	kfree(cmd);
	if (!ret) {
		fm->scan_next_channel += count;
		wiphy_delayed_work_queue(fm->wiphy, &fm->scan_timeout,
					MT7932_SCAN_TIMEOUT);
	}
	return ret;
}

void mt7932_cfg80211_scan_complete(struct mt7932_fullmac *fm, u8 sequence)
{
	lockdep_assert_wiphy(fm->wiphy);
	if (!fm->scan_request || (sequence & MT7932_SCAN_SEQUENCE_MASK) !=
	    (fm->scan_seq & MT7932_SCAN_SEQUENCE_MASK))
		return;
	wiphy_delayed_work_cancel(fm->wiphy, &fm->scan_timeout);
	if (fm->scan_next_channel == fm->scan_request->n_channels)
		mt7932_cfg80211_scan_done(fm, false);
	else if (mt7932_scan_batch(fm))
		mt7932_cfg80211_scan_done(fm, true);
}

static int mt7932_cfg_scan(struct wiphy *wiphy,
			  struct cfg80211_scan_request *request)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);
	int ret;

	if (fm->scan_request || fm->connecting || fm->disconnecting)
		return -EBUSY;
	if (READ_ONCE(fm->removing) || fm->dev->fw_assert)
		return -EIO;
	if (test_bit(MT76_RESET, &fm->dev->mphy.state))
		return -EBUSY;
	if (!request->n_channels || request->n_ssids > MT7932_SCAN_MAX_SSIDS ||
	    request->ie_len > MT76_CONNAC_SCAN_IE_LEN)
		return -EINVAL;
	fm->scan_request = request;
	fm->scan_next_channel = 0;
	ret = mt7932_scan_batch(fm);
	if (ret)
		fm->scan_request = NULL;
	return ret;
}

static int mt7932_cfg_connect(struct wiphy *wiphy, struct net_device *netdev,
			     struct cfg80211_connect_params *sme)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);
	struct mt7932_connect_params *params;
	struct cfg80211_bss *bss;
	int ret;

	if (fm->connecting || fm->connected || fm->disconnecting || fm->scan_request)
		return -EBUSY;
	if (READ_ONCE(fm->removing) || fm->dev->fw_assert)
		return -EIO;
	if (test_bit(MT76_RESET, &fm->dev->mphy.state))
		return -EBUSY;
	/* Advertise and accept only the firmware security path verified so far. */
	if (sme->crypto.n_akm_suites != 1 ||
	    sme->crypto.akm_suites[0] != WLAN_AKM_SUITE_SAE ||
	    sme->crypto.n_ciphers_pairwise != 1 ||
	    sme->crypto.ciphers_pairwise[0] != WLAN_CIPHER_SUITE_CCMP ||
	    sme->crypto.cipher_group != WLAN_CIPHER_SUITE_CCMP ||
	    sme->mfp != NL80211_MFP_REQUIRED || !sme->crypto.sae_pwd ||
	    !sme->crypto.sae_pwd_len || sme->crypto.sae_pwd_len > 64)
		return -EOPNOTSUPP;
	if (!sme->ssid_len || sme->ssid_len > IEEE80211_MAX_SSID_LEN)
		return -EINVAL;
	bss = cfg80211_get_bss(wiphy, sme->channel, sme->bssid,
			      sme->ssid, sme->ssid_len, IEEE80211_BSS_TYPE_ESS,
			      IEEE80211_PRIVACY_ON);
	if (!bss)
		return -ENOENT;
	/* Temporary calibration coverage bound; never substitute another
	 * frequency's DPD coefficients. General coverage is a separate task.
	 */
	if (bss->channel->band != NL80211_BAND_6GHZ ||
	    bss->channel->hw_value != 5) {
		ret = -EOPNOTSUPP;
		goto put_bss;
	}
	params = kzalloc(sizeof(*params), GFP_KERNEL);
	if (!params) {
		ret = -ENOMEM;
		goto put_bss;
	}
	params->ssid_len = sme->ssid_len;
	memcpy(params->ssid, sme->ssid, sme->ssid_len);
	params->password_len = sme->crypto.sae_pwd_len;
	memcpy(params->password, sme->crypto.sae_pwd, params->password_len);
	params->band = MT7932_BAND_6GHZ;
	params->channel = bss->channel->hw_value;
	ether_addr_copy(params->bssid, bss->bssid);
	fm->connecting = true;
	ret = mt7932_offload_connect(fm->dev, params);
	kfree_sensitive(params);
	if (ret)
		fm->connecting = false;
	else
		wiphy_delayed_work_queue(wiphy, &fm->connect_timeout,
					MT7932_CONNECT_TIMEOUT);
put_bss:
	cfg80211_put_bss(wiphy, bss);
	return ret;
}

static int mt7932_cfg_disconnect(struct wiphy *wiphy, struct net_device *netdev,
				u16 reason)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);
	int ret;

	if (READ_ONCE(fm->removing) || fm->dev->fw_assert)
		return -EIO;
	if (test_bit(MT76_RESET, &fm->dev->mphy.state) || fm->disconnecting)
		return -EBUSY;
	if (!fm->connected && !fm->connecting)
		return -ENOTCONN;
	ret = mt7932_offload_disconnect(fm->dev, fm->bssid);
	if (ret)
		return ret;
	fm->disconnecting = true;
	fm->disconnect_reason = reason;
	wiphy_delayed_work_cancel(wiphy, &fm->connect_timeout);
	wiphy_delayed_work_queue(wiphy, &fm->connect_timeout,
				MT7932_DISCONNECT_TIMEOUT);
	return 0;
}

static int mt7932_cfg_get_station(struct wiphy *wiphy, struct wireless_dev *wdev,
				  const u8 *mac, struct station_info *sinfo)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);
	const struct rtnl_link_stats64 *stats;
	struct rtnl_link_stats64 storage;

	if (!fm->connected || !ether_addr_equal(mac, fm->bssid))
		return -ENOENT;
	stats = dev_get_stats(fm->netdev, &storage);
	sinfo->rx_bytes = stats->rx_bytes - fm->station_start_stats.rx_bytes;
	sinfo->tx_bytes = stats->tx_bytes - fm->station_start_stats.tx_bytes;
	sinfo->rx_packets = stats->rx_packets - fm->station_start_stats.rx_packets;
	sinfo->tx_packets = stats->tx_packets - fm->station_start_stats.tx_packets;
	sinfo->connected_time = (jiffies - fm->connected_at) / HZ;
	sinfo->inactive_time = jiffies_to_msecs(jiffies - READ_ONCE(fm->last_activity));
	sinfo->sta_flags.mask = BIT(NL80211_STA_FLAG_AUTHENTICATED) |
				BIT(NL80211_STA_FLAG_ASSOCIATED) |
				BIT(NL80211_STA_FLAG_AUTHORIZED);
	sinfo->sta_flags.set = sinfo->sta_flags.mask;
	sinfo->filled = BIT_ULL(NL80211_STA_INFO_RX_BYTES64) |
			BIT_ULL(NL80211_STA_INFO_TX_BYTES64) |
			BIT_ULL(NL80211_STA_INFO_RX_PACKETS) |
			BIT_ULL(NL80211_STA_INFO_TX_PACKETS) |
			BIT_ULL(NL80211_STA_INFO_CONNECTED_TIME) |
			BIT_ULL(NL80211_STA_INFO_INACTIVE_TIME) |
			BIT_ULL(NL80211_STA_INFO_STA_FLAGS);
	if (READ_ONCE(fm->signal_valid)) {
		sinfo->signal = READ_ONCE(fm->last_signal);
		sinfo->filled |= BIT_ULL(NL80211_STA_INFO_SIGNAL);
	}
	/* No invented rate, retry, ACK-failure or RF measurements. */
	return 0;
}

static int mt7932_cfg_dump_station(struct wiphy *wiphy, struct wireless_dev *wdev,
				   int index, u8 *mac, struct station_info *sinfo)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);

	if (index || !fm->connected)
		return -ENOENT;
	ether_addr_copy(mac, fm->bssid);
	return mt7932_cfg_get_station(wiphy, wdev, mac, sinfo);
}

static void mt7932_cfg_regulatory(struct wiphy *wiphy,
				  struct regulatory_request *request)
{
	struct mt7932_fullmac *fm = mt7932_wiphy_priv(wiphy);
	struct mt792x_dev *dev = fm->dev;
	int ret;

	memcpy(dev->mt76.alpha2, request->alpha2, sizeof(dev->mt76.alpha2));
	dev->mt76.region = request->dfs_region;
	dev->country_ie_env = request->country_ie_env;
	dev->regd_change = true;
	if (READ_ONCE(fm->removing) || dev->fw_assert || dev->pm.suspended ||
	    test_bit(MT76_RESET, &dev->mphy.state))
		return;
	/* Preserve the complete domain AND transmit-power/SAR programming
	 * previously supplied by the mac80211 regulatory notifier.
	 */
	ret = mt7921_mcu_regd_update(dev, dev->mt76.alpha2,
				    dev->country_ie_env);
	if (ret) {
		wiphy_err(wiphy, "regulatory %.2s update failed: %d\n",
			  request->alpha2, ret);
		return;
	}
	wiphy_info(wiphy, "regulatory %.2s region %u applied\n",
		   request->alpha2, request->dfs_region);
}

static const struct cfg80211_ops mt7932_cfg80211_ops = {
	.scan = mt7932_cfg_scan,
	.abort_scan = mt7932_cfg_abort_scan,
	.connect = mt7932_cfg_connect,
	.disconnect = mt7932_cfg_disconnect,
	.get_station = mt7932_cfg_get_station,
	.dump_station = mt7932_cfg_dump_station,
};

int mt7932_cfg80211_register(struct mt7932_fullmac *fm)
{
	static const u32 ciphers[] = { WLAN_CIPHER_SUITE_CCMP,
				       WLAN_CIPHER_SUITE_AES_CMAC };
	static const u32 akms[] = { WLAN_AKM_SUITE_SAE };
	struct wiphy *wiphy;
	int i, ret;

	wiphy = wiphy_new(&mt7932_cfg80211_ops, sizeof(fm));
	if (!wiphy)
		return -ENOMEM;
	fm->wiphy = wiphy;
	*(struct mt7932_fullmac **)wiphy_priv(wiphy) = fm;
	set_wiphy_dev(wiphy, fm->dev->mt76.dev);
	wiphy->interface_modes = BIT(NL80211_IFTYPE_STATION);
	wiphy->signal_type = CFG80211_SIGNAL_TYPE_MBM;
	wiphy->max_scan_ssids = MT7932_SCAN_MAX_SSIDS;
	wiphy->max_scan_ie_len = MT76_CONNAC_SCAN_IE_LEN;
	wiphy->cipher_suites = ciphers;
	wiphy->n_cipher_suites = ARRAY_SIZE(ciphers);
	wiphy->akm_suites = akms;
	wiphy->n_akm_suites = ARRAY_SIZE(akms);
	wiphy->reg_notifier = mt7932_cfg_regulatory;
	ether_addr_copy(wiphy->perm_addr, fm->dev->mphy.macaddr);
	wiphy_ext_feature_set(wiphy, NL80211_EXT_FEATURE_SAE_OFFLOAD);
	for (i = 0; i < NUM_NL80211_BANDS; i++)
		wiphy->bands[i] = fm->dev->mphy.hw->wiphy->bands[i];
	wiphy_delayed_work_init(&fm->scan_timeout, mt7932_scan_timeout);
	wiphy_delayed_work_init(&fm->connect_timeout, mt7932_connect_timeout);
	ret = wiphy_register(wiphy);
	if (ret)
		goto free_wiphy;
	fm->wdev.wiphy = wiphy;
	fm->wdev.iftype = NL80211_IFTYPE_STATION;
	fm->wdev.netdev = fm->netdev;
	fm->netdev->ieee80211_ptr = &fm->wdev;
	strscpy(fm->netdev->name, "wlan%d", IFNAMSIZ);
	ret = register_netdev(fm->netdev);
	if (!ret) {
		mt7932_diag_init(fm->dev, wiphy->debugfsdir);
		return 0;
	}
	wiphy_unregister(wiphy);
free_wiphy:
	wiphy_free(wiphy);
	fm->wiphy = NULL;
	return ret;
}

void mt7932_cfg80211_unregister(struct mt7932_fullmac *fm)
{
	unregister_netdev(fm->netdev);
	wiphy_lock(fm->wiphy);
	mt7932_cfg80211_scan_done(fm, true);
	mt7932_cfg80211_disconnected(fm, WLAN_REASON_DEAUTH_LEAVING);
	wiphy_delayed_work_cancel(fm->wiphy, &fm->scan_timeout);
	wiphy_delayed_work_cancel(fm->wiphy, &fm->connect_timeout);
	wiphy_unlock(fm->wiphy);
	wiphy_unregister(fm->wiphy);
	wiphy_free(fm->wiphy);
}
