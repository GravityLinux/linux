// SPDX-License-Identifier: GPL-2.0-only
/* Cold radio PCI/DMA lifetime adapter. Protocol and firmware metadata are Rust. */
#include <linux/dma-mapping.h>
#include <linux/etherdevice.h>
#include <linux/io.h>
#include <linux/interrupt.h>
#include <linux/iommu.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/pci.h>
#include <linux/sizes.h>
#include <linux/slab.h>
#include <linux/skbuff.h>
#include <linux/workqueue.h>
#include <net/cfg80211.h>
#include "centauri_glue.h"

#define CENTAURI_RADIO_BAR_MIN_SIZE 0x1080
#define CENTAURI_WIFI_DMA_BYTES (SZ_2M + SZ_8M)
#define CENTAURI_WIFI_SERVICE_BUDGET 64
#define CENTAURI_SCAN_CHANNELS 11
#define CENTAURI_SCAN_TIMEOUT msecs_to_jiffies(10000)
#define CENTAURI_CONNECT_TIMEOUT msecs_to_jiffies(75000)

/* cfg80211 uses units of 100 kbit/s. Firmware negotiates the physical rate. */
static const int centauri_legacy_bitrates[] = {
	10, 20, 55, 110, 60, 90, 120, 180, 240, 360, 480, 540,
};

struct centauri_radio {
	struct pci_dev *pdev;
	void __iomem *bar;
	void *resource;
	dma_addr_t resource_dma;
	size_t resource_bytes;
	void *arena;
	dma_addr_t arena_dma;
	size_t arena_bytes;
	void *data_state;
	struct work_struct link_work;
	struct sk_buff_head tx_queue;
	int vectors;
	int requested_irqs;
	void *state;
	struct work_struct service_work;
	/* Serializes queue service with sleepable management operations. */
	struct mutex state_lock;
	bool service_running;
	bool shutting_down;
	struct wiphy *wiphy;
	struct wireless_dev wdev;
	struct net_device *netdev;
	struct ieee80211_supported_band band;
	struct ieee80211_channel channels[CENTAURI_SCAN_CHANNELS];
	struct ieee80211_rate rates[ARRAY_SIZE(centauri_legacy_bitrates)];
	struct cfg80211_scan_request *scan_request;
	struct work_struct scan_work;
	struct delayed_work scan_timeout;
	u8 scan_cookie;
	struct centauri_connect_params *connect_params;
	struct cfg80211_bss *connect_bss;
	struct work_struct connect_work;
	struct delayed_work connect_timeout;
	bool connecting;
	bool associated;
	u8 connected_bssid[ETH_ALEN];
	u8 association_ie[260];
	size_t association_ie_bytes;
};

void centauri_radio_receive(void *owner, const u8 *frame, size_t bytes)
{
	struct centauri_radio *radio = owner;
	struct sk_buff *skb;

	if (!radio->netdev || !radio->associated || radio->shutting_down ||
	    !netif_running(radio->netdev))
		return;
	skb = netdev_alloc_skb_ip_align(radio->netdev, bytes);
	if (!skb) {
		DEV_STATS_INC(radio->netdev, rx_dropped);
		return;
	}
	skb_put_data(skb, frame, bytes);
	skb->protocol = eth_type_trans(skb, radio->netdev);
	DEV_STATS_INC(radio->netdev, rx_packets);
	DEV_STATS_ADD(radio->netdev, rx_bytes, bytes);
	netif_rx(skb);
}

static void centauri_radio_link_work(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(work, struct centauri_radio, link_work);
	int error;

	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !radio->associated ||
	    !READ_ONCE(radio->service_running) || !netif_running(radio->netdev))
		goto unlock;
	if (!radio->data_state) {
		error = centauri_wifi_data_create(radio->state, radio,
						  (void __force *)radio->bar,
						  radio->arena, radio->arena_dma,
						  radio->arena + CENTAURI_WIFI_DMA_BYTES,
						  radio->arena_dma + CENTAURI_WIFI_DMA_BYTES,
						  radio->arena_bytes - CENTAURI_WIFI_DMA_BYTES,
						  &radio->data_state);
		if (error) {
			WRITE_ONCE(radio->service_running, false);
			pci_clear_master(radio->pdev);
			dev_err(&radio->pdev->dev, "Ethernet queue creation failed: %d\n", error);
			goto unlock;
		}
	}
	/*255 correlates the final PMK-only status0 with bidirectional IP data;
	 * a deliberately wrong PMK produces status1 and never reaches this path.
	 */
	cfg80211_port_authorized(radio->netdev, radio->connected_bssid, NULL, 0, GFP_KERNEL);
	netif_carrier_on(radio->netdev);
	netif_wake_queue(radio->netdev);
	queue_work(system_dfl_wq, &radio->service_work);
	dev_info(&radio->pdev->dev, "native Ethernet packet transport enabled\n");
unlock:
	mutex_unlock(&radio->state_lock);
}

