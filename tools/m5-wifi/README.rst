.. SPDX-License-Identifier: GPL-2.0-only

Audience: For LLMs; working evidence pending human review.

M5 Centauri Wi-Fi and Bluetooth bring-up
========================================

The current task is complete independent cold startup and standard Linux
integration. See ``COLD-START.rst`` for acceptance, experiments and remaining
work. The retained-state traffic proofs below do not satisfy that objective.

The cold control driver now boots both firmware stages, performs the native
PCI reset/removal/rescan transition, and publishes generated control IPC and
four shared queues using Linux-owned coherent DMA. See ``COLD-START.rst`` and
``t8142-centauri-cold.dtsi``. It additionally copies the current Wi-Fi firmware
resource into PF1-owned DMA and publishes a generated Wi-Fi IPC context, with
native MSI and control-supplier lifetime ordering. Generated basic queues
and management now activate a factory-address interface and service received
credits through repeated wraps. Native cfg80211 passive2.4GHz scans now
return real kernel BSS results, including the test network. The single-BSS
WPA2-PSK/CCMP path also associates, disconnects and reconnects through normal
nl80211 PMK inputs. Native Ethernet queues now pass fresh DHCP, google.com
HTTP301, reconnect and sustained HTTP traffic across all data-ring wraps.
The normal authorization event is verified. Cold Bluetooth now generates its
own context/four HCI rings, initializes the factory address from DT, pairs
freshly and exchanges encrypted standard SDP data through Linux sockets.
Native267 additionally qualifies unmodified wpa_supplicant2.11, fresh DHCP/
google.com, normal disconnect/reconnect and concurrent encrypted Bluetooth
data after a full Linux GPIO/PCI reset. Native275 additionally connects to
the SAE-only Pixel hotspot with an ordinary supplicant, required PMF and CCMP,
obtains fresh DHCP and returns google.com HTTP301. Normal SAE reconnect and
concurrent encrypted Bluetooth SDP responses pass on that same kernel.
Native298 additionally qualifies the standard supplicant ``sae_pwe=1``
H2E-only policy: capable-peer admission, explicit RSNXE, an observed H2E
commit, required PMF, fresh DHCP, HTTP301 and reconnect. SAE BOTH and WPA2
regressions pass on that kernel. Forced hunting/pecking remains unsupported;
see ``COLD-START.rst`` for the implementation and negative-test scope.
The first SAE/AKM negatives and Python qualification remain documented.
Broader bands/security/link-loss,
Bluetooth profiles and automatic power sequencing remain pending. The older packet
transports below use a reserved-memory handoff; see ``DEVICETREE.rst`` and ``t8142-centauri-pci.dtsi``.

Branch ``bringup/m5-wifi-7.3`` starts at Linux v7.3,
``c03c1a14507dfaf8b856c2608f9ac216021fc222``. The initial Rust kernel driver
in ``drivers/net/wireless/apple/centauri`` now registers ``cen0`` on the M5.
It returns HTTP 301/219 bytes and HTTPS 200/87,125 bytes through that kernel
interface. Its Rust core owns register access, descriptor encoding, DMA
translation, completion processing and receive recycling. C adapts Linux
net_device/NAPI and reserved-memory mappings, since this kernel's Rust APIs
do not yet provide net_device operations. There is no Python/TAP packet
process in this test.

Bluetooth was developed and probed in Python first. A newly generated 16-KiB
host DMA page with four two-slot rings passed fresh pairing and 2,000 encrypted,
changing-payload 512-byte echoes with the local Mac Mini. The Rust port then
registered native ``hci0`` and repeated that full exact-data test, with the
Python hardware backend stopped. Wi-Fi HTTPS returned 200 in the same boot.
The controller reported zero RX/TX errors. See ``bluetooth-protocol.rst`` for
the bounded protocol findings, experiments and remaining limitations.

The tested packet paths are now kernel Rust drivers with C adapters for
net_device/NAPI, HCI/skbs and generic reserved-memory mappings. Bluetooth uses
one newly generated page instead of the original 121 SID3 pages, and an empty
FF0B replaces the captured startup parameter block. No auxiliary queue bodies
or vendor event bodies enter the driver. The current-boot Bluetooth handoff is
224 bytes of named identity/address/geometry fields.

Driver loading and the Python tools do not compare system serial numbers.
Serial fields remain informational in the existing handoff formats; PCI/DT,
current-boot nonce, memory and queue validation still apply. Proxy tools require
an explicit ``M1N1DEVICE`` selection without a device-node whitelist.

The preceding cen0 and reserved-memory HCI results are historical: those
versions adopted firmware/association state and a Python-created Bluetooth
context. Current cold wlan0/HCI instead use Linux-owned firmware loading and
fresh DMA/context/rings; no previous owner or reserved-memory radio handoff
is accepted. Antenna-SKU metadata now generates NVRAM. An experimental AP-policy
encoder uses DT values and a fresh record4 input; cold259 passes both radios,
but that field's meaning remains unassigned. Calibration now comes from this
device's BWC3 factory file: ``--calibration-factory-record`` extracts unchanged
data blocks and generates the host directory. The resulting complete image
matches cold259 exactly, without captured host-state inputs. Native264 additionally resets the complete chip and reloads both radios through
Linux SMC/GPIO and PCI APIs. Automatic power sequencing, runtime DT assembly
remain open. Ordinary supplicant association/reconnect now passes in native267
with the passive_scan setting. EL2 still provides console and wait-instruction
workarounds in the private cold tests. The earlier long-idle Wi-Fi firmware
error and the full meaning of
Wi-Fi TX metadata bit 17 remain open. Meeting the curl/pair/data bring-up checks
does not establish production stability or complete chip protocol coverage.

Development began with a standalone host Python driver, then a Linux-local
Python prototype, before the Rust kernel port. The sections below record
those experiments in order. Linux-side code is GPL-2.0-only; bootloader-side
changes remain GPL-2.0-or-later and TAINTED. Apple binary inspection is excluded
from this Wi-Fi/Bluetooth work; evidence comes from typed runtime hardware
observations, owned transfers and controlled probes.

