/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __MT7932_MCU_H
#define __MT7932_MCU_H

/* Private, observed MT7932 ABI. Evidence and unresolved fields are in
 * Documentation/networking/device_drivers/wifi/mt7932-abi.txt. These are legacy
 * flat command IDs, not CONNAC UNI commands with coincidentally equal IDs.
 */
enum mt7932_mcu_command {
	MT7932_CMD_BASIC_CONFIG = 0x02,
	MT7932_CMD_ADD_REMOVE_KEY = 0x07,
	MT7932_CMD_ACTIVATE_BSS = 0x11,
	MT7932_CMD_SET_BSS = 0x12,
	MT7932_CMD_SET_STA = 0x13,
	MT7932_CMD_REMOVE_STA = 0x14,
	MT7932_CMD_SET_RLM = 0x19,
	MT7932_CMD_DBDC = 0x28,
	MT7932_CMD_QUEUE_MANAGEMENT = 0x6b,
	MT7932_CMD_CUSTOMER_CONFIG = 0x70,
	MT7932_CMD_RUNTIME_93 = 0x93, /* Meaning not established on this ABI. */
	MT7932_CMD_PROBE_PRIVACY = 0x9b,
	MT7932_CMD_BEACON_PARSE = 0xaf,
	MT7932_CMD_AIS_MAC = 0xb8,
	MT7932_CMD_CAL_PROFILE = 0xbf,
	MT7932_CMD_PRECAL = 0xd6,
	MT7932_CMD_CAL_D7 = 0xd7,
	MT7932_CMD_RTS = 0xdc,
	MT7932_CMD_SME_CONNECT = 0x40ea,
	MT7932_CMD_SME_DISCONNECT = 0x42ea,
	MT7932_CMD_SME_DHCP_RELEASE = 0x47ea,
	MT7932_CMD_SME_CONFIG = 0x4eea,
	MT7932_CMD_SME_RNR = 0x60ea,
	MT7932_CMD_CAL_EXT_21 = 0x21ed,
};

enum mt7932_fw_band {
	MT7932_BAND_2GHZ = 1,
	MT7932_BAND_5GHZ = 2,
	MT7932_BAND_6GHZ = 3,
};

enum mt7932_bss_index {
	MT7932_AIS_BSS = 0,
	MT7932_DISCOVERY_BSS = 1,
};

#define MT7932_NO_WCID			U8_MAX
/* Retain the existing defensive bound; this is not a decoded table size. */
#define MT7932_PEER_WCID_LIMIT		48
#define MT7932_SCAN_SEQUENCE_MASK	GENMASK(6, 0)
#define MT7932_SCAN_DISABLE_DBDC		BIT(1)
#define MT7932_SCAN_SPECIFIED_SSIDS	BIT(3)
#define MT7932_SCAN_DWELL_MS		150
#define MT7932_SCAN_MIN_DWELL_MS		120
#define MT7932_SCAN_FW_TIMEOUT_MS		10000

/* MT7932 boot replies are one-based, unlike mt76's generic PATCH_* enum. */
enum mt7932_patch_sem_status {
	MT7932_PATCH_SEM_BUSY = 1,
	MT7932_PATCH_ALREADY_LOADED = 2,
	MT7932_PATCH_SEM_ACQUIRED = 3,
};

#define MT7932_PATCH_SEM_REQUEST		2
#define MT7932_PATCH_WIFI_SECTION		0x100
#define MT7932_FW_SCATTER_CHUNK		2048
#define MT7932_FW_MAX_REGIONS		10

/* Private board-calibration replay file. Command descriptors/sequence
 * numbers are constructed afresh; this records payloads and command policy.
 */
#define MT7932_CAL_MAGIC			"MT7932C1"
#define MT7932_CAL_MAX_FILE_SIZE		SZ_64K
#define MT7932_CAL_MAX_RECORDS		128
#define MT7932_CAL_MAX_PAYLOAD		1536
#define MT7932_CAL_EXT_FRAGMENT		3
#define MT7932_CAL_EXT_MAX_DATA		SZ_1K
#define MT7932_CAL_PROFILE_SELECTOR	9 /* Selector semantics unresolved. */