static int centauri_radio_send_queued(struct centauri_radio *radio)
{
	struct sk_buff *skb;
	int error;

	if (!radio->associated || !radio->data_state ||
	    !netif_running(radio->netdev))
		return 0;
	skb = skb_dequeue(&radio->tx_queue);
	if (!skb)
		return 0;
	if (skb_linearize(skb)) {
		DEV_STATS_INC(radio->netdev, tx_dropped);
		dev_kfree_skb(skb);
		netif_wake_queue(radio->netdev);
		return 0;
	}
	error = centauri_wifi_data_xmit(radio->data_state, skb->data, skb->len);
	if (error == -EAGAIN) {
		skb_queue_head(&radio->tx_queue, skb);
		return 0;
	}
	if (!error) {
		DEV_STATS_INC(radio->netdev, tx_packets);
		DEV_STATS_ADD(radio->netdev, tx_bytes, skb->len);
	} else {
		DEV_STATS_INC(radio->netdev, tx_errors);
	}
	dev_kfree_skb(skb);
	return error;
}

/* cfg80211 owns the scan request until exactly one completion. All protocol
 * calls and these callbacks share state_lock; Rust copies a bounded frame
 * before returning its DMA cookie to firmware.
 */
void centauri_radio_scan_frame(void *owner, u16 frequency, s8 signal,
			       const u8 *frame, size_t bytes)
{
	struct centauri_radio *radio = owner;
	struct cfg80211_inform_bss data = {};
	struct cfg80211_bss *bss;

	if (!radio || !radio->scan_request)
		return;
	data.chan = ieee80211_get_channel(radio->wiphy, frequency);
	if (!data.chan || data.chan->flags & IEEE80211_CHAN_DISABLED)
		return;
	data.signal = signal * 100; /* cfg80211 signal type is mBm. */
	data.boottime_ns = ktime_get_boottime_ns();
	bss = cfg80211_inform_bss_frame_data(radio->wiphy, &data,
					     (struct ieee80211_mgmt *)frame, bytes, GFP_KERNEL);
	if (bss)
		cfg80211_put_bss(radio->wiphy, bss);
}

void centauri_radio_scan_done(void *owner, bool aborted)
{
	struct centauri_radio *radio = owner;
	struct cfg80211_scan_info info = { .aborted = aborted };
	struct cfg80211_scan_request *request;

	if (!radio || !radio->scan_request)
		return;
	request = radio->scan_request;
	radio->scan_request = NULL;
	cancel_delayed_work(&radio->scan_timeout);
	cfg80211_scan_done(request, &info);
}

static void centauri_scan_cancel(struct centauri_radio *radio)
{
	if (!radio->scan_request)
		return;
	/* No invented firmware cancel command. Fence late reports and refuse a
	 * new Rust scan until the old firmware completion or full teardown.
	 */
	centauri_wifi_scan_detach(radio->state);
	centauri_radio_scan_done(radio, true);
}

static void centauri_scan_timeout(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(to_delayed_work(work), struct centauri_radio, scan_timeout);

	mutex_lock(&radio->state_lock);
	centauri_scan_cancel(radio);
	mutex_unlock(&radio->state_lock);
}

static void centauri_scan_work(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(work, struct centauri_radio, scan_work);
	u32 channels[CENTAURI_SCAN_CHANNELS];
	int error;

	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !radio->scan_request)
		goto unlock;
	for (int i = 0; i < radio->scan_request->n_channels; i++)
		channels[i] = radio->scan_request->channels[i]->hw_value;
	error = centauri_wifi_scan_start(radio->state, radio, ++radio->scan_cookie,
					 channels, radio->scan_request->n_channels);
	if (error) {
		dev_warn(&radio->pdev->dev, "scan start failed: %d\n", error);
		centauri_radio_scan_done(radio, true);
	} else if (radio->scan_request) {
		queue_delayed_work(system_dfl_wq, &radio->scan_timeout,
				   CENTAURI_SCAN_TIMEOUT);
	}
unlock:
	mutex_unlock(&radio->state_lock);
}

static struct centauri_radio *centauri_wiphy_radio(struct wiphy *wiphy)
{
	return *(struct centauri_radio **)wiphy_priv(wiphy);
}

static int centauri_cfg_scan(struct wiphy *wiphy,
			     struct cfg80211_scan_request *request)
{
	struct centauri_radio *radio = centauri_wiphy_radio(wiphy);
	int error = 0;

	/* The qualified format has no SSID vector. Extra probe-request IEs have
	 * no role in this passive scan, so retain them only in cfg80211's request.
	 * On-air probe suppression still needs independent qualification before
	 * upstream use.
	 * Do not approximate active/directed scan requests here.
	 */
	if (request->n_ssids || request->flags ||
	    !request->n_channels || request->n_channels > CENTAURI_SCAN_CHANNELS)
		return -EOPNOTSUPP;
	for (int i = 0; i < request->n_channels; i++)
		if (request->channels[i]->band != NL80211_BAND_2GHZ ||
		    request->channels[i]->hw_value < 1 ||
		    request->channels[i]->hw_value > CENTAURI_SCAN_CHANNELS)
			return -EOPNOTSUPP;
	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !READ_ONCE(radio->service_running)) {
		error = -ENODEV;
	} else if (radio->scan_request || radio->connecting || radio->connect_params) {
		error = -EBUSY;
	} else {
		radio->scan_request = request;
		queue_work(system_dfl_wq, &radio->scan_work);
	}
	mutex_unlock(&radio->state_lock);
	return error;
}

