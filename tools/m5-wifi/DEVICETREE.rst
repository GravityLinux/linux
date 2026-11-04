.. SPDX-License-Identifier: GPL-2.0-only

Audience: For LLMs; working evidence pending human review.

Centauri Linux device tree
==========================

For LLMs: hardware-description work from saved operator ADT and owned runtime
PCI/queue observations only. No Apple executable contents were inspected.

This page describes the older retained-state transport. Native cold startup
uses ``t8142-centauri-cold.dtsi`` and the control/radio bindings; see
``COLD-START.rst``. Its enabled native DART, coherent DMA and control-supplier
links are a different ownership contract from the handoff nodes below.

Linux now binds PCI functions described by DT, rather than creating synthetic
platform devices at module load. The drivers have no root-compatible, chip-ID,
serial-number or fixed MMIO-address comparisons. They match PCI 106b:1902 and
106b:1903 and require the corresponding enabled DT function node. BAR0 comes
from PCI enumeration. The firmware-owned DART status resource comes from the
function's ``apple,firmware-dart`` phandle. Metadata addresses must agree with
these resources; that is a stale-handoff check, not a device identity gate.

This integration builds against Linux 7.3 and its trees validate with dtc and
dtschema. It has not yet been boot-tested. The earlier working curl/pair/data
proof used the platform adapter; the Rust packet protocol is unchanged here.

ADT information and ownership
------------------------------

``t8142-centauri-pci.dtsi`` is the static, disabled-by-default topology for the
saved T8142 ADT. Addresses are translated through ``/arm-io/ranges``; blindly
copying ADT register cells would put most resources at the wrong CPU address.

====================== =========================== =============================
Resource               Value                       Source
====================== =========================== =============================
ECAM                   0x1cb0000000 / 0x10000000    /arm-io/apcie reg 0
Bus range              0..8                        /arm-io/apcie bus-range
32-bit PCI aperture    0x80000000 -> 0xb80000000   ADT ranges, size 0x40000000
64-bit prefetch window 0xbc0000000 -> 0xbc0000000  ADT ranges, size 0x20000000
DART register bank     0x488000000 / 0x20000       /arm-io/dart-apcie0 reg 0
DART interrupt         AIC 1463                    /arm-io/dart-apcie0 interrupts
Control/WLAN/Bluetooth  01:00.0 / .1 / .2           ADT children + PCI observations
Runtime PCI IDs        106b:1901 / 1902 / 1903      Runtime enumeration
Function DART streams  1 / 2 / 3                   Runtime RID-to-SID observation
Factory MAC addresses  Injected per device         ADT alpha and bluetooth nodes
====================== =========================== =============================

ADT PCI ranges store ``flags, low, high``. Linux DT requires ``flags, high,
low``. The generator explicitly converts this order. The numeric DART stream
IDs are runtime evidence: ADT mapper phandles alone do not establish them.
The active Wi-Fi station address can be randomized and need not equal the
factory ``local-mac-address`` copied into DT.

The existing Linux Apple PCIe host binding does not yet cover this T8142
controller. The present tree uses the documented firmware-initialized generic
ECAM binding. It assumes the existing bootloader has already powered/trained
the port, assigned BARs and configured all live DART translations. It does not
claim native T8142 host-controller initialization. No MSI, native IOMMU attach,
PHY, clock, power-domain or reset operation is requested by this polling path.
Generic ECAM enumeration and resource adoption still require live validation.

For later native host ownership, the ADT additionally supplies APCIE register
banks 1..25 (RC, shared PHY, PHY-IP and per-port registers), power-gate IDs
76/74/109/110/144/152/155/156, port IRQs 1462/1471/1480, MSI base 1741/count32,
PERST on GPIO0 pin192, CLKREQ on GPIO0 pin164, and SMC keys ``gP13`` and ``bpli``.
The centauri error-detect signal is AOP GPIO pin45. These are not encoded as
invented Linux reset/clock bindings: native provider support, polarity and
ownership must be established before they are used. Firmware owns them here.

Current-boot queue handoff
---------------------------

Each function's ``memory-region`` references immutable typed host metadata,
with a fixed ``reg`` populated by the loader. The driver reads CENH0001 for
Wi-Fi and CENB0002 for Bluetooth directly from that reserved RAM. It no longer
loads these metadata files through request_firmware. Do not set ``no-map``:
the implementation requires reserved normal RAM, mapped Normal-NC. The DMA
allocations/page tables referenced by the metadata must also remain reserved.

Keep the DART node disabled. The native DART driver would reinitialize live
translation domains; the handoff driver refuses a DART node marked available.
``apple,firmware-dart`` supplies a read-only error-status resource and is not
an ``iommus`` attachment. Native IOMMU takeover needs a separate implementation.
The YAML binding documents this temporary ownership contract explicitly.