``centauri.py`` is the direct transport prototype. It reads the root
configuration, follows a bounded capability list and checks link state before
accessing the advertised downstream buses. The default probe only reads state.
``--initialize-root`` enables controller power, applies the ADT PHY and
PHY-IP tables, performs the observed clock handshakes and releases the port
reset. ``--start-link`` also releases the chip reset through the bridge's
SMC key and GPIO PERST, then trains the link. ``--assign-bars`` sizes and
assigns the endpoint BARs and reads the startup bank; endpoint DMA remains
disabled. These operations do not provide a working network interface.

Fresh-reboot direct validation (``direct-bars-05-startup-bank.log``) reaches
root ``106b:100c``, endpoint ``106b:1900``, an active Gen1 x1 link, and
successful startup-bank reads with no hypervisor or macOS running. BAR0 is
64 KiB at PCI ``0x82000000`` / physical ``0xb82000000``; BAR4 is 32 MiB at
PCI ``0x80000000`` / physical ``0xb80000000``. BAR4 contents are not read.
The startup bank at BAR0 + ``0x8000`` returns the same four identity words
as the reference trace. Reading BAR0 + ``0`` at this stage faults; that
later-stage bank is deliberately excluded from the cold probe.

The validated root sequence applies PHY-IP calibration at ADT bank 4 and
prepares the GPIO pad before enumeration. It matches the reference clear of
offset ``0x90`` bit 16; a controlled test establishes that this clear is not
required for root initialization or Gen1 x1 link training. Root-ID reads
alone did not establish complete PHY initialization. See ``REGISTERS.rst``
for meanings, evidence, and provisional
interpretations. This cold probe alone does not establish command queues,
association or a network interface; later retained-state tests appear below.

``probe_bootstrap_dma.py`` owns its DMA pages and SID 1 translation. It can
submit empty/zero-buffer experiments or transfer an opaque firmware asset
unchanged. The validated opaque image reaches image-ready status 4 in about
139 ms, with no DART errors. A port-only warm restart then enumerates control
1901, Wi-Fi 1902 and beta 1903 at Gen3 x1, and allocates their BARs.
The image is copied and hashed as data; its code is not inspected or decoded.

After a fresh reset and RAM-chainload, the tested workflow is::

    M1N1DEVICE=/dev/cu.usbmodemJMQ12P63RJ1 PYTHONPATH=/path/to/bootloader/proxyclient \
      /path/to/bootloader/.venv/bin/python tools/m5-wifi/probe_bootstrap_dma.py \
      --enable-msi --reference-pcie --reference-aux \
      --opaque-payload /private/path/centauri-runtime-26B123-opaque.bin \
      --boot-runtime --shell

The default IOVAs (record ``0x01000000``, image ``0x02000000``) and the
independent reference allocations both pass full cold-boot validation; they
are host-chosen addresses, not chip-register offsets. ``--boot-runtime`` performs the warm restart only
after image-ready and keeps chip power on. ``--shell`` preserves the current
Python heap and proxy for subsequent protocol work. Endpoint bus mastering
and its DMA stream are disabled after the image transfer; runtime functions
enable memory decoding while leaving their DMA off.

The experimental ``--runtime-image`` step resubmits the retained, owned image
and record pages through runtime control's ``+0x1070`` doorbell. It does not
rewrite their contents, preserving any device updates. A separate
``--runtime-opaque-payload`` tests a new opaque copy instead. These experiments
currently reach control state 3 rather than the reference's state 2, with no
DART error; they do not establish successful runtime firmware initialization.
The same negative result occurs with the captured second-stage asset at its
original IOVAs. ``centauri_ipc.py`` reconstructs the observed host context and
publish handshake, but requires runtime image state 2 before DMA enable and
has not yet been exercised successfully on hardware.

``--runtime-boot-buffer /private/path/companion.bin`` adds the observed opaque
``0xae3c00``-byte companion buffer, mapped at a separate owned IOVA. It
publishes that address/length at PCI configuration +0xf88/+0xf8c/+0xf90 and
the observed Unix-microsecond timestamp at +0xff8/+0xffc, with register
readbacks. The first direct replay of these fields still reaches state 3;
correct publication alone does not establish successful firmware startup.
The companion format is not decoded.

``--unmapped`` and ``--payload-unmapped`` exercise missing mappings with
host-owned zero records. Empty records return submission status 5; the tested
16-KiB candidate records return 2. Their precise error-code meanings remain
unresolved. ``--reuse-link`` skips the cold link reset for an already trained,
unsubmitted bootstrap function. Use a fresh boot for independent experiments.

The operator confirmed RID 1 and ``/dev/cu.usbmodemJMQ12P63RJ1`` for proxy
access, and ``/dev/cu.usbmodemJMQ12P63RJ3`` for the separate macOS UART.
Confirm the target before reusing these instructions in another session.

Evidence and source boundary
----------------------------

New Linux work uses runtime hardware observations, device-tree metadata and
permitted source code. No Apple binaries, firmware instructions, executable
pages or disassembly are inspected. The existing private hypervisor used to
collect observations contains TAINTED SPTM/RingGate implementation work;
it is infrastructure, and is not copied into this Linux branch. Any changes
to that bootloader must remain TAINTED. Do not import unrelated private Wi-Fi
implementations or their binary artifacts into this work.

Initial inputs, all local (no web resources used for this initial capture):

* Operator-device ADT captured as ``device-JMQ12P63RJ/adt.bin`` in the
  bootloader's ignored ``build/m5-sptm`` directory. It identifies
  ``centauri-control``, ``centauri-alpha`` and ``centauri-beta`` behind
  ``/arm-io/apcie/pci-bridge0``; alpha carries the Wi-Fi MAC address.
  Each has a separate DART mapper. Naming does not establish the protocol.
* Gravity/Asahi bootloader ``proxyclient/m1n1/adt.py`` for ADT parsing and
  its PCI address-word convention; ``m1n1/hv`` for the tracing API;
  ``m1n1/trace/pcie.py`` for existing standard configuration definitions.
  The existing Broadcom WLAN protocol decoder is not reused.
