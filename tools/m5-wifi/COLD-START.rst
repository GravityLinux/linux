.. SPDX-License-Identifier: GPL-2.0-only

Audience: For LLMs; working evidence pending human review.

Independent Centauri startup and Linux integration
===================================================

Operator request: replace the macOS-initialized handoff with complete cold
startup, keeping the Linux implementation suitable for upstream review.
Develop and explain the hardware protocol in Python first, then port the
verified implementation to Rust. Apple executable payloads remain opaque;
no disassembly, strings scans or firmware-code inspection are permitted.

Acceptance requires a fresh chip reset without executing macOS, driver-owned
firmware loading and DMA allocations, Wi-Fi scan/authentication/association
through cfg80211/nl80211 and ordinary supplicant tools, and normal Linux HCI
controller initialization, fresh Bluetooth pairing and encrypted data tests.
Repeat cold boots, disconnect/reconnect and driver teardown/reprobe. Use
proper PCI resources, DMA/IOMMU and interrupt APIs, DT-described providers,
external firmware resources and device capabilities; no serial restrictions,
retained reference mappings or captured opaque initialization sequences.
Source remains GPL-2.0-only. Bootloader instrumentation remains private and
GPL-2.0-or-later/TAINTED. No claim of upstream acceptance precedes review.

Current checkpoint: native Linux298 reloads both radios after an actual full
chip reset through existing Linux SMC/GPIO and PCI ownership APIs. The Rust
loader owns both firmware stages, fresh DMA/IOMMU domains, generated control,
Wi-Fi packet and Bluetooth HCI queues. Standard net_device traffic obtains
ordinary wpa_supplicant WPA2 and SAE/required-PMF association, fresh DHCP and
google.com HTTP301, including explicit H2E-only policy in298. Native275
also passes fresh Bluetooth pairing and 128 encrypted SDP responses after
its full Linux reset. Earlier native255..258 additionally
qualify sustained Wi-Fi traffic, every packet-ring wrap, wrong-key recovery,
2000 encrypted SDP responses and joined supplier teardown. No previous
radio owner, retained queue or Python packet/HCI transport is required.

All captured host-state inputs have been eliminated. Vendor firmware and the
signed manifest come unchanged from the IPSW. NVRAM is generated from antenna
SKU metadata; an experimental AP-policy encoder uses DT fields and an explicit
fresh record4 input. Calibration blocks are copied opaquely from this device's
existing BWC3 factory file, with a generated host directory. The full image
matches qualified259 byte-for-byte. Factory calibration contents and several
profile fields remain opaque or unassigned; they are not blind runtime replay.

The additional SAE-only Pixel test passes in native275, after Python-first
SAE qualification and the ordinary-supplicant adapter corrections described
below. Concurrent encrypted Bluetooth data also passes on this new kernel.
That Bluetooth coexistence proof belongs to275;298 changes only Wi-Fi and
does not repeat pairing. Native298 independently observes a group19 H2E
commit with H2E-only required, completes required-PMF association and passes
DHCP, HTTP and reconnect. Its negative capability test is Linux fault
injection, not a physical legacy-only AP test. See the final section below.

Automatic chip power sequencing, runtime DT-to-image assembly and installer
extraction/storage remain open. Ordinary supplicant integration now passes
for passive2.4GHz WPA2-PSK/CCMP and SAE/required-PMF on the tested hardware.
H2E-only is supported through peer admission, explicit standard RSNXE and
observed-method enforcement; no exclusive vendor PWE-selector bit is claimed.
Forced hunting/pecking remains unsupported. Initial
global PCI host PHY/link training still runs in the private bootloader.
Wider bands, link-loss handling, Bluetooth profiles/LE/SCO/IRQ/power coverage
and Neo hardware comparison remain future qualification. See the dated
experiments below for the limits of each individual proof.

Reset qualification: earlier experiment labels such as "cold" or "fresh
reset" describe host reboot, fresh DMA and firmware transfers. Their link
startup released the ADT chip-reset line without first asserting it. Do not
use those labels as evidence of a complete chip reset or absence of retained
radio state. The runtime allocation is generated from vendor metadata. Explicit chip-reset
proofs begin at166; current native264 additionally qualifies a full reset and
reload through Linux GPIO/PCI APIs, with independent factory-sourced inputs.

Starting evidence
------------------

The existing Python prototype powers/trains PCIe, uploads a separately
stored opaque image through freshly allocated SID1 buffers, reaches initial
image-ready4 and re-enumerates runtime PCI functions1901/1902/1903. The
second image submission has previously reached state3 rather than ready2.
The global control IPC publication works only after reference firmware
preparation. The verified Rust Wi-Fi datapath adopts an existing association
and queues. The verified Bluetooth datapath adopts live shared firmware and
a context generated by Python; its fresh pairing/data proof is independent
of old pairing keys, but is not whole-chip startup.

First isolate runtime-image readiness with the existing direct Python
prototype and declared transfer resources, then qualify independently
generated control/Wi-Fi/Bluetooth contexts. Existing retained-state tests
are regression evidence only and do not satisfy this cold-start objective.

2026-11-01 experiments118..123
------------------------------

Fresh direct118 reproduces initial ready4, runtime enumeration and runtime
state3 using the matched boot22 opaque assets and freshly allocated buffers.
No Apple executable contents are inspected. Native119 confirms runtime
ready2 and supplies full control-register snapshots. Its initial eight PIO
header samples used untranslated engine addresses and are invalid; exclude
them from protocol findings. Observer setup120 and hypervisor initialization
121 fail before a new hardware result. Explicit host initialization122
succeeds and the revised observer translates the ADT PIO mapper's SID17.

The122 PIO records encode identifiable endpoint configuration reads. A
separate controlled request uses a newly allocated/zeroed16KiB host page
at independently chosen IOVA0x0e000000, added to SID17 while the reference
guest is paused. Generated request/result buffers return PCI ID0x1901106b
and the selected completion tag. This proves a generated configuration read,
not cold PIO initialization or independent wireless firmware startup. Reboot
before reusing the reference guest. Private cold-122/owned-pio-proof.json
records the measured request geometry and result. Capture123 extends only
the explicit16-byte PIO descriptor pairs, with a2048-request bound, to
identify configuration writes missing from ordinary ECAM tracing.

Owned123 also generates a reversible PCI Command INTx-mask write:406to6,
then406, with exact readbacks. Both profiles and their packed destination
nibble are captured in centauri_piodma.py and its hardware-vector tests.
Native124 decodes223 configuration writes, following only qualified scalar
configuration-write data of1/2/4 bytes. It identifies Unix-time publication
before the first image and D3hot+PME before the runtime link restart. Initial
time publication alone (cold125) and adding D3hot (cold126) both still give
runtime state3. They do not establish a fix. Cold127 next enables PTM before
both image stages; previous PTM probes enabled it only after the first stage.
All of125..127 are direct proxy tests after fresh resets, without macOS or
the hypervisor executing. Optional CLI flags retain controlled comparisons;
they are not asserted as sufficient cold-start requirements.

2026-11-01 experiments127..132
------------------------------

Cold127 enables the observed root/endpoint PTM configuration before the
initial upload. It stalls at initial main state0, submission state1; it
never tests runtime startup. Cold128 substitutes the unchanged FTAB member
from the operator-provided IPSW and reaches initial state0, submission
state6. A raw archive member therefore does not yet replace the uploaded
firmware resource. No firmware bodies were decoded or inspected.

Cold129 keeps the first-stage DMA domain and all owning allocations alive
across the PCIe restart. The second submission uses new, previously unmapped
IOVAs in that same process-owned domain. Initial state4 still advances to
runtime state3. Cold130 additionally applies the ADT's masked DART register
parameters and confirms readbacks; it also reaches runtime state3. Domain
lifetime and those ADT parameters are insufficient explanations on their own.
The optional experiment flags are not upstream initialization requirements.

A cache-synchronized reference export and generated-record control test131
was intentionally aborted after guest secondary CPU7 preparation stalled
before N1 initialization. It supplies no firmware or runtime result. The
private DART call tracer132 initially exceeded the protected chainload
footprint; the chainload overlap guard rejected it before execution. Its
storage now uses an explicit proxy heap allocation. The next reference
records raw DART input/output registers only, without pointer dereferences,
to detect monitor-mediated setup absent from the stage2 MMIO trace.

2026-11-01 experiments133..139
------------------------------

Reference133 panics on the display DART before N1 startup. Reference134
reaches runtime ready2. Its cache-synchronized opaque image export has a
new whole-file hash, identical between its two stages; the companion hash
matches the earlier resource. This is only a byte-equality observation,
not an interpretation of firmware contents. Direct136 uses those fresh
exports, as verified by both saved arguments and live process arguments,
and still reaches initial4/runtime3 with no DART error. Export freshness
alone is insufficient. Trial135 accidentally used the older file paths;
it is a repeated old-image result, not a fresh-image comparison.

Reference137 fails during host hypervisor initialization before the guest.
Reference138 submits our generated 32-byte record at a new mapped address,
pointing to the unchanged native image. Runtime reaches ready2. This checks
the record layout and mapping under reference preparation; it does not
establish independent cold startup. The next comparison replaces the image
with the old opaque resource under otherwise reference preparation.

The DART call observer in133/134/138 incorrectly used Mach-O symbol offsets
for a raw bootloader image. Those offsets differ by16KiB; the intended
observer stayed disabled and unrelated private BSS was modified. All empty
call snapshots are invalid and excluded. The raw-build symbols are used
for139. No absence-of-calls conclusion is supported by those earlier runs.
No code or register semantics from the private emulator implementation are
used in the Linux source. Only measured hardware/API boundary observations
can support subsequent protocol changes.

2026-11-01 experiments139..143
------------------------------

Corrected capture139 records 11575 DART calls through runtime ready2. The
operator ADT identifies dart-apcie0 as dart-id 0. Calls for that ID include
stream 17 (PIO) and stream 1 (control) before runtime submission. Streams2/3
appear during the interval through readiness. This is register-boundary
evidence only; no pointed-to call argument bodies or emulator algorithms
are inspected to derive Linux behavior. The private trace records the raw
input and result values without assigning names to unqualified arguments.

More decisively,139 copies the older boot22 runtime image unchanged into
freshly allocated pages and publishes a generated 32-byte record through a
new mapping while retaining reference preparation. Runtime reaches ready2.
This validates that image and record at the runtime boundary; full cold
startup still fails. It does not establish the sufficiency of the old image
for the initial stage, which is the next controlled comparison.

Native139 enables the ADT-declared reserved streams 16/18 with TCR 0 and
invalid TTBRs. Direct140 observes reset TCR 1 instead and stops at the guard
before uploading firmware. Direct141 explicitly rebuilds the two declared
reserved stream controls and enables their bits, with ADT DART parameters
and a continuously owned stream 1 domain. Initial ready4 still advances to
runtime state3 without a DART error. The reserved stream difference alone
is insufficient; its relationship to MSI remains unqualified. Direct142
adds the observed PTM profile and D3hot request, initializing the declared
DART controls first. It stalls at initial state0/submission1, so it provides
no runtime result. No additional configuration bits are inferred from that
failure. Reference143 next tests a generated initial-stage record pointing
to the native image as the control for an initial-image substitution.

Opaque export now cleans and invalidates input cache lines before copying,
preserving CPU writes instead of discarding dirty lines. Existing134 exports
used invalidation only; their metadata records that exact procedure. All
payload bytes stay opaque. The twenty existing capture/DMA/PIO tests pass;
hardware experiments remain separately classified above.

2026-11-01 initial-stage and restart isolation143..146
-----------------------------------------------------

Reference143 replaces the initial submission record with a generated record
pointing to the unchanged native image: initial ready4, then runtime ready2.
Reference144 additionally replaces the initial image with the unchanged
older boot22 bytes in newly allocated pages: again initial ready4, then
runtime ready2. Together with139's runtime substitution, both old firmware
assets and both submission records work under reference preparation. The
whole-file hash differences do not, on their own, explain independent
startup failure. Neither test decodes or inspects firmware contents.

In145, the reference completes the initial upload and is held synchronously
at its first ready4 read, before its own port restart. Python then performs
the restart, BAR/MSI/PCIe preparation and runtime upload, creating its own
SID1 root and image/companion mappings. Runtime still changes 1 to 3 after
about 171 ms with DART error 0. The reference's global controller preparation
and successful first upload are therefore insufficient for this takeover
path. This narrows the remaining comparison to the restart and subsequent
runtime preparation. Other guest hardware accesses stay trapped while the
synchronous callback holds the reference. The guest must not be resumed
after this diagnostic replaces its hardware state.

Trial146 lets the reference perform the restart and pauses at its first
runtime state1 read, before the runtime image is published. Python takes
over only runtime preparation and uses a new SID1 root. The result will
separate the restart from remaining runtime-setup differences. All takeover
runs explicitly retain some reference preparation and cannot satisfy the
independent cold-start acceptance criteria.

The private launcher now pins a raw bootloader image and its raw-build
symbol offsets per trial, records their hashes, and loads that exact copy.
This prevents subsequent local builds from changing the observer addresses.
No private instrumentation implementation is copied into Linux.

2026-11-01 submission/resource isolation146..154
------------------------------------------------

Trial146 lets the reference complete its restart, then takes over at the
first runtime state1 read. Its own runtime preparation still gives state3,
with native root/endpoint PTM controls1/0x1001 and no DART error. Trial147
takes over still later, immediately before the native runtime doorbell,
and only replaces the upload/mappings/resource publication. It also gives
state3. The observed TLB command readback is retained as raw metadata;
no write profile or field interpretation is derived from that readback.

Owned-buffer experiments148/149 run while147's reference remains paused.
A generated PCI configuration read first writes the expected 0x1901106b ID to
physical page A. Retargeting the same SID17 IOVA to owned page B without
invalidation still writes A. Operation1 stream invalidation at DART+0x80
makes the next result land in B; restoring the PTE and invalidating returns
the result to A. A fresh SID17 root pointing to page C then receives the
result after the same invalidation, and restoring the original root returns
the result to A. DART error stays zero. This qualifies both cached leaf
replacement and root replacement on SID17, including a stale-cache control.
It does not establish the semantics of other command bits or every stream.

Trial150 panics on the display DART before the intended comparison. In151,
all reference preparation and the SID1 root remain intact; new image,
record and companion allocations are added only at unused addresses.
Runtime still gives state3. Retaining the reference domain is insufficient.

The older 11,418,624-byte companion capture's whole-file SHA256 exactly
matches a generated zero buffer of that length. In152, a cache-synchronized
copy at the final runtime doorbell instead has SHA256
7b98e7d7f7fa79b17b52861c9486ba65c1e131852b24170a5103e4d9058a43f9.
At runtime ready2 its hash is
a22416280c713049b61e19ffb78c8dc688a28fd49521f716d8b5a81145bd7b10.
The earlier publication-time capture preceded initialization and is not the
submitted resource. No resource contents were decoded or inspected.
Opaque page hashes show that only the first 16 KiB of the submission-time
input is nonzero. That prefix plus generated zeros reconstructs the entire
input byte-for-byte; its field meanings and writer remain unqualified.

Cold153 changes the resource to152's submission-time copy, keeping the
matched boot22 images and the141 owned-domain/ADT/reserved-stream profile.
After a fresh reset with no macOS or hypervisor executing, initial state4
advances to runtime state2, submission status1 and DART error0. All DMA
roots and allocations belong to this process. This proves an independent
firmware-startup path with opaque captured input, not elimination of that
input dependency or completed Linux acceptance.

The same run publishes a generated control context at independently chosen
IOVA0x0c000000 within its own existing SID1 domain; IPC reaches2. The new
OwnedBluetoothHost builds a fresh 32 KiB SID3 allocation and its own command,
event and full-MTU ACL queues. Read Buffer Size reports ACL MTU 2586. Factory
identity comes from ADT; empty FF0B, Reset and Read Local Version complete.
After FF77=0, Create Connection to the currently verified host controller
BC:74:EA:B6:3B:D5 succeeds, Role Discovery returns peripheral1, and the
connection disconnects cleanly. No old pairing keys or host context are
imported. The link is unencrypted; Linux pairing/data acceptance is pending.
The existing 19 Bluetooth tests pass after adding allocation-owner validation
as an alternative to legacy handoff validation.