static void centauri_cfg_abort_scan(struct wiphy *wiphy,
				    struct wireless_dev *wdev)
{
	struct centauri_radio *radio = centauri_wiphy_radio(wiphy);

	mutex_lock(&radio->state_lock);
	centauri_scan_cancel(radio);
	mutex_unlock(&radio->state_lock);
}

static void centauri_reg_notifier(struct wiphy *wiphy,
				  struct regulatory_request *request)
{
	struct centauri_radio *radio = centauri_wiphy_radio(wiphy);
	int error;

	if (!request || request->alpha2[0] < 'A' || request->alpha2[0] > 'Z' ||
	    request->alpha2[1] < 'A' || request->alpha2[1] > 'Z')
		return; /* No guessed firmware encoding for world domain00. */
	mutex_lock(&radio->state_lock);
	if (!radio->shutting_down) {
		error = centauri_wifi_set_country(radio->state, (const u8 *)request->alpha2);
		if (error)
			dev_warn(&radio->pdev->dev, "firmware country request failed: %d\n", error);
		else
			dev_info(&radio->pdev->dev, "firmware country readback verified\n");
	}
	mutex_unlock(&radio->state_lock);
}

void centauri_radio_connect_done(void *owner, const u8 *bssid, u16 status)
{
	struct centauri_radio *radio = owner;
	struct cfg80211_bss *bss;

	if (!radio || !radio->connecting)
		return;
	radio->connecting = false;
	WRITE_ONCE(radio->associated, status == 0);
	if (!status)
		memcpy(radio->connected_bssid, bssid, ETH_ALEN);
	cancel_delayed_work(&radio->connect_timeout);
	bss = radio->connect_bss;
	radio->connect_bss = NULL;
	/* Transfer the held BSS reference to cfg80211 with the result. Looking
	 * it up only now can lose a scan entry while firmware is connecting.
	 */
	cfg80211_connect_bss(radio->netdev, bssid, bss, radio->association_ie,
			     radio->association_ie_bytes, NULL, 0,
			     status ? WLAN_STATUS_UNSPECIFIED_FAILURE : WLAN_STATUS_SUCCESS,
			     GFP_KERNEL, NL80211_TIMEOUT_UNSPECIFIED);
	/* Queue setup may submit basic admin requests, so defer it until the
	 * current Rust management callback has returned and released state_lock.
	 */
	if (!status)
		queue_work(system_dfl_wq, &radio->link_work);
	dev_info(&radio->pdev->dev, "cfg80211 connection outcome: %u\n", status);
}

static void centauri_connection_cancel(struct centauri_radio *radio, u16 reason)
{
	if (radio->state)
		centauri_wifi_connect_detach(radio->state);
	if (radio->connecting) {
		radio->connecting = false;
		cfg80211_connect_bss(radio->netdev, NULL, radio->connect_bss,
				     NULL, 0, NULL, 0, WLAN_STATUS_UNSPECIFIED_FAILURE,
				     GFP_KERNEL, NL80211_TIMEOUT_UNSPECIFIED);
		radio->connect_bss = NULL;
	}
	if (radio->associated) {
		WRITE_ONCE(radio->associated, false);
		cfg80211_disconnected(radio->netdev, reason,
				      NULL, 0, true, GFP_KERNEL);
	}
	if (radio->netdev) {
		netif_carrier_off(radio->netdev);
		netif_tx_disable(radio->netdev);
	}
	skb_queue_purge(&radio->tx_queue);
	cancel_delayed_work(&radio->connect_timeout);
}

static void centauri_connect_timeout(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(to_delayed_work(work), struct centauri_radio, connect_timeout);

	mutex_lock(&radio->state_lock);
	centauri_connection_cancel(radio, WLAN_REASON_DEAUTH_LEAVING);
	mutex_unlock(&radio->state_lock);
}

static void centauri_connect_work(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(work, struct centauri_radio, connect_work);
	int error;

	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !radio->connecting ||
	    !READ_ONCE(radio->service_running))
		goto release;
	error = centauri_wifi_connect_start(radio->state, radio, radio->connect_params);
	if (error) {
		if (error == -EPROTO) {
			WRITE_ONCE(radio->service_running, false);
			pci_clear_master(radio->pdev);
		}
		dev_warn(&radio->pdev->dev, "connection start failed: %d\n", error);
		centauri_radio_connect_done(radio, radio->connect_params->bssid, 1);
	} else if (radio->connecting) {
		queue_delayed_work(system_dfl_wq, &radio->connect_timeout,
				   CENTAURI_CONNECT_TIMEOUT);
	}
	queue_work(system_dfl_wq, &radio->service_work);
release:
	kfree_sensitive(radio->connect_params);
	radio->connect_params = NULL;
	mutex_unlock(&radio->state_lock);
}

static bool centauri_bss_supports_h2e(struct cfg80211_bss *bss)
{
	const struct cfg80211_bss_ies *ies;
	const u8 *rsnxe;
	bool supported = false;

	rcu_read_lock();
	ies = rcu_dereference(bss->ies);
	if (ies) {
		rsnxe = cfg80211_find_ie(WLAN_EID_RSNX, ies->data, ies->len);
		if (rsnxe)
			supported = centauri_wifi_rsnxe_h2e(rsnxe + 2, rsnxe[1]);
	}
	rcu_read_unlock();
	return supported;
}