* Gravity/Asahi ``src/pcie.c`` for comparison with the established PHY clock,
  reset and root-controller sequence. No initialization function has been
  copied into the direct prototype at this stage.
* Linux v7.3 source and standard PCI configuration layout. No custom device
  register meanings are inferred from names alone.
  ``include/uapi/linux/pci_regs.h`` supplies MSI/MSI-X enable bits and
  PCIe Device Control field definitions for transport-state comparisons.

Capture
-------

On the matching, RAM-chainloaded hypervisor build, supply
``--script /absolute/path/to/tools/m5-wifi/trace_centauri.py`` to the existing
macOS guest launcher. Use its ``--no-disassembly`` option (private TAINTED
bootloader checkpoint ``9c55a2b2``) to prevent automatic fault-site code reads.
Set ``CENTAURI_TRACE`` to a new JSONL filename; opening
an existing file fails rather than overwriting it. Original macOS and Exclave
images are loaded by the existing launcher. This script does not open them.

The script observes the internal controller's full advertised configuration
aperture. Observed BAR assignments install asynchronous traces on their first
16 KiB only. That coverage is provisional, not a claim about actual BAR size.
Controller registers and guest DART MMIO are also traced; EL2 monitor operations
bypass stage 2 and are not captured by the DART observer. IRQ events appear in
the hypervisor log. Unidentified PCI memory apertures receive read-address-only
discovery, with no returned values or write payloads preserved. At most 32
distinct offsets per 16 KiB page are emitted, plus periodic read counts.

``CENTAURI_FOCUS_PAGES`` accepts a JSON array of hexadecimal physical-page
strings. It traces the first 256 bytes of each selected page with values.
``CENTAURI_FOCUS_OFFSETS`` optionally selects a JSON array of hexadecimal
offsets within each page, defaulting to ``["0x0"]``. The successful two-CPU
boot-07 address-only write capture found an additional bank at offsets
``0x1000`` through ``0x107c`` on these pages. A follow-up capture therefore
uses offsets ``["0x0", "0x1000"]``, keeping both windows bounded to 256 bytes.
``CENTAURI_WRITE_DISCOVERY`` accepts JSON pairs of hexadecimal address and
size strings; those ranges record write addresses with no payloads. Both
options reject ranges outside the captured internal PCI apertures.

Observed boot-04 control initialization, not a decoded ABI:

* Pages ``0xb82008000`` and ``0xb82028000`` return matching words at offsets
  4, 8, 12 and 16, including ``0x900681`` and ``0x20260840``. Their roles
  are unassigned; no firmware contents were examined to interpret them.
* Both receive low/high-looking writes at offsets ``0x44``/``0x48`` and
  ``0x20`` at ``0x4c``, followed by a read of 1 at ``0x40``. The first
  observed low words are ``0x30c4000`` and ``0x3bac000``. These are candidate
  DMA addresses, not established ring definitions.
* Pages ``0xb82000000`` and ``0xb82010000`` also receive candidate address
  words. Device status observations transition through 0, 1 and 2.
* Interrupt vectors 1741, 1745 and 1759 occur in the advertised MSI range.

Boot-04 uses the explicit no-disassembly guard. An earlier failed capture
triggered the old hypervisor's automatic short fault-site disassembly. That
exception text is excluded from Linux evidence; no instruction data from it
is used for these hardware observations or the driver.

JSONL sequence numbers order host callback receipt. ``host_ns`` is host
monotonic receive time, not a device timestamp or cross-CPU ordering proof.
No tracer callback proactively reads device or guest memory. Synchronous
configuration writes pass through the existing hypervisor's normal MMIO path.

Two optional reference scripts have narrower, explicit metadata reads:
``trace_centauri_power.py`` reads at most four bytes of SMC shared SRAM for
only the reset keys named by the device ADT. ``trace_centauri_bootstrap.py``
observes the initial fixed-size startup submission and reads DART page-table
metadata to establish its DMA address translation. Its page-table reader
rejects non-page sizes, unaligned addresses and addresses outside DRAM. It
does not read submission contents by default or follow any submission pointers. It also
snapshots standard endpoint PCI configuration before submission, including
command, MSI/MSI-X message control and PCIe Device Control. Boot-10
confirms the control endpoint's RID ``01:00.0`` uses SID 1; the candidate
32-byte buffer is mapped there. See ``REGISTERS.rst`` for that evidence.

``CENTAURI_BOOTSTRAP_RECORD=1`` additionally captures exactly the 32-byte
initial runtime submission record, immediately before the submit write.
This selects the bootstrap device 1900 and only the stream named by its
RID-to-SID mapping. The translated range must be exactly 32 bytes inside
DRAM. Embedded addresses and any referenced firmware payload remain opaque;
the capture never follows them. Larger transfers are rejected. This option
decodes runtime protocol data, not executable images or instructions.

``CENTAURI_RUNTIME_RECORD=1`` enables a separate observer for the relocated
runtime control interface at physical ``0xb82028000``. It reads the fixed
32-byte image record at doorbell ``+0x1070`` and the reused control allocation
at handshake ``+0x107c = 2`` and the first ring-index doorbell ``+0x1000``.
Function 0 must be device 1901 and its RID-to-SID mapping must resolve inside
DRAM. A recognized context signature allows a bounded ``0x3b0``-byte read of
host IPC metadata within the same mapped allocation page. Arbitrary embedded
pointers are never followed. The observer also snapshots all three runtime
functions' standard MSI address, data and message counts.

``CENTAURI_PCI_RECORD=1`` additionally snapshots the root and present endpoint
functions' 4-KiB PCI configuration registers before each image transfer.
At the first observed ready state, it samples nine previously observed vendor
configuration words to distinguish changes during transfer from port reset.
This covers standard and vendor configuration fields, not BAR memory or
executable contents. It helps compare setup that macOS accesses through the
controller's PIO-DMA engine rather than directly through ECAM.