Owned global data queues complete all four registrations and receive the
ControlApp greeting. Five additional Bluetooth auxiliary receive queues
also register, with their bodies skipped. Wi-Fi startup remains zero after
those steps and after publishing the older53 opaque 2 MiB resource through
its own SID2 mapping. These are negative Wi-Fi startup results, not failures
of the successful control or Bluetooth interfaces.

Trial154 allocates a zero-filled runtime resource and waits three seconds
after publishing it. Its first16KiB still hashes exactly as zeros and runtime
reaches state3. That delay does not reproduce native initialization. The
maintained probe accepts an allocation size without a copied resource;
its 17 existing DMA/capture tests pass. No ready-field meaning is asserted
from this diagnostic delay. Trial155 observes only first-page CPU-store
addresses, sizes and PCs, without values or instruction inspection. Its
initial RAM trap requires a hardware watchpoint to pass an unsupported store
before the later first-page initialization can be observed.

2026-11-01 generated runtime allocation155..156
------------------------------------------------

In155, the host's allocation initially hashes exactly as zeros. Its CPU
zeroing store cannot be emulated using the available syndrome. Removing
that RAM trap and using a hardware store watchpoint on a later page lets
the zeroing operation execute without instruction inspection. Re-arming the
first-page store observer then records 76 eight-byte stores, covering only
offsets 0..0x250, with overlapping final stores. The final initialized extent
is 592 bytes. The observer records addresses/widths/PCs only, not store values
or Apple instructions. The pre-submission resource hash matches152 exactly.

This qualifies the bounded host initialization record as container metadata.
Its magic is `rkosftab`, with a 48-byte header and 34 sixteen-byte directory
entries: 592 bytes total. The vendor IPSW FTAB contains a `2ftb` entry of
exactly 592 bytes whose opaque hash matches that complete runtime record.
The directory describes region offsets/capacities; its maximum region end
is 0xae3c00. Copying this vendor metadata into a zero-filled allocation of
that extent reproduces the entire native input exactly. No firmware payload
or executable code is decoded. The remaining header word meanings are not
inferred from names or code; vendor-provided metadata is preserved unchanged.

The clean `centauri_ftab.py` parses only fixed headers/directories and derives
the runtime allocation from `2ftb`. It checks bounded input regions and the
prototype allocation limit. Three synthetic metadata tests cover the generated
extent/zero contents, malformed directory/payload bounds and excessive layout
allocation. A separate actual-input comparison confirms exact whole-buffer
equality for both the raw IPSW FTAB and the prepared upload container.

Cold156 uses `--runtime-boot-buffer-from-image`, with no captured companion
resource argument. After a fresh reset, generated runtime memory again reaches
initial4/runtime2, submission status1, DART error0. This eliminates 11,418,624
recorded runtime bytes, including the formerly retained 592-byte initializer;
only normal vendor firmware metadata is used to create the allocation. The
memory capacity itself remains allocated as requested by that metadata.

Container-directory and opaque component hashes also show all 83 original
vendor entries unchanged in the prepared boot22 upload. The prepared file
adds a 7772-byte manifest and four entries: `nvrm` 42 bytes, `_cal` 35692,
`_apb` 512 and `_bpb` 200. Across boot22/134/138, the manifest and all added
entry hashes except `_apb` match. `_apb` varies. Its field meanings and source
must be established. The native manifest matches none of the nine J813
variants in this IPSW, and these added inputs do not directly match whole
ADT byte properties. This is a source/provenance problem to resolve; the
prepared primary upload remains an external captured input at this checkpoint.
No component payload instructions have been inspected.

The layout's `msww` region has the same 2 MiB capacity as PF1's published boot
resource. Cold156 copies that current output region opaquely into newly owned
SID2 memory and publishes it. Wi-Fi startup stays zero. Newly generated control
IPC, four control data queues, the kernel greeting and type 9/8/10 queries all
complete; fresh Bluetooth command/event/ACL queues also complete Read Buffer
Size. These results do not establish that `msww` is PF1's required input or
that copying it suffices for Wi-Fi startup. Its role remains under investigation.

Linux integration references
-----------------------------

Standard PCI PM/PTM field definitions and the 10 ms D3hot transition delay
were checked against Linux v7.3 include/uapi/linux/pci_regs.h,
drivers/pci/pci.h and drivers/pci/pcie/ptm.c (Intel Corporation, GPL-2.0).
No device-specific implementation is copied from unrelated RE projects.

The official kernel cfg80211 documentation requires the standard wireless
configuration API and describes wiphy/capability registration and regulatory
integration: https://docs.kernel.org/driver-api/80211/cfg80211.html . The DMA
guide distinguishes device bus addresses from CPU physical addresses and
documents ownership/mapping contracts:
https://docs.kernel.org/core-api/dma-api-howto.html . These guide the target
architecture; no device-specific implementation is taken from these pages.

Hardware selection remains the operator-confirmed RID1 / JMQ12P63RJ,
proxy /dev/cu.usbmodemJMQ12P63RJ1 and UART JMQ12P63RJ3. Other connected
devices must not be touched. Test credentials stay outside source/history.

2026-11-01 experiments157..158
------------------------------

Native157's first observed Wi-Fi status read is already2. The capture contains
all1031 PIO configuration submissions before it (808 reads and223 writes),
well below the expanded16384 cap. No earlier direct Wi-Fi BAR write appears.
The separately mapped PF1 2MiB resource has exactly the same SHA-256 as the
current global allocation's complete `msww` region. The mappings are distinct
physical allocations; this establishes a copy correspondence, without decoding
or interpreting the resource. An attempted export of the earlier primary
image is refused because its declared source is already unmapped; no freed
memory is read.

Direct158 uses the newer134 main images, the generated runtime layout and
no optional ADT DART parameters or reserved-stream configuration. It reaches
initial4/runtime2 with no DART fault, and a fresh control context reaches IPC2.
Copying its current `msww` region into new SID2 memory and publishing it before
upper control/Bluetooth queues still leaves Wi-Fi at0. A diagnostic copy of
native157's resource, then individually matching the observed +0xf80 and
+0xd0c values, also leaves Wi-Fi at0. The diagnostic reference resource is
not independent-input evidence. Neither experiment establishes that buffer
publication alone starts Wi-Fi, or that a changed primary image fixes it.
The next observer polls known status registers around native host operations
to narrow the actual transition. No Apple executable contents are inspected.

2026-11-01 experiments159..161 and structured packaging
-----------------------------------------------------

Trial159 times out during host secondary-CPU preparation before N1 work;
it supplies no wireless result. Native160 polls Wi-Fi status around known
host register operations. The status is0 at the kernel greeting doorbell
and2 at the next observed queue8 replenishment. Its kind0/sequence0
``CentauriTransport`` greeting matches the generated request. The descriptor's
bit26 is already a measured device completion indication; it is not a newly
required input flag. No HCI command precedes this transition. Both published
radio resources exactly match their current global `msww`/`mswb` regions by
opaque hashes. The observer did not then cover writes across the large PCI
apertures, so it cannot exclude such intervening writes.

Direct161 tests the complete measured ordering: fresh Bluetooth context and
core queues, fresh controlIPC2, external control queues8/2, both current
firmware-produced radio resources, inline control queues4/3 and the greeting.
Every registration and the greeting reply complete with no DART fault;
Wi-Fi stays0. No reference runtime allocation or resource is imported.
This rules out that ordering alone as the missing operation.

``centauri_ftab.prepare_image`` and ``prepare_centauri_firmware.py`` now make
primary image packaging explicit. Container metadata places the signed
manifest after the enlarged directory, shifts every unchanged vendor payload,
then appends the four named host records and zero-pads to4KiB. With the
native160 external manifest/records, reconstruction from the IPSW vendor
member reproduces all31252480 bytes exactly, SHA-256
``942a5bf9bb592794ef447fd73313b6e1750e9140654634d9e98ebf06b7b49216``.
Five synthetic-metadata tests cover allocation bounds, component preservation,
manifest placement, padding and invalid inputs. No vendor payload is decoded.
Manifest and device-input sourcing still require qualification; mechanical
packaging is not a claim that the old primary inputs have been eliminated.

2026-11-01 experiments162..165 and reset qualification
-----------------------------------------------------

Observer162 faults when polling PF1 before validating its BAR availability;
it supplies no Wi-Fi transition result. The corrected163 checks function
identity and memory decode. Address-only write discovery covers both ADT PCI
apertures. It finds only known control-page writes, including Wi-Fi context
and window publication before the sampled status2. The DART register-call
observer stays enabled through that transition. No executable contents or
large-aperture write data are recorded. Interleaving prevents asserting that
the control greeting is the trigger:163 reaches2 before that greeting.

Direct164 uses the structured firmware package with the same complete bytes
as native160. Generated runtime allocation, current radio resources and fresh
host contexts reach Wi-Fi IPC2. All twelve basic queue registrations complete;
queries8/10/47 return570 bytes of valid parameter TLVs, countryXZ/0 and56
channels. DART reports no fault. Status2 precedes the Wi-Fi IPC handshake, so
this does not demonstrate that the previous status precondition was wrong.

The prior launcher only released chip reset. New ``--assert-chip-reset``
explicitly asserts the ADT-defined reset, holds it for a prototype100ms, then
uses normal link startup to release it. It cannot be used with ``--reuse-link``.
Direct165 reads back SMC reset0 before release and loads both firmware stages
successfully from newly allocated buffers, with generated output allocation.
ControlIPC2, Bluetooth context/core queues, current radio-resource publication
and the control greeting work; Wi-Fi stays0 and its first handshake times out.
This exposes a remaining startup dependency rather than qualifying164 as cold.

After that explicit reset,165's generated Bluetooth setup programs its own
ADT factory address, uses the empty FF0B radio initialization, resets HCI,
reads its version and clears the measured Classic-create inhibit. An actual
connection to the separately verified host controller succeeds (status0,
handle1); Role Discovery returns role1 and explicit disconnect completes.
This is unencrypted connection metadata, not Linux pairing/data acceptance.
No old host context, mapping or pairing key is imported. The next native boot
asserts the same chip reset before observing its startup.

The packaging and DMA/capture checks pass22 tests using the proxyclient
Python environment. The first attempted DMA test invocation lacked that
module path; it was an environment failure, corrected before validation.

Native166 starts after the same explicit SMC reset assertion (readback0) and
reaches Wi-Fi ready2/IPC2. Its signed manifest and nvrm/cal/bpb records match
160; all vendor component hashes match, and only APB bytes36..39 differ.
Address-only tracing still sees only known control-page writes. Native
software can therefore start Wi-Fi after this reset; a required pre-OS radio
initialization has not been demonstrated. Next compare the native D3hot
transition before the first-stage port restart with the now-correct generated
allocation. The old D3hot experiments used the zero allocation and failed
earlier, so they do not exclude a separate Wi-Fi startup requirement.

2026-11-01 experiment167: shared cold control planes
---------------------------------------------------

Direct167 repeats165 with the same structured main firmware, asserted chip
reset, generated runtime allocation and own mappings/contexts. Saved command
arguments differ only by ``--bootstrap-d3hot``; the bootloader image hash is
also unchanged. This applies the observed D3hot plus PME-enable transition
before the first-stage PCIe port restart. It reaches Wi-Fi ready2/IPC2 and
all twelve basic queue registrations. Queries8/10/47 return valid parameter
TLVs, countryXZ/0 and56 channels; DART reports no fault.

The same run programs the Bluetooth factory address from ADT, uses the empty
radio initialization, resets HCI, queries its version and clears Classic-create
inhibit. A real connection to the verified host succeeds with status0/handle1,
Role Discovery returns1 and explicit disconnect completes. No macOS or
hypervisor executes during167, and no old host mapping or context is imported.
This qualifies the shared firmware and generated host control planes after
explicit reset; it does not implement scan/association or Linux data tests.

The successful transition changes D3hot and PME enable together. Their
individual contribution remains unqualified. The earlier D3hot-only wording
names the existing CLI experiment, not evidence that PME is unnecessary.
Main manifest/device-record provenance and Linux Rust integration remain open.
The next reset run substitutes the IPSW-provided manifest in the reconstructed
container while preserving the other explicit device records.

Experiment168 replaces the captured signed manifest with the unmodified
IPSW-provided J813 manifest in the structured package. Both image stages
reach ready after explicit chip reset and the D3hot/PME transition. Generated
runtime resources and host contexts again complete Wi-Fi IPC/basic queues,
parameter/country/channel queries and a real Bluetooth connection/role reply
with explicit disconnect. The prepared image has SHA-256
``ad8ec4aa52f23be3bf30048e70b2737ac35744b3e78aa88cc004cb4cef554290``.
This qualifies the provided IPSW manifest for this observed firmware/device
configuration; a captured manifest is no longer required. The four separately
supplied device records still require proper sourcing/generation. Exact
subsequence comparisons against all byte-valued ADT properties find no full
match for any of the four records; only their independently documented field
correspondences are established. No executable payload is inspected.

Experiment169 changes only ``--bootstrap-d3hot-no-pme`` relative168. Both
firmware stages, controlIPC and generated Bluetooth core queues still work,
but Wi-Fi remains0 and its first IPC handshake times out. Enabling PME later
in D0 on function0, then1, then2 has matching readbacks but does not recover
Wi-Fi. This establishes that PME enable during the measured pre-restart
transition matters; it does not isolate whether D3hot itself is also needed.
The optional no-PME flag keeps that bit test reproducible and never changes
the successful profile's default. The next firmware-input experiment removes
only the200-byte `_bpb` record while keeping vendor components, IPSW manifest
and the other device records unchanged.

Experiment170 omits the complete200-byte `_bpb` host record. All vendor
components and the nvrm/cal/apb records remain unchanged, using the IPSW
manifest and working asserted-reset/D3hot/PME profile. It reaches both firmware
ready states, Wi-Fi IPC2, all basic queue registrations and valid parameter,
country and channel-list replies. The real Bluetooth connection/role reply
and explicit disconnect also pass, with no DART error. This qualifies omission
for these control-plane/radio-link tests, not for still-unimplemented Wi-Fi
association or all device models.

The clean packager now allows omission of `_bpb`; ``--boot-parameters`` is
optional. It reproduces170's experimental image exactly (SHA-256
``0330b3f1a8131b8ff94cbaedc28a953099fd89dea99e1a7d23194989452c2a4e``).
All200 retained bytes of that record are eliminated from this prototype path.
The remaining three supplied host records total36246 bytes; their factory
sources and unqualified fields still need work. The updated packaging and
DMA/capture suite passes23 tests. No Apple executable payload is inspected.

2026-11-01 experiments171..172: generated runtime module
------------------------------------------------------

The verified initialization is now in clean GPL-2.0-only
``centauri_runtime.RuntimeHost``. It validates current loader-owned firmware
memory, allocates radio regions from `2ftb` metadata, creates fresh Bluetooth,
control and Wi-Fi contexts and queues, and advertises its owned DMA extent.
The Wi-Fi resource allocation also establishes SID2's root, eliminating the
old16KiB scratch anchor. No native-driver mappings or contexts are adopted.
``--initialize-runtime-host`` integrates it into the image-loader CLI.

Explicit-reset171 uses this module for initialization, with the IPSW primary
image/manifest, three device records, generated output allocation and measured
D3hot/PME transition. Wi-Fi management8/10/47 and a real Bluetooth connection,
role reply and disconnect pass. Close disables bus mastering on all three
functions and DART streams1/2/3; the closed-object snapshot guard rejects use.

Explicit-reset172 adds only ``--detach-bootstrap-domain``. Initial firmware
DMA is stopped after ready4; runtime loading creates a new root/domain with
new submission/image IOVAs. The module still initializes, Wi-Fi management
replies and the Bluetooth link test pass without a DART error. The old domain
is not retained. This supports a conventional PCI driver lifetime design;
it does not yet prove Linux device removal/rescan or reclaim all first-stage
allocations, which remain process-owned during the Python diagnostic.