static int centauri_cfg_connect(struct wiphy *wiphy, struct net_device *netdev,
				struct cfg80211_connect_params *sme)
{
	struct centauri_radio *radio = centauri_wiphy_radio(wiphy);
	struct centauri_connect_params *input;
	struct cfg80211_bss *bss;
	struct ieee80211_channel *channel = sme->channel ?: sme->channel_hint;
	const u8 *bssid = sme->bssid ?: sme->bssid_hint;
	const u8 *rsn;
	bool sae;
	int error = 0;

	if (!sme->ssid_len || sme->ssid_len > 32 ||
	    (bssid && !is_valid_ether_addr(bssid)) ||
	    sme->crypto.n_akm_suites != 1 ||
	    sme->crypto.n_ciphers_pairwise != 1 ||
	    sme->crypto.ciphers_pairwise[0] != WLAN_CIPHER_SUITE_CCMP ||
	    sme->crypto.cipher_group != WLAN_CIPHER_SUITE_CCMP ||
	    (sme->flags & ~(CONNECT_REQ_MLO_SUPPORT | CONNECT_REQ_EXTERNAL_AUTH_SUPPORT)))
		return -EOPNOTSUPP;
	sae = sme->crypto.akm_suites[0] == WLAN_AKM_SUITE_SAE;
	if (sae) {
		if ((sme->crypto.wpa_versions != NL80211_WPA_VERSION_3 &&
		     sme->crypto.wpa_versions != NL80211_WPA_VERSION_2) ||
		    !sme->crypto.sae_pwd || !sme->crypto.sae_pwd_len ||
		    sme->crypto.sae_pwd_len > sizeof(input->password) ||
		    (sme->auth_type != NL80211_AUTHTYPE_SAE &&
		     sme->auth_type != NL80211_AUTHTYPE_AUTOMATIC) ||
		    sme->mfp != NL80211_MFP_REQUIRED)
			return -EOPNOTSUPP;
		/* H2E-only requires a capable selected peer and explicit RSNXE.
		 * Firmware's H2E negotiation is independently observed on queue4.
		 * Forced hunting/pecking has no qualified firmware mapping.
		 */
		if (sme->crypto.sae_pwe != NL80211_SAE_PWE_UNSPECIFIED &&
		    sme->crypto.sae_pwe != NL80211_SAE_PWE_BOTH &&
		    sme->crypto.sae_pwe != NL80211_SAE_PWE_HASH_TO_ELEMENT)
			return -EOPNOTSUPP;
	} else if (sme->crypto.akm_suites[0] != WLAN_AKM_SUITE_PSK ||
		   sme->crypto.wpa_versions != NL80211_WPA_VERSION_2 ||
		   !sme->crypto.psk ||
		   (sme->auth_type != NL80211_AUTHTYPE_OPEN_SYSTEM &&
		    sme->auth_type != NL80211_AUTHTYPE_AUTOMATIC) ||
		   sme->mfp == NL80211_MFP_REQUIRED) {
		return -EOPNOTSUPP;
	}
	/* These bits declare userspace capabilities. This offloaded connection
	 * selects one BSS and does not request MLO or external authentication.
	 */
	rsn = cfg80211_find_ie(WLAN_EID_RSN, sme->ie, sme->ie_len);
	/* The firmware constructs association frames from its own capabilities
	 * and the selected RSN element. Standard supplicants also supply optional
	 * extended-capability IEs. The offloaded four-way handshake owns EAPOL;
	 * packet queues/carrier remain stopped until port authorization completes.
	 */
	if (!rsn || !rsn[1])
		return -EOPNOTSUPP;
	bss = cfg80211_get_bss(wiphy, channel, bssid, sme->ssid,
			       sme->ssid_len, IEEE80211_BSS_TYPE_ESS, IEEE80211_PRIVACY_ON);
	/* Hints express a preference, rather than a mandatory BSS restriction. */
	if (!bss && (sme->channel_hint || sme->bssid_hint))
		bss = cfg80211_get_bss(wiphy, sme->channel, sme->bssid, sme->ssid,
				       sme->ssid_len, IEEE80211_BSS_TYPE_ESS,
				       IEEE80211_PRIVACY_ON);
	if (!bss)
		return -ENOENT;
	if (sae && sme->crypto.sae_pwe == NL80211_SAE_PWE_HASH_TO_ELEMENT &&
	    !centauri_bss_supports_h2e(bss)) {
		cfg80211_put_bss(wiphy, bss);
		return -EOPNOTSUPP;
	}
	if (bss->channel->band != NL80211_BAND_2GHZ ||
	    bss->channel->hw_value < 1 || bss->channel->hw_value > CENTAURI_SCAN_CHANNELS) {
		cfg80211_put_bss(wiphy, bss);
		return -EOPNOTSUPP;
	}
	input = kzalloc_obj(*input);
	if (!input) {
		cfg80211_put_bss(wiphy, bss);
		return -ENOMEM;
	}
	memcpy(input->ssid, sme->ssid, sme->ssid_len);
	input->ssid_bytes = sme->ssid_len;
	ether_addr_copy(input->bssid, bss->bssid);
	input->channel = bss->channel->hw_value;
	input->security = sae ? CENTAURI_SECURITY_SAE : CENTAURI_SECURITY_PSK;
	if (sae) {
		input->h2e_only = sme->crypto.sae_pwe == NL80211_SAE_PWE_HASH_TO_ELEMENT;
		input->password_bytes = sme->crypto.sae_pwd_len;
		memcpy(input->password, sme->crypto.sae_pwd, input->password_bytes);
	} else {
		memcpy(input->pmk, sme->crypto.psk, sizeof(input->pmk));
	}
	memcpy(input->rsn, rsn + 2, rsn[1]);
	input->rsn_bytes = rsn[1];
	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !READ_ONCE(radio->service_running)) {
		error = -ENODEV;
	} else if (radio->connecting || radio->associated ||
		   radio->connect_params || radio->scan_request) {
		error = -EBUSY;
	} else {
		radio->connect_params = input;
		radio->connect_bss = bss;
		radio->connecting = true;
		memcpy(radio->association_ie, rsn, 2 + rsn[1]);
		radio->association_ie_bytes = 2 + rsn[1];
		if (input->h2e_only) {
			size_t offset = radio->association_ie_bytes;

			radio->association_ie[offset++] = WLAN_EID_RSNX;
			radio->association_ie[offset++] = 1;
			radio->association_ie[offset++] = WLAN_RSNX_CAPA_SAE_H2E;
			radio->association_ie_bytes = offset;
		}
		queue_work(system_dfl_wq, &radio->connect_work);
	}
	mutex_unlock(&radio->state_lock);
	if (error) {
		kfree_sensitive(input);
		cfg80211_put_bss(wiphy, bss);
	}
	return error;
}