``CENTAURI_CONTROL_RECORD=1`` samples the first 256 bytes of the known
bootstrap/runtime control register bank before each observed submission and
at the first image-ready state. This compares diagnostic words across
successful reference boots and failed standalone attempts. It reads no
firmware memory and follows no addresses returned by those registers.
Runtime state 3 also triggers a single diagnostic snapshot when observed.

``trace_centauri_control.py`` is an additional, explicitly selected observer
for the first sixteen global control doorbells. It validates the published
context signature, captures its bounded ring metadata, and reads only the
four- or 52-byte host control frames advertised by recognized descriptors.
The initial 52-byte command also permits a 64-byte prefix of its fresh host
queue allocation. No further pointers or packet/firmware payloads are read.
DART initialization is deferred until the runtime interface is powered and
IPC state is 2. Page-table and data translations must remain inside DRAM.
Boot-30/31 captured the eight initial commands and zeroed queue prefixes;
their field meanings remain provisional in ``REGISTERS.rst``.

``centauri_pio.py`` recognizes the six observed boot-buffer configuration
writes in root PIO command prefixes. It returns bounded metadata only; it
never follows source pointers. Unknown command shapes and destinations are
rejected before any caller may read a configuration word.

``centauri_queues.py`` maintains the observed external-buffer and inline
queues. ``DataArena`` reserves separate ring storage and receive pools before
control transfers begin. Its mappings stay fixed while queues are active.
``DataQueue`` publishes receive credits, sends inline runtime messages and
reads completed owned buffers. Queue 8's payloads and larger queue-2 reports
remain opaque. Runtime-message helpers decode the length/type and sequence
fields without assigning meanings to undocumented query numbers.

Bare target-40 validates the firmware greeting and query types 9, 8 and 10,
receive-buffer recycling, inline-ring wrap and the maintained queue methods.
This still requires reference firmware preparation and retained boot buffers;
the cold runtime transfer continues to reach state 3. Wi-Fi's startup fields
remain zero, and no association or Linux network interface is claimed.

``CENTAURI_PCI_BOOT_WATCH=1`` (with ``CENTAURI_RUNTIME_RECORD=1``) samples the
known vendor configuration words when the guest reads PIO register ``0xdc``.
It runs only between the first image submission and runtime ``+0x1070``,
checks root topology, link state and endpoint ID, and caps observations at
4096. Only changed configuration words and candidate DART translations are
recorded. It never reads the PIO engine's unknown descriptors/data buffers.
``CENTAURI_PCI_BOOT_OPAQUE_EXPORT=/private/path/boot.bin`` explicitly copies
the observed ``0xae3c00``-byte transfer candidate once, with complete DART and
DRAM checks, outside the repository. These bytes remain opaque; their role
must be established through protocol observations and replay.

``CENTAURI_RUNTIME_OPAQUE_EXPORT=/private/path/stage2.bin`` explicitly enables
the same bounded opaque-copy facility for the second-stage image transfer.
Only the image record's known length and zero reserved fields select this
copy; the IPC context is never treated as firmware. Copies stay outside this
repository, are hashed as opaque data and are never parsed or disassembled.

``CENTAURI_BOOTSTRAP_OPAQUE_EXPORT=/private/path/image.bin`` additionally
copies this build's observed initial transfer buffer as an opaque asset for
replay on the operator device. It requires record capture, a path outside
the Linux repository, a complete DART translation wholly inside DRAM, and
the known 32-byte record with its observed transfer length. The image bytes
are copied and hashed without parsing, decoding, displaying or disassembling
them; no code is derived from their contents. The private export and its
origin/hash metadata are not part of the Linux source or its licence.

Offline tests, using the Gravity virtual environment::

    PYTHONPATH=/path/to/bootloader/proxyclient python tools/m5-wifi/test_trace_centauri.py

Wi-Fi control prototype
----------------------

The Wi-Fi interface is distinct from the smaller ControlApp and sibling
profiles. The target-64 handoff stops every guest CPU at Wi-Fi firmware
state 2, before its first IPC publication. Existing firmware buffers and DMA
mappings stay alive. Python adds an owned two-MiB context/ring arena and
eight-MiB packet pool on stream 2, inside the existing advertised window.
``make_wifi_context`` constructs the initial host metadata, indices and admin
rings. The first handshake publishes this new context and reaches IPC 2.
This still requires opaque firmware preparation by the reference boot.

``centauri_wifi.WiFiControl`` verifies the owned arena, mapping and context
geometry. Its ``submit`` transfers a fixed 44- or 52-byte control record from
separate per-slot storage. It validates independent completion/submission
cursors, acknowledges the previous completion and always writes 1 to the
notification register. The context and its auxiliary area reserve the first
64 KiB; admin command storage begins after that reservation.

``centauri_wifi_queues.WiFiBasicQueues`` configures the six observed basic
queue pairs, posts receive buffers and recycles completions. It uses modulo
arithmetic for the non-power-of-two rings. Queue 1 carries management
requests; queue 2 receives their replies. Their fixed 24-byte runtime
headers carry a transaction identifier echoed by the reply. Selected queries
can explicitly copy their completed management body before recycling its
buffer; other queue bodies remain unread. A numeric command 8
with no body returns a matching 594-byte reply. Its 570-byte body's meaning
is unresolved. Target-64 validates owned initialization and a subsequent
exchange through the maintained queue class. Target-65 repeats the entire
fresh setup through its maintained ``configure`` and ``query`` methods.
No Wi-Fi scan, association,
packet data or Linux interface has been demonstrated.

Target-67 validates increasing transaction identifiers and completed bodies
for commands 8, 10, 30 and 47 through owned queues. The maintained
``centauri_wifi_messages`` helpers decode packed runtime TLVs, the observed
country reply and counted channel words, preserving unassigned fields.
The initial country is XZ with 56 channel words; the configured reference
reports US with 167. Channel flags and regulatory configuration remain work.
Target-70 identifies country setting command 9: an owned US/1 request changes
the country readback and channel count to those configured-reference values,
without a firmware or DART error. Other regulatory fields remain unresolved.