A bounded address-command probe supplies the current ADT factory Wi-Fi
address to runtime command1 and receives an empty reply with prefix word3=0.
Command0's144-byte reply is unchanged and contains no address match, so it
is not established as address readback. Commands20/22 from an unconfigured
interface return prefix word3=0x01430000; this value's meaning is unresolved.
No command names are assigned from these probes alone.

The ADT ``walk_tree`` helper skipped direct children and deeper levels. Its
private bootloader fix and regression now traverse all436 nodes instead of181,
including `/chosen`, which supplies ``mac-address-wifi0``. The earlier claim
of checking every byte-valued ADT property was too broad. Repeating the exact
record-subsequence comparison with a complete recursive traversal still finds
no whole match for nvrm/cal/apb/bpb. Thus the sourcing question remains open,
now with the traversal scope corrected. No Apple executable is inspected.
The module changes pass the existing23 packaging/DMA/capture tests plus real
hardware qualification; the ADT regression passes independently.

Native173 selectively samples six-byte address requests, five-/27-byte
command2 requests with small fields0/11, five-byte command3 requests with
fields0/0, command19's byte1 and command21's two-byte value28205. The22-byte
command2 tail does not equal the test SSID. A bounded64-byte prefix search
for recognizable broadcast80211 Probe Request headers in command0x1000f
finds none; its function is not thereby established. No arbitrary packet
body, key, credential or executable content is stored.

Later native context turnover produces guarded mapping failures; those
samples are not absence evidence. The here-document launcher also supplied
stdinEOF, so interactive holds resumed automatically. The subsequent direct
experiment uses a normal terminal launcher and probes the smallest observed
command2 form and command19/readbacks, keeping unqualified enum fields
explicit rather than turning them into named production operations.

2026-11-01 experiments174..175: interface control and MAC readback
----------------------------------------------------------------

Direct174 starts the generated module with a fresh runtime domain. After
setting the ADT factory address, command2's five-byte form activates the
interface: command20 changes from an8-byte state0 prefix to a38-byte state2
record carrying that MAC and TLV0x017d/26. Command3 returns it to state0;
reactivation restores the same record. These observations support interface
activation/deactivation, not an assertion that a Linux interface was created
or that a wireless association exists. The initial private probe labels
"create/delete" describe this lifecycle hypothesis and are superseded here.

Command1 updates the address template observed on the next activation.
An alternate locally administered unicast address is acknowledged while the
interface is active, but its current descriptor remains unchanged. After
reactivation it reports the alternate; another deactivate/set/activate cycle
restores the original ADT address. Thus MAC readback is now causally checked,
rather than inferred from an empty command reply.

Command19 with selector1 returns byte-for-byte the same709-byte channel-list
body as command47 with selector1. It is not a power setter: probing its byte0
and1 requests did not change the interface record. Their complete scope and
selector semantics remain unqualified. Low creation-word values0,1,2,3,4,8,
9,10,11 all give the same observed descriptor; this does not establish the
absence of other effects. Five zero bytes suffice for tested activation.
An empty command3 body deactivates, but an empty command2 body faults Wi-Fi
(status3, IPC2, DARTerror0). No crash memory or instructions are inspected;
all host DMA is stopped and the next run explicitly resets the chip.

Clean ``centauri_wifi_messages`` now names the address-template, interface
activation/deactivation, interface-info and interface-channel-query commands,
encodes the five-byte operation body, validates unicast addresses and decodes
the fixed selector/state/MAC prefix plus bounded TLVs. Field meanings that
are not established stay explicit; no captured optional22-byte tail or
unqualified nonzero creation word is imported into the helper's defaults.

Fresh175 validates those helpers using options0. It activates with the ADT
MAC, decodes state2/MAC/TLV readback, then applies the US/1 country request
also observed in native173. The channel count changes56to167 and the unit
and global queries still match exactly. Another real Bluetooth connection,
role reply and explicit disconnect pass with no DART error. This is a usable
firmware-control baseline; scan/authentication/association and Linux network
verification remain incomplete. The message/queue/control tests pass15
checks, in addition to the23 packaging/DMA/capture checks.

2026-11-01 experiments 176..177: independent Python scan
------------------------------------------------------

Native 176 observes bounded host runtime requests without executable reads.
The scan candidate has a 14-byte prefix and tag 0104 channel vectors. A
connection request is identified separately by the test SSID. Its security
contents are removed from saved observations; only layout metadata remains.

Fresh direct 177 uses generated runtime contexts and a detached bootstrap
domain after explicit chip reset. It activates with the ADT address and
sets US/1. A generated single-channel request receives asynchronous scan
events. The original exact-command query times out because the successful
acknowledgment is 4001000f, not 1000f. The maintained queue class now takes
an explicit reply command and supports bounded receive/event callbacks.
It also reports the observed same-command/nonzero-status rejection path.

Single-variable tests isolate bit 31 as enabling queue-4 radio reports.
Other observed option words and optional native TLVs are unnecessary for
this scan. The bodies are standard radiotap plus 802.11 frames, not firmware
logs. ``centauri_wifi_scan`` generates the request and decodes completion,
radio fields and beacon/probe-response information elements. A generated
three-channel request returns 22 frames with zero parser errors, matching
the completion's count, and finds the test network on channel 11. The
24 relevant queue/message/scan tests pass. No association or Linux network
acceptance follows from this result. A broad raw-country-table request is
rejected; its additional channel metadata still needs qualification.

Fresh explicit-reset 178 imports the maintained implementation at startup;
no interactive class replacement or prior scan probes occur. After only
address/activation/country setup, its minimal generated 1/6/11 request with
zero option words and 50/50/110 timing completes with status 0, count 24,
elapsed 167 ms and cookie 1. Management transaction is 4. All 24 frames
decode without errors; two match the test SSID. Bluetooth then initializes,
connects to the host, returns its role and disconnects. Wi-Fi and IPC remain
ready with no DART error. Current-scan public BSS/RSN metadata is saved for
association work. This is still a Python proof with three firmware device
input records of unresolved source, not Linux cold-network acceptance.

Later 178 connection probes use the current scan's public BSS/RSN data and
a freshly derived WPA2 PSK. Password/key contents are not stored. A generated
159-byte prefix plus SSID, RSN and password fields is initially rejected with
reply command 40020000 and status prefix 015b0000. Explicit timeout words
alone do not resolve it. With those words retained, setting the u16 field
at offset 132 to 1 changes acknowledgment to status 0. Its count/role meaning
remains a hypothesis; it is not given a production default or driver name.

Acceptance of the request does not prove association. Subsequent c0000001
and 80000000 events occur, interface activity remains 2, and the native
2000e query that returned a 44-byte body instead returns empty. Changing
the candidate authentication selector or capability/listen fields changes
the final status prefix but gives no verified link. A diagnostic A/B with
the recorded 324-byte runtime capability profile also produces the same
outcome; that profile is not added to the Linux implementation or counted
as cold-start acceptance. Proper connection-field/event decoding remains
the next task. Asynchronous acknowledgments themselves can carry nonzero
status, so the queue helper now reports both original-command and aliased
reply-command rejection paths explicitly.

2026-11-01 experiments 180..183: connection format correction
------------------------------------------------------------

Fresh native 181 confirms the security prefix starts at byte 70: selector,
AKM type, cipher type, flags, then a little-endian u16 key length at 74.
Its observed values are 3/8/4/0 and length 32; the opaque key starts at 76.
The initial 180 observer started one byte too late, so its security values
were discarded. No retired request buffer is treated as a valid observation.
No key/password payload is retained by the corrected observer.

This corrects the private 178 request generator, which placed these fields
one byte late and advertised an 8192-byte key length. Its failed joins cannot
qualify authentication modes or exclude missing capability configuration.
``centauri_wifi_connection`` now encodes this boundary without shifting the
key or adjacent fields. Three wire/outcome tests cover the native prefix,
neighbor preservation and bounded public result decoding.

The 90-byte 80000000 outcome begins with 32 SSID bytes, u32 SSID length,
six BSSID bytes and u16 result status at offset 42. Native success reports
status 0. Owned failures report 1; their header status detail can differ.
The remaining 46 bytes are uninterpreted. Command acknowledgment is a
separate event and is not proof of association.

Fresh owned 182 still fails to join with the corrected generated PSK/CCMP
fields and authentication selector 0. A single-field selector-2 probe faults
Wi-Fi firmware (state 3, IPC 2, no DART error). DMA is stopped and the chip
reset; no fault memory or instructions are inspected. Fresh 183 uses the
reference activation option 11 and the maintained security/outcome helpers;
scan and Bluetooth response pass, but connection status remains 1. Optional
tag 013c/67 and three nonsecret reference control words are tested separately
and together without producing a link. These unqualified options remain
private diagnostics and are not driver defaults. Association is incomplete.

Reference 184 stops during hypervisor preinitialization and yields no N1
evidence; its launch is confirmed terminal before a fresh 185 reboot.
Reference 185 patches only the completed 19/47 channel-list bodies to the
valid channels 1..11, preserving their extent and all remaining protocol.
The native host then chooses the target WPA2 BSS on channel 11 and connects
successfully. Its firmware AKM selector is 4, although the standard RSN IE
uses PSK suite type 2. The earlier generated request incorrectly passed 2
directly. The maintained encoder now names firmware selectors separately
and adds the native WPA2 boundary vector to its tests. SAE's selector is 8;
its coincidence with RSN type 8 must not be generalized to PSK. Owned cold
association is tested again with this corrected mapping below.

2026-11-02 experiments186..187
------------------------------

Explicit-reset186 connects using firmware PSK selector4 and generated
request fields, with the native optional control words and records absent.
Four freshly allocated Ethernet queues suffice for DHCP, ARP, HTTP301 and
HTTPS200. An isolated network namespace on the build server supplies the
TCP/IP stack through a TAP bridge; it has no other network interface. A
bridge-off negative control times out. This proves the Python/M5 data path,
not a Rust driver or Linux running on the M5.

After the bridge stops and receive servicing is idle, firmware state3 is
observed before the proposed TX metadata probe starts. The probe sends no
candidate and changes no metadata; it cannot qualify bit17's meaning or
necessity. Keep polling and replenishing receive rings during idle periods.

Fresh explicit-reset187 builds a 212-byte PMK-only connection request with
the maintained encoder, omitting the 33-byte plaintext passphrase record.
Association status0, DHCP, Bluetooth link/role/disconnect and a repeated
isolated TCP test succeed: HTTP301/219 bytes and HTTPS200/87179 bytes.
Both runs use vendor firmware and a normal signed manifest, three explicit
host-input records, fresh domains/contexts/buffers and generated rings.
They do not run macOS. The mandatory leading byte, word132, SSID flags,
five timing words and descriptor/profile selector bits still need causal
qualification. No captured connection or Ethernet queue records are inputs.

The live187 ARP comparison alternates descriptor metadata bit17/zero twice.
Every TX completes, but only the bit17 requests obtain router ARP replies.
Firmware/IPC remain2 and DART error0. This qualifies a functional difference,
not a precise bit name. Unlike the earlier186 attempt, these candidates are
actually posted while the device is healthy and receive credits are serviced.

2026-11-02 kernel port and restart comparisons188..191
------------------------------------------------------

The first Rust cold components now build and link into centauri.ko against
Linux7.3: bounded FTAB metadata validation, generated runtime allocation and
both image transfers through coherent DMA allocations. Fourteen Rust host
checks pass using synthetic Python-packaged fixtures and modeled MMIO. C
provides generic PCI/DMA/firmware/MMIO services; protocol fields stay in Rust.
The probes reject retained firmware domains and require a Linux DMA IOMMU
domain. They are not yet hardware-tested and do not connect the warm network
or HCI paths to independent startup. See README for the provisional opt-in.

Fresh188 first reaches initial ready4, requests D3hot+PME, asserts standard
bridge secondary-bus-reset bit6 for10ms and waits after deassertion. Runtime
configuration reads remain ffffffff during the five-second observation.
Fresh189 instead asserts the ADT PERST GPIO for100ms;190 additionally requests
LTSSM stop/start. PERST readbacks confirm the requested GPIO levels. In190,
LTSSM reads0 after the re-enable request; successful retraining is not proven.
Both fail to enumerate runtime functions. These comparisons do not change PHY clocks
or performs the known full controller-port restart. They do not establish
that every possible shorter sequence fails or identify one necessary write.

Fresh191 repeats the existing full port restart as a positive control.
Initial ready4, runtime shared ready2, generated control/Wi-Fi/Bluetooth IPC
and all basic queue registrations succeed with DART error0. DMA is explicitly
closed after recording the snapshot. Linux needs a proper host restart path;
standard secondary-bus reset alone is not a qualified substitute here.

2026-11-02 first Linux image and MSI qualification192..202
----------------------------------------------------------

Native192 hands off from the loader and its USB interfaces disappear. No
runtime console evidence is obtained, so native kernel execution and driver
results are unproven. The remaining unlabeled serial interface is traced
through the host registry to the other device and is not opened. Subsequent
Linux tests use the existing console hypervisor, with device MMIO and DMA
going directly to hardware and no macOS execution.

Linux193 boots a fresh16KiB-page Linux7.3 image with the native DART driver
and Rust image probe built in. DART initializes42-to42-bit translation and
attaches the endpoint to a DMA domain. The transfer times out at main0,
submission1, DART error0. The standard DMA domain is present; it is not a
retained firmware domain. Rust now refuses a new probe with a nonzero initial
submission status before enabling DMA, and reports both states on timeout.

The standalone comparisons isolate the startup prerequisite using the same
opaque image, fresh reset, owned DMA and generated record.194 omits all MSI
setup and reproduces0/1.195 disables only INTx and still fails.196 writes only
the separate AXI control and still fails; that register reads0 afterward.
197 programs only the endpoint's standard MSI capability with four messages
and reaches initial4 without enabling the root's MSI receiver.198 enables
only the root receiver and still fails.201 programs the same endpoint fields
as197 but leaves MSI Enable clear and fails.202 enables one message and
reaches4. MSI Enable is a qualified prerequisite; four initial messages are
unnecessary. Successful standalone runs then use the existing full restart
and full runtime MSI profile to reach shared2 and generated radio IPC.

Linux199 is excluded from native-host conclusions: dtc resolved a stale
source-directory include, leaving the generic host in the actual guest tree.
Absolute include paths and compiled-DTB assertions now prevent that error.

Linux200 uses the T8142 Apple host match, standard MSI allocation and the
existing Apple requester-mapping allocator. The loader asserts the chip reset
and prepares only host power/link; it supplies no image, host context, MSI
configuration, valid DART root or requester mapping. Linux allocates the image
and record, validates the container and reaches initial4. Readbacks are
main4, submission0, DART0, TCR9 and CAM slot0=80010100. Two MSI vector0 events
arrive through the normal Linux interrupt domain. A paused bounded capacity
probe preserves all valid mappings and restores every test write: slots0..15
are implemented and slot16 is RAZ/WI. Slot0 retains Linux's mapping exactly.
CAM index is an allocation choice, not the stream ID. The kernel's normal
four-level DART configuration works for this initial transfer.

The Rust probe now uses one initial MSI vector, keeps the runtime four-vector
profile and checks runtime's observed awaiting-image state1. Seventeen Rust
host checks pass. The cold tree's PCI-host, control-function and DART nodes
pass dtc and focused schema validation; both edited bindings pass document
validation. Host root/PHY initialization is still provided by the loader;
native port restart/re-enumeration and the full Rust radio path remain open.

Fresh Linux203 repeats initial4 with only one MSI vector allocated by the
kernel. Readbacks again show main4/submission0/DART0, and the single vector0
receives two events. The control node's PCI personality aliases and the
correct native host/MSI properties are present in the verified compiled
tree. The firmware, record and domain remain Linux-owned. Runtime kernel
upload, radio contexts, firmware device-input sourcing and network/HCI
acceptance are still incomplete; this is first-stage firmware evidence only.