static int centauri_cfg_disconnect(struct wiphy *wiphy, struct net_device *netdev, u16 reason)
{
	struct centauri_radio *radio = centauri_wiphy_radio(wiphy);
	int error;

	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !READ_ONCE(radio->service_running)) {
		error = -ENODEV;
		goto unlock;
	}
	centauri_scan_cancel(radio);
	centauri_wifi_connect_detach(radio->state);
	/* Rust completes the qualified disconnect20001/event80000004 before
	 * deactivation. Joined interfaces reject direct command3 (250).
	 * The firmware's zero-body fields do not yet encode the host reason;
	 * report the locally requested reason to cfg80211 after completion.
	 */
	error = centauri_wifi_interface_stop(radio->state);
	if (!error) {
		centauri_connection_cancel(radio, reason);
		error = centauri_wifi_interface_start(radio->state, netdev->dev_addr);
	}
unlock:
	mutex_unlock(&radio->state_lock);
	return error;
}

static const struct cfg80211_ops centauri_cfg_ops = {
	.scan = centauri_cfg_scan,
	.abort_scan = centauri_cfg_abort_scan,
	.connect = centauri_cfg_connect,
	.disconnect = centauri_cfg_disconnect,
};

static int centauri_net_open(struct net_device *netdev)
{
	struct centauri_radio *radio = *(struct centauri_radio **)netdev_priv(netdev);
	int error;

	netif_carrier_off(netdev);
	netif_stop_queue(netdev);
	mutex_lock(&radio->state_lock);
	if (radio->shutting_down || !READ_ONCE(radio->service_running))
		error = -ENODEV;
	else
		error = centauri_wifi_interface_start(radio->state, netdev->dev_addr);
	mutex_unlock(&radio->state_lock);
	return error;
}

static int centauri_net_stop(struct net_device *netdev)
{
	struct centauri_radio *radio = *(struct centauri_radio **)netdev_priv(netdev);

	netif_stop_queue(netdev);
	mutex_lock(&radio->state_lock);
	centauri_scan_cancel(radio);
	centauri_connection_cancel(radio, WLAN_REASON_DEAUTH_LEAVING);
	if (READ_ONCE(radio->service_running))
		centauri_wifi_interface_stop(radio->state);
	mutex_unlock(&radio->state_lock);
	return 0;
}

static netdev_tx_t centauri_net_xmit(struct sk_buff *skb, struct net_device *netdev)
{
	struct centauri_radio *radio = *(struct centauri_radio **)netdev_priv(netdev);

	if (!READ_ONCE(radio->service_running) || !READ_ONCE(radio->associated)) {
		dev_kfree_skb_any(skb);
		DEV_STATS_INC(netdev, tx_dropped);
		return NETDEV_TX_OK;
	}
	/* ndo_start_xmit runs in atomic context. The IRQ-driven worker owns the
	 * sleepable Rust state lock and coherent protocol access.
	 */
	netif_stop_queue(netdev);
	skb_queue_tail(&radio->tx_queue, skb);
	queue_work(system_dfl_wq, &radio->service_work);
	return NETDEV_TX_OK;
}

static const struct net_device_ops centauri_radio_net_ops = {
	.ndo_open = centauri_net_open,
	.ndo_stop = centauri_net_stop,
	.ndo_start_xmit = centauri_net_xmit,
	.ndo_validate_addr = eth_validate_addr,
};