Ethernet transport prototype
----------------------------

``centauri_ethernet.EthernetQueues`` adopts a current registered network-queue
snapshot after the reference guest has fully exited. Firmware, association,
rings and receive allocations stay alive; transmit packets use the separate
owned arena already checked by ``WiFiControl``. The class does not restart
IPC, reset a DMA domain, configure association or inspect firmware buffers.

``latest_queue_registry`` selects only records after the last observed IPC
publication. Logical completion queue numbers differ from their context
cursor indices, and both can change after a reference-driver restart.
Target-73 initially rejected a stale context, then attached the current
publication and its newly numbered queues. Never use earlier ring addresses
after a restart.

The verified transport uses sixteen-byte format-3 receive descriptors,
32-byte format-3 transmit descriptors and 32-byte completions. Transmit
completion format 0x0502 differs from receive format 0x0403. Receive byte
counts include two alignment bytes before the Ethernet frame; transmit
buffers start directly with the Ethernet header. Unknown extended metadata
stays unassigned. Receive recycling is explicitly enabled after draining
the former owner's pending completions. A buffer cookie identifies its
allocation independently of its current submission slot. Adoption builds a
cookie registry from the descriptors, including rings recycled by a previous
prototype instance; duplicate retired descriptors must agree.

Bare target-73 completes DHCP DISCOVER/OFFER/REQUEST/ACK, gateway ARP and an
ICMP echo using its own transmit buffers. An isolated TAP namespace on the
build server supplies a standard host-side IP stack over a framed SSH link
to the Python packet driver. It has no external route besides that TAP.
``curl google.com`` returns HTTP 301 and 219 bytes; HTTPS returns HTTP 200 and
86,976 bytes. Stopping the Python bridge makes the same routed connection
time out, and restarting restores traffic. Firmware and IPC stay 2 with no
DART error.
Repeating HTTPS through the maintained ``EthernetQueues`` class returns
HTTP 200 and 87,124 bytes. The offline tests include actual descriptor vectors, the observed context
restart and the recycled-cookie adoption regression.

Target-77 repeats DHCP, ARP and ICMP, then recycles receive buffers while
forwarding an isolated host-side TAP stack. Two runs of 3,000 and 1,500
1,200-byte gateway pings return every reply (zero loss). Both the 2,048-entry
submission ring and 4,096-entry completion ring wrap. HTTPS after each run
returns HTTP 200 (87,019 and 87,079 bytes). In total 4,845 receive buffers are
reposted, with firmware/IPC 2 and DART error 0 throughout. The firmware echoes
cookie 1 after submission slot 2047, and cookie 2 after slot 0, confirming
that the cookie is independent of the slot. This addresses receive credit
replenishment; the earlier target-73 firmware failure's cause is unproven.
Reattaching the maintained class to those already-recycled rings discovers
2,046 source-12 and 72 source-13 allocations by cookie; gateway ARP and ICMP
then succeed again.

These are Python host-prototype tests. Linux runs on the build server for
this IP-stack test, not on the M5. Those earlier tests do not establish an on-device Linux result; target-82
below supplies that separate evidence. The test uses retained
reference association state and does not supply or record Wi-Fi credentials.

Rust kernel queue driver
------------------------

``CONFIG_CENTAURI=m`` builds the Rust hardware implementation and its
net_device/NAPI and HCI glue. Enable ``CONFIG_RUST`` and ``CONFIG_BT`` and build the module with the
same kernel configuration/toolchain as the Image. The reserved physical
pages are mapped Normal-NC with ``vmap``; all DMA cursor accesses are volatile
and bracketed by arm64 ordering barriers. The kernel owns packet I/O through
``cen0``; the previous Python packet process must be stopped before loading.

``pack_kernel_handoff.py`` writes ``gravity/centauri-handoff``, an explicitly
typed metadata file read through the kernel firmware-loader API. This is our
address/geometry schema, not an Apple image or a saved device-payload dump.
It records current mappings and registered queue geometry, plus identity and
a fresh nonce. The driver validates those fields and builds descriptors from
lengths, addresses and cookies. It never replays captured setup messages.
Reducing this 26,192-byte handoff and the much larger retained DMA allocation
set are separate tasks; a smaller metadata file alone does not release device
memory.

The kernel test uses a 10 ms polling interval while PCIe MSI ownership is
being implemented. The interval is a software bring-up policy, exposed as
``poll_interval_us``; 1 ms polling overwhelmed the tethered debug harness.
With 10 ms polling and ordinary timer accounting, HTTP and HTTPS work through
``cen0`` and sysfs identifies its driver module as ``centauri``.

TX descriptor metadata bit 17 is exposed as a controlled ``tx_metadata``
probe parameter. Clearing it gives a five-second same-IP HTTP timeout while
firmware/IPC remain ready; restoring it immediately returns HTTP 301. The
submission and completion cursors still advance with zero metadata. This
establishes its necessity for the tested Ethernet path, but does not yet
assign a complete firmware-field interpretation. Other fields remain named
only to the extent supported by runtime evidence.

Linux-local Python packet driver
--------------------------------

``centauri_linux.py`` supplies a Linux-local backend for the same
``EthernetQueues`` protocol. It has no bootloader Python dependency.
``centauri_native.c`` supplies arm64 ordering barriers and aligned MMIO
accesses; compile it for the target alongside the script::

    aarch64-linux-gnu-gcc -shared -fPIC -O2 -Wall -Wextra -Werror \
        centauri_native.c -o centauri_native.so
    python3 centauri_linux.py /run/centauri-handoff.json

The private loader exports current runtime metadata, including the exact
system serial, context and queue configurations, SID-2 translation ranges,
reserved physical allocations, DART/BAR0 addresses, owned transmit arena and
configured station MAC. It writes a fresh 32-byte nonce into an unused part
of its owned arena and records that location. The Linux process checks the
nonce, device identity and current IPC context before adoption. Never reuse
metadata after a reset or replace the active DART domain.