``generate_centauri_dt.py`` works offline, using a saved ADT and current-boot
Wi-Fi/Bluetooth metadata. It writes a self-contained DTS using the minimal J813 base, two metadata
blobs, and a physical load manifest. All retained DMA/page-table reservations
from the input are emitted as memreserve entries. Its staging ranges
must be separate, reserved, outside Bluetooth DMA and in the unused tail of the
host-owned Wi-Fi TX arena. Packet slots occupy offsets below 0x60000; the
example below uses offsets +0x100000 and +0x108000. Pass the corresponding
physical addresses from the current boot; allocator addresses are not static
board properties::

    PYTHONPATH=/path/to/bootloader/proxyclient python3 generate_centauri_dt.py \
        /private/current-adt.bin /private/wifi.json /private/bluetooth.json output \
        --wifi-handoff-physical "$WIFI_METADATA_PA" \
        --bluetooth-handoff-physical "$BLUETOOTH_METADATA_PA"
    dtc -I dts -O dtb -i /path/to/tools/m5-wifi \
        -o output/centauri.dtb output/centauri.dts

The bootloader must copy each manifest blob to its physical address and clean
that range to the point of coherency before entering Linux with the generated
DTB. The loader must also preserve its own resident code, stack and EL2 allocations,
plus kernel/initrd/FDT regions, as in the existing launcher. The generator
preserves the DMA/page-table reservation list; its two metadata nodes do not
replace those reservations. Preserve firmware resource
assignments via the generated ``/chosen/linux,pci-probe-only = <1>`` property,
and avoid PCIe power/service
changes (``pcie_aspm=off pcie_ports=compat``). This needs live validation before
being treated as the new working boot procedure. It does not boot the chip or
associate Wi-Fi from cold.

Cold image-transfer ownership
------------------------------

``t8142-centauri-cold.dtsi`` is the separate initial image-transfer profile.
Its enabled DART node and PCI-host ``iommu-map`` give Linux a normal DMA
domain. Use a16KiB kernel: the4KiB prototype kernel cannot create translated
domains for this16KiB DART. No reserved queue metadata or ``firmware-dart``
property is consumed by the cold probe.

The host uses ``apple,t8142-pcie``, with ECAM, RC, port0 and phy0 resources
from the ADT. It shares the existing Apple driver's MSI domain, interrupt handling
and requester-routing allocator. MSI ranges describe AIC1741..1772 and the
port interrupt is AIC1462; DART uses AIC1463. INTx routing follows the existing
Apple port interrupt-controller binding. The initial host link remains
firmware-trained; Linux now performs the per-port clock/reset transition
between image stages. It never adopts prior wireless firmware, contexts or
DMA mappings. Linux218/219 verify automatic initial4/runtime2 startup.
Full root/PHY initialization from power-off remains unfinished.

The physical control node remains the same across PCI1900/1901 personalities,
so its primary compatible is ``apple,t2026-centauri-control`` and the PCI
personality aliases are also listed. It supplies ``firmware-name`` plus the
provisional packed host platform identity described by the new control
binding. For J813 that value is board24/chip8142. The three external device
input sources remain unresolved; this test tree is not a completed upstream
cold wireless integration. Wi-Fi and HCI nodes will be enabled after the
generated radio paths are connected to the kernel.

Compared with the older firmware-owned handoff, Linux now owns the page tables,
MSI allocation and CAM entries. The kernel selects CAM slot0 for RID0100/SID1;
the prototype's slot1 was not a hardware requirement. T8142 implements16 CAM
slots, independently of its19 DART streams. Its Linux42-bit domain uses TCR9,
while the earlier prototype's narrower mappings used TCR1. Existing Apple
host variants keep their clock/reset path; the new link-preserving path is
selected only for the T8142 host compatible.

The cold profile now supplies a normal GPIO0 provider at0x39a000000/0x100000
with224 pins, AIC347..353 and ``gpio-reserved-ranges = <78 1>, <203 1>``.
Those exclusions decode J813 ADT's ``no-restore-gpio-pins`` as two u32 pin
indices; they are not a scalar mode mask. Port0's active-low ``reset-gpios``
selects pin192. Named phy0 is0x48f010000/0x4000. These are platform DT values,
not constants in the wireless driver. The legacy GPIO register-cache snapshot
reads excluded pin78 and faults; sparse on-demand regcache and the standard
reserved-line mask avoid those accesses.

The native host reset callback is selected by the T8142 compatible and uses
the port's descriptor/resources. It requires the old endpoint/requester
mapping to be removed before reset. An independent worker holds PCI/module
references across bootstrap removal and standard rescan; no device object's
vendor/device ID is patched in place. Other host compatibles retain their
existing reset/clock path, while the generic GPIO cache improvement avoids
bulk snapshots on all devices. Do not copy J813's reserved-pin indices to
Neo/other boards without their own ADT evidence.

The cold Wi-Fi node now carries the wifi0 alias. Existing loader address
fixup copies the current ADT chosen mac-address-wifi0 into local-mac-address;
Linux uses its standard firmware MAC lookup and validates activation by
readback. The generic driver can generate a local unicast fallback. No serial
or board check is added. Trials246/247 verify the property and native request
against the actual ADT address. This is independent of Broadcom's wlan
calibration-node fixup, whose source node is absent on this J813.