struct mt7932_cal_file_header {
	u8 magic[8];
	u8 mac[ETH_ALEN];
	u8 reserved[2];
	__le32 count;
} __packed;

struct mt7932_cal_file_record {
	u8 cid;
	u8 ext_cid;
	u8 set_query;
	u8 wait_response;
	__le32 len;
	u8 data[];
} __packed;

struct mt7932_cal_ext_fragment {
	u8 type;
	u8 reserved;
	__le16 len;
	u8 data[];
} __packed;

struct mt7932_cal_profile_query {
	u8 unknown_0[3];
	u8 selector;
	u8 unknown_4[16];
} __packed;

struct mt7932_cal_profile_reply {
	u8 unknown_0[12];
	u8 cid;
	u8 unknown_13[7];
} __packed;

/* Bits/values here belong to MT7932's password-offload ABI, not nl80211
 * suite selectors or gen4m's separate authentication-mode enum.
 */
#define MT7932_SME_AUTH_SAE		BIT(10)
#define MT7932_SME_CIPHER_CCMP		BIT(4)
#define MT7932_SME_CONNECT_FLAGS		BIT(16)
#define MT7932_SME_PASSWORD_FORMAT	10
#define MT7932_SME_PMF_REQUIRED		2
#define MT7932_SME_NATIVE_AUTH_POLICY	(BIT(30) | BIT(2))
/* Units and individual semantics of these observed native policies unknown. */
#define MT7932_SME_NATIVE_DEADLINE	120
#define MT7932_SME_NATIVE_OPERATION	8
#define MT7932_SME_NATIVE_COMPATIBILITY	20
#define MT7932_SME_NATIVE_PROFILE		0x00020407
#define MT7932_DISCONNECT_POLICY		8
#define MT7932_DISCONNECT_PEER_TAG	1
#define MT7932_DISCONNECT_TRAILER		30
#define MT7932_POWER_MAX_CHANNELS		8
#define MT7932_POWER_RATE_ENTRIES		121
#define MT7932_POWER_WIRE_VERSION		3
#define MT7932_POWER_WIRE_FORMAT		0x0a /* Individual format bits unknown. */
/* Existing conservative 10 dBm bring-up ceiling; firmware units are 0.5 dBm. */
#define MT7932_POWER_CEILING_HALF_DBM	20

struct mt7932_power_channel {
	u8 channel;
	s8 rate_limit[MT7932_POWER_RATE_ENTRIES];
} __packed;

struct mt7932_basic_config {
	u8 native_80211;
	u8 assert_path;
	u8 debug_level;
	u8 unknown_3;
	__le16 rx_checksum;
	__le16 tx_checksum;
	u8 segmentation;
	u8 reserved[3];
	u8 unknown_extension[12];
} __packed;

/* MT7932 extends the legacy 12-byte config. Preserve the observed extension
 * while its individual controls remain unresolved. One shared initializer
 * keeps raw-802.11 startup and Ethernet-offload startup in sync.
 */
#define MT7932_BASIC_CONFIG_INITIALIZER(_native) { \
	.native_80211 = (_native), \
	.unknown_3 = 3, \
	.unknown_extension = { 1, 0, 2, 3, 0, 0x20, 0, 0, 2 }, \
}

/* Privacy bitmap: ordinary IE IDs occupy 256 bits, followed by 64 extension
 * IDs. The inclusive final-byte index is 39; extension 58 is in that byte.
 */
#define MT7932_PRIVACY_ORDINARY_BYTES	(U8_MAX / BITS_PER_BYTE + 1)
#define MT7932_PRIVACY_EXTENSION_BYTES	8
struct mt7932_probe_privacy {
	u8 subcommand;
	u8 unknown_1[3];
	u8 last_bitmap_byte;
	u8 unknown_policy0;
	u8 unknown_policy1;
	u8 bitmap[MT7932_PRIVACY_ORDINARY_BYTES + MT7932_PRIVACY_EXTENSION_BYTES];
	u8 reserved_tail[13];
} __packed;