static int centauri_wireless_register(struct centauri_radio *radio, const u8 *address)
{
	static const u32 ciphers[] = { WLAN_CIPHER_SUITE_CCMP, WLAN_CIPHER_SUITE_AES_CMAC };
	static const u32 akms[] = { WLAN_AKM_SUITE_PSK, WLAN_AKM_SUITE_SAE };
	struct wiphy *wiphy;
	struct net_device *netdev;
	int error;

	wiphy = wiphy_new(&centauri_cfg_ops, sizeof(struct centauri_radio *));
	if (!wiphy)
		return -ENOMEM;
	*(struct centauri_radio **)wiphy_priv(wiphy) = radio;
	set_wiphy_dev(wiphy, &radio->pdev->dev);
	wiphy->interface_modes = BIT(NL80211_IFTYPE_STATION);
	wiphy->signal_type = CFG80211_SIGNAL_TYPE_MBM;
	wiphy->max_scan_ssids = 0;
	wiphy->max_scan_ie_len = IEEE80211_MAX_DATA_LEN;
	wiphy->cipher_suites = ciphers;
	wiphy->n_cipher_suites = ARRAY_SIZE(ciphers);
	wiphy->akm_suites = akms;
	wiphy->n_akm_suites = ARRAY_SIZE(akms);
	wiphy_ext_feature_set(wiphy, NL80211_EXT_FEATURE_4WAY_HANDSHAKE_STA_PSK);
	wiphy_ext_feature_set(wiphy, NL80211_EXT_FEATURE_SAE_OFFLOAD);
	wiphy->reg_notifier = centauri_reg_notifier;
	memcpy(wiphy->perm_addr, address, ETH_ALEN);
	for (int i = 0; i < CENTAURI_SCAN_CHANNELS; i++) {
		radio->channels[i].band = NL80211_BAND_2GHZ;
		radio->channels[i].hw_value = i + 1;
		radio->channels[i].center_freq = 2412 + i * 5;
		/* Passive scanning only; TX power/control is not qualified yet. */
		radio->channels[i].flags = IEEE80211_CHAN_NO_IR;
	}
	for (int i = 0; i < ARRAY_SIZE(radio->rates); i++)
		radio->rates[i].bitrate = centauri_legacy_bitrates[i];
	radio->band.band = NL80211_BAND_2GHZ;
	radio->band.channels = radio->channels;
	radio->band.n_channels = ARRAY_SIZE(radio->channels);
	radio->band.bitrates = radio->rates;
	radio->band.n_bitrates = ARRAY_SIZE(radio->rates);
	wiphy->bands[NL80211_BAND_2GHZ] = &radio->band;
	radio->wiphy = wiphy;
	error = wiphy_register(wiphy);
	if (error)
		goto free_wiphy;
	netdev = alloc_netdev(sizeof(struct centauri_radio *), "wlan%d",
			      NET_NAME_ENUM, ether_setup);
	if (!netdev) {
		error = -ENOMEM;
		goto unregister_wiphy;
	}
	*(struct centauri_radio **)netdev_priv(netdev) = radio;
	radio->wdev.wiphy = wiphy;
	radio->wdev.iftype = NL80211_IFTYPE_STATION;
	radio->wdev.netdev = netdev;
	netdev->ieee80211_ptr = &radio->wdev;
	netdev->netdev_ops = &centauri_radio_net_ops;
	netdev->max_mtu = ETH_DATA_LEN;
	SET_NETDEV_DEV(netdev, &radio->pdev->dev);
	eth_hw_addr_set(netdev, address);
	netif_carrier_off(netdev);
	error = register_netdev(netdev);
	if (error) {
		free_netdev(netdev);
		goto unregister_wiphy;
	}
	radio->netdev = netdev;
	return 0;
unregister_wiphy:
	wiphy_unregister(wiphy);
free_wiphy:
	wiphy_free(wiphy);
	radio->wiphy = NULL;
	return error;
}

static void centauri_wireless_unregister(struct centauri_radio *radio)
{
	mutex_lock(&radio->state_lock);
	radio->shutting_down = true;
	centauri_scan_cancel(radio);
	centauri_connection_cancel(radio, WLAN_REASON_DEAUTH_LEAVING);
	mutex_unlock(&radio->state_lock);
	cancel_work_sync(&radio->scan_work);
	cancel_delayed_work_sync(&radio->scan_timeout);
	cancel_work_sync(&radio->connect_work);
	cancel_work_sync(&radio->link_work);
	cancel_delayed_work_sync(&radio->connect_timeout);
	kfree_sensitive(radio->connect_params);
	radio->connect_params = NULL;
	if (radio->netdev) {
		unregister_netdev(radio->netdev);
		free_netdev(radio->netdev);
		radio->netdev = NULL;
	}
	if (radio->wiphy) {
		wiphy_unregister(radio->wiphy);
		wiphy_free(radio->wiphy);
		radio->wiphy = NULL;
	}
}

