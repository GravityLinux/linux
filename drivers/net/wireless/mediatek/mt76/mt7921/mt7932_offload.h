/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __MT7932_OFFLOAD_H
#define __MT7932_OFFLOAD_H

/* Host configuration, not the firmware's connection command. No cached keys. */
struct mt7932_connect_params {
	u8 ssid_len;
	u8 password_len;
	u8 band;
	u8 channel;
	u8 bssid[ETH_ALEN];
	u8 ssid[IEEE80211_MAX_SSID_LEN];
	u8 password[64];
};

struct mt7932_cal_cache;
int mt7932_offload_disconnect(struct mt792x_dev *dev, const u8 *bssid);

int mt7932_offload_connect(struct mt792x_dev *dev,
			   const struct mt7932_connect_params *params);
int mt7932_precal_load(struct mt792x_dev *dev, struct mt7932_cal_cache **out);
int mt7932_precal_preload(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			   u8 band);
int mt7932_precal_apply(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			 u8 band, u32 center_khz);
int mt7932_precal_request(struct mt792x_dev *dev, struct mt7932_cal_cache *cache,
			   const u8 *request, size_t len);
void mt7932_fullmac_connecting(struct mt792x_dev *dev, const u8 *bssid);
int mt7932_fullmac_preload(struct mt792x_dev *dev, u8 band);
void mt7932_fullmac_reset(struct mt792x_dev *dev);
#endif