/* The native activation command's last byte is 1, although an older public
 * gen4m structure calls it reserved. Do not infer a new meaning from that.
 */
struct mt7932_activate_bss {
	u8 bss_idx;
	u8 active;
	u8 network_type;
	u8 omac_idx;
	u8 mac[ETH_ALEN];
	u8 bmc_wcid;
	u8 unknown_11;
} __packed;

struct mt7932_ais_mac {
	u8 bss_idx;
	u8 reserved;
	u8 mac[ETH_ALEN];
} __packed;

/* Legacy RLM layout, corroborated against gen4m CMD_SET_BSS_RLM_PARAM. */
struct mt7932_rlm {
	u8 bss_idx;
	u8 band;
	u8 primary_channel;
	u8 secondary_channel_offset;
	u8 erp_protection;
	u8 ht_protection;
	u8 greenfield_operation;
	u8 tx_rifs;
	__le16 ht_operation3;
	__le16 ht_operation2;
	u8 ht_operation1;
	u8 short_preamble;
	u8 short_slot_time;
	u8 vht_width;
	u8 center_channel1;
	u8 center_channel2;
	__le16 vht_basic_mcs;
	u8 tx_nss;
	u8 rx_nss;
} __packed;

/* The prefix through ext_channels matches mt76_connac_hw_scan_req, then
 * MT7932 inserts two
 * bytes before ext_ssids; named tail fields prevent accidental use of the
 * generic bssid/random_mac offsets on this wire format.
 */
struct mt7932_scan_request {
	u8 common[offsetof(struct mt76_connac_hw_scan_req, ext_ssids)];
	u8 unknown_before_ext_ssids[2];
	struct mt76_connac_mcu_scan_ssid ext_ssids[6];
	u8 bssid[ETH_ALEN];
	u8 random_mac[ETH_ALEN];
	u8 unknown_tail[112];
} __packed;

enum mt7932_event {
	MT7932_EVENT_PRECAL_REQUEST = 0xd7,
	MT7932_EVENT_PRECAL_DONE = 0xd9,
	MT7932_EVENT_MLME = 0xea,
	MT7932_EVENT_MLME_ALT = 0xee,
	MT7932_EVENT_COMMAND_RESULT = 0xfd,
};

#define MT7932_BOOT_PACKET_TYPE		0xe000
#define MT7932_ROM_PACKET_TYPE		7
#define MT7932_RUNTIME_PACKET_TYPE	0x3800
#define MT7932_BOOT_SUCCESS		1
/* ROM has no four-byte extended event tail. */
#define MT7932_BOOT_HEADER_SIZE		offsetof(struct mt76_connac2_mcu_rxd, ext_eid)

struct mt7932_command_result {
	u8 cid;
	u8 status;
	u8 reserved[2];
} __packed;

struct mt7932_register_result {
	__le32 address;
	__le32 value;
} __packed;

/* Input header produced by mt76_connac_mcu_set_channel_domain. */
struct mt7932_channel_domain_input {
	u8 alpha2[4];
	u8 bw_2g;
	u8 bw_5g;
	u8 bw_6g;
	u8 reserved;
	u8 n_2ch;
	u8 n_5ch;
	u8 n_6ch;
	u8 reserved2;
} __packed;

/* Native variant of that header. Preserve observed policy fields without
 * interpreting them as the generic header's bandwidth bytes.
 */
struct mt7932_channel_domain {
	u8 alpha2[4];
	__le16 unknown_policy4;
	u8 unknown_policy6;
	u8 flags;
	u8 n_2ch;
	u8 n_5ch;
	u8 n_6ch;
	u8 unknown_fcc_policy;
} __packed;

struct mt7932_domain_channel {
	__le16 number;
	__le16 reserved;
	__le32 flags;
} __packed;