2026-11-02 experiments203..205: Linux runtime image
---------------------------------------------------

Fresh203 repeats initial ready4 with one Linux MSI vector, two delivered
interrupts, submission0 and DART0. Linux then unbinds/removes the bootstrap
function, releasing its coherent allocations and MSI. Device removal clears
PME enable; a port-restart implementation must account for that ordering.
A shorter sequence cycles PERST, per-port reset, PHY clock requests and the
application clock while omitting the observed quiesce/startup presets and
port initialization. Link training times out. Full recovery after this
failure also fails; it is not an independent full-sequence comparison.
Trial204 never executes its planned restart because the host orchestration
lacks writable stdin. Exclude it from reset-sequence comparisons.

Fresh205 loads the first image in Linux, removes the bootstrap device, then
pauses the guest. The private Python host sets D3hot+PME after removal and
executes the full previously qualified controller-port restart. PCI IDs
1901/1902/1903 appear. Linux resumes and its standard PCI rescan assigns the
BARs and creates three normal translated IOMMU groups. The Rust control
probe generates its companion allocation from FTAB directory metadata,
allocates a fresh image/record through coherent DMA and four MSI vectors,
and reaches shared ready2. DART error stays0; Linux IRQ41 and42 each receive
one event. The generated companion is published at IOVA0xfd000000, length
0xae3c00. Runtime main2/submission1 is distinct from initial main4/submission0.

No initial image mapping, MSI allocation or PCI device object is reused by
the runtime probe. Python supplies only the host port restart and restores
Linux-owned bridge/IRQ configuration while the guest is paused. This is
not a native Linux host reset implementation or a Linux radio/data test.
Both image transfers and the runtime companion are now qualified under
Linux7.3 on the target. Three properly sourced device inputs, controller
reset integration, generated Linux radio contexts and cfg80211/HCI startup
remain open. The private cold-205/linux-console.log and
full-port-restart.json preserve the measurements; no Apple executable was
inspected.

2026-11-02 experiments206..214: port reset without presets
----------------------------------------------------------

Linux206 qualifies the named PERST/PHY clock/port reset/application-clock
sequence plus only the port0x88 quiesce/startup writes. It omits the full
initialize_port presets, ADT per-port tunables and debug-register writes;
standard PCI rescan and the Rust runtime image still reach ready2/DART0.
Linux207 additionally drops startup bit4 (0x110 to0x100) and succeeds.
Every measured Linux MSI, RID/SID and interrupt-mask register is unchanged
across the reset before any restore. These results support preserving host
interrupt/routing ownership across the controller-port transition.

Own hypervisor CPU preparation times out in208/209 before Linux or N1 starts.
They provide no wireless-register comparison. Direct Python210 omits the
quiesce write while retaining startup0x100 and fails link training. Direct211
uses only quiescebit0 instead of0x11, still reaches runtime ready2 and generated
radio IPC. A follow-up with DMA disabled isolates the port0x88 handshake:
bit4 reads set; writing bit4 clears it without changing the link/control
registers; writing onlybit0 sets bit4 within the first2ms observation while
the link stays up; writing bit4 clears it again. This establishes a request,
completion and W1C acknowledgment, not an opaque persistent configuration.

Direct212 drops startupbit8 as well (writes0 instead of0x100) and still
loads runtime firmware and generated radio IPC. Thus neither startupbit8
nor either old bit4-containing preset is required for this reduced reset.
Do not infer a resume function for bit8 from the old combined values.
Direct213 attempts GPIO/quiesce without the PHY/port clock/reset cycle.
All runtime IDs are0xffffffff despite link UP remaining set; BUSY is set too.
The raw diagnostic's success label means only that the reset function
returned. It is invalid as an enumeration result; BAR assignment correctly
rejects the missing runtime function. The prototype now requires UP and
BUSY-clear before reporting link training success.

The clean Python warm restart now clears stale bit4, issues named bit0,
polls named completionbit4 with a deadline, cycles the qualified port/PHY
clock/reset controls, acknowledges completion and retrains the link. It
preserves MSI/requester routing and does not replay port/debug presets.
Direct214 qualifies this exact implementation with one initial MSI vector,
a detached bootstrap DMA domain, runtime ready2 and generated Wi-Fi/Bluetooth
IPC; the host is then closed and all three endpoint BME bits are verified
clear, DART error0. Earlier210..213 use four initial vectors because their
CLI enable step overwrote the early one-vector hook;214 explicitly omits
that step and its saved configuration is the one-vector evidence.

Functional names PORT_QUIESCE_CONTROL/REQUEST/COMPLETE describe the measured
handshake. Its exact effect on in-flight traffic remains unqualified. PHY
bit31 and cold controller initialization still need further interpretation;
this is not a claim that every host register is understood. Relative to
the existing Apple PCIe driver, MSI domains, RID/SID allocation and native
DART translation work through the existing frameworks. T8142's per-port
clock/reset sequence and handshake are the measured additional work; do not
apply them to Neo/other controller compatibles without qualification.
Native Linux reset integration, radio context startup and device-input
sourcing remain open. No Linux-on-M5 cold curl/Bluetooth acceptance is claimed.

Fresh216 repeats both Linux image transfers using the exact committed reduced
Python reset between them. Private debug preinitialization selects only the
boot CPU, preserving the original ADT; this avoids the208/209/215 secondary
CPU stall for the already one-CPU wireless test. It is not SMP qualification
and changes no Linux source. Bootstrap removal frees the first DMA/MSI;
Python resets the host while the guest is paused; every sampled Linux MSI,
RID/SID and mask register remains unchanged before restore. Linux rescan
creates all three runtime functions/IOMMU groups and the Rust runtime image
reaches ready2/submission1/DART0. One vector0 event is sampled. The kernel
remains running with Linux-owned runtime allocations. Native reset/radio
integration and the three device-input sources remain incomplete.

The reset DT inputs are now bounded to ADT resources: port PHY0 at
0x48f010000/0x4000 and PERST pin192 on GPIO0 at0x39a000000/0x100000.
GPIO0 declares224 pins and seven interrupt groups, AIC347..353. Use a normal
GPIO provider/reset descriptor and named PHY resource when porting the
sequence; do not embed their physical addresses in the wireless driver.

2026-11-02 experiments217..219: native Linux transition
------------------------------------------------------

The T8142 Apple PCI host now implements ``reset_root_port`` with the measured
quiesce request/completion, GPIO PERST and port/PHY clock/reset sequence.
An enabled matching control DT node requires this callback before initial
image submission. A separate transition work allocation retains PCI/module
references, so PCI removal can release the old driver's DMA/MSI and devm
storage without invalidating the worker. It requests D3hot+PME after removal,
saves the root bridge, uses the normal PCI bus reset (with no old registered
children to restore), and rescans the bus. New runtime PCI objects own fresh
resources; no IDs, descriptors or mappings are adopted from the old object.

Trial217 stops before N1 startup in the GPIO driver's raw-default snapshot.
The fault is GPIO0 offset0x138, pin78. ADT's eight-byte no-restore property
decodes to two u32 pin indices78/203. The existing REGCACHE_FLAT configuration
bulk-reads512 registers; its replacement REGCACHE_FLAT_S reads controls on
demand and does not invent initial register values. The board describes
standard ``gpio-reserved-ranges`` so gpiolib excludes firmware-owned lines.
No T8142-specific pin blacklist is embedded in the generic GPIO driver.

Fresh218 completes the native pipeline: initial4, bootstrap removal,
D3hot+PME, host reset, runtime1901/1902/1903 enumeration and Rust sharedready2
at1.763017s. DART error0, main2/submission1; control IRQ48/49 each receive
one event. GPIO IRQ parents change Linux virq numbering, without requiring
any driver constants. Linux owns GPIO/PHY/PCI/DART/MSI and every image,
record and companion allocation. The loader provides only the initial host
link and a completely uninitialized N1 firmware interface. The console
hypervisor supplies direct mappings; no macOS or Python port reset executes.

An intentional runtime unbind218 drains/frees all MSI allocations and clears
BME (PCIcommand2). Rebind correctly fails EBUSY(-16) before allocating MSI
or enabling BME, with DART0. This checks safe refusal of stale firmware state;
it does not implement whole-chip reinitialization on reprobe. A subsequent
fresh explicit chip reset219 repeats automatic runtime ready2 at1.751518s.
The219 build also clears drvdata if transition-work allocation fails.

Full16KiB kernel builds/link, PCI/bootstrap C checkpatch, changed-line GPIO
checkpatch, modified schema validation and focused PCI/control/DART/GPIO DT
validation pass. The full GPIO file retains an unrelated existing const-array
style warning. There is no Apple executable inspection. Linux remains clean
GPL-2.0-only; private bootloader orchestration remains separate/TAINTED.

Differences from existing drivers: the standard Apple MSI domain, RID/SID
allocator, native DART and GPIO ABI are reused. T8142 adds its qualified
per-port reset handshake and separate PHY resource; the control endpoint
changes PCI identity and needs removal/rescan instead of a retained object.
The generic GPIO cache avoids bulk snapshots on all boards, while reserved
pads stay board-specific DT data. No Neo hardware was probed in these tests.
Full host initialization from power-off, resets with live requester mappings,
normal cold reprobe, native radio IPC/cfg80211/HCI and the three firmware
input sources remain incomplete. These results do not satisfy cold Linux
curl or Bluetooth acceptance.

2026-11-02 experiments220..226: generated global control IPC
-----------------------------------------------------------

Direct220 starts PF0 global control independently of Bluetooth initialization
or radio resource publication. Twenty alternating generated queue registration
and qualified four-byte transport requests complete, including ring wrap,
with IPC2/DART0. The four-byte request's high-level function remains unassigned.
Direct221 changes only header+0x44/+0x46 from16 to8 and host ring arithmetic;
twenty requests wrap at8 with IPC2/DART0. These are separate u16 ring lengths,
not an indivisible captured word.

Direct222 zeros only PF0 header+0x48, retaining the sibling's original profile.
Generated Wi-Fi management queries8/10/47 and a real Bluetooth connection,
role response and disconnect all pass, DART0. Its function is still unknown;
zero is sufficient in this tested path. Direct226 separately zeros the Beta
header's same field and repeats those radio operations successfully. Both
clean Python builders now generate zero rather than retain0xffff0000. The
Beta bit26 at+0x50 still selects independently measured32-byte completions;
these distinct fields are not conflated.

Direct223 disables peer streams2/3 and invalidates their roots, then registers
all four shared queues using only PF0's domain. Queue2 receives two buffers,
IPC2/DART0. Direct224 tests the same ownership through complete generated
radio initialization, Wi-Fi management and a Bluetooth connection/role/
disconnect. Translation checks show the global arena mapped only in SID1,
with SID2/3 unmapped. Shared queue memory no longer needs aliases in the
radio functions' domains. The Python allocator now requires only its owning
IPC stream; it does not rewrite the peer roots to share this arena.

Linux225 ports this generated protocol to ``centauri_control.rs``. After the
native firmware/reset/rescan pipeline, PF0 allocates a16KiB context and2MiB
arena through its normal DMA API domain. The Rust builder generates relocated
pointers, explicit queue counts1/13, ring counts16/16, zero header+0x48 and
all four52-byte registration frames. Queue8 and2 use external receive buffers;
queue4 is inline receive and queue3 inline transmit. No captured context or
firmware-owned mapping is adopted. Transport completion0x401 must match the
submitted length and cookie before a slot is reused. Bounded waits, DMA range
checks and preflight rejection of stale IPC precede publication.

Linux225 reaches image2/IPC2 at1.793190s. A bounded read-only capture of owned
DMA metadata verifies all four registrations completed, queue8/2/4 credits
3/15/7 and actual firmware consumption of queue8=3 and queue2=4 buffers. DART0,
stable interrupt counts11/1 on vectors0/1. Runtime unbind removes all four MSI
handlers and leaves DART0. This first version posts initial buffers only;
it does not yet service receive queues or initialize the radio functions.

Compared with existing drivers, PCI/MSI and DMA/DART remain ordinary Linux
framework ownership. The added protocol is Centauri PF0's generated control
context, queue registration and shared event service. Peer functions retain
their own normal domains; no cross-function global-arena mapping is required.
This was measured on T8142/J813, not Neo; other compatibles need equivalent
qualification rather than model/serial checks. Three external input sources
(``nvrm``, ``_cal``, ``_apb``), cold radio integration and actual Linux curl/HCI
acceptance remain unresolved. No Apple executable inspection was performed.

2026-11-02 experiments227..229: native shared receive servicing
--------------------------------------------------------------

Rust now keeps host-owned receive cursors and validates device indices,
posted windows, lengths and buffer DMA pointers before returning any completed
batch. Queue8/2 payloads stay opaque. It reposts only free slots, ordering
descriptor writes before the host cursor and the cursor before its doorbell.
Queue4 ControlApp replies remain intact for the later initialization state
machine; this event worker neither consumes nor discards them.

C schedules a serialized worker from MSI, with an initial drain for events
that arrived during polled startup. It uses Linux7.3's ``system_dfl_wq``, with
no recurring polling timer. Removal disables scheduling/mastering, synchronizes
all IRQs, cancels work, destroys Rust bookkeeping and only then releases DMA.
Malformed event metadata stops service and clears mastering before any free.

Trial227 fails the private chainloader's reconnect/NOP before Linux entry and
is excluded. Linux228 services seven events and unbinds cleanly, but reports
the deprecated ``system_wq`` API;229 repeats with its supported replacement.
The228 private read-only snapshot omitted explicit cache invalidation for the
proxy's alias. Its apparent device-cursor wrap is not valid evidence. The
corrected229 capture invalidates each bounded physical metadata range before
reading it and agrees with the kernel's teardown counter.

Linux229 reaches image2/IPC2 at1.795569s with no workqueue warning. It services
four queue2 and three queue8 completions. Their device/host cursors are4/3
and3/2 respectively, retaining15/3 posted credits. Host producers wrap during
replenishment; firmware consumption of the replacement buffers is not yet
demonstrated by this idle startup. Interrupt counts settle at11/1 and DART0.
Unbind reports seven serviced events, removes all MSI handlers and leaves
DART0. Eleven Rust control tests include repeated receive wrap, partial
batches, preserved ControlApp replies and malformed cursor/length/address
rejection. Thirteen focused Python tests and a full16KiB kernel build/link
pass; changed C/header checkpatch reports zero errors, warnings or checks.

This extends only global control/event ownership. Normal per-function radio
resources, native Bluetooth/Wi-Fi startup, whole-chip reprobe and the three
external device-input sources remain open. No cold Linux network or Bluetooth
acceptance is claimed.

2026-11-02 experiments230..242: independent Wi-Fi startup requirements
---------------------------------------------------------------------

The presumed dependency on Bluetooth initialization and the ControlApp hello
is removed for Wi-Fi. Direct230 creates only PF0 control/service queues and
PF1's own copied ``msww`` resource. Beta's domain remains disabled and its
context/resource are untouched. Wi-Fi firmware reaches2 within the sampled
approximately70ms. Direct231 fails private helper scaffolding before context
publication and is excluded. Corrected232 publishes generated Wi-Fi IPC,
registers all twelve basic queues and successfully queries8/10/47 with no
Beta or greeting. Its DMA window excludes the global control arena. The clean
Python runtime now computes only PF1's window;234 qualifies the change through
full generated radio startup and an actual Bluetooth link/role/disconnect.

The Rust port parses only the external FTAB directory to locate each radio
region. The current firmware-produced allocation is the source, never a
retained reference image. The real Wi-Fi source starts at offset640, so only
destination size/address alignment is imposed. The consumer's ``apple,control``
phandle must refer to the same multifunction PCI device's control node. A
managed device link orders supplier removal; its device lock protects the
bounded opaque copy. The child owns its fresh resource/context/pool through
the normal DMA API. Linux IOMMU/MSI frameworks handle all translation/routing.