The loader must reserve every live DMA allocation and translation-table page
in the FDT reservation map. The test kernel enables ``CONFIG_DEVMEM`` and
``CONFIG_TUN`` and disables ``CONFIG_STRICT_DEVMEM``. ``/dev/mem`` is opened
with ``O_SYNC``: Linux arm64 maps reserved system RAM as Normal-NC, allowing
unaligned Ethernet copies without cached aliases in this process. Do not use
``no-map`` for these reservations; that changes the mapping to Device memory.
MMIO uses volatile 32-bit accesses with ``dsb sy`` barriers. The process
translates SID-2 metadata without reading firmware buffers or walking device
failure pointers. It acknowledges unrelated receive classes without copying
their bodies.

After the process creates ``centauri0``, configure its station MAC, address,
route and DNS from the current DHCP lease. The target-82 RAM filesystem uses
Ubuntu Noble arm64 Python, curl and iproute2 from the official Ubuntu Ports
archive, with package/origin manifests retained privately. The private boot
harness preserves PCIe/DART state, clears its previous guest's interrupt and
timer state, and traps WFI/WFE while Linux boots with one CPU. Linux reaches
``/init`` and reports ``7.3.0-gb90b2984e2f7``. Its interfaces are loopback and
``centauri0``, with the sole external route through that TAP.

The local process then exchanges HTTP/TLS traffic over the M5's Wi-Fi queues:
HTTP Google returns 301/219 bytes, HTTPS Google returns 200/87,132 bytes.
Pausing only that process makes a curl to the same resolved Google address
time out with status 28 and HTTP 000 after 3,002 ms; resuming restores HTTP
301/219 bytes. A subsequent on-device run sends 3,000 gateway ICMP
echoes with 1,200-byte payloads and receives every reply in 66.874 seconds.
The submission ring wraps and a fresh HTTPS request returns 200/87,179 bytes.
Firmware and IPC remain 2 and the DART error status remains 0.
This establishes the on-device Linux ``curl`` criterion for the Python
prototype. Independent firmware startup, authentication and the Rust kernel
port are still required for a standalone driver.

Build workspace
---------------

The operator-supplied build server is ``lab@192.168.170.255``. Existing key
authentication works. The dedicated source and output paths are
``/dev/shm/m5-wifi-linux-src`` and ``/dev/shm/m5-wifi-linux-build``. The remote
branch starts at the exact same Linux release commit; unrelated worktrees
are untouched. These directories are volatile; preserve patches, configuration
and artifacts locally. Monitor free space: the first arm64 defconfig link
ran out of tmpfs space with debug information. Retrying with
``CONFIG_DEBUG_INFO_NONE=y`` retains the same baseline functionality.

No credentials belong in this branch. Supply test network credentials only
to runtime connection tools, keeping literal SSID punctuation as data.

Owned Bluetooth startup
-----------------------

``OwnedBluetoothHost`` in ``centauri_bluetooth_host.py`` initializes fresh
Bluetooth DMA, context and HCI queues after the shared firmware is running.
It consumes no handoff file or previous host context. It requires the Bluetooth
function to have DMA disabled and IPC unpublished; keep the owner alive until
``close()``. For a host proxy session with ``u`` and a prepared ``driver``::

    from centauri_bluetooth_host import OwnedBluetoothHost
    from centauri_bluetooth import command_complete
    bluetooth = OwnedBluetoothHost(u, driver)
    version = command_complete(bluetooth.transport, 0x1001)

Cold153 verifies this path, radio initialization and a real Classic connection.
The runtime allocation is generated from vendor FTAB metadata; preparation of
the primary firmware manifest/device inputs, Wi-Fi startup and the Rust Linux
cold-start port remain pending. See
``COLD-START.rst`` for exact evidence and remaining acceptance requirements.

The firmware probe's ``--runtime-boot-buffer-from-image`` option constructs
the runtime allocation using the supplied image's ``2ftb`` directory entry.
It takes no recorded companion buffer. ``centauri_ftab.py`` reads container
metadata only and leaves all executable payloads opaque.

``prepare_centauri_firmware.py`` assembles an external transfer image from
an opaque vendor FTAB, an explicit signed manifest, three required device
records and an optional boot-parameter record. It preserves vendor components, inserts the manifest into the
expanded directory's following space, appends the named records and pads to
4KiB. It refuses to overwrite an existing output. Container reconstruction
with the native160 manifest/records reproduces the full prepared image exactly.
This does not solve manifest or device-record provenance: those remain explicit
inputs, and no normal source for all device records has yet been qualified.

Use ``--assert-chip-reset`` when qualifying absence of retained radio state.
This asserts the ADT-defined reset before normal link startup releases it;
it rejects ``--reuse-link``. Earlier reboot/PERST tests only released this
line. Explicit-reset165 verifies shared firmware/control and a real Bluetooth
connection, but Wi-Fi remains unready. See the reset qualification in
``COLD-START.rst`` before interpreting earlier Wi-Fi results.

Explicit-reset167 additionally uses ``--bootstrap-d3hot`` before the first
port restart. The same structured firmware and generated runtime resources
then support Wi-Fi IPC/basic queues and parameter/country/channel queries,
alongside another real Bluetooth connection. This observed transition also
enables PME; those two fields' individual necessity is not yet isolated.
Use the latest checkpoint in ``COLD-START.rst``; Linux association/data and
proper sourcing of all firmware device inputs remain pending.

``--bootstrap-d3hot-no-pme`` requires ``--bootstrap-d3hot`` and isolates the
PME-enable bit during that transition. It is a diagnostic profile:169 still
boots shared firmware but fails Wi-Fi startup. Re-enabling PME later in D0
does not recover that run. The working167/168 profile retains PME enable.

Run170 also passes those control-plane and Bluetooth-link tests without the
entire200-byte `_bpb` record. ``--boot-parameters`` is therefore optional in
the packaging tool. The three other device records remain required inputs;
this does not qualify omission for all future Wi-Fi operations or devices.

Generated runtime module
------------------------