#define MT7932_DOMAIN_HAS_DFS_REGION	BIT(0)
#define MT7932_DOMAIN_HAS_6GHZ		BIT(3)
#define MT7932_DOMAIN_DFS_REGION		GENMASK(7, 4)
#define MT7932_CHANNEL_PASSIVE		BIT(8)
#define MT7932_CHANNEL_DFS		BIT(3)

enum mt7932_mlme_event_type {
	MT7932_MLME_ASSOCIATED = 0x40,
	MT7932_MLME_AUTHORIZED = 0x41,
	MT7932_MLME_DISCONNECTED = 0x44,
	MT7932_MLME_CONNECTING = 0x4d,
};

struct mt7932_mlme_event {
	__le32 unknown_0; /* Only the observed zero-valued envelope is handled. */
	__le32 type;
	__le32 len;
	u8 data[];
} __packed;

struct mt7932_mlme_connecting {
	u8 unknown_0[4];
	u8 bssid[ETH_ALEN];
	u8 center_channel;
	u8 unknown_11;
	u8 band;
	u8 unknown_13[11];
} __packed;

struct mt7932_mlme_associated {
	__le32 status;
	u8 unknown_4;
	u8 bssid[ETH_ALEN];
	u8 unknown_11[10];
	u8 peer_wcid;
} __packed;

struct mt7932_mlme_authorized {
	__le32 status;
	u8 bssid[ETH_ALEN];
	u8 unknown_10[10];
} __packed;

static_assert(sizeof(struct mt7932_activate_bss) == 12);
static_assert(sizeof(struct mt7932_basic_config) == 24);
static_assert(sizeof(struct mt7932_power_channel) == 122);
static_assert(sizeof(struct mt7932_probe_privacy) == 60);
static_assert(offsetof(struct mt7932_probe_privacy, bitmap) == 7);
static_assert(offsetof(struct mt7932_basic_config, unknown_extension) == 12);
static_assert(sizeof(struct mt7932_ais_mac) == 8);
static_assert(sizeof(struct mt7932_rlm) == 22);
static_assert(offsetof(struct mt7932_rlm, short_slot_time) == 14);
static_assert(sizeof(struct mt7932_scan_request) == 1236);
static_assert(offsetof(struct mt7932_scan_request, ext_ssids) == 0x380);
static_assert(offsetof(struct mt7932_scan_request, bssid) == 0x458);
static_assert(offsetof(struct mt7932_scan_request, random_mac) == 0x45e);
static_assert(sizeof(struct mt7932_mlme_event) == 12);
static_assert(sizeof(struct mt7932_mlme_connecting) == 24);
static_assert(offsetof(struct mt7932_mlme_connecting, band) == 12);
static_assert(sizeof(struct mt7932_mlme_associated) == 22);
static_assert(offsetof(struct mt7932_mlme_associated, peer_wcid) == 21);
static_assert(sizeof(struct mt7932_mlme_authorized) == 20);
static_assert(MT7932_BOOT_HEADER_SIZE == 32);
static_assert(sizeof(struct mt76_connac2_mcu_rxd) == 36);
static_assert(offsetof(struct mt76_connac2_mcu_rxd, eid) == 28);
static_assert(sizeof(struct mt7932_command_result) == 4);
static_assert(sizeof(struct mt7932_register_result) == 8);
static_assert(sizeof(struct mt7932_channel_domain_input) == 12);
static_assert(sizeof(struct mt7932_channel_domain) == 12);
static_assert(sizeof(struct mt7932_domain_channel) == 8);
static_assert(sizeof(struct mt7932_cal_file_header) == 20);
static_assert(sizeof(struct mt7932_cal_file_record) == 8);
static_assert(sizeof(struct mt7932_cal_ext_fragment) == 4);
static_assert(sizeof(struct mt7932_cal_profile_query) == 20);
static_assert(sizeof(struct mt7932_cal_profile_reply) == 20);
static_assert(offsetof(struct mt7932_cal_profile_reply, cid) == 12);

#endif