Native233 boots the Wi-Fi firmware independently, but the first draft uses
Beta's handshake offset. Native235 corrects this to Wi-Fi+0x1028, reaches state1
and then fails publication with firmware3/IPC1, DART0. Both error paths clear
mastering and release allocations. Stale233 rebind refuses EBUSY before DMA.
Direct236 is excluded for a chainloader reconnect timeout before N1 testing.

Direct237 isolates MSI Enable: disabling only the Wi-Fi endpoint's MSI before
context publication reproduces firmware3/IPC1 at the qualified low addresses.
Direct238 uses the native near-4GiB addresses with MSI16 and passes all twelve
basic queues and queries8/10/47. Direct239 shows one enabled message suffices
for these operations; full data-queue interrupt routing still needs work.
The Linux port now allocates/owns all16 handlers in its current profile.

Direct240/242 separately expose a resource addressing limit: high config
address word1 leaves Wi-Fi firmware0 despite valid translation and no DART
error.242 places the resource exactly at4GiB and waits three seconds before
attempting any context handshake. Direct241 instead keeps the resource below
4GiB and places context/pool at4GiB; all twelve registrations and three queries
pass. Linux now constrains only the resource allocation to32 bits, restores
the wider coherent mask for context allocation and checks representable
window bounds. The context's high word is functional; these are not equivalent
address fields. Header+0x60 profile0x00500000 remains individually unassigned.

The cold Wi-Fi node has a chip compatible plus PCI identity fallback, without
serial/model gates or a firmware-DART/reserved-memory handoff. Native control
and radio schemas select their specific compatibles; the older handoff schema
selects its single-compatible nodes. Existing handoff transport and new cold
initialization remain separate. No Neo hardware was tested: the reused PCI,
DMA/IOMMU and device-link APIs apply generally; only the measured T8142 host
reset and Centauri protocol profile are qualified here. Native basic queues,
cfg80211/HCI, whole-chip reprobe and three external input sources remain open.

2026-11-02 experiments243..244: native Wi-Fi resource and IPC
------------------------------------------------------------

Linux243 reaches Wi-Fi firmware2/IPC2 automatically at1.947514s; fresh244
repeats at1.939007s. The consumer copies only opaque output from the current
Linux-owned companion into its own2MiB resource. The32-bit resource mask is
restored to the wider coherent mask before allocating its10MiB context/pool.
Rust generates the context and validates range/window/unused-state invariants
before enabling DMA. PCI allocates16 Wi-Fi MSI handlers in addition to the
four control handlers; vector0 receives three Wi-Fi events. No Beta context,
mswb resource, ControlApp greeting or reserved-memory handoff participates.

The first243 private observer incorrectly reads the unbound Bluetooth BAR
with its memory decoding disabled and faults in the bootloader. Its incomplete
snapshot and teardown are excluded; the already sampled Wi-Fi ready2 values
remain a startup observation. Corrected244 reads only the enabled, owned Wi-Fi
BAR and bounded typed metadata, invalidating the proxy's cache alias first.
It verifies header equality with the generated Python geometry, counts234/192,
zero initial admin/data cursors and all context/resource translations in PF1.
Context DMA is0xff000000, resource0xffc00000 and window length0x00e00000.
Beta's PCI command and resource length remain zero; its BAR is not accessed.

The kernel exposes the control supplier and native IOMMU links for PF1.
Unbinding only the control supplier automatically unbinds the Wi-Fi consumer
first. Both PCI command words finish2 (mastering clear); all20 MSI handlers
are removed, global service reports eight events and DART remains0. This
qualifies dependency teardown, not whole-chip restart/reprobe. The used-state
guard still requires reset before a new radio publication.

Seven native Rust radio tests cross-check context geometry and the handshake
register against independent Python definitions, including a context above
4GiB, resource-address rejection, stale state, MSI/config failures and bounded
timeouts. Eighteen parser/bootstrap tests and three DT generation tests pass.
The full16KiB kernel builds/links without warnings; new and changed C/header
checkpatch is clean. All three Centauri binding documents and focused schema
validation of both cold and legacy trees pass, including their distinct
selectors. No Apple executable contents were inspected.

The next native step is the Python-qualified basic/admin queues and management
commands, followed by cfg80211 authentication/association and Ethernet traffic.
Native Bluetooth cold context/HCI initialization remains a separate port.
The existing Linux packet transports still use warm handoffs; no cold Linux
curl or Bluetooth acceptance is established by this milestone.

2026-11-02 experiments245..247: native basic queues and interface
----------------------------------------------------------------

Rust now generates all twelve basic queue registrations, owns per-cookie
buffer lifetime, and services management and opaque auxiliary completions.
Six pairs use the independently qualified Python geometry. A consumed SQ
entry does not release its buffer: only its matching validated CQ cookie
returns ownership, including out-of-order completions. Replenishment respects
both the ring gap and outstanding ownership. Unknown queue3..6 bodies stay
opaque. Queue2 copies only bounded, transaction-matched solicited bodies.

Linux245 queries initial parameters/country/channels at2.059500s:18 TLVs,
country XZ and56 channel records. A cache-invalidated private observation
compares all twelve generated frames with the independent Python encoders,
checks drained cursors and all five RX-credit targets. During approximately
17 minutes Linux processes3994 receive completions, proving firmware
consumption of replacement buffers and repeated queue6 SQ/CQ wrap. Data MSI2
is active; admin/startup MSI0 totals15. The exact queue-profile interrupt
field remains under qualification. No recurring polling timer is required.

The standard wifi0 alias lets existing bootloader DT fixup copy chosen
mac-address-wifi0 into the Wi-Fi node's local-mac-address. No bootloader code
change or board/serial gate is involved. C uses the firmware MAC property or
a generated local unicast fallback; Rust sets the template and verifies the
active interface's returned address. Deactivation and activation take the
qualified five-byte zero selector/options body, never an empty body.

First draft246 rejects the first deactivation reply because it incorrectly
treats the fourth prefix word as a universal status. Earlier172 already
observed a valid inactive reply with nonzero word3. Corrected247 preserves
command-specific normal reply words; explicit asynchronous rejection aliases
retain their qualified error contract. Hardware247 confirms word3=01430000
on inactive commands3/20, then activates the factory-address interface at
2.206877s. All eight generated requests match their typed definitions and
the address bytes match the current ADT. This proves activation, not a link.

Unbinding the control supplier deactivates the interface while DMA is live,
verifies inactive state, then clears mastering, synchronizes interrupts,
drains work and frees Rust state and DMA. Trial247 logs TX10/RX1280 at357s;
245 logs TX3/RX3994. Both leave PCI command2 on control/Wi-Fi, remove all20
interrupt handlers and retain DART0. The246 error path also frees its consumer
resources. Full whole-chip restart/reprobe remains separate work.

Twenty-three Rust tests cross-check Python-generated registrations and
exercise both ring wraps, cookie ownership, malformed completion rejection,
request correlation, partial-budget service and interface/address lifecycle.
The full16KiB kernel links; C/header checkpatch and cold DT schema validation
pass. This cold path uses normal PCI/DMA APIs rather than the older warm
transport's reserved-memory adoption. Neo has not been tested; no J813
identity is baked into the radio protocol. cfg80211 scan/association, Ethernet
traffic, native cold HCI, the three firmware input sources and final goal
acceptance remain pending. No Apple executable inspection was performed.

2026-11-02 experiment248: native cfg80211 passive scanning
---------------------------------------------------------

The cold radio now registers a normal station wiphy and wlan0. C adapts
cfg80211, net_device lifetime and work scheduling; Rust generates the scan
command, correlates its reply/event and owns report decoding. No handoff,
Python network process or macOS initialization supplies scan results.
The first version advertises only passive2.4GHz scanning on channels1..11;
active/directed requests, extra IEs and unsupported flags return EOPNOTSUPP.
Carrier and TX queues stay stopped because association/data are not ported.
The no-SSID firmware format is used for passive requests; on-air probe
suppression has not yet been independently qualified. Thus this initial
mapping and its NO_IR advertisement still need RF validation before an
upstream-ready passive-scanning claim, even though real BSS discovery works.

Scan requests use the qualified zero option words and50/50/110 timing profile,
with the explicit channel vector from cfg80211. Their individual timing/option
bit roles remain under qualification. Queue4's completed body is bounded and
copied before returning its DMA cookie. The standard radiotap header length
locates the802.11 frame; signed signal and channel frequency become cfg80211
metadata. FCS removal follows only the standard flag. Malformed RF frames
are dropped locally, without stopping all DMA service or following pointers.

Control event80000005 may be drained before the final queue4 batch. Rust
waits for its declared report count before completing cfg80211, matching the
independent scan cookie rather than the management transaction. Interface
down, abort or a ten-second host timeout detaches the cfg80211 request and
fences late reports. Since no firmware cancellation command is qualified,
a new firmware scan remains blocked until the old real completion or full
teardown; the host does not guess a cancel operation.

Native248 registers the interface at2.209729s. The small userspace
centauri_scan_test.c client uses only standard nl80211 and has no libnl/iw
runtime dependency. It is built statically for the test initramfs. Eleven
scans finish with status0, exactly388 delivered/decoded reports and zero
invalid reports. A kernel BSS dump sees the operator's network at2462MHz,
approximately-40/-38dBm. All eleven advertised2.4GHz channels are exercised.
The initial scan works with the firmware's queried XZ country profile; the
new standard regulatory notifier/country-readback path is not yet separately
hardware-qualified. A normal signed regulatory.db is supplied in the private
initramfs. No regulatory domain or country is hard-coded in the driver.

Bringing wlan0 down during a scan and back up permits a subsequent scan.
Unbinding only the control supplier during the twelfth scan removes wlan0,
the wiphy and all20 interrupt handlers, verifies firmware inactive before
DMA release, and logs TX22/RX2562. Both command registers finish2 and DART0.
No warning/stall is observed. Thirty Rust tests cover generated scan
bytes against Python, bounded radio decoding, completion-before-reports,
wrong cookies, malformed RF data, presence extension alignment, country
readback validation and fenced cancellation. The16KiB kernel
build/link and changed C/header checkpatch pass without diagnostics.

Compared with existing Broadcom drivers, the cfg80211/BSS interfaces and
standard frame formats are reused; Centauri's command IDs, two-queue scan
completion and independent cookie remain chip-specific Rust protocol. No Neo
hardware was tested. The partial capability advertisement is intentionally
limited to the qualified receive path; wider bands, active scanning, normal
authentication/association, Ethernet queues and cold HCI remain pending.
The three external firmware input sources and final curl/Bluetooth goal
acceptance are also still unresolved. This milestone is scanning only.

2026-11-02 experiment249: final scan source and teardown guard
--------------------------------------------------------------

Final source2dac395fd10d rechecks service_running under the state lock, so a
worker waiting behind removal cannot resume queue service after shutdown.
Fresh249 registers the native scan interface at2.213787s, completes36/36
reports with invalid0 in614ms and finds the test BSS at2462MHz/-39dBm.
Removal during the following in-flight scan verifies inactive state before
DMA release, logs TX12/RX393, removes netdev/wiphy/all20 handlers and leaves
both PCI command words2 and DART0. No warning, BUG, SError or stall appears;
the private console harness exits normally. Exact compiled-source hashes
match the committed files. Thirty Rust tests, the16KiB kernel build/link and
C/header checkpatch pass. Native association/Ethernet/HCI, firmware input
provenance, regulatory/passive-RF qualification and whole-chip reprobe remain
open; this still does not satisfy the curl/Bluetooth goal.

2026-11-02 experiments250..254: native WPA2 association and disconnect
---------------------------------------------------------------------

The cold cfg80211 adapter now accepts the independently qualified single-BSS
WPA2-PSK/CCMP path, with a PMK supplied through standard nl80211. Rust builds
the159-byte fixed prefix plus explicit SSID/RSN TLVs; there is no plaintext
passphrase record or native connection snapshot. Firmware AKM4 is mapped
separately from IEEE RSN PSK2. Leading byte1, word132=1, SSID flags4 and five
timing words remain individually unassigned qualified profile fields. The
interface operation can remain five zero bytes: native250 and Python251/252
prove association without the former options11 value.

Rust waits for the distinct40020000 acknowledgment and later80000000
outcome. The latter's90-byte extent, public44-byte prefix, management
transaction, SSID and BSSID must match before reporting success. Its remaining
46 bytes stay opaque. An early outcome cannot override a rejected ack, and
cancelled operations fence late callbacks. C adapts cfg80211 request/work
lifetime and retains a BSS reference from request acceptance through
cfg80211_connect_bss(). It rejects a missing scan entry before firmware.
Native253 first connects/disconnects successfully, but its second firmware
connection exposes an expired-BSS warning in the first draft's late lookup.
The held-reference fix is qualified in fresh254, rather than ignoring it.

Native250 sets US through the normal regulatory request, verifies firmware
readback and associates with status0 and matching target metadata. Direct
interface-deactivate3 then returns word3=01470000 and info remains active2.
The first draft incorrectly cleared its active flag before readback; this
is fixed. A joined interface requires a separate disconnect operation.

Python251 identifies command20001, ack40020001 and empty completion80000004,
with matching transactions. A generated sixteen-byte native-profile request
works. Python252 removes optional TLV148/value30 and varies the first word:
eight-byte bodies with values8,3 and0 all disconnect, permit deactivation
and support subsequent fresh PMK-only joins on the same chip/context. A
four-byte value3 is rejected with01530000 and no completion; deactivation
still fails01470000. Firmware/IPC remain2 and DART0. The Rust port therefore
generates eight zero bytes and waits for both ack and completion before
deactivation. These two words are not assigned an IEEE reason encoding;
C reports the locally requested reason after the actual disconnect completes.
This removes the optional record and all unexplained nonzero disconnect data.

Native254 performs two PMK-only associations, with an ordinary nl80211
disconnect, firmware deactivation/reactivation and rescan between them.
Both final outcomes are0 and both kernel BSS dumps carry status1 (associated).
The scans deliver34/34 and35/35 reports, invalid0. Firmware/IPC stay2/DART0.
Packet queues and carrier remain stopped: these observations prove native
association, not a Linux network/data or security-authorization notification.
The broader supplicant/active-scan/control-port integration is still pending.
Joined supplier removal in254 verifies disconnect completion and inactive
state before releasing DMA: TX36/RX11278, global499 events, both command
words2, netdev/wiphy/all20 IRQs removed and DART0. No warning is observed.

Credentials enter the private test client through echo-disabled stdin, never
argv or stored configuration. C frees its copied input with kfree_sensitive;
Rust wipes the temporary encoder and the returned transmit slot. A bounded,
cache-invalidated private253 audit reads only our completed Linux TX slots
and records booleans: both entire8192-byte connection slots are zero. No key
bytes are saved or printed. Destructor fallback wipes an unreleased key slot
after DMA is stopped. Thirty-eight Rust tests include independent Python
encodings, request/result correlation, ack/event ordering, cancellation,
returned-buffer wiping, disconnect timeout fencing and proper stop/rejoin.
Full16KiB kernel build/link, static nl80211 helper compilation and changed
C/header checkpatch pass without diagnostics.

This differs from existing Broadcom drivers in the firmware command/event
protocol and generated host-owned queues; cfg80211 PMK/BSS/request lifetime
uses normal Linux interfaces. No Neo hardware was tested or serial gate
introduced. Cold Ethernet queues/net_device packet I/O, exact handshake and
link-loss notifications, native cold HCI, three independent firmware input
sources and complete power/reprobe integration remain open. No Apple
executable contents were inspected; final curl/Bluetooth acceptance is not
established by association alone.

2026-11-02 experiments255..256: native Linux Ethernet and authorization
----------------------------------------------------------------------

The Rust cold port now generates four extended data registrations, descriptors
and buffers from fresh coherent DMA. The allocation consists of the existing
10MiB context/basic prefix followed by a separate0x4b8000-byte packet region;
the whole range is published before firmware IPC startup. Basic queue storage
and packet storage cannot overlap. No existing association, retained packet
buffer or reference context is adopted. Firmware inputs nvrm/_cal/_apb still
have unresolved independent sources; these successful tests do not remove
that provenance requirement.