``centauri_runtime.RuntimeHost`` owns fresh control, Bluetooth and Wi-Fi
contexts, radio resources and basic queues after the shared image loader.
``probe_bootstrap_dma.py --initialize-runtime-host`` runs it with
``--runtime-image --keep-ready-dma --shell`` and exposes ``runtime_host`` in
the host shell. Use the established explicit-reset/D3hot/PME firmware-loading
profile above. For example, after initialization::

    reply = runtime_host.wifi_queues.query(8, read_body=True)
    bluetooth = runtime_host.bluetooth.transport

The module validates the loader's current owned allocation/domain, derives
radio capacities/offsets from firmware metadata, generates every context and
queue, and publishes a DMA window sized for its own allocations. Its IOVAs
are host choices. It consumes no former driver's context, mapping or handoff
file. Keep the object/image owners alive until reset or ``close()``. Close
stops PCI bus mastering and disables the three owned streams; it does not
resume a previous host driver. The firmware's three device-record inputs
still require proper sourcing. This module does not implement association.

``--detach-bootstrap-domain`` additionally closes the initial image's DMA
before the PCIe restart and creates a fresh runtime domain. Run172 reaches
the same Wi-Fi management and Bluetooth connection results with this option;
its old initial submission/image IOVAs are absent from the new domain. This
qualifies domain independence, not early memory reclamation or Linux PCI
re-enumeration, which still need implementation and direct validation.

Interface activation helpers
----------------------------

The runtime module leaves the firmware interface available for explicit
configuration. Experiment175 validates this sequence through the clean helpers::

    import centauri_wifi_messages as message
    queue = runtime_host.wifi_queues
    address = bytes(u.adt['/chosen'].getprop('mac-address-wifi0'))
    queue.query(message.ADDRESS_TEMPLATE_SET, message.address_template(address))
    queue.query(message.INTERFACE_ACTIVATE, message.interface_operation_body(0, 0))
    reply = queue.query(message.INTERFACE_INFO_QUERY, read_body=True)
    info = message.decode_interface_info(reply['body'])

State2 and a matching address confirm activation, not association. To apply
another address, deactivate, change the template and activate again. The
operation body's individual fields remain under qualification; the tested
five-byte zero form is generated without a captured parameter blob. Keep its
length: an empty activation body faults Wi-Fi. Command19/selector1 and the
existing command47/selector1 currently return identical channel lists.

Independent Python scan
-----------------------

After activation and country selection, ``centauri_wifi_scan`` builds scans
from explicit numeric fields and decodes completion plus standard radiotap
and 802.11 beacon/probe-response data. Experiment 177 finds the test SSID
with this generated request, without native scan settings or SSID records::

    from centauri_wifi_scan import (scan_request, SCAN_START, SCAN_START_REPLY,
                                     SCAN_COMPLETE_EVENT, decode_scan_complete,
                                     decode_scan_receive)
    from centauri_wifi import decode_control_header

    results, completions = [], []
    def receive(metadata, packet):
        header = decode_control_header(packet, metadata['bytes_completed'])
        results.append((header['transaction'], decode_scan_receive(packet[24:])))

    queue.observe_receive(4, 2560, receive)
    queue.observe_control_event(SCAN_COMPLETE_EVENT, 9,
        lambda header, body: completions.append(decode_scan_complete(body)))
    body = scan_request(1, [1, 6, 11], report_frames=True,
                        option_words=(0, 0), timing_words=(20, 20, 110))
    queue.query(SCAN_START, body, reply_command=SCAN_START_REPLY)
    # Continue queue.poll() until the completion's cookie is 1.

The request's cookie is distinct from the management transaction. The scan
acknowledgment uses a different command. Rejections use the original or reply
command with a nonzero status prefix, exposed as
``ManagementCommandError``. Optional observers copy only bounded completed
owned buffers, before recycling credits. No packet pointer is followed.
Callbacks should handle malformed or unsupported radio reports according to
their caller's needs. Raw country-table words carry additional unqualified
metadata and cannot all be used directly as a scan list. The Linux cfg80211
cold-start integration remains incomplete.

Connection format work
-----------------------

``centauri_wifi_connection`` contains the qualified security-prefix encoder
and public connection-outcome decoder. It fixes a private prototype's
one-byte security-field shift: the six-byte prefix starts at 70, key length
at 74 and opaque key at 76. The encoder takes explicit selectors and a
32-byte key; do not log the returned prefix. The outcome decoder extracts
SSID/BSSID and result status from the 90-byte completion. Status 0 in an
acknowledgment is distinct from status 0 in that later connection outcome.
``psk_connection_request`` builds the complete single-BSS request from an
explicit PMK, RSN IE, station target and ``ConnectionParameters``. A fresh
reset187 verifies successful association without a plaintext passphrase
record. The remaining leading byte, word132, SSID flags and five timing
words are explicit unqualified parameters, not inferred semantics.

Firmware AKM selectors are distinct from RSN suite types: the native WPA2
PSK path uses ``AKM_SELECTOR_PSK`` (4), while its RSN IE uses type 2. Native
SAE uses selector 8. Callers must provide firmware selectors, not pass RSN
type numbers through unchanged.

Independent Python Ethernet queues
-----------------------------------

``centauri_ethernet_owned.OwnedEthernetQueues`` allocates new submission,
completion and packet memory inside the live runtime owner's DMA window.
It registers four generated records: one default TX source/completion pair
and one Ethernet RX source/completion pair. No native registry, mapping or
descriptor snapshot is consumed. Logical completion queue IDs differ from
their context cursor indices and are encoded separately.

Explicit-reset186 and PMK-only187 obtain DHCP, resolve ARP and pass HTTP and
HTTPS through these queues. An isolated build-server namespace supplies the
TCP stack for this prototype; this is not Linux-on-M5 acceptance. Keep
servicing RX, basic management, global IPC and HCI during idle periods.
TX bit17 and the profile/service selectors remain qualified values whose
individual meanings need controlled probes. The Rust network/HCI paths still
adopt live firmware and queues. The image transfer port below does not yet
connect those paths to cold boot.

Rust image-transfer port
-------------------------