static void centauri_radio_work(struct work_struct *work)
{
	struct centauri_radio *radio =
		container_of(work, struct centauri_radio, service_work);
	bool tx_done = false;
	int basic_done, data_done = 0, error = 0;

	/* Acquire the probe's release-store before using initialized queue state. */
	if (!smp_load_acquire(&radio->service_running))
		return;
	mutex_lock(&radio->state_lock);
	if (!READ_ONCE(radio->service_running)) {
		mutex_unlock(&radio->state_lock);
		return;
	}
	basic_done = centauri_wifi_queues_poll(radio->state, CENTAURI_WIFI_SERVICE_BUDGET);
	if (basic_done < 0) {
		error = basic_done;
		goto fault;
	}
	if (radio->data_state) {
		data_done = centauri_wifi_data_poll(radio->data_state,
						    CENTAURI_WIFI_SERVICE_BUDGET, &tx_done);
		if (data_done < 0) {
			error = data_done;
			goto fault;
		}
		if (tx_done && radio->associated && netif_running(radio->netdev))
			netif_wake_queue(radio->netdev);
		error = centauri_radio_send_queued(radio);
		if (error)
			goto fault;
	}
	mutex_unlock(&radio->state_lock);
	if ((basic_done == CENTAURI_WIFI_SERVICE_BUDGET ||
	     data_done == CENTAURI_WIFI_SERVICE_BUDGET) &&
	    READ_ONCE(radio->service_running))
		queue_work(system_dfl_wq, &radio->service_work);
	return;
fault:
	WRITE_ONCE(radio->service_running, false);
	if (error == -EPROTO)
		centauri_connection_cancel(radio, WLAN_REASON_UNSPECIFIED);
	if (radio->netdev) {
		netif_carrier_off(radio->netdev);
		netif_tx_disable(radio->netdev);
	}
	pci_clear_master(radio->pdev);
	mutex_unlock(&radio->state_lock);
	dev_err(&radio->pdev->dev, "Wi-Fi queue service stopped: %d\n", error);
}

/* The Rust worker drains all owned queues, independent of MSI vector number. */
static irqreturn_t centauri_radio_irq(int irq, void *data)
{
	struct centauri_radio *radio = data;

	/* The state and its DMA buffers are live before IRQ dispatch starts. */
	if (smp_load_acquire(&radio->service_running))
		queue_work(system_dfl_wq, &radio->service_work);
	return IRQ_HANDLED;
}

int centauri_radio_enable_irqs(void *owner, unsigned int count)
{
	struct centauri_radio *radio = owner;
	int error, i;

	if (radio->vectors)
		return -EBUSY;
	error = pci_alloc_irq_vectors(radio->pdev, count, count, PCI_IRQ_MSI);
	if (error < 0)
		return error;
	radio->vectors = error;
	for (i = 0; i < radio->vectors; i++) {
		error = request_irq(pci_irq_vector(radio->pdev, i),
				    centauri_radio_irq, 0, "centauri-radio", radio);
		if (error)
			return error;
		radio->requested_irqs++;
	}
	return 0;
}

void centauri_radio_enable_dma(void *owner)
{
	struct centauri_radio *radio = owner;

	/* Publish both initialized coherent allocations before firmware can DMA. */
	dma_wmb();
	pci_set_master(radio->pdev);
}

int centauri_radio_config_read(void *owner, u16 offset, u32 *value)
{
	struct centauri_radio *radio = owner;

	if (offset & 3 || offset > PCI_CFG_SPACE_EXP_SIZE - sizeof(*value))
		return -EINVAL;
	return pcibios_err_to_errno(pci_read_config_dword(radio->pdev, offset, value));
}

int centauri_radio_config_write(void *owner, u16 offset, u32 value)
{
	struct centauri_radio *radio = owner;

	if (offset & 3 || offset > PCI_CFG_SPACE_EXP_SIZE - sizeof(value))
		return -EINVAL;
	return pcibios_err_to_errno(pci_write_config_dword(radio->pdev, offset, value));
}

static void centauri_radio_free(struct centauri_radio *radio)
{
	WRITE_ONCE(radio->service_running, false);
	centauri_wireless_unregister(radio);
	/* Stop the firmware interface while DMA still works. The worker shares
	 * this lock; the final IRQ/work drain below also covers previously queued
	 * callbacks which passed the running check before removal started.
	 */
	if (radio->state) {
		int error;

		mutex_lock(&radio->state_lock);
		error = centauri_wifi_interface_stop(radio->state);
		mutex_unlock(&radio->state_lock);
		if (error)
			dev_dbg(&radio->pdev->dev, "interface stop failed: %d\n", error);
	}
	pci_clear_master(radio->pdev);
	pci_wait_for_pending_transaction(radio->pdev);
	for (int i = 0; i < radio->requested_irqs; i++)
		free_irq(pci_irq_vector(radio->pdev, i), radio);
	/* No IRQ can queue work after synchronization. The worker cannot retain
	 * protocol state or coherent pages after this drain completes.
	 */
	cancel_work_sync(&radio->service_work);
	if (radio->data_state)
		centauri_wifi_data_destroy(radio->data_state);
	skb_queue_purge(&radio->tx_queue);
	if (radio->state)
		centauri_wifi_queues_destroy(radio->state);
	if (radio->vectors)
		pci_free_irq_vectors(radio->pdev);
	if (radio->arena)
		dma_free_coherent(&radio->pdev->dev, radio->arena_bytes,
				  radio->arena, radio->arena_dma);
	if (radio->resource)
		dma_free_coherent(&radio->pdev->dev, radio->resource_bytes,
				  radio->resource, radio->resource_dma);
}