TX uses source25, completion76 and context cursor192. RX uses source12,
completion15 and cursor207. Both source rings have2048 entries; completions
have8192 TX and4096 RX entries of32 bytes. The RX pool contains2048 freshly
generated2048-byte buffers;2047 are posted initially. Cookies identify buffer
ownership independently of submission slots. A bounded deferred-return queue
preserves completed RX cookies when a completion precedes the device SQ cursor.
Source consumption alone never releases a buffer. TX permits one outstanding
frame and rotates64 generated buffers. Aligned volatile u16 cursor accesses,
DMA barriers and source-only doorbells preserve publication order.

The Linux adapter queues skbs in ndo_start_xmit without sleeping, then services
them in the existing serialized worker. It copies completed RX Ethernet data
into normal skbs after removing the qualified two-byte padding. Basic and data
queues each receive a bounded service budget. A separate link worker creates
the packet state after the matching successful PMK-only connection; it avoids
reentering the Rust basic state from its own callback. Disconnect synchronizes
TX and purges software packets before a new association can start. Removal
drains both workers/IRQs and stops DMA before freeing either allocation.

Actual M5 Linux255 obtains10.0.0.159 through udhcpc and performs curl through
wlan0 with proxy use disabled: google.com returns HTTP301/219 bytes, exit0;
www.google.com returns HTTP200/87179 bytes, exit0. The only other interface is
loopback. A normal disconnect, a deliberate synthetic all-zero PMK failure
(firmware outcome1), then a correct PMK reconnect (outcome0) is followed by
fresh DHCP and another successful curl. An earlier expired-BSS rejection is
ENOENT before firmware submission and is not counted as wrong-key evidence.

Two further native stress runs complete180 HTTP200 responses containing
15,684,982 response-body bytes. Final teardown reports TX9053, RX15863 and
15863 recycled RX buffers, exercising all four source/completion wraps.
Interface errors remain0; RX drops0. The21 TX drops occur during the deliberate
offline/wrong-key period, so zero lifetime TX drops are not claimed. Joined
supplier removal verifies disconnect/inactive state, removes all20 IRQs and
leaves both PCI commands2 and DART0. No kernel warning or queue-service fault
is observed. A private metadata observer confirms descriptors/cursors/window,
but consumed admin payloads disagree with its expected bytes: that comparison
is inconclusive, not runtime exact-frame verification.

Correct-key bidirectional data and the wrong-key failure establish the mapping
used for cfg80211_port_authorized. Fresh Linux256 observes standard nl80211
CONNECT status0 followed by PORT_AUTHORIZED, carrier1, a fresh DHCP lease and
google.com HTTP301/219 bytes/exit0. Joined removal drains TX24/RX68, marks the
interface inactive and removes all20 IRQs. The private observer was unavailable
after this boot's teardown; the final PCI/DART readback belongs to255 only.

Forty-four Rust checks pass, including independently generated Python wire
fixtures,8300 changing TX frames,5000 changing RX frames, both wrap dimensions,
delayed SQ publication, malformed completions and wrong-cookie ownership.
The full16KiB Linux kernel builds/links, the static nl80211 helper compiles
with -Wall -Wextra -Werror, and changed C/header checkpatch passes.

Compared with existing Broadcom/Asahi network support, Centauri uses its own
firmware command protocol and these generated extended rings. Linux DMA,
IOMMU, PCI/MSI, skbs and cfg80211 remain ordinary kernel interfaces. Neo was
not hardware-tested; there is no model or serial restriction. No NAPI, wider
band/cipher qualification, active/directed scan, ordinary supplicant workflow,
remote link-loss recovery or complete chip power/reprobe support is claimed.
MSI3/8 traffic correlates with the data service profiles but does not isolate
the individual registration fields. Profile flags and TX bit17 retain qualified
names until Python probes establish their meanings. Cold native HCI, actual
Linux Bluetooth connection/response and the three independent input sources
are still required before completing the combined goal. No Apple executable
contents were inspected.

2026-11-02 experiments257..258: cold native HCI and encrypted data
----------------------------------------------------------------

The Rust Bluetooth port replaces its reserved-memory constructor with a
fresh16KiB coherent allocation owned by PF2. The control supplier copies only
its current opaque mswb firmware output into PF2-owned DMA; Linux retains the
supplier link until consumer teardown. There is no CENB/nonce/context handoff,
raw DART programming or previous host queue adoption. DT selects the physical
Centauri Bluetooth function and supplies standard local-bd-address; the existing
bluetooth0 bootloader fixup converts the ADT address to HCI little-endian order.
Neither serial nor model gates are introduced. GPL-2.0-only applies throughout
this Linux implementation; no Apple executable contents are inspected.

Rust constructs the qualified Beta header,22 source indices,16-entry global
admin rings with32-byte completions, and a single reusable52-byte registration
frame at+f80. Command/event rings begin at+1000/+1240 and each have two slots
with260-byte inline capacity, derived from standard HCI header/parameter bounds.
Read Buffer Size runs before ACL registration. Its reported2586-byte MTU yields
2592-byte capacity after the HCI ACL header and alignment. The TX/RX starts
are derived at+1480/+2900; no reported controller MTU or queue DMA address is
retained as a fixture. The one-page bound rejects unsupported larger layouts.
Capacity words use the Python-qualified mantissa/exponent formula rather than
replaying30510001; the equivalent20a20001 encoding is exercised natively.

The C adapter uses normal PCI, IOMMU, coherent DMA, skbs and HCI registration.
Its process-context mutex permits bounded receive allocations. The qualified
polling profile allocates no PF2 MSI handlers; interrupt service is future work.
Only the driver's setup may send FC06 with its own DT address and empty FF0B.
A standard Read BD_ADDR verifies the FC06 byte order. Successful HCI Reset
Complete remains held until the driver's FF77=0 completes; normal HCI then
owns initialization, pairing and data. Vendor-event bodies remain unread.

Native257 registers hci0 at2.361412s, finishes factory/empty-init setup at
2.776611s and reaches UP RUNNING, ACL MTU2586/8, no HCI RX/TX errors. Standard
Linux tools create an actual Classic link to the verified test host, receive
RSSI and retrieve its name/LMP version/feature pages. Actual ACL control traffic
reaches6 TX/6 RX. Concurrent wlan0 obtains fresh DHCP and google.com HTTP301.
Joined Wi-Fi/HCI supplier removal drains both consumers: Bluetooth commands115,
events149, ACL6/6, vendor-skipped3; Wi-Fi data TX25/RX164/recycled164. All20 IRQs
are removed, all three PCI commands read2, DART0 and the private harness exits0.

The aligned clean-and-invalidate observer in257 verifies the generated Bluetooth
header and last registration, plus all four Wi-Fi Ethernet registration frames,
against Python builders. This is exact runtime metadata evidence for257. The
invalidate-only255 consumed-frame mismatch remains inconclusive; no driver
change was inferred from it and it is not retroactively counted as a match.

Fresh258 repeats cold startup, then starts standard D-Bus/BlueZ with an empty
Linux pairing store. The host's previous test bond is removed. An initial SDK
pair call fails, but explicit host connection triggers new pairing: kernel HCI
reports pairing status0, encryption enabled2 and successful key-size query.
A Linux L2CAP socket verifies security level2 and key size16. Its encrypted
standard SDP request receives a valid129-byte response. A further2000 requests
with changing transaction IDs validate every response envelope/ID and return
258000 response bytes for40000 request bytes, SHA256
3a7f19eb9d4aa4852af68814d17b4bf0edb5fe7b8aca195c42b60a8efe94828d.
HCI totals then show2075 ACL TX/2071 ACL RX with zero errors. Both two-slot
inline ACL rings repeatedly wrap under actual native Linux traffic.

Twenty HTTP200 downloads also finish while the latter Bluetooth transactions
run; wlan0 shows RX9301/TX4865 with no interface errors or drops at that sample.
Those response bodies total1743301 bytes. Supplier removal with the authenticated,
encrypted ACL link and BlueZ still active removes both radio consumers and20
IRQs. Final Bluetooth counters are commands116/events1210/ACL2075TX/2071RX,
vendor-skipped2; Wi-Fi data TX4865/RX9627/recycled9627. All three PCI commands
read2, DART0, no kernel warning is observed and the console harness exits0.
The test userspace archive itself was downloaded over wlan0 and hash-verified.
This does not use an external network namespace, Python hardware transport or
VHCI. Python supplies ordinary sockets and test validation only.

The larger Mac SDK echo attempt is not a pass. Synchronous channel open returns
E00002BC after Linux accepts an encrypted channel; asynchronous open returns0
but no successful completion/payload arrives before the peer channel closes.
Likewise l2ping prints exit0 despite five lost replies. These are preserved as
unresolved tests, not proof of2000 changing512-byte echoes or a diagnosed driver
fault. The previously passing full-size Python/warm-Rust echoes remain separate
historical evidence. Native larger payload/profile, LE/SCO/interrupt/low-power
coverage, complete chip power/reprobe and the three independent configuration
record sources remain open. Successful encrypted native SDP and Wi-Fi curl do
not alone complete the no-macOS-dependency goal.

Four new Rust model checks compare context/registrations to independent Python,
exercise a different reported MTU, reject used state/corrupt admin/missing MTU,
and verify Reset/FF77 ordering plus100 changing ACL round trips. Together with
the44 radio checks, the full16KiB kernel build/link, changed C/header checkpatch,
Bluetooth binding document and prepared-node validation pass. Schema compilation
also emits existing unrelated PCI binding type warnings; these are not Bluetooth
node failures. The host-only event metadata tool observes Linux HCI after driver
normalization and excludes key/passkey/vendor bodies. The SDP stress tool has an
explicit peer argument and no fixed device/peer identity gate.

Compared with Asahi hci_bcm4377, Centauri retains the Create Transfer Ring/footer
concept but requires the qualified capacity scaling, Beta context and in-place
queue profile. Shared PF0 firmware/control ownership, factory FC06, empty FF0B,
Reset/FF77 sequencing and F0/F1 ACL link-type normalization differ from ordinary
USB/UART HCI transport. Neo hardware has not been tested; the same physical chip
binding and standard per-device identity/resource properties are used, with host
PCI/power topology left to its own DT/providers. No Neo compatibility claim is
inferred solely from the M5 test.

Antenna-SKU NVRAM source259
----------------------------

The42-byte nvrm input is a nonexecutable host configuration store with one
wsku entry. Its X0 payload matches /arm-io/centauri's wifi-antenna-sku-info
in the original258 ADT. The existing bootloader antenna fixup interprets the
last8 bytes of that16-byte property as the apple,antenna-sku string; its current
/arm-io/wlan path is absent on this T8142. This is an observed M5 metadata-path
difference; the Neo path/value has not been tested. No kboot.c change is made.

The clean Python encoder now builds this store from an explicit SKU or the
standard terminated DT string property. It reads no captured NVRAM record.
The qualified layout is::

    +00 u32 signature 4e767653
    +04 u32 CRC32 over the entry bytes at+24..end
    +08 u32 total record length
    +12 u32 same total length
    +16 u32 profile1, individual role unassigned
    +20 u32 profile1, individual role unassigned
    +24 four-byte entry tag wsku
    +28 u32 profile0, individual role unassigned
    +32 u32 SKU payload byte count
    +36 u32 profile00020000, individual role unassigned
    +40 ASCII SKU bytes without terminator

Do not rename the unassigned words count/version/type solely from this one
single-variable record. The lengths and checksum are computed, and the SKU
comes from platform data. There is no X0/device/serial default in the encoder.
prepare_centauri_firmware.py accepts mutually exclusive --nvram, --antenna-sku
or --antenna-sku-file; calibration/AP-policy still require explicit sources.

Private independent-inputs-259 derives X0 from the current ADT and regenerates
all42 bytes, SHA256 cdbee9751ea37cbbd4fe857dff6ad06f0f5d8c10aa69fe334bbb3ccdbfa8a8f7.
Replacing the nvrm entry in the qualified opaque container leaves the entire
image SHA256 exactly0330b3f1a8131b8ff94cbaedc28a953099fd89dea99e1a7d23194989452c2a4e.
This is input-source equivalence with the255..258 hardware-tested image, not
a claim of a new hardware run or qualification of other SKU values/formats.
No Apple executable payload is inspected. The private initrd now uses that
new generated-input artifact instead of reading the captured NVRAM file.

The captured input remainder falls by42 to36204 bytes: _cal35692 and _apb512.
Runtime DT-to-kernel firmware assembly and the missing T8142 antenna fixup still
need integration; generating a package from a DT property is not that runtime
implementation. Four synthetic encoder checks plus a DT-to-packager CLI check
and the six existing FTAB checks pass. They cover changing SKU/CRC/length,
missing/oversized/non-ASCII input, exact termination and vendor/input separation.

Cold259: generated AP policy and an independent record4 input
-------------------------------------------------------------

The nonexecutable512-byte _apb host input contains two-byte TLV headers: u8
kind and u8 total length including the header. Type1/length31 carries29 policy
bytes. They exactly match the current M5 ADT, in this order: six ordered cells
from wifi_power_bo_2g_table narrowed to bytes, its temperature threshold byte,
the same for5g, six duty-cycle table cells plus its threshold, then the raw
32-bit low/high voltage-threshold representations in little-endian order.
No table axes or threshold units are inferred beyond the named source fields.

Type2/length3 contains the qualified profile byte1. Type4/length6 contains four
boot-varying bytes. Type255/length2 terminates the list, with zeros through512.
Boot22 and134 have identical first34 bytes and differ only at36..39. Thus the
four-byte value is not a deterministic checksum solely over those static policy
fields. Its meaning/source is not established; do not call it a nonce, time or
cryptographic seed. The clean experimental encoder requires an explicit value.
Three synthetic checks cover table order/width, TLV lengths/padding, voltage
bit preservation and changes limited to record4.

The private259 probe supplies fresh OS CSPRNG bytes for record4 and derives all
policy values from ADT. Its only difference from the qualified reference _apb
is bytes36..39. Generated nvrm remains byte-identical; all vendor executable,
signed-manifest and calibration components are copied opaquely without change.
The new full image SHA256 is
1746f07f3dacce9b71d76acf8bb5b2af7cee83b6053a5d55e2aadb59348cac3c.

Cold Linux259 registers HCI at2.464662s and finishes setup at2.928283s. Wi-Fi
scans28/28 reports, associates with outcome0, obtains fresh DHCP and returns
google.com HTTP301/219 bytes/exit0. BlueZ starts with hci0 DOWN and powers it
through MGMT; discoverability is retried after the asynchronous power completion.
The first immediate host connect fails, and a subsequent Linux-initiated
pair reports authentication status5. A later host connection on the unchanged
image completes new pairing status0 and encryption enabled2. Standard host
support readback has SSP/LE/SC enabled; no firmware/driver change is made.
The failed attempt is not attributed to record4 or a diagnosed startup bug.

An encrypted Linux socket then validates128 SDP replies at security2/key16:
2560 request bytes,16512 response bytes, SHA256
63582a64c5de6f2c227bda4d0e4d49b62ff692e7045e8e73aef73b37b8fa238d.
HCI error counters stay0. Paired/encrypted supplier removal with BlueZ active
removes both consumers/all20 IRQs; Bluetooth ends at commands118/events240/
ACL179TX/181RX/vendor-skipped2, Wi-Fi TX50/RX763/recycled763. All three PCI
commands2, DART0, no kernel warning and console harness terminal0 are verified.

This establishes the tested functions accept the independently generated APB
input. It does not establish record4's purpose, entropy requirements, universal
value acceptance or wider power/profile behavior. Runtime DT/kernel assembly
and these semantics still need work. The sole remaining captured input is now
_cal35692 bytes. Its nonexecutable container header describes three contiguous
opaque components:736 bytes at256,21994 at992 and12706 at22986. None matches a
raw property in the complete current ADT. Factory-source discovery remains
open; no component payload or Apple executable instructions are interpreted.