``drivers/net/wireless/apple/centauri/centauri_bootstrap.rs`` implements the
two image submissions and their distinct doorbells/readiness states. It
publishes generated 32-byte address/length records, host identity, Unix time
and the runtime companion address/size. ``centauri_firmware.rs`` validates
only FTAB directory metadata and generates the zeroed companion allocation
from the vendor layout. Executable payloads and signed manifests stay opaque.

``centauri_bootstrap_glue.c`` supplies PCI enumeration, firmware requests,
coherent DMA allocation, generic configuration/MMIO access and teardown.
The bootstrap/control probes require enabled matching PCI DT nodes with
``firmware-name`` and a Linux DMA IOMMU domain. The initial1900 function also
requires the provisional ``apple,host-platform-id`` property, encoding host
board ID in the upper16 bits and host chip ID in the lower16. These bring-up
properties are not a finalized upstream binding. A disabled firmware-DART
node and retained-domain metadata are not accepted for the image probes.

Linux7.3 compilation and module linking pass on the build server. Seventeen
Rust host checks cover synthetic Python/kernel container agreement, every
fixture truncation, malformed ranges/tags/manifest, generated allocation,
both submission stages, 64-bit DMA, publication failures, wrong peer identity
and bounded timeout. They also reject a pending submission before reprobe,
check MSI-allocation failure before DMA publication and distinguish runtime's
waiting state1 from initial state0. The control node has the stable physical
compatible ``apple,t2026-centauri-control`` plus both PCI personality aliases.

Fresh Linux218/219 now perform both image stages and the intervening host
reset automatically: initial ready4, PCI removal of the bootstrap object,
D3hot+PME, the native T8142 reset callback, normal PCI rescan, then runtime
ready2 at about1.75 seconds. Linux owns coherent DMA, native DART translation,
MSI allocation, requester routing, PERST GPIO and port PHY reset. The loader
prepares only the initial host link; it uploads no N1 image, context or queue.
The console hypervisor maps these hardware resources directly; macOS does
not execute. Native192's USB disappearance remains no runtime evidence.

These probes do not initialize radio contexts or provide cold cfg80211/HCI
operation. The three remaining firmware host-input sources are unresolved.
Normal chip reinitialization after driver unbind remains to be implemented;
the current runtime probe safely rejects used firmware state. No Linux-on-M5
cold networking/Bluetooth acceptance is claimed.

The initial image requires endpoint MSI enabled even when completion is
polled. Fresh201 programs the same address, data and four-message fields but
leaves MSI Enable clear; it fails. Fresh202 enables one message and reaches
ready4. Thus the Rust initial probe allocates one MSI vector; runtime control
retains its qualified four-vector profile. MSI allocation uses the PCI API.
Fresh Linux203 verifies the one-vector change: initial4, submission0, DART0
and two events on its sole MSI vector. Linux radio startup remains unfinished.

The C PCI adapter schedules the identity transition in a separate work owner
with retained PCI/module references. Removing the old PCI device drains its
handlers and releases its coherent allocations before requesting D3hot+PME.
The PCI core resets the empty bus through ``reset_root_port`` and restores
the saved bridge; rescan constructs new device objects and assigns runtime
BARs. The old device identity, image mappings and MSI allocation are not
reused. The reset requires an empty requester map; resets with live consumers
and full root/PHY initialization from power-off are not yet qualified.

The GPIO provider uses sparse flat regcache and standard DT reservations.
The former512-pin cache snapshot aborts on firmware-owned GPIO0 pin78;
J813's ADT also excludes pin203. GPIO192 supplies active-low PERST through
the normal GPIO API. The driver reads controls on demand and gpiolib excludes
reserved lines. Other boards must supply their own reservations.

The Python warm restart now uses the measured port0x88 request/completion
handshake and named clock/reset controls, preserving interrupt/routing state
without the old port/debug presets. Direct214 qualifies it with one initial
MSI vector and reaches shared ready2/generated radio IPC; all DMA is closed
afterward. Link readiness now requires UP and BUSY-clear because213 proves
UP can remain latched with inaccessible PCI configuration. See REGISTERS.rst
and COLD-START.rst for the bit-level comparisons and remaining uncertainties.

Native cold scan smoke test
----------------------------

Build centauri_scan_test.c with a normal static AArch64 C toolchain, then
include it in the Linux initramfs. With the cold radio bound::

    ip link set wlan0 up
    scan-test trigger wlan0
    # Wait for the asynchronous scan completion, then inspect kernel results.
    scan-test dump wlan0

The helper uses standard nl80211 requests and prints SSIDs as hex bytes.
It does not access hardware, initialize firmware or carry network credentials.
Only passive scans are currently supported by the cold cfg80211 port.

The same smoke-test helper accepts country, connect and disconnect requests.
Connect takes public SSID/BSSID hex and frequency arguments, then reads the
PMK from stdin with terminal echo disabled. It never takes a credential in
argv. The private host helper derives the PMK from an anonymous credential
input and sends it only after the target's explicit readiness marker. These
laboratory steps do not replace ordinary supplicant integration. The driver
currently requires a matching, current scan BSS. Native cold Linux255/256 now
qualify fresh generated Ethernet queues, DHCP, google.com HTTP301 and data
ring wraps; COLD-START.rst records the scope and remaining input dependencies.

The helper also accepts ``scan-test events wlan0``. It joins the standard
nl80211 MLME multicast group and reports connection status and authorization
events without dumping key attributes. Native256 observes CONNECT status0
followed by PORT_AUTHORIZED before obtaining its DHCP lease and HTTP response.

The firmware packager can generate nvrm without a captured record. Supply
``--antenna-sku-file /path/to/apple,antenna-sku`` for the standard terminated
DT property, or ``--antenna-sku VALUE`` for an explicit platform value.
``--nvram`` remains an alternative external input, not a required capture.
The M5 ADT-derived result reproduces the qualified firmware image exactly.
Calibration and AP-policy provenance, runtime assembly and unassigned record
profile fields remain open; see the source259 checkpoint in COLD-START.rst.