static int centauri_radio_probe(struct pci_dev *pdev,
				const struct pci_device_id *id)
{
	struct centauri_radio *radio;
	struct iommu_domain *domain;
	u8 address[ETH_ALEN];
	int error;

	if (!of_device_is_available(pdev->dev.of_node) ||
	    !of_device_is_compatible(pdev->dev.of_node,
				     "apple,t2026-centauri-wifi"))
		return -ENODEV;
	domain = iommu_get_domain_for_dev(&pdev->dev);
	if (!domain || !iommu_is_dma_domain(domain))
		return dev_err_probe(&pdev->dev, -EOPNOTSUPP,
				     "radio requires a Linux DMA IOMMU domain\n");
	if (!(pci_resource_flags(pdev, 0) & IORESOURCE_MEM) ||
	    pci_resource_len(pdev, 0) < CENTAURI_RADIO_BAR_MIN_SIZE)
		return -EINVAL;
	error = pcim_enable_device(pdev);
	if (error)
		return error;
	pci_clear_master(pdev);
	error = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(36));
	if (error)
		return error;
	radio = devm_kzalloc(&pdev->dev, sizeof(*radio), GFP_KERNEL);
	if (!radio)
		return -ENOMEM;
	radio->pdev = pdev;
	INIT_WORK(&radio->service_work, centauri_radio_work);
	INIT_WORK(&radio->link_work, centauri_radio_link_work);
	skb_queue_head_init(&radio->tx_queue);
	mutex_init(&radio->state_lock);
	INIT_WORK(&radio->scan_work, centauri_scan_work);
	INIT_DELAYED_WORK(&radio->scan_timeout, centauri_scan_timeout);
	INIT_WORK(&radio->connect_work, centauri_connect_work);
	INIT_DELAYED_WORK(&radio->connect_timeout, centauri_connect_timeout);
	radio->bar = pcim_iomap_region(pdev, 0, KBUILD_MODNAME);
	if (IS_ERR(radio->bar))
		return PTR_ERR(radio->bar);
	/* Only the firmware resource has this narrower address contract. Probe
	 * owns all allocations, so no concurrent allocator observes the change.
	 */
	error = dma_set_coherent_mask(&pdev->dev, centauri_wifi_resource_dma_mask());
	if (error)
		return error;
	error = centauri_control_copy_radio(pdev, &radio->resource,
					    &radio->resource_dma,
					    &radio->resource_bytes);
	if (error)
		return dev_err_probe(&pdev->dev, error, "radio firmware resource\n");
	error = dma_set_coherent_mask(&pdev->dev, DMA_BIT_MASK(36));
	if (error)
		goto free;
	radio->arena_bytes = CENTAURI_WIFI_DMA_BYTES + centauri_wifi_data_bytes();
	radio->arena = dma_alloc_coherent(&pdev->dev, radio->arena_bytes,
					  &radio->arena_dma, GFP_KERNEL);
	if (!radio->arena) {
		error = -ENOMEM;
		goto free;
	}
	error = centauri_wifi_start(radio, (void __force *)radio->bar,
				    radio->resource_dma, radio->resource_bytes,
				    radio->arena, radio->arena_dma,
				    radio->arena_bytes);
	if (error)
		goto free;
	error = centauri_wifi_queues_create((void __force *)radio->bar,
					    radio->arena, radio->arena_dma,
					    CENTAURI_WIFI_DMA_BYTES, &radio->state);
	if (error)
		goto free;
	/* Standard firmware MAC properties; no serial or board identity gate.
	 * The independently qualified address-template API also supports a
	 * generated local address when a platform omits its factory property.
	 */
	if (device_get_mac_address(&pdev->dev, (char *)address))
		eth_random_addr(address);
	error = centauri_wifi_interface_start(radio->state, address);
	if (error)
		goto free;
	error = centauri_wireless_register(radio, address);
	if (error)
		goto free;
	pci_set_drvdata(pdev, radio);
	/* Publish the initialized state before an IRQ can schedule its worker. */
	smp_store_release(&radio->service_running, true);
	queue_work(system_dfl_wq, &radio->service_work);
	dev_info(&pdev->dev, "Wi-Fi cfg80211 scan interface registered\n");
	return 0;
free:
	centauri_radio_free(radio);
	return error;
}

static void centauri_radio_remove(struct pci_dev *pdev)
{
	centauri_radio_free(pci_get_drvdata(pdev));
	pci_set_drvdata(pdev, NULL);
}

static const struct pci_device_id centauri_radio_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_APPLE, 0x1902) },
	{}
};
MODULE_DEVICE_TABLE(pci, centauri_radio_ids);

static struct pci_driver centauri_radio_driver = {
	.name = "centauri-radio",
	.id_table = centauri_radio_ids,
	.probe = centauri_radio_probe,
	.remove = centauri_radio_remove,
};

int centauri_radio_register(void)
{
	return pci_register_driver(&centauri_radio_driver);
}

void centauri_radio_unregister(void)
{
	pci_unregister_driver(&centauri_radio_driver);
}