Cold260: calibration sourced from the device factory file
--------------------------------------------------------

All35692 bytes of the remaining captured calibration input now have an
independent source. The M5's existing Hardware APFS volume contains a BWC3
factory record beneath
``FactoryData/System/Library/Caches/com.apple.factorydata``. The private
single-user macOS research boot mounts that volume explicitly read-only,
copies only this nonexecutable file and verifies its SHA-256 on both sides.
The38183-byte file has SHA-256
``0026475b431de18103043fcbbf396d3e95a1ac2d2fb0bb1c14424d592fcc14f1``.
This is an installed factory-data source, rather than a DMA transfer or an
initialized radio context. The research method uses macOS filesystem tools;
the resulting driver inputs require no running macOS service or radio state.
An installer still needs to arrange the per-unit file's extraction/storage.
Do not substitute calibration from another machine.

The IMG4 envelope names an IM4P BWC3 payload. Its nonexecutable FDR DER
container includes an SPAY SET of named OCTET strings. Numeric identifiers
encode little-endian FourCC names. The three blocks map as follows; their
internal calibration fields remain opaque:

* W2CS:736 bytes, SHA-256
  ``549859553804f354a6c4e191350fdf5943bc5ea9e9f5fb029cee212593f43951``.
* WFCS:21994 bytes, SHA-256
  ``365e6ac5255d4a0fb7122f8e24a388d68787778badaba8069910052fa45e3e62``.
* BTCS:12706 bytes, SHA-256
  ``a66ad301cb33b5f487408eb5ac75a7fd1cdfd2f8549e3e1e65f85215204d2b9d``.

Each matches the corresponding captured component exactly. The clean Python
extractor identifies blocks by their names, independent of DER SET ordering
or their sizes, and generates the256-byte host directory. Its result matches
the complete previously qualified calibration record byte-for-byte, SHA-256
``f92d0dd8e3781e8b52a105e1bc8df3a9f0d2d4a4c9f7fe64677d3e4c62e8153e``.
No captured calibration bytes enter the extraction operation. The host Mac's
separate factory record also parses with a different BTCS size, without using
its data on the M5. This is a container-format comparison, not qualification
of another board's radio driver.

``prepare_centauri_firmware.py --calibration-factory-record FILE`` now accepts
this explicit source instead of ``--calibration``. It requires the three named
blocks and the currently qualified selector, rejects duplicates, missing or
empty components, other image types, ambiguous/truncated envelopes and
nonminimal DER lengths/tags. It does not verify IMG4 signatures, create
signatures or inspect executable firmware. Signed-envelope verification and
installer provenance are separate responsibilities. The actual packager uses
only the IPSW vendor/manifest files, ADT-derived SKU, generated APB and this
factory file. The complete31252480-byte image is exactly identical to the
qualified cold259 image, SHA-256
``1746f07f3dacce9b71d76acf8bb5b2af7cee83b6053a5d55e2aadb59348cac3c``.
No radio retest or new behavioral qualification is claimed for an identical
image. Five synthetic extraction/bounds/packaging checks pass, alongside the
existing eleven NVRAM/FTAB checks.

Captured host-state input bytes are now zero. The35692 bytes of per-unit
factory calibration remain necessary opaque device data; discovering their
source does not explain their RF contents. APB record4 and the retained
profile fields still have unassigned meanings. Native DT-to-firmware assembly,
installer integration, complete chip power/reprobe, wider Wi-Fi/BT coverage
and Neo hardware comparison remain open. There are no identity gates in the
extractor. Existing Broadcom support obtains calibration through an ADT blob;
this Centauri prototype instead obtains named blocks from a factory FDR file.

Cold260 is a filesystem-source experiment. The read-only Hardware volume's
unmount returns EBUSY because macOS marks it as a system mount. SIGINT does
not reach a hypervisor prompt within the bounded observation; the exact owned
loader is terminated with SIGTERM. This is not a successful radio-teardown
qualification. The next hardware run must explicitly reset the device/chip.
Private cold260 artifacts contain the source-file hash, transfer proof,
component comparison and full-image equivalence. No Apple executable
inspection or internet resources are involved.

Cold261..264: existing SMC GPIO API and native full-chip reprobe
--------------------------------------------------------------

The ADT reset action's0x800000 parameter is different from Linux's existing
SMC GPIO CMD_OUTPUT (1<<24). It is no longer necessary to assume the two
commands are interchangeable. Python261 uses only the ADT-named gP13 key:
CMD_PINMODE returns1, CMD_OUTPUT reads0 and CMD_INPUT reads0. The key reports
four-byte ui32 data. Python262 explicitly writes CMD_OUTPUT|0/1, checks the
output readback and obtains a1900 bootstrap endpoint through initial startup
and a reset/link cycle. Pin-mode replies1/3 agree with the existing GPIO
API's output/value bits. This does not establish an external voltage rail
or give a meaning to the old action's bit23.

Python263 starts both firmware stages and generated radio contexts with
CMD_OUTPUT-based reset control and the entirely independently sourced image
from260. Both Wi-Fi management and standard HCI version requests respond.
After all three PCI bus-master bits are cleared and owned DMA is stopped,
the test asserts PERST, toggles the selected SMC output and resets the port.
The endpoint returns from1901/1902/1903 to1900 with bootstrap status0. A
second process reuses only that unsubmitted link, reloads both firmware stages
into fresh owned allocations and again obtains Wi-Fi and HCI replies. Both
Wi-Fi snapshots report firmware2/IPC2/DART error0; final bus mastering is off
and gP13 output is0. No macOS or hypervisor executes in these Python tests.
The private as-run263 script has an incorrect comment calling the chosen
Bluetooth address little-endian; the bytes and FC06 request actually use the
correct network/display order, verified by standard readback. The Linux DT
local-bd-address representation remains a separate, little-endian interface.

Native264 enables the existing MFD_MACSMC and GPIO_MACSMC kernel drivers and
adds ``t8142-centauri-smc.dtsi``. No SMC or GPIO driver change is necessary.
ADT resources translate to SMC38c600000, mailbox38c608000 and SRAM38de00000
of size120000. The mailbox FIFO interrupts are633/634/635/636 in Linux's
empty/not-empty ordering. Native RTKit protocol12 and the SMC GPIO controller
register normally; the receive interrupt counter advances on raw AIC636.
Only GPIO19 (gP13) is selected by this lab reset test, through the normal
GPIO-v2 character API. It reads high while both radio interfaces run.

The binding now accepts the already implemented generic apple,smc compatible
without adding more SoC names to its legacy list. The ASC mailbox binding
adds T8142 with the existing v4 fallback. Both binding documents and the actual
SMC/mailbox DT nodes validate. The other MFD children log absent DT nodes and
RTKit logs four unhandled oslog messages; these are recorded rather than
misreported as an entirely warning-free platform boot. No unused hwmon,
reboot or RTC provider is added merely to suppress those diagnostics.

``gpio_v2_line.py`` provides the standard Linux GPIO-v2 structures and IOCTLs;
its read-only default preserves the current pin direction. The opt-in
``centauri_reset_test.py`` requires explicit bridge, supplier, GPIO chip and
line arguments. It unbinds the shared supplier, removes all three PCI devices,
requests the selected GPIO as an output, drives0 then1 with100ms holds,
invokes the existing bridge reset_subordinate API and rescans. It does not
read raw MMIO or adopt a prior driver's mappings. Native264 qualifies this
sequence entirely through Linux:

* 190.618s: old Wi-Fi/HCI contexts are removed;190.692s: link-down event.
* 191.093s:1900 re-enumerates;191.282s: the Rust loader reaches initial ready4.
* 192.565s: new shared firmware/control ready2;192.974s: Wi-Fi active.
* 193.037s: a new generated HCI context registers;193.453s: factory/empty init done.

After that full chip reset,32/32 native scan reports precede PMK-only WPA2
association, fresh DHCP10.0.0.159 and google.com HTTP301/219 bytes, curl exit0.
The body SHA-256 is
``2178eedd5723a6ac22e94ec59bdcd99229c87f3623753f5e199678242f0e90de``.
The Linux pairing store starts empty and only the host's previous scratch bond
is removed. The first host connect pairs successfully (status0), enables
encryption2 and completes128 changing-transaction SDP exchanges through a
normal Linux socket: security2/key size16,2560 request bytes and16512 response
bytes, SHA-256
``63582a64c5de6f2c227bda4d0e4d49b62ff692e7045e8e73aef73b37b8fa238d``.
HCI reports185 ACL RX/184 ACL TX and zero errors at that checkpoint.

This proves full chip reset and firmware/radio reprobe with native Linux
providers; it does not automatically integrate a complete power sequencer
into the PCI probe. Initial global host PHY/link training is still performed
by the private bootloader. Ordinary supplicant integration, runtime DT image
assembly and broader protocol/profile coverage remain separate work. The
shared SMC/GPIO API uses the existing driver rather than a Centauri-specific
SMC implementation; other boards still require their own DT wiring. No Neo
hardware comparison or identity gate is introduced.

Native265..267: ordinary supplicant compatibility and cold acceptance
--------------------------------------------------------------------

The independently sourced firmware image remains unchanged throughout these
runs. Only the C cfg80211 adapter changes; Rust firmware/control/packet/HCI
protocol code is unchanged. The unmodified wpa_supplicant2.11 binary and CLI
already present on the private Linux build server provide the standard
userspace path. Its725 C/header files match the archived release exactly.
Jouni Malinen and contributors are the original authors; the source archive
origin recorded by that existing build is
https://w1.fi/releases/wpa_supplicant-2.11.tar.gz, SHA-256
``912ea06f74e30a8e36fbb68064d6cdff218d8d591db0fc5d75dee6c81ac7fc0a``.
Its original BSD license and the original libnl/OpenSSL component licenses
remain in force. No third-party supplicant source is imported into this tree.
The tested AArch64 binary has SHA-256
``29236b5ef671bb63846d95da6f4e161a117a92c8243df7f9dd4f6041e0af9620``.
All resources used for this investigation are local primary source/runtime
observations; no web search or Apple executable inspection is involved.

The preceding custom nl80211 client selected a fixed BSS and sent only a
minimal RSN IE. Ordinary supplicant requests expose these adapter mismatches:

* 264: even a passive scan includes a13-byte optional Extended Capabilities IE.
  Advertising max_scan_ie_len0 makes cfg80211 reject the request with EINVAL.
  The adapter now accepts a bounded standard IE vector. With no directed
  SSID vector, those probe-request IEs have no role in this passive operation;
  they are not copied into a guessed firmware field.
* 265: ordinary scans and RSN selection pass, but the original four-entry CCK
  rate table makes userspace reject the AP's required6Mbps basic OFDM rate.
  The table now reports the standard CCK/OFDM legacy rates in cfg80211's
  100kbit/s units. Firmware still negotiates physical rates. This is not a
  forced-rate qualification of every advertised modulation/rate combination.
* 266: userspace reaches NL80211_CMD_CONNECT with BSSID/frequency hints, a PMK,
  optional association IEs and the standard controlled-port setting. Hints
  now select a referenced BSS from the kernel cache, falling back to mandatory
  filters when a preference has no match. The firmware uses the selected RSN
  element and its own association capabilities; packet queues/carrier stay
  stopped until authorization. Normal requests still fail EOPNOTSUPP because
  the callback rejects every nonzero connect flag.
* 267 changes only that final flag mask relative266. The standard supplicant
  unconditionally declares MLO_SUPPORT on full-MAC connections; Linux maps it
  to CONNECT_REQ_MLO_SUPPORT, bit6. It declares a userspace capability, rather
  than requiring the driver to create an MLD. Accepting that bit on the
  single-BSS path makes the normal connection succeed. Other unsupported
  connection-mode flags remain rejected.

Native267 first removes the radio functions, toggles the chip through Linux
SMC GPIO and resets/rescans through the existing PCI APIs. It re-enumerates1900,
reloads both firmware stages and creates fresh Wi-Fi and HCI contexts. The
normal supplicant uses its passive_scan1 setting, discovers the broadcast
SSID, selects the BSS and completes WPA2-PSK/CCMP association and port
confirmation. No custom nl80211 connect client runs for this acceptance.
The safe lab control client uses only the ordinary Unix control socket:
``supplicant_pmk_test.py`` accepts hexadecimal SSID input and a non-echoing
terminal PMK, then configures/selects an in-memory network. No credential is
put in argv, a config file, a PMK file or output. It can be run with::

    wpa_supplicant -Dnl80211 -iwlan0 -C/run/wpa_supplicant &
    wpa_cli -p/run/wpa_supplicant -iwlan0 set country US
    python3 supplicant_pmk_test.py --control /run/wpa_supplicant/wlan0 \
        --ssid-hex "$SSID_HEX" --scan-frequency "$FREQUENCY" --passive-scan

A daemonized supplicant redirects standard output unless built with file
logging; the lab uses foreground execution redirected by the shell. Default
key redaction remains enabled. Earlier client timeouts and the unsupported
-f build option are setup negatives, not radio successes. Network selection
uses SELECT_NETWORK rather than assuming ENABLE_NETWORK starts a connection.

At411.792s the native scan completes10/10 reports, at411.942s firmware reports
connection status0, and at411.977s generated Ethernet queues/carrier are ready.
The unmodified supplicant reports COMPLETED with WPA2-PSK, pairwise CCMP and
group CCMP. Fresh DHCP assigns10.0.0.159 and ordinary curl bound to wlan0
returns google.com HTTP301/219 bytes, exit0. The body SHA-256 is
``2178eedd5723a6ac22e94ec59bdcd99229c87f3623753f5e199678242f0e90de``.

BlueZ starts with an empty Linux pairing store; only the host's prior scratch
bond is removed. Fresh pairing succeeds with status0 and encryption2.
While another Wi-Fi curl returns301/219 bytes,128 changing-transaction standard
SDP exchanges complete through a normal Linux socket: security2/key size16,
2560 request bytes,16512 response bytes and SHA-256
``63582a64c5de6f2c227bda4d0e4d49b62ff692e7045e8e73aef73b37b8fa238d``.
Both processes exit0. HCI reports zero RX/TX errors. The same ordinary
supplicant then disconnects, rescans and reassociates successfully at668.558s;
COMPLETED and another google.com301 follow without a credential file. At the
following checkpoint Wi-Fi counters are TX49/RX271,4497/44759 bytes, errors0,
drops0; HCI has184 ACL TX/186 ACL RX and errors0.

This meets the cold Linux connection/response acceptance for the tested
profile, with zero captured initialization-state input bytes. Per-unit factory
calibration remains an opaque static firmware input. Normal userspace owns
policy and pairing; the kernel Rust driver owns packet/HCI transport and
fresh DMA. The console-only private EL2 harness does not execute macOS or
emulate these radio interfaces. Initial global PCI PHY/link training still
runs in the bootloader. Automatic power sequencing, runtime DT image assembly,
installer automation, wider bands/security/roaming, LE/SCO/profile/IRQ/power
coverage and other-board hardware qualification remain upstream integration
work; this does not claim upstream acceptance or production completeness.

The complete kernel links and the changed946-line C adapter passes checkpatch
with zero errors/warnings. No Rust code change requires repeating the already
qualified48 core checks. Public Python tools additionally remove their three
residual host-chip identity guards; four existing metadata-bound trace checks
pass. Linux matching still uses the actual PCI functions and DT compatibles.
No serial/model gate or new hardware claim for another board is introduced.
Private native267 artifacts pin the image, kernel/config/DT/initrd, source and
results. Native264's final independent observer was inconclusive due to an
exec-name shadowing mistake and a post-exit proxy timeout; native265/266
instead use builtins.exec before exit and verify all three PCI commands2,
DART error0,20 radio IRQs removed and HV terminal0. Do not borrow that proof
retroactively for264.

