/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef _CENTAURI_GLUE_H
#define _CENTAURI_GLUE_H
#include <linux/types.h>

/* Generic mappings and net_device glue; hardware protocol lives in Rust. */
int centauri_register(void);
void centauri_unregister(void);
int centauri_bootstrap_register(void);
void centauri_bootstrap_unregister(void);
int centauri_radio_register(void);
void centauri_radio_unregister(void);
void centauri_radio_enable_dma(void *owner);
int centauri_radio_enable_irqs(void *owner, unsigned int count);
u64 centauri_wifi_resource_dma_mask(void);
int centauri_radio_config_read(void *owner, u16 offset, u32 *value);
int centauri_radio_config_write(void *owner, u16 offset, u32 value);
int centauri_wifi_start(void *owner, void *bar, u64 resource_dma,
			size_t resource_bytes, u8 *arena, u64 arena_dma,
			size_t arena_bytes);
int centauri_wifi_queues_create(void *bar, u8 *arena, u64 dma, size_t bytes,
				void **state);
int centauri_wifi_queues_poll(void *state, u32 budget);
void centauri_wifi_queues_destroy(void *state);
int centauri_wifi_interface_start(void *state, const u8 *address);
int centauri_wifi_interface_stop(void *state);
u32 centauri_boot_read(void *bar, size_t offset);
void centauri_boot_write(void *bar, size_t offset, u32 value);
void centauri_boot_publish_dma(void *owner);
int centauri_boot_enable_irqs(void *owner, unsigned int count);
u64 centauri_boot_time_us(void);
int centauri_boot_config_read(void *owner, u8 function, u16 offset, u32 *value);
int centauri_boot_config_write(void *owner, u8 function, u16 offset, u32 value);
void centauri_boot_sleep(void);
void centauri_control_dma_wmb(void);
void centauri_control_dma_rmb(void);
int centauri_control_start(void *owner, void *bar, u8 *context,
			   size_t context_bytes, u64 context_dma, u8 *arena,
			   size_t arena_bytes, u64 arena_dma);
void *centauri_control_service_create(void *bar, u8 *context, u64 context_dma,
				      u8 *arena, u64 arena_dma);
int centauri_control_service_poll(void *state);
void centauri_control_service_destroy(void *state);
int centauri_bootstrap_layout(const u8 *image, size_t bytes, size_t *allocation);
int centauri_bootstrap_radio_layout(const u8 *image, size_t bytes, u8 function,
				    size_t *offset, size_t *length);
int centauri_bootstrap_allocation(const u8 *image, size_t bytes,
				  u8 *output, size_t output_bytes);
int centauri_bootstrap_image(void *owner, void *bar, const u8 *image,
			    size_t bytes, u64 image_dma, u8 *record,
			    u64 record_dma, u32 platform, bool runtime,
			    u64 companion_dma, size_t companion_bytes);
void *centauri_map_ram(u64 physical, size_t length);
void centauri_unmap_ram(void *address);
void *centauri_map_io(u64 physical, size_t length);
void centauri_unmap_io(void *address);
void centauri_barrier(void);
struct device;
struct pci_dev;
int centauri_control_copy_radio(struct pci_dev *consumer, void **data,
				dma_addr_t *dma, size_t *bytes);

/* Physical resources supplied by PCI enumeration and the function's DT node. */
struct centauri_resources {
	u64 bar;
	u64 bar_length;
	u64 dart;
	u64 dart_length;
};
struct centauri_handoff {
	const u8 *data;
	size_t size;
};
int centauri_pci_prepare(struct pci_dev *pdev, struct centauri_resources *resources);
int centauri_load_handoff(struct device *dev, struct centauri_handoff *handoff);
void centauri_release_handoff(struct centauri_handoff *handoff);
void centauri_queue_rx(void *glue, const u8 *frame, size_t length);
u32 centauri_tx_metadata(void);

int centauri_rust_create(void *glue, const u8 *metadata, size_t length,
			void **state, u8 *mac, const struct centauri_resources *resources);
void centauri_rust_destroy(void *state);
int centauri_rust_xmit(void *state, const u8 *frame, size_t length);
int centauri_rust_poll(void *state, int budget, u32 *tx_done);
int centauri_bt_register(void);
void centauri_bt_unregister(void);
u64 centauri_bt_now_ms(void);
void centauri_bt_enable_dma(void *owner);
int centauri_bt_config_read(void *owner, u16 offset, u32 *value);
int centauri_bt_config_write(void *owner, u16 offset, u32 value);
int centauri_bt_rust_create(void *owner, void *bar, u8 *cpu, u64 dma,
			    size_t bytes, u64 resource_dma, size_t resource_bytes,
			    void **state);
void centauri_bt_rust_destroy(void *state);
int centauri_bt_rust_send(void *state, u8 packet_type, const u8 *frame,
			 size_t length, bool setup);
int centauri_bt_rust_poll(void *state, int budget);
void centauri_bt_queue_rx(void *glue, u8 type, const u8 *frame, size_t length);

/* Cold cfg80211 scan transport; all state calls are serialized by C. */
int centauri_wifi_scan_start(void *state, void *owner, u8 cookie,
			     const u32 *channels, size_t count);
void centauri_wifi_scan_detach(void *state);
int centauri_wifi_set_country(void *state, const u8 *country);
void centauri_radio_scan_frame(void *owner, u16 frequency, s8 signal,
			       const u8 *frame, size_t bytes);
void centauri_radio_scan_done(void *owner, bool aborted);

/* Private input selectors, distinct from firmware/IEEE AKM selectors. */
enum centauri_security {
	CENTAURI_SECURITY_PSK,
	CENTAURI_SECURITY_SAE,
};

/* Credential-bearing input. The adapter frees it with kfree_sensitive(). */
struct centauri_connect_params {
	u8 ssid[32];
	u32 ssid_bytes;
	u8 bssid[6];
	u16 channel;
	u8 pmk[32];
	u8 rsn[255];
	u32 rsn_bytes;
	u32 security;
	u8 password[63];
	u32 password_bytes;
	u32 h2e_only;
};

bool centauri_wifi_rsnxe_h2e(const u8 *capabilities, size_t bytes);

int centauri_wifi_connect_start(void *state, void *owner,
				const struct centauri_connect_params *input);
void centauri_wifi_connect_detach(void *state);
void centauri_radio_connect_done(void *owner, const u8 *bssid, u16 status);

size_t centauri_wifi_data_bytes(void);
int centauri_wifi_data_create(void *basic, void *owner, void *bar,
			      u8 *context, u64 context_dma, u8 *data, u64 dma,
			      size_t bytes, void **output);
int centauri_wifi_data_xmit(void *state, const u8 *frame, size_t bytes);
int centauri_wifi_data_poll(void *state, u32 budget, bool *tx_done);
void centauri_wifi_data_destroy(void *state);
void centauri_radio_receive(void *owner, const u8 *frame, size_t bytes);
#endif