Native267 additionally tests the operator's SAE-only ``Pixel_5254`` hotspot.
The native scan finds BSSID ea:b9:df:ef:b1:fd at2412MHz and -20dBm. Its RSN
AKM is SAE8 only, capabilities0x00cc (management-frame protection capable and
required), and RSNXE advertises Hash-to-Element. ``supplicant_sae_test.py``
configures a RAM-only SAE/CCMP/MFP-required network and sae_pwe1 through the
ordinary control socket. Its non-echoing password input is separate from the
WPA2 PMK test. Default supplicant redaction is verified for password and
intermediate secret values. No credential enters a file or command argument.

The network is discovered and selected, but the driver's advertised AKM set
contains no SAE. Supplicant reports AP/profile key_mgmt0x400 with available0,
then fails to select authenticated key management. The bounded30-second test
therefore fails before a firmware SAE request; it establishes a missing Linux
capability/implementation, not failed silicon authentication or an incorrect
password. The current callback also explicitly rejects SAE and required PMF.
Do not merely advertise SAE to bypass this guard: qualify firmware SAE, PMF
and H2E behavior in Python first. Existing typed observations identify firmware
auth3/AKM8 and the optional passphrase TLV139, but do not qualify a complete
independent SAE join. Selecting the original WPA2 network after this negative
returns COMPLETED and google.com HTTP301/219 bytes again.

Finally, the F0 supplier is unbound with ordinary supplicant and BlueZ alive,
Wi-Fi joined and the Bluetooth ACL authenticated/encrypted. Both consumer
interfaces and all20 radio IRQs disappear; native SMC mailbox IRQs remain.
Final data completions are TX63/RX685/recycled685, basic52/62096; HCI107
commands/184 ACL TX/216 events/186 ACL RX, one vendor event skipped; shared
service count7723. Last live netdev errors/drops are zero. Before exiting the
EL2 harness, a configuration-only observer verifies PCI command2 on all three
functions and DART error0. Owned userspace stops and HV terminal0 follows.
No disabled BAR is read. Private native267 artifacts save the complete console,
source/input hashes, SAE negative evidence, acceptance and final observer.

Python269..271 and Linux273: independent SAE qualification
----------------------------------------------------------

The additional operator hotspot is SAE-only, rather than a transition-mode
AP accepting a WPA2 fallback. With a fresh full chip reset and the same
independently sourced image, Python269 generates auth3/firmwareAKM8/CCMP4,
zero flags, a zero-filled32-byte key area, the scanned SAE RSN IE and the
length-prefixed password TLV139. No captured native extension record or
precomputed WPA2 PMK is supplied. The firmware returns connection status0
matching both target SSID and BSSID. Its subsequent data setup stops before
allocation: the old Ethernet helper's defaultIOVA0xa000000 is outside the
newer, narrower PF1 window. This is an allocation-check negative, not evidence
of DHCP. All three busmasters are disabled and DART error0 at close.

Python270 chooses an unused host allocation at0x10000000 within that published
window. SAE association again completes0. Fresh generated Ethernet queues
exchange DHCP DISCOVER/OFFER/REQUEST/ACK and receive10.246.233.16. TX/RX
completions and recycling advance, firmware/IPC remain2 and DART error0.
Python271 repeats from a new full reset with a deliberately wrong password:
request acknowledgment succeeds, but the distinct connection outcome is1
with word3 detail0x00730000. No authorized Ethernet/DHCP path is created.
The driver therefore must distinguish command acknowledgment from successful
authentication; a credential rejection is never reported as a joined link.
All test credentials remain anonymous-memory inputs, never files or argv.

The existing public encoder now names this qualified SAE request. The kernel
Rust encoder adds a bounded1..63-byte password field to the private C/Rust
input ABI, uses a zero key slot and creates the password TLV. The host ABI's
PSK/SAE selectors are distinct from firmware and IEEE AKM values. Rust's local
request wipe and the completed DMA-slot wipe cover the longer credential.
The46 radio core tests include independent Python format agreement, password
bounds, rejection of an accidental nonzero SAE PMK and completed-slot erasure.
The C adapter keeps credential copies under its existing state lock and
releases them with kfree_sensitive.

Ordinary supplicant2.11 uses the standard NL80211_ATTR_SAE_PASSWORD path when
SAE_OFFLOAD is advertised. Native273 selects the actual SAE AKM and required
PMF, but its connect request carries NL80211_WPA_VERSION_3 (0x4). The original
common guard still required WPA_VERSION_2 and returned EOPNOTSUPP before a
firmware request. The adapter now validates WPA versions per authentication
profile: PSK requires2, SAE accepts3 or the older RSN2 representation while
still requiring SAE authentication and PMF. An explicit PSK/SAE wiphy AKM list
also replaces inappropriate generic defaults. GET_CAPABILITY key_mgmt from
this supplicant omits SAE with only the offload flag, even though its actual
association path correctly selects the SAE offload AKM; use the actual
request/error evidence rather than that summary alone. Native273 removes all
20 radio IRQs, all3 PCI commands read2, DART error0 and HV exit0.

These Python tests qualify firmware's default SAE PWE negotiation. The AP
advertises H2E support, but there is no RF observation distinguishing the
algorithm used. Do not claim forced H2E or hunting/pecking control. The Linux
callback accepts unspecified/both mechanisms and explicitly rejects a forced
single mechanism until its firmware mapping is known. The generic SAE lab
client now defaults to both and offers an explicit --sae-pwe option for
negative/qualification tests. Required PMF and AES-CMAC are exposed for this
SAE path; PSK with required PMF remains outside the qualified profile.

Compared with existing Linux Wi-Fi drivers, the userspace contract is the
same cfg80211/nl80211 SAE offload, AKM and cipher advertisement. Centauri's
firmware operation is its own connection command20000 with packed selectors
and TLV139, rather than Broadcom's firmware configuration protocol. No
Broadcom-specific transport or captured reference record is reused. Existing
SMC/GPIO/PCI providers and the DT wiring remain unchanged by SAE. This adds
no model/serial gate and makes no new Neo hardware claim.

Boot268 stopped at an insufficient fixed USB-reappearance delay before any
firmware transfer; the retry restores the existing bounded wait. Boot272
instead copied the private bootloader but timed out during post-chainload
NOP reconnection before Linux execution. Both are setup negatives, separate
from the Python authentication/data proofs and native273 adapter rejection.

Native274 exposes correct PSK/SAE AKMs, the SAE password offload flag and
management cipher, but ordinary connect still returns EOPNOTSUPP. A one-shot
hardware breakpoint at our own Linux callback inspects compiler-derived
cfg80211 scalar fields only: SSID length10, auth_typeSAE4, RSN IE vector35,
MFP-required1, WPA versions4, CCMP group/pairwise, one SAE AKM, password
present/length9, no PMK and PWE_BOTH3. Flags are0x48: MLO_SUPPORT bit6 plus
EXTERNAL_AUTH_SUPPORT bit3. The standard supplicant declares both capabilities
on this full-MAC SAE path, even though authentication is offloaded. The
remaining guard rejects bit3. No credential contents are read or printed by
the observer; the breakpoint removes itself after one hit. Source layout and
symbols belong to our Linux build, not an Apple executable. Final removal
again gives20 radio IRQs gone, all3 PCI commands2, DART0 and HV0.

Native275 accepts those two known userspace capability declarations. The
actual authentication still requires the independently qualified SAE offload
request; declaring external-auth capability does not request that the driver
use it. Other unsupported connection flags remain rejected. Only the C flag
mask changes relative274; Rust protocol and firmware assets remain identical.

Native275: ordinary SAE/PMF, fresh DHCP and encrypted BT coexistence
------------------------------------------------------------------

After a full chip reset through existing Linux SMC/GPIO and PCI APIs, the new
Rust connection encoder and C adapter accept the unmodified supplicant's
SAE/password/default-PWE request. Native scans return13/13 and9/9 reports,
then firmware association completes0 at47.742s, cfg80211 completes0 at47.745s
and generated Ethernet transport becomes ready at47.777s. Ordinary supplicant
reports COMPLETED, key_mgmtSAE, CCMP pairwise/group, pmf2 and BIP management
cipher on the SAE-only Pixel hotspot. No WPA2 fallback occurs.

Fresh DHCP obtains10.246.233.16/24 from10.246.233.28, which supplies the default
route and DNS. Curl bound to wlan0 returns google.com HTTP301/219 bytes,
exit0; the body SHA-256 remains
``2178eedd5723a6ac22e94ec59bdcd99229c87f3623753f5e199678242f0e90de``.
Ordinary disconnect/reassociate then completes SAE again at108.503s and another
curl returns301/219. Passwords live in the supplicant's RAM-only network and
short-lived kernel/firmware request; no config/credential file is written.

BlueZ starts with an empty Linux pairing store. The first host connection and
local SDP request overlap before pairing is complete; that attempt reports
pairing status5/auth timeout and SDP ENOSYS, and is not counted as success.
After removing only that failed scratch peer, explicit host pairing in a
serialized retry returns0. Linux observes fresh pairing_complete0 and enabled2
encryption, with read-key-size success. The same kernel then completes128
changing SDP exchanges with security2/key size16:2560 request bytes and16512
response bytes, SHA-256
``63582a64c5de6f2c227bda4d0e4d49b62ff692e7045e8e73aef73b37b8fa238d``.
A simultaneous WPA3 Wi-Fi curl returns301/219, both processes exit0, and HCI
reports184 ACL TX/186 ACL RX with zero RX/TX errors. The initial pairing
negative remains visible in the saved evidence rather than being rewritten
as first-attempt success. The small lab D-Bus configuration also logs an
unknown unused bluetooth group; this is not an entirely warning-free platform.

The native Rust SAE negative also succeeds as a negative: a deliberately
wrong password produces repeated firmware/cfg80211 status1 without an
authorized data link. Selecting the original correct RAM-only network first
fails once as well, then succeeds on its following331.767s attempt. The curl
started before confirmed recovery returns DNS error6; it is retained as a
negative rather than counted as a response. The same kernel then switches to
the original WPA2/CCMP network, completes at398.429s, obtains fresh DHCP159
and google.com301/219. Finally it selects the original Pixel SAE network,
waits explicitly for COMPLETED, obtains fresh DHCP10.246.233.16 and again
returns301/219, exit0. The Bluetooth ACL remains authenticated/encrypted
through these Wi-Fi network switches. A stale credential-ready marker also
causes the host sender to refuse transmission; the interrupted stdin helper
is restarted with a fresh marker before any PMK is sent.

Final live Wi-Fi counters are TX90/RX74,8434/14117 bytes, RX/TX errors0,
TX drops6/RX drops0. These include the deliberately disconnected/wrong-key
period and the early failed curl; do not claim zero lifetime drops. F0 supplier
unbind with SAE/PMF Wi-Fi and encrypted Bluetooth joined removes both consumer
interfaces and all20 radio IRQs. Final Rust counters are dataTX90/RX74/
recycled74, basicTX94/RX16487; HCI122 commands/184 ACL TX/244 events/186 ACL RX,
with2 vendor events skipped; shared service6802. All three PCI commands read2,
DART error0, owned userspace stops and HV terminal0. Only configuration/DART
registers are observed after removal; no disabled BAR is read. The host's
owned scratch pairing is removed after the test.

The final972-line C adapter passes checkpatch with zero errors, warnings or
checks; the full Linux7.3 kernel links. The46 Rust radio tests and five existing
Python connection boundary tests pass. Source hashes and the unchanged image
pin the tested implementation. There are no macOS runtime dependencies or
captured initialization-state bytes in this cold radio path. Per-unit static
factory calibration remains opaque firmware data. Initial host PHY training,
automatic probe power sequencing, runtime DT/image assembly, installer work,
forced-PWE control and broader protocol/board coverage remain the integration
limits described above; successful functional acceptance is not upstream
approval or a production-completeness claim.

H2E-only qualification: Python276..297 and native Linux298
----------------------------------------------------------

The bounded protocol investigation and public authentication-header decoder
are recorded in ``REGISTERS.rst``. Firmware already negotiates H2E with the
Pixel AP. No tested unassigned vendor field is promoted into a PWE selector.
Instead, the H2E-only implementation accepts standard nl80211
HASH_TO_ELEMENT, checks the selected BSS's well-formed RSNXE capability,
appends a generated standard RSNXE244/length1/payload0x20 to the request, and
reports that capability in cfg80211 association IEs. Rust observes only
peer-matched public SAE metadata and rejects an observed hunting/pecking
commit under that policy. A mismatch stops Wi-Fi queue service and DMA;
normal UNSPECIFIED/BOTH behavior remains available. Forced hunting/pecking
still returns EOPNOTSUPP. This does not claim a newly decoded exclusive vendor
mode bit or require a new authentication exchange on a cached reconnect.

Python296 uses the public H2E encoder for three independently selected station
addresses and observes fresh group19 H2E commits and successful associations.
Its final generated DHCP exchange receives an OFFER but no ACK and is not
counted as DHCP acceptance. Python297 repeats with the factory address after
full reset, observes H2E commit/confirm0/association0 and obtains a DHCP ACK
for10.246.233.16. All owners close with busmaster disabled on all three
functions and DART error0. No Apple executable is inspected.

Native298 boots the new Rust/C driver on Linux7.3. As in275, Linux then
performs another full SMC/GPIO chip reset and PCI remove/reset/rescan before
re-uploading both firmware stages into newly generated contexts. Captured
host-state input remains zero bytes; factory calibration is static opaque
firmware data. The new Image.gz SHA256 is
``529280e0676dbc3a93de1e685dae3948699b88940faeda5354f24962e4e0c3f7``.
The initramfs and independently sourced firmware image are unchanged from275.

First, a breakpoint in our own Linux pure RSNXE capability function forces
its return false. Unmodified supplicant2.11 with ``sae_pwe=1`` repeatedly
gets EOPNOTSUPP (five calls) and stays disconnected; no connection request
is submitted. This tests the admission path on real Linux, not a physical
legacy-only AP: the actual Pixel peer continues to advertise H2E. The hook
is removed before the positive test. Synthetic tests separately cover absent,
malformed and non-H2E capability bodies, peer matching, and a legacy commit
arriving in the same poll as a success event: EPROTO occurs before any success
callback. No physical legacy-only AP is available for that additional test.

At316.541s the unmodified supplicant's H2E-only connection produces the Rust
report ``H2E=true group=19 required=true``. It completes SAE/CCMP with PMF2
and BIP, obtains fresh DHCP10.246.233.16 via10.246.233.28, and google.com
returns301/219 bytes with curl exit0. Ordinary disconnect/reconnect again
returns301/219. Setting ``sae_pwe=2`` and reconnecting passes the BOTH-mode
regression. Switching to the original WPA2/CCMP network at2462MHz obtains
DHCP10.0.0.159 and301/219; returning to the same RAM-only Pixel profile with
``sae_pwe=1`` obtains fresh DHCP and301/219 again. Cached reconnects need not
produce a new authentication frame. Passwords/PMKs use non-echoing stdin and
the ordinary RAM-only control socket, never a saved credential/config file.

Keep the setup negatives separate: the first DHCP invocation omitted the
initramfs configuration script, so no DNS was installed; explicit
``-s /etc/udhcpc.script`` succeeds. The first WPA2 attempt incorrectly limits
scanning to2412MHz and times out without association; the known2462MHz
channel succeeds. BusyBox lacks ``ip -s``; that diagnostics command's exit1
does not negate the independently recorded successful final curl exit0.

With H2E-only Wi-Fi joined and Bluetooth HCI initialized, supplier unbind
removes both interfaces and all20 radio IRQs. Final data counters are TX78/
RX76/recycled76, basicTX92/RX5914, HCI45 commands/45 events/no ACL traffic,
shared service291. PCI commands read2/2/2, DART error0 and HV terminal0.
Bluetooth pairing is not repeated in298: the275 coexistence proof remains
the applicable result. No disabled consumer BAR is accessed after teardown.
The full kernel links,48 Rust radio tests pass, and the1011-line C adapter
passes checkpatch with zero errors, warnings or checks. Python auth/scan
boundary tests also pass. Other integration limits above remain unchanged.
