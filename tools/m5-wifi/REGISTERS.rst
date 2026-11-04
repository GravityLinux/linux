.. SPDX-License-Identifier: GPL-2.0-only

Audience: For LLMs; working evidence pending human review.

Register meanings and evidence
=============================

Names in ``centauri_regs.py`` are relative to the stated ADT bank. Runtime
register traffic and controlled host-side operations provide the evidence;
Apple executable inspection is excluded. Preserve these meanings when porting
the standalone Python prototype to Rust.

PIO-DMA configuration descriptors
----------------------------------

The ADT apcie0-piodma mapper selects DART SID17. Its explicitly published
descriptor addresses are IOVAs, not host physical pointers. Initial119
untranslated samples are invalid and excluded. Corrected122/123 observations
read only two16-byte descriptor headers after complete translation/bounds
checks. An independently generated request in a newly zeroed16KiB page at
IOVA0x0e000000 returns vendor/device ID0x1901106b with the selected completion
tag. Generated123 writes change PCI Command406to6and back406, confirmed by
ECAM readback. No captured descriptors or payloads are replayed in these
owned requests. The reference guest remains paused and is rebooted afterward.

The measured input descriptor is two little-endian u64 words. First word:
source low36 address bits at bits59:24, destination low nibble at bits63:60,
and the selected configuration-transfer profile at bits23:0. Second word:
destination address bits35:4 at bits31:0 and scalar byte count at bits63:32.
Profiles0x04c007 and0x20c007 select configuration read and write, respectively;
their individual flag bits remain unnamed. Transfers of1/2/4 bytes are
observed. centauri_piodma.py encodes/decodes these profiles with bounds,
using actual successful generated-read/write vectors as tests.

Engine register0x4c/0x50 publishes the input descriptor address;0x44/0x48
publishes the initially zero response-header buffer. Register0xd8 supplies
the request tag; completion0xdc echoes it. The tested submission control is
0x411. The page holding all generated data stays mapped for the transaction.
No endpoint firmware memory is read. Native124 additionally captures only
the1/2/4-byte scalar of a qualified configuration-write descriptor, after
SID17 translation and ECAM-target validation. That establishes223 writes,
including host time before the first image and D3hot before the link reset.
Cold PIO initialization and independent firmware readiness remain unverified.
This is a host-controller transport; it is not a Centauri firmware IPC ABI.

Established transport registers
-------------------------------

* Global PHY bank 3, offset 0: clock 0/1 requests in bits 0/1 and their
  acknowledgements in bits 2/3. The standalone prototype has observed both
  request/acknowledgement handshakes. Bit 4 is the reset control used by the
  existing Gravity/Asahi PCIe implementation and matching M5 traffic.
* Global PHY offset 8, bit 0: clock-ready status. The reference sets offset
  4 bit 4 before polling this status, then selects common-clock mode 1.
  Offset 4 is named startup control; its individual bit functions need more
  investigation before assigning narrower hardware names.
* Port bank 8, offset 0x800 bit 0: application-clock enable; offset 0x804
  bit 0: port-running status; offset 0x82c bit 0: port reset release.
  These match the existing Gravity/Asahi controller sequence and M5 traffic.
* Port offset 0x208: link status, with link-up bit 0 and initialization-busy
  bit 2. Port offset 0x80 bit 0 starts the LTSSM (link-training state machine).
  Reference captures show its enable followed by link-up observations.
* PCI configuration offset 0x18: primary/secondary/subordinate bus numbers,
  a standard PCI bridge field, not a chip-private command register.
* Root bank 1, offset 0x50 bit 0 requests controller run; offset 0x58 bit 0
  reports ready. Port 0x800 bit 8 disables application-clock gating; clearing
  it permits gating. PHY offset 0 bits 9/10 enable reference clocks. These
  names also appear in Linux v7.3 ``drivers/pci/controller/pcie-apple.c``
  (CORE_RC_CTL/STAT, PORT_APPCLK_CGDIS, PHY_LANE_CFG_REFCLKEN) and match the
  observed T8142 sequence. The adjacent still-unresolved fields stay unnamed.
* Port offset 0x124 is MSI configuration, and offset 0x16c is the low MSI
  destination address in the T602x layout. The RID-to-SID table starts at
  0x3000, with valid bit 31 and SID starting at bit 16 in that layout.
  These standard Gravity/Asahi/Linux definitions support interpreting the
  reference transport; actual T8142 stream assignments still need observation.

Pad and chip reset
------------------

The device ADT names GPIO controller 161 pin 192 as PERST and pin 164 as
CLKREQ. The existing Gravity/Asahi GPIO layout puts the data level in bit 0
and the pin mode in bits 3:1: mode 0 is input and mode 1 is output.

Boot-09 observes PERST ``0x70a02 -> 0x70a00`` before root enumeration: the
pad changes from output-low to input. A snapshot at the root bus-number write
confirms ``0x70a00``. Later it changes to ``0x70a03`` (output-high). CLKREQ
remains ``0x74a11`` in that phase. These are distinct pad configuration and
output-level operations; merely writing the data bit does not reproduce both.

The ADT bridge reset function names SMC key ``gP13`` with command argument
``0x800000``. Boot-09 records a successful write of ``0x800001`` before PERST
goes high. The direct host prototype can independently query this key as
four-byte ``ui32`` data. No unrelated SMC key payloads are collected.
A later read returns zero despite the earlier successful ``0x800001`` write.
Treat this as an observed SMC reset request, not proof of a latched physical
power level. The warm sequence retains the request by avoiding a matching
deassert write; the exact external signal and readback encoding remain open.

PHY calibration bank
--------------------

Boot-09 establishes ADT bank 4 as the PHY-IP tuning bank: its observed offsets
match the PLL and AUSPMA tunable tables. Seven table operations only read
already-matching values; the observed conditional writes otherwise match the
metadata. At the end, offset 0x90 changes from ``0x80010005`` to ``0x80000005``:
bit 16 is cleared after tuning, before the global clock-ready sequence.

The controlled ``direct-phy-bit16-01-hold.log`` experiment omits the software
clear after observing ``0x80010005``. Global
clock ready, root ready, port running, port idle and link-up all pass; the
endpoint still enumerates as 1900 at Gen1 x1. Thus a software release of
this bit is not required for these operations. Its physical state was not
sampled at every stage, so hardware self-clearing is not excluded. The constants
are now neutral ``PHY_IP_REG_90`` / ``PHY_IP_REG_90_BIT16`` until the role is
established. The normal prototype retains the reference clear, without
claiming that it enables the PHY. Effects at other link rates or power
states remain untested. Individual analog tuning fields likewise retain
their metadata roles without invented hardware meanings.

A repeat with physical samples, ``direct-phy-bit16-03-stages.log``, confirms
bit 16 remains set at each checkpoint: clock ready, root ready, port running,
port idle, both port-clock acknowledgements and link-up. Before the eventual
software clear the register reads ``0x00010005``; afterward it reads
``0x00000005``. Thus hardware self-clearing does not explain this successful
startup. Bit 31 changes between samples but its role remains unresolved.
Sampling starts after calibration: an earlier diagnostic read before the
global PHY was ready faulted, so this bank must not be probed at that stage.

Device queues still under investigation
---------------------------------------

Bootstrap and runtime PCI functions are different identities. A live boot-11b
configuration snapshot establishes this mapping after re-enumeration:

=================== ========== ============ =====================
Stage/function      Device ID  BAR0 PCI     BAR0 physical
=================== ========== ============ =====================
Bootstrap 01:00.0   1900       0x82000000   0xb82000000
Runtime control .0  1901       0x82020000   0xb82020000
Runtime alpha .1    1902       0x82010000   0xb82010000
Runtime beta .2     1903       0x82000000   0xb82000000
=================== ========== ============ =====================

The ADT names function/unit 1 alpha and supplies the Wi-Fi MAC there; function
2 is beta. Runtime .0/.1/.2 report class/revision ``0xff000001``,
``0x0d200001``, ``0x0d110001`` and multifunction header bit 7. BAR4 remains
on control .0 at PCI ``0x80000000``. PCI command is ``0x0406`` for all three.
Do not confuse the runtime beta bank with the earlier bootstrap bank that
occupied the same physical address. The current allocator explicitly accepts
only ID 1900; runtime multifunction resource assignment is not implemented.

The boot-11b address-only observer covers BAR4's full 32 MiB and records no
CPU writes there, including before the first submission. This rules out a
CPU upload through BAR4 in that capture; it does not rule out a DMA upload.
No BAR4 data or firmware payload is read to reach that conclusion.

Boot-12b captures standard PCI configuration before the initial 1900
submission: root command 7, endpoint command ``0x0406``, PCIe Device Control
``0x213f`` and MSI message control ``0x00ab``. MSI is enabled with four
messages (MME = 2) and 64-bit message addresses. The direct prototype's
default has MSI control ``0x008a`` (disabled) and Device Control ``0x2110``.
The reference therefore enables MSI before bootstrap; direct MSI setup is
a concrete remaining transport task. No custom command meaning is inferred
from these standard capability fields. Definitions are from Linux v7.3
``include/uapi/linux/pci_regs.h``.

The direct ``--enable-msi`` transport experiment now reads back MSI control
``0xab`` and programs endpoint command ``0x0406`` during DMA enable. It programs the
ADT MSI address, four messages with base data 0, the 32-entry receiver map,
and the interrupt-to-AXI enable used by Gravity/Asahi. CPU AIC interrupts are
not enabled; the prototype polls state. With the empty submission this still
returns startup status 0, submission status 5 and no DART fault. Thus MSI
configuration alone does not establish a valid command or explain status 5.

Direct host access now establishes two distinct startup stages: BAR0 +
``0x8000`` works before any protocol commands, while BAR0 + ``0`` faults.
The startup bank's offset 0 is named status because the reference repeatedly
polls it and observes a change from 0 to 4 after a submission. Offsets
4, 8, 12, 16 are stable identity words, matched between the direct prototype
and reference: ``0x900681``, ``0x20260840``, ``0xba02ee36``, ``0x14c8f1ef``.
The word at offset 8 is identified below; the other encodings remain unresolved.
No chip-version or timestamp claim is made. ``BOOTSTRAP_BANK`` describes this observed
interface; they do not establish that its implementation is ROM or firmware.

IPSW BuildManifest metadata now identifies the word at relative offset 8:
``Wireless1,ChipID = 0x2026`` and ``Wireless1,BoardID = 0x0840`` exactly match
the register's upper/lower 16 bits, ``0x20260840``. It is therefore named
``BOOTSTRAP_CHIP_BOARD_ID``. The host-written offset 0x7c, ``0x248142``,
matches ``ApBoardID = 0x24`` and ``ApChipID = 0x8142`` in the manifest and
the operator device's ADT. It is named ``BOOTSTRAP_HOST_PLATFORM_ID`` and
constructed from those host IDs. Other identity words remain unresolved.
These meanings use container metadata, not firmware code inspection.

The first reference submission writes startup offsets ``0x44/0x48`` with
a low/high-looking address, ``0x4c`` with 32, then writes 1 to ``0x1000``.
Offset ``0x40`` reads 1 and status subsequently reads 4. A later controller
reset follows. Direct opaque-image replay now establishes this as the image
transfer path, rather than a descriptor ring.

Boot-10 provides the missing DMA-address evidence without reading payloads:
RID-to-SID entry 1 is ``0x80010100`` (valid, RID ``01:00.0``, SID 1).
At the first submit, ``0x6948000`` / length 32 maps through SID 1, TCR 1,
to physical ``0x1032d2c8000``. Active SID 17 has no mapping for that IOVA;
the ADT explicitly assigns SID 17 to the PCIe PIO-DMA engine. Thus startup
``0x44/0x48`` are named submission DMA address low/high, ``0x4c`` submission
length, and ``0x1000`` submit. These names describe the observed transfer;
they do not yet identify its buffer layout or call it a descriptor ring.
The default mapping observer bounds page-table reads to aligned DRAM pages.
Explicit record capture and opaque-copy modes are documented separately.

The 32-byte image record has source IOVA at offset 0 and byte length at offset
8, followed by sixteen zero bytes in the observed record. The upper 32 bits
of both populated slots are zero in this build. Their full type widths and
the remaining zero fields are not distinguished by these observations.
The named ``BOOTSTRAP_IMAGE_*`` offsets preserve this wire layout.

Boot-14's record points to 31,252,480 bytes. A separately enabled exporter
copies those bytes unchanged for replay, with complete DART translation and
DRAM bounds checks. Only the byte count and SHA-256 are inspected; no image
format, instructions or referenced code are parsed. The private asset remains
outside the Linux repository. Its origin metadata records the device transfer.

Direct replay through owned SID 1 pages yields submission status 1 at 48.6 ms
and image-ready status 4 at 139 ms. A warm port restart, retaining SMC power
and the shared PHY, then enumerates 1901/1902/1903 at Gen3 x1. This verifies
firmware bootstrap without macOS or a hypervisor running. During shutdown,
port link status can remain ``0x9b001345`` (UP plus L2/BUSY); waiting for UP
to clear before resetting the controller incorrectly stalls the transition.

A full-size negative control, ``direct-bootstrap-full-unmapped-01.log``, keeps
the identical 32-byte record mapped but deliberately leaves its 31,252,480-byte
opaque payload unmapped. It remains at main state 0 and submission status 2,
where the mapped case reaches main state 4. Thus successful bootstrap depends
on the payload's DMA mapping. DART's error register remains zero even for this
failure, so no DART error alone cannot establish a successful DMA transfer.
Re-mapping and re-submitting without a chip reset does not recover this run;
its unchanged failure state does not distinguish a latched error from a
different retry protocol.

Two fresh cold-boot runs execute the complete pipeline without manual fixes.
The second uses independently chosen IOVAs (record ``0x01000000``, image
``0x02000000``), rather than the captured macOS allocation. Both reach image
ready, re-enumerate all three runtime functions, and read their initialized
BARs. Thus the copied image can be relocated within the owned DART mappings;
the captured host IOVAs are not fixed hardware addresses.

Runtime BAR allocation reproduces the table above. Control's bank at +0x8000
then reports state 1; Wi-Fi +8/+0xc and beta +0/+0x18 remain zero before the
second image transfer and global IPC setup. BAR4 contents are never read.

Runtime image and IPC handshakes
-------------------------------

Boot-17 distinguishes the second image transfer from IPC publication. The
runtime bank first receives the same address/32-byte image-record convention
at +0x44/+0x48/+0x4c, but its doorbell is +0x1070 (value 1). The record still
contains a source IOVA and length ``0x1dce000``, with sixteen reserved zero
bytes. Control state changes from 1 to 2, with submission status +0x40 = 1.
Identical record sizes do not prove identical image contents. Replaying the
first-stage asset at this second doorbell instead reaches state 3 with no
DART error (``direct-runtime-image-01.log``); state 3's exact meaning remains
unresolved. Boot-18's separate opaque second-stage copy has SHA-256
``623262918da8338c00b6ec31fb892dab7c1ca3e121d8c2eb903f99938e6b4980``,
different from the first-stage copy's hash. No byte contents are interpreted.

The host next writes 1 to +0x107c and reads 1 from +0x50, then publishes the
IPC context address at +0x54/+0x58 and writes 2 to +0x107c, reading 2 from
+0x50. These are named ``CONTROL_IPC_HANDSHAKE``, ``CONTROL_IPC_STATE`` and
``CONTROL_IPC_ADDRESS_LOW/HIGH``. This is separate from image submission;
the image-record length of 32 is not the IPC context's size.

Boot-28 verifies this handshake from bare Python after a reference-guest
firmware handoff. The bootloader reports all CPUs exited; its own
``hv_started_cpus`` array and ``hv_cpus_in_guest`` mask are all zero. Existing
firmware DMA mappings and bus mastering remain enabled. Python adds a new,
previously unmapped context page at IOVA ``0x0f000000`` without replacing the
old DART root, then publishes it through +0x107c/+0x54/+0x58. The device
acknowledges IPC states 1 and 2, stays in image state 2 on subsequent checks,
and reports no DART error. Both index pairs and rings remain empty. This
establishes context publication, not command processing or Wi-Fi operation.
It still depends on reference firmware preparation. Boot-27's earlier
handoff disabled bus mastering and the endpoint streams and delayed publication;
the image state subsequently became 3. These experiments do not yet distinguish
DMA teardown from a startup deadline as the cause of that regression.

The bounded context capture before +0x107c = 2 contains a header with word
``0x00680300`` and internal DMA pointers. All ring entries and indices are
zero at this point. The host supplies two state words of 2 at +0x68/+0x6c
before publication; these values alone do not establish device readiness.

================= =============================================
Context offset    Observed role or value
================= =============================================
0x08              Pointer to state words at context +0x68
0x10              Device completion producer at +0x78
0x18              Device queue-consumer array at +0x7a
0x20              Host completion consumer at +0x94
0x28              Host queue-producer array at +0x96
0x30              Unresolved packed value 0x000d0001
0x34 / 0x3c       Pointers to ring areas B (+0x1b0) / A (+0xb0)
0x44              Unresolved packed value 0x00100010
0x48              Unresolved packed value 0xffff0000
0x58              Pointer to auxiliary area (+0x2b0), role unresolved
================= =============================================

Areas A/B are 256 bytes apart, consistent with sixteen 16-byte entries and
the packed ``0x00100010`` field. The auxiliary area's geometry and role are
unproven; it is not named a third ring. Directions and entry fields remain
neutral until correlated with host/device ownership. The first
+0x1000 write varies (1 in boot-15/17, 2 in boot-16); it is a later ring-index
doorbell, not the initial image/context submission. Immediately before that
write in boot-17, ring A entry 0 contains words ``0x3401, 0x5888000, 0, 0``
and pair B is ``0,1``; pair A remains ``0,0``. These observations are protocol
metadata only and do not inspect the pointed-to allocation.

Boot-30/31 add a bounded capture of these published host control frames.
Each ring-A descriptor has observed layout ``<u32 length_format, u64 iova,
u32 cookie>``; the pointer at offset 4 is unaligned. Format byte 1 and
``length_format >> 8`` describe the observed 4- and 52-byte frames. Cookies
run from 0 through 7 and frame allocations are 128 bytes apart. Before a
coalesced doorbell of 2, index pair B is 0/2; before the next doorbell of 3,
it is 2/3 and pair A is 2/2. This is consistent with pair-B's second member
being the host producer and first member being the device consumer, pending
a controlled submission from our own driver.

Boot-35 performs that first controlled submission after all reference guest
CPUs have exited. Python posts the captured four-byte word 0x00030003 in its
own buffer, writes ring-A entry 0 and advances only host index +0x96 to 1.
After the +0x1000 write, the device changes both indices at +0x78/+0x7a to
1 and writes ring-B entry 0 as words ``0x401, 0x40000, 0, 0``. The owned
input frame remains 0x00030003. This establishes ring A as host submission,
ring B as device completion, and device ownership of both +0x78/+0x7a
indices. The response word's exact meaning remains unresolved. A subsequent
52-byte setup command remains pending without a DART error or immediate
image-state change, so repeated command processing is not yet established.

Boot-37 establishes repeated transfers with an owned context at IOVA 0x10000,
separate 128-byte frame storage per slot, and queue storage mapped before
the first command. Twenty transfers complete, including reuse of slots 0/1
after the sixteen-entry ring wraps from index 15 to 0. The final two use the
maintained ``ControlIPC.submit`` method. Four-byte and 52-byte frames receive
completion words ``0x401, (byte_count << 16) | cookie, 0, 0``; cookies echo
the submitted slot tag. Thus boot-35's 0x40000 response reflects four bytes
and cookie zero, not evidence of error code 4. These are transport completions,
not proof of the higher-level queue command's result. Main state stays 2,
IPC state stays 2 and DART reports no error. Wi-Fi's +8/+0xc remain zero.
The exact cause of boot-35's pending second transfer is still unresolved;
its higher IOVAs, shared frame storage and intervening page-table changes
have not yet been isolated individually.

The runtime +0x1000 register is now named ``CONTROL_SUBMISSION_DOORBELL``;
the owned test writes the new producer index, including zero at wraparound.
This shares an offset with the cold bootstrap submit register but has a
different role after firmware initialization. Completion validation checks
the returned byte count and cookie before allowing frame storage to be reused.

Boot-39 distinguishes the completion cursor from thirteen per-queue cursors.
Each side occupies fourteen u16 words: device completion at +0x78 followed
by queue consumers at +0x7a, and host completion at +0x94 followed by queue
producers at +0x96. Queue q's cursors are therefore +0x7a + 2*q and
+0x96 + 2*q. Observed queues 2, 3, 4 and 8 use doorbells +0x1000 + 4*q.
The earlier pair-A/pair-B views describe the completion and queue-0 cursors.

Queues 2 and 8 use 16-byte external-buffer descriptors of the same packed
``<u32 length_format, u64 iova, u32 cookie>`` form. The reference offers
fifteen 32-KiB receive buffers on queue 2 and three 4-KiB buffers on queue 8.
Their initial length/format words are 0x800001 and 0x100001. After device
consumption, byte counts change and cookie bit 26 becomes set. No external
receive payload was read for this observation.

Queues 3 and 4 use inline records spaced 0x410 bytes apart: a 16-byte header
and 1024 bytes of inline storage. Header pointer words are 0xffffffff. Queue
3's outgoing records use format byte 2; queue 4's receive credits use format
byte 0 and initially advertise 1024 bytes. Live header reads at offsets 0,
0x410 and 0x820 verify this spacing; the earlier candidate 64-byte stride
lands within inline message bodies. Sixteen inline records require more
than a 16-KiB page and must receive separate 32-KiB allocations.

One observed host greeting is an 18-byte runtime packet: little-endian
u32 length/type 0x12, u32 sequence 0, and ``centaurid`` followed by NUL.
Inline runtime packets encode length in the first word's low 24 bits and
type in its high byte, followed by the sequence word. Reference requests
include type 9/sequence 1 with an eight-byte header only, type 8/sequence 2
with a zero u16 argument, and type 10/sequence 3 with a header only.
Responses have matching types/sequences and observed lengths 10, 532 and
64. Type 0 returns a textual RTKit greeting. These are runtime protocol
messages; their observation reads no firmware file or executable memory.

Bare target-40 reproduces this traffic using an owned 2-MiB DMA arena, fixed
before the first queue command. External queues 2/8 and inline queues 3/4
receive the captured setup records. After receive credits and the centaurid
greeting, queue 4 returns the RTKit/CentauriControlApp version packet. The
three runtime queries return the same types, sequences and lengths as the
reference. The maintained data-queue methods also complete sixteen further
type-9 requests, crossing the sixteen-entry inline transmit/receive wrap.
External receive-credit recycling crosses queue 8's four-entry wrap; those
payloads are never read. DART remains clear and main/IPC states remain 2.

Queue 2 initially supplies two 16-byte framing records with u64 pairs
``1,31`` and ``2,5324``, each followed by a report of the indicated byte
count. The larger report remains unread. Type 9 returns u16 0x55. Type 8's
response contains repeated apparent u64 counters and u32 event numbers,
rather than a textual service list. Type 10 contains several u64 values,
the first matching an apparent counter in type 8. Boot-timing or trace
queries are plausible interpretations; their meanings are not established
and the implementation retains numeric query types.

Wi-Fi's +8/+0xc remain zero after this control-app exchange. Reference
boot-17 reads +8 = 2 before its first Wi-Fi-function doorbell at +0x1028.
All three primed PCI functions are already in D0 with bus mastering and
their MSI capabilities enabled, so those initial standard settings do not
explain the remaining startup difference.

Four initial 52-byte frames publish distinct host allocations. Words 1 and
7 contain repeated low halves 8, 2, 4 and 3; word 7's high half is 0x40.
Words 2/3 publish the allocation pointer, words 4/5 are 0xffffffff, and word
6 is 0xffff0004 for the first frame and 0xffff0010 for the others. Word 0
is 1 for the first two and 0x40100001 for the next two. Remaining words
8 through 12 are zero. Each referenced allocation has a zeroed 64-byte
prefix before publication. The reference later sends four-byte words
0x00040003 and 0x00030003, then repeats the two 52-byte frames whose low
halves are 4 and 3. Queue creation/removal and interrupt routing are plausible
roles, but these observations alone do not establish their complete ABI.

The runtime MSI allocation is control: four messages at data 0; beta: two at
data 4; Wi-Fi: sixteen at data 16. All use ADT destination ``0xfffff000``.
Thus in boot-15, with AIC base 1741, IRQ 1759 belongs to Wi-Fi vector 2.
Boot-17/18 allocate control at data 4 and beta at data 0 instead, confirming
these vector ranges are host allocations, not fixed firmware opcodes.

Boot-19's PCI configuration snapshot exposes another lead outside the BARs:
control function's vendor registers +0xf88/+0xf8c contain its first image
IOVA (0x31bc000), +0xf90 contains 0xae3c00, and +0xf94/+0xf98 contain
0x4acfd000/0x20. The direct failed-image run has zeros there; its +0xf80 reads
3, while the reference before runtime image submission reads 0. These are
unresolved vendor registers, not yet programmable driver ABI. Their writers
and exact relationships to transfer metadata need tracing. The separate
boot-19 first/second image copies have the same SHA-256
``8d3e24af5474d479454ecc55b2923e1e73f9971178cb369c77865010dfc00e96``;
hash differences between separate boots alone therefore do not establish
that the two stages require distinct firmware images.

Boot-20 narrows the vendor-register changes to the restart interval: the
words are zero both before initial submission and at its first main state 4,
then +0xf88/+0xf90 are populated before runtime +0x1070. The direct run also
has zeros at initial state 4, but retains zeros at +0xf88/+0xf90 after restart.
Keeping SID 1's original mapping and TTBR active across the port restart
still yields runtime state 3 (``direct-retained-domain-01.log``). A separate
trial enabling beta bus mastering with a deny-only DART domain also fails;
neither omission alone explains the missing setup.

The paired runtime configuration words +0xff8/+0xffc decode as Unix time in
microseconds. Boot-19's value is ``1793331873451122`` (2026-10-30 03:44:33.451122
UTC); boot-20's is ``1793333657182409`` (04:14:17.182409 UTC), near the respective
guest startups. They are named ``CENTAURI_PCI_UNIX_TIME_US_LOW/HIGH``. This
identifies representation and units, without asserting the writer or whether
the update happens at boot, clock synchronization or another transition.

Boot-21's scoped watch observes the low and high timestamp words changing on
separate PIO observations, followed by +0xf88 changing to 0x5570000 and
+0xf90 to 0xae3c00. SID 1 maps that candidate buffer to DRAM; a separate
explicit exporter copies its 11,418,624 bytes opaquely. SHA-256 is
``2100842c94260919c7ae781e2ac828b65c00d96fa706fd9a4daeac22f8b7022e``.
The published DMA address/length fields are named
``CENTAURI_PCI_BOOT_BUFFER_LOW/HIGH/LENGTH``. The buffer's format and exact
role remain unresolved; it is not decoded as executable code or headers.

The direct companion replay maps this file at independently chosen IOVA
0x06000000 and reads back the programmed address/length and Unix timestamp.
Runtime main state still changes from 1 to 3, with no DART error
(``direct-runtime-companion-01.log``). Thus these missing fields are real
setup differences, but replaying them alone has not enabled runtime boot.

Synthetic zero-buffer hashing identifies the companion's initial contents:
SHA-256 of 11,418,624 zero bytes is exactly its captured hash above. It is a
zero-initialized host allocation, with its input/output role still unresolved.
The larger 31,252,480-byte buffer has a different hash. Submitting a same-size
zero-filled buffer through the first interface leaves main state 0 and
submission status 2 (``direct-bootstrap-full-zero-01.log``), so that interface
depends on the larger buffer's contents as well as its mapping.

Reference boot-23's bounded register snapshots show offsets +0x70/+0x74/+0x78
all zero before runtime submission, at main state 2 and during IPC setup.
The failing standalone run instead reports +0x70 = 0x4ad64e10, +0x74 = 0 and
+0x78 = 0x2000. These are failure-associated diagnostic words. The values
suggest an address/size pair but do not yet establish their format or mapping;
no returned address has been followed to read device firmware memory.

The standard PTM capability/control fields also differ: reference root
capability +0x2b0 has control +0x2b8 = 1, and control function capability
+0x28c has control +0x294 = 0x1001. Linux v7.3
``include/uapi/linux/pci_regs.h`` identifies enable bit 0 and effective clock
granularity in bits 15:8; ``drivers/pci/pcie/ptm.c`` describes both link partners'
participation. Matching these readbacks in a fresh direct run still reaches
state 3. A subsequent experiment also matches the reference's standard LTR,
ASPM, L1-substate timing/enables, PME and PCIe Device Control settings, retains
Gen3 x1, and still reaches state 3. These experiments do not identify the
remaining missing functionality and are not added as mandatory driver steps.

The standalone empty-submission experiment uses a newly allocated zeroed
page, its own SID 1 translation and an empty 32-byte record. Offset 0x40
returns 5, status remains 0, DART reports no fault, and the owned buffer is
unchanged. Thus 0x40 is named submission status, without naming its bits.
The difference from reference value 1 is evidence of a different outcome;
it does not yet prove a DMA transfer or establish an error-code encoding.
Endpoint bus mastering is disabled again when the experiment exits.
Repeating from a fresh boot with that IOVA deliberately unmapped produces
the same status values and no DART fault. Therefore the empty command has
not demonstrated that the chip attempts to read this buffer. Do not label
value 5 as a DMA completion or a specific rejection reason on this evidence.

The first chip control windows expose candidate DMA-address pairs at 0x44/0x48,
a value of 32 at 0x4c, and status at 0x40. The bank at 0x1000 contains changing
values consistent with producer/consumer indices and repeated kick writes.
Their exact queue ownership and descriptor layout are not yet established.
No ring-register names should imply a proven ABI until correlated with actual
DMA metadata and completions.

Wi-Fi function startup frontier
------------------------------

Reference-41 confirms the first inline client greeting is
``CentauriTransport`` plus its terminating NUL, for a 26-byte message including
the eight-byte runtime header. A later userspace client greets as ``centaurid``.
Bare target-43 sends the initial kernel greeting through the maintained queues
and receives ControlApp's greeting, but Wi-Fi PF1 BAR0 +8 stays zero. Thus
ControlApp transport and this client name alone do not initialize Wi-Fi.
Reference startup polls PF1 +8 until it reports 2; the names
``WIFI_STARTUP_STATUS`` and ``WIFI_STARTUP_READY`` describe this observed gate.
They do not assert a bit layout or a subsequent operational network state.

Reference-41/42 publish a separate 2-MiB allocation through PF1 configuration
+0xf88/+0xf8c/+0xf90. These are the same per-function address/length offsets
already named ``CENTAURI_PCI_BOOT_BUFFER_LOW/HIGH/LENGTH`` on PF0. PF1's buffer
is distinct from PF0's 0xae3c00-byte allocation. Opaque whole-buffer copies
have nonzero hashes, but neither their contents nor their format are inspected.
The earliest copy was made before the observer saw PF1 +8 report 2; that does
not establish whether the device had already modified the allocation.

Bare target-43 reserves a second owned 2-MiB allocation, maps it before any
control command and publishes its initially zero contents through PF1's
address/length fields. Neither that publication nor the kernel greeting
advances PF1 +8 to 2. Matching the reference's observed +0xf80 = 0 and
+0xd0c = 0x3fffffdd readbacks, acknowledging +0xd1c, and finally writing 1
to BAR0 +0x1028 also leave it zero. Control main/IPC states remain 2 and DART
reports no error. These experimental writes are not mandatory driver steps;
the missing functionality and the buffer's input/output role remain unresolved.

Writes to PF1 configuration +0xf80/+0xf84/+0xf88/+0xf8c/+0xf90 set bits
0/1/2/3/4 respectively in +0xd1c; writing its current value clears those bits.
This establishes change-latch and write-one-to-clear behavior for those bits.
It does not yet establish +0xd0c's mask direction, +0xf80/+0xf84's protocol,
or the meaning of other +0xd1c bits. No broad interrupt-register names are
assigned on this evidence.

There are no observed SMC requests between ControlApp main state 2 and the
first PF1-ready read in reference-41. Relevant ADT children also expose no
additional per-function power operation. At the bare control-transport
checkpoint all three PCI functions already have D0 and MSI enabled. These
observations narrow the next boot-protocol investigation without claiming
that any untested power or PCI operation is unnecessary.

Bare target-48 repeats the publication and greeting test with the whole 2-MiB
opaque copy from reference-42, mechanically copied without parsing. It too
leaves PF1 +8 at zero through the same experimental stages and a later check;
ControlApp remains at main/IPC 2 with no DART error. Unlike target-43,
+0xf80 initially reads zero in this run; it changes to 5 around the greeting.
Matching the final +0xf80/+0xd0c readbacks therefore does not establish the
missing transition. The two runs support investigating the actual publication
protocol; they do not prove that the captured buffer has suitable input contents.

Sibling control geometry and PIO publication
-------------------------------------------

Bare target-52 reserves both the 2-MiB Wi-Fi allocation and a separate
1.5-MiB sibling allocation, with fixed mappings made before command processing.
It reproduces reference-51's external-queue setup, both buffer publications,
inline-queue setup and kernel greeting. Wi-Fi startup remains zero, including
a later republication; ControlApp stays responsive and DART remains clear.
Thus the sibling buffer and this ordering alone do not resolve startup.

Reference-53 samples Wi-Fi BAR0 +8 at ControlApp's first main-state-2 read:
it is actually zero. It remains zero through the six PIO configuration
writes described below. ControlApp's 104-byte header retains its initial
values throughout observed global submissions; no later header change has
been found that the maintained context builder omits.

The sibling function has already published its own host control context by
that checkpoint. A bare target-52 capture and reference-53's initial capture
agree on format word 0x00680300, state at +0x68, device indices at +0x78,
and queue count 22 in packed word +0x30 = 0x00160001. Its two index blocks
contain 23 u16 values each, giving host indices at +0xa6, submission entries
at +0xd4 and completion entries at +0x1d4. Header +0x50 is 0x04000000 and
+0x58 points to context +0x3d8. Those extra fields' meanings remain unresolved;
PF0's thirteen-queue offsets must not be reused for this interface. The
reference continues thirteen sibling queue-setup commands before Wi-Fi's
first observed ready read. This is an additional active interface to study,
not yet evidence that all of it is required for Wi-Fi.

The root PIO engine's +0x44 and +0x4c low words translate through DART
stream 17 to small host transport allocations; corresponding +0x48/+0x50
words are both 0x100. The high words' meanings remain unresolved. The first
six commands after ControlApp readiness have sixteen-byte prefixes at the
+0x4c allocation. Words are 0x9020c007, a varying source/destination word,
a varying destination word, and byte count 4. The +0x44 allocation's first
sixteen bytes remain zero in these transactions.

For this exact command shape, source IOVA is ``(word0 >> 24) |
((word1 & 0xffffff) << 8)``. Destination token is ``(word2 << 4) |
(word1 >> 28)``. Tokens 0xcb0101f88/0xcb0101f8c/0xcb0101f90 and the analogous
function-2 tokens correlate with the respective PCI configuration fields.
Reference-53 translates each source through stream 17, reads exactly its
four-byte configuration value, and verifies the destination readback after
completion. The writes publish function 1's address/zero-high-word/0x200000
length, then function 2's address/zero-high-word/0x180000 length.
``centauri_pio.decode_boot_buffer_config_write`` recognizes only these six
destinations and this observed command shape. Unknown bits, lengths and
destinations cannot authorize source reads.

Before the first function-1 address write, the synchronized observer copies
the complete mapped 2-MiB allocation opaquely, with the published address and
Wi-Fi status still zero. Its hash differs from an all-zero buffer. This
establishes nonzero contents before this publication, unlike earlier copies
after the first address observation. The allocation's format and role remain
uninspected. It remains a private replay asset outside source control.

Bare target-54 replays that pre-publication allocation unchanged into owned
storage and follows the target-52 ordering; Wi-Fi remains at startup zero.
This does not identify the allocation's contents or exclude references tied
to its original address. A separate owned stream-17 scratch page then drives
the PIO engine directly with our encoded configuration-write commands.
Eight transactions complete with matching tags and interrupt word 0x41,
including a sibling length change from 0x180000 to 0x180004 and restoration.
Readbacks confirm the writes, ControlApp remains at main/IPC 2, and DART
reports no error. Replaying the six boot-buffer writes through PIO still
leaves Wi-Fi unready. This establishes the bounded PIO submission mechanism
without claiming that PIO mediation resolves startup.

The exercised register sequence writes tag +0xd8, then +0x50 = 0x100,
+0x4c = command IOVA, +0x48 = 0x100, +0x44 = separate zeroed host-area IOVA,
and finally +0x54 = 0x411. Polling +0xdc returns the submitted tag. Writing
the returned +4 interrupt word acknowledges it. The command and separate
host areas each reserve 256 bytes; only the sixteen-byte command and four-byte
configuration source are populated. The separate area's first sixteen bytes
remain zero, so its detailed result format is not inferred.

The sibling window's base/length interpretation follows from three captures:
0x02f38000 + 0x3cfc8000, 0x05020000 + 0x3aee0000 and 0x023cc000 + 0x3db34000
all equal 0x3ff00000. BAR0 +4/+8 is the context address; +0xc/+0x10 is the
window base and +0x14 its length. Wi-Fi's reference-17 +0x18/+0x1c/+0x20
fields describe the same window, containing its distinct context address.

Bare target-55 moves handoff to the first sibling firmware-state +0x18 = 2,
before its IPC state +0 = 0 is changed. At that point SID3 has no valid DART
root; the prototype creates that domain using owned pages, retaining valid
domains 1/2/17. It publishes a new sibling context with window
0x07000000..0x3ff00000 and an owned context inside it. Handshake +0x107c
values 1/2 produce IPC states 1/2. The first owned queue-12 setup receives
completion 0x401/0x340000, with firmware state 2 and no DART error.
Posting 127 receive credits has not yet demonstrated packet consumption;
no received payload is read. ControlApp continues to exchange its greeting,
but Wi-Fi startup remains zero. Resetting an already published sibling
interface on target-54 failed and subsequently reported firmware state 3;
target-55 instead owns the first publication from state zero.

The maintained make_beta_context builder reproduces reference-53's complete
212-byte header/state/index prefix. It does not initialize packet queues,
interpret auxiliary data or implement the full sibling driver. These fields
and the successful first publication are separate from Wi-Fi operation.

Sibling packet exchange and extended completions
-----------------------------------------------

Reference-56 correlates the sibling's queues 1 and 7 with HCI commands and
events. Both use mode word 0x00410001, sixteen-byte inline descriptor headers,
260-byte payload areas and stride 0x114. Queue 1 has five entries; queue 7
has seventeen. Header reads at offsets 0, 0x114, 0x228 and 0x33c confirm
the spacing on both queues. The reference's initial vendor command contains
a device address; that address and command are not reused by the prototype.
Wi-Fi's status is already 2 before the first observed HCI command, so that
transmit handshake is not established as a Wi-Fi startup requirement.

Target-57 owns the sibling context and those two queues after an early
firmware handoff. A standard HCI Read Local Version command, opcode 0x1001
from Linux include/net/bluetooth/hci.h, travels through its own queue 1.
Queue 7 returns a fourteen-byte Command Complete event with matching opcode
and status zero; both device consumers advance to 1. Sibling firmware and
IPC remain 2, ControlApp remains 2, and DART reports no error. This is a
working owned HCI request/reply, not a Wi-Fi association or Linux interface.

The second sibling queue-setup command also establishes a different global
completion stride. Its device cursors advance to 2, but completion slot 1
at +0x1d4+16 is zero; the actual matching 0x401/0x340001 record is at
+0x1d4+32. The next setup completes at +0x1d4+64 with cookie 2. Therefore
BETA_CONTEXT_COMPLETION_ENTRY_SIZE is 32, while submission entries remain
16 bytes. Sixteen completion records end at +0x3d4, followed by the observed
eight-byte-aligned auxiliary pointer +0x3d8. The extra completion words'
semantics remain unresolved and are not interpreted as packet data.

Wi-Fi control context and first bare commands
--------------------------------------------

Target-58 stops every reference guest CPU before the first Wi-Fi BAR0
+0x1004 = 1 write, after firmware +8 and IPC +0xc both report 2. The pending
command has not reached the device. Existing contexts and firmware DMA
allocations stay alive; no interface is reset.

Its host context retains format word 0x00680300. Packed word +0x30 is
0x00c000ea, describing 234 completion cursors and 192 submission cursors.
Device completion cursors start at +0x78; device submission cursors at
+0x24c. The host arrays start at +0x3cc and +0x5a0 respectively. Each side
contains 234 u16 values followed by 192 u16 values; the two sides end at
+0x720. Admin submission entries start there, and completion entries at
+0x820. Both admin rings have sixteen sixteen-byte entries. The +0x58
auxiliary pointer targets +0x920; word +0x60 is 0x00500000, meaning unresolved.

Before the pending kick only host submission cursor zero is 1; all other
index values are zero. Submission entry zero describes a forty-four-byte
frame with words 2, 0x00060006, a queue IOVA, zero, 0xffff0400, 0x0000ffff,
zero, 2, zero, zero, zero. Writing the pending notification from bare Python
advances device completion/submission cursor zero to 1 and returns completion
words 0x401, 0x002c0000, zero, zero.

A separately owned two-MiB allocation is then added at unused IOVA
0x0f000000 on stream 2, preserving old mappings and the advertised DMA window.
A second forty-four-byte record uses its own frame and queue storage and
receives matching byte-count/cookie completion. The maintained WiFiControl
class attaches that owned arena and sends the next record successfully:
slot 2 returns 0x401/0x002c0002 with both device cursors at 3, firmware/IPC 2
and no DART error. The operation's higher-level result is not inferred from
this transport completion.

The Wi-Fi notification always writes 1 to +0x1004, independently of the
host producer value; this differs from ControlApp's index-valued doorbell.
The different cursor counts also explain why the smaller profiles' adjacent
cursor pairs cannot be used here. Cold firmware startup and Wi-Fi data-plane
commands remain unresolved despite this working primed control channel.

Owned Wi-Fi context and basic queues
-----------------------------------

Reference-61 captures the initial auxiliary area's first 64 bytes as zero,
before publication. Its basic queue records establish two wire layouts:

* Completion configuration: 44 bytes, eleven u32 words
  ``[2, q<<16|q, ring_lo, ring_hi, 0xffff0000|entries, 0xffff,
  0, 2, 0, 0, 0]``.
* Submission configuration: 52 bytes, thirteen u32 words
  ``[1, q<<16|q, ring_lo, ring_hi, 0xffffffff, 0xffffffff,
  q<<16|entries, 0x80000, 0, 2, 0, 0, 0]``.

The submission record's upper word at +0x18 identifies the paired completion
queue in these six examples. Unknown flag fields retain their observed values.
Setup order is CQ6, CQ5, SQ6, SQ5, CQ1, SQ1, CQ2, SQ2, CQ3, SQ3, CQ4, SQ4.

.. list-table:: Basic queue profiles
   :header-rows: 1

   * - Queue
     - SQ entries
     - CQ entries
     - Capacity
     - Initial RX
   * - 1
     - 128
     - 128
     - 8192
     - TX only
   * - 2
     - 154
     - 154
     - 4096
     - 153
   * - 3
     - 96
     - 96
     - 4096
     - 95
   * - 4
     - 512
     - 512
     - 2560
     - 511
   * - 5
     - 16
     - 16
     - 2048
     - 8
   * - 6
     - 128
     - 1024
     - 8192
     - 32

Queue 1's capacity is a prototype allocation, not an observed receive size.
All basic descriptors and completions occupy sixteen bytes. Submission
descriptors pack ``<u32 length_or_capacity<<8|1, u64 buffer_iova, u32 cookie>``.
Completion word 0 has format 0x0401 in its low half and source submission
queue in its high half. Word 1 has actual byte count in its high half and
cookie in its low half. The last two words remain unassigned. Queue 6's
observed completion ``[0x60401, 0x1fdc0000, 0, 0]`` thus reports 8156 bytes
from source 6, cookie 0. Receive bodies on queues 3/4/5/6 remain unread.

Management queue 1's first request is 24 bytes, and queue 2 returns a matching
594-byte reply. Their runtime header is ``<4IHHI>``: four prefix words whose
meaning is unresolved, total length at +0x10, transaction identifier at +0x12 and numeric
command at +0x14. The request has zero prefix words, length 24, transaction 1 and
command 8. The reply has length 594, transaction 1 and command 8. Its 570-byte body
has not been assigned a protocol meaning. This exchange precedes the extended
54/62-byte queue configurations, which are not needed for this initial query.

Target-64 exits all guest CPUs when firmware +8 first reports 2 with IPC +0xc
still zero. Python retains existing firmware mappings, adds owned context/ring
and buffer allocations, and publishes its own context with handshake +0x1028
values 1/2. IPC reaches 2, all twelve basic configurations complete, and
owned TX/RX buffers exchange command 8. A second query through the maintained
queue class also succeeds after receive credits are recycled. Firmware and
IPC remain 2, with no DART error. This proves owned basic IPC after reference
firmware priming; cold firmware startup and network operation remain open.
Target-65 repeats the full initialization through the maintained queue class:
all twelve admin cursors advance to 12, its first query returns length 594,
transaction 1 and command 8, and firmware/IPC remain 2 with no DART fault.

Reference-66 captures the first 64 management requests. Their +0x12 values
increase from 1 through 64, independently of command number, and replies echo
them. The first two requests both use command 8 with identifiers 1 and 2;
the fiftieth request repeats command 8 with identifier 50. This corrects the
earlier tentative interpretation as a message type. The maintained query
method now matches both command and transaction. Identifier rollover has not
yet been observed.

A bounded continuation reads only the runtime bodies of queries 8, 10, 30 and
47, never configuration commands or firmware buffers. Command 10's observed
three-byte reply is 0x55, 0x53, 0x01 (US and an additional byte). Command 30's
sixteen-byte reply starts with those same bytes; further fields change during
operation. Command 47's one-byte request is 1, and its 709-byte reply starts
with 167 followed by 167 little-endian u32 values. Values include 1 through
11, 0x100c, 0x100d, 0x124, 0x128, 0x12c and 0x130, consistent with Wi-Fi
channel numbers plus flags. Forty trailing bytes are zero in this capture.
The country and channel interpretations are inferred from these runtime
values; flag meanings and the final area's purpose remain unresolved.

Target-67 repeats these read-only queries with its own context, queues and
buffers. Transactions 1/2 both query command 8, then identifiers 3/4/5 query
10/30/47. All replies match both command and transaction; firmware and IPC
remain 2 with no DART error. Before configuration, country query 10 returns
XZ and extra byte 0; channel query 47 reports 56 active words, followed by
484 zero bytes. Both channel captures have the same 709-byte body size.

Command 8's 570-byte body parses exactly as eighteen packed fields with
little-endian u16 tag and u16 length, without alignment padding. Tags/lengths
are fe09/4, fe08/4, fe0b/4, ff05/20, fe00/44, fe01/20, ff04/56, ff12/40,
0126/12, fa06/14, fa00/19, 0106/119, 0107/21 twice, 00bf/14, 00ff/28,
0157/9 and 0158/49. Values and tag meanings remain unassigned. The maintained
TLV, country and channel decoders all pass against these live owned replies.

Reference-69 observes country query transaction 38 returning XZ/0. A
three-byte command-9 request at transaction 109 immediately precedes country
query 110 returning US/1. Target-70 tests that interpretation directly:
command 9 with body 0x55, 0x53, 0x01 returns a matching empty reply, subsequent
command 10 returns US/1, and command 47's count changes from 56 to 167.
Firmware and IPC stay 2 with no DART error. COUNTRY_SET therefore names
command 9; COUNTRY_QUERY names command 10. The third byte's semantics remain
unassigned. The test reproduces the reference's selected country.

Startup configuration also uses the packed TLV format. Reference-69's first
command 51 contains five tag-0159/length-8 fields. Each of the first two
command-52 requests contains tag 0154/length 10. Early command-12 requests
carry tags 014d/5, 0150/2 and 0151/2. Later command-51 requests contain
015e/17, or 015a/25 followed by 015e/17. These are bounded candidate field
header observations; their values were skipped and meanings remain unresolved.

Extended Ethernet queues
------------------------

Target-73 reads only the currently published ring metadata after verifying
the active context matches its captured publication. Extended transmit queues
25 through 29 have 32-byte entries. Two consecutive entries at +0 and +0x20
contain format 3, packet lengths 0x5d/0x62, external pointers, cookies 0/1 and
trailing words 0x20000, 0, 0, 0. Receive source queues 12 and 13 instead have
sixteen-byte entries: format 3, capacity 2048, external pointer and cookie.
Consecutive cookies 0 through 3 appear at +0, +0x10, +0x20 and +0x30.

Completion configuration word 1 separates the logical queue number in its
low half from the context cursor index in its high half. For example,
0x00cf000f registers logical completion queue 15 at cursor index 207.
Device and host completion index 207 advance, while index 15 stays zero.
The queue's configuration word 4 binds source queue 12 in its high half and
depth 4096 in its low half. Pending entries at consumer 417 are zero using
a sixteen-byte stride but valid at a 32-byte stride; entry 633 similarly
matches the latest producer. This directly establishes the completion size.

Receive completion words start with 0x000c0403 for source 12. Word 1 packs
received bytes in its upper half and cookie in its lower half. The remaining
six words retain unassigned metadata. The completed source descriptor's
cookie locates its known posted buffer, whose first two bytes are alignment
padding. Four samples verify Ethernet type 0x86dd, IPv6 version 6 and exact
lengths: 200 = 2 + 14 + 40 + 144, and 88 = 2 + 14 + 40 + 32. IPv6 next-header
58 and hop-limit 255 are observed. Packet headers are runtime network data;
no executable or firmware allocation is inspected.

A synchronized transmit observation captures source queue 28, slots 10
through 15, before BAR0 +0x1030 = 1. The first descriptor is
0x4e03, packet IOVA, zero, cookie 10, 0x20000, zero, zero, zero. Its packet
starts directly with an Ethernet header and contains exactly 78 frame bytes.
The trailing metadata word 0x20000 therefore must not be treated as receive
alignment padding. Its meaning remains unresolved.

The matching transmit completion is 32 bytes and starts with 0x001c0502,
identifying source 28 and format 0x0502. Word 1 is 15, the last submitted
cookie in that group; word 2 is 0x600. The group may be batched, but a count
interpretation is not required by the prototype and remains unassigned.
Owned single-packet submissions later return their exact cookie with word
2 equal to 0x100. Transmit word 1 is not interpreted as a receive byte count.

During the same reference boot the driver publishes a new context and assigns
new network queue numbers. The active default transmit queue becomes 32,
paired with logical completion queue 80 at cursor 196. The old queue 25 and
cursor 192 must not be reused. The maintained registry loader discards all
records from earlier publications and checks the context against WiFiControl.

After every guest CPU exits and both the bootloader's guest-CPU array and
guest mask are zero, the Python driver adopts that latest context. It adds a
new owned transmit arena to SID2 without replacing existing mappings. Its
Ethernet DHCP, ARP, ICMP, HTTP and HTTPS exchanges succeed through those
queues. Association and receive buffers remain reference-prepared. A separate
isolated Linux-server TAP stack verifies TCP/TLS; it is not Linux on the M5.
Target-77 replenishes receive credits by reposting each completed buffer
into the next source slot, preserving its original pointer, capacity and
cookie, then publishing the host submission cursor and writing 1 to
BAR0 +0x1050. Cookie 1 completes after reposting in slot 2047, and cookie 2
after slot 0: a cookie is not the physical source slot. The host leaves one
submission entry unused to distinguish full and empty. Retired slots can
contain duplicate records for the same cookie; adoption requires them to
identify the same allocation. Both source and completion ring wraps survive
4,500 gateway ping exchanges with zero loss and subsequent HTTPS transfers.
Target-82 subsequently runs the same queue protocol locally in Linux on the
M5 through a Normal-NC physical-memory backend and TAP interface. HTTP and
HTTPS succeed, and pausing that local process makes the same-IP curl fail.
Firmware/association remain retained; independent startup and the Rust kernel
port remain outstanding.

Firmware-defined runtime allocation
----------------------------------

The host initializer for PF0's PCI+0xf88/+0xf8c/+0xf90 allocation is the
vendor FTAB's complete `2ftb` directory entry. Runtime store-only observation155
qualifies its first592 bytes; all other input bytes are zero. Native152/155
pre-submission hashes agree, and generated input matches them byte-for-byte.
Cold156 reaches runtime-ready2 with this generated allocation. Earlier21/22
opaque companion captures were taken before initialization and equal zeros;
their failed replays did not test the initialized resource.

The fixed directory has eight little-endian u32 header words, an eight-byte
`rkosftab` magic at+0x20, entry count at+0x28 and reserved word at+0x2c.
Entries start at+0x30, each containing a four-byte tag, u32 offset, u32 size
and u32 flags. Header word1 is observed as1; nested allocation layouts have
word2=0x42. Other header/flag meanings remain unassigned and vendor metadata
is preserved. The prepared input container's header+0x10/+0x14 point to its
manifest offset/length. Firmware payloads stay opaque.

The current layout has34 entries, whose maximum offset+size is0xae3c00.
The allocation generator derives that extent instead of retaining the recorded
11.4MB resource or hardcoding its capacity. Names/sizes including `msww`2MiB,
`mswb`1.5MiB and `mswc`3MiB are metadata; their function is not established
by their spelling. Publishing a current opaque `msww` copy alone does not
advance PF1 startup to2. See COLD-START.rst for the controlled experiments.

Independent PIO reads also qualify DART operation1 stream invalidation for
SID17. Without invalidation, retargeting its IOVA still writes the old physical
page. After invalidation the generated PCI result reaches the new page. The
same operation works across a fresh root and restoration, with no DART error.
These148/149 tests use only owned pages and PCI configuration data; other
streams/command flags are not generalized from that result.

Native157 opaque resource correspondence
---------------------------------------

At the first observed Wi-Fi ready2 read, PF1's published SID2 resource and
PF0's SID1 allocation's `2ftb/msww` region both contain2097152 bytes and have
SHA-256 ``3d8a3bbf118e2ef0907a62c9b379451635139d4a40e95f325b7aaad844d7a797``.
Their physical mappings differ, establishing an opaque copy correspondence.
No contents or executable formats are decoded. This narrows the role of the
layout entry without establishing that publication is the startup trigger.
Direct158's generated-layout run reaches initial4/runtime2 without optional
DART parameter/reserved-stream experiments, but its generated-region copy
still leaves PF1 +8 at0. Main image preparation and Wi-Fi startup are pending.

Reset assertion qualification
-----------------------------

The bridge's ADT ``function-reset`` selects the existing SMC pin transport.
The former ``start_link`` sequence only wrote its release level. Explicit
reset165 writes the assert level first (SMC readback0), holds100ms as a
prototype parameter and later releases it through the ordinary link sequence.
Shared firmware and Bluetooth work after this assertion; Wi-Fi remains0.
Run164's Wi-Fi queries after reset release alone do not prove absence of
retained radio state. Neither test establishes a physical power cycle.

Firmware interface lifecycle
----------------------------

Owned174/175 qualify these runtime commands on primary selector0:

* 1: six-byte address template, reflected by the next activation's info.
* 2: activation with a five-byte u8/u32 body. The zero form works; tested
  low-word variants through0xb give identical observed interface descriptors.
  Their other possible effects are not established. An empty body faults
  Wi-Fi firmware; do not send a header-only activation request.
* 3: deactivation. The observed zero u8/u32 form works.
* 20: interface information. Fixed prefix: byte0 retains the selector value,
  byte1 reports0 after deactivation and2 after activation, bytes2..7 carry
  the interface MAC. Bounded packed TLVs follow from offset8. The activated
  record contains tag0x017d/26, whose value remains uninterpreted.
* 19: selector1 returns exactly the same channel-list body as command47/1
  in the tested active interface. It is not established as a power operation.

The fourth header-prefix word is0 in the successful tested replies. An
inactive command20 reply reports0x01430000; the complete field encoding is
not decoded. Activity/MAC readback does not prove scan, association or data
traffic. Firmware input provenance and the Linux cold-start port remain open.

Independent scan protocol
-------------------------

Experiment 176 identifies command ``0x1000f`` as scan-related. Its 14-byte
prefix is followed by packed TLVs. Tag ``0x0104`` contains a byte count and
that many little-endian u32 channel words: successive requests select
1/6/11, 2/3/4 and 5/7/8. Native optional tags include 0165/5 and 0102 with
SSID descriptors. A separate command ``0x20000`` is a connection request;
its security contents are omitted from the saved observation, retaining
only fixed-prefix length and TLV layouts.

Experiment 177 generates a smaller scan: ``<I5H>`` header, one 0104 channel
vector, no captured SSID/configuration records. Header word 0's low byte is
the scan cookie. Isolating bit 31 enables queue-4 frame reports. Separately
testing option words 0200 or 1800 with bit 31 clear delivers none; those
words' other meanings remain unqualified and zero works. The five u16
words remain explicit inputs. Changing the two first timing words from
20/20 to 50/50 changes the completion's elapsed value from 26 to 57.

The normal acknowledgment is ``0x4001000f`` with the management transaction
echoed. Rejection instead echoes ``0x1000f`` and a nonzero fourth prefix
word; passing all 167 raw country-table words produces 01510000. The table
contains extra metadata, and is not directly a valid scan-channel vector.
This is reported as a firmware rejection rather than a transport timeout.

Queue 2 event ``0x80000005`` has a nine-byte ``<HHIB>`` body: status,
delivered frame count, observed elapsed milliseconds and scan cookie.
For the three-channel scan: status 0, count 22, elapsed 78, cookie 9;
management transaction 13 is distinct. All 22 queue-4 reports decode and
echo cookie 9 in their runtime header's transaction field. Queue-4 command
0 bodies contain a standard radiotap header followed by an 802.11 beacon
or probe response. Radiotap's own length locates the frame. Its standard
channel and signed signal/noise fields apply; later vendor fields remain
opaque. The test SSID is observed at 2462 MHz/channel 11, about -50 dBm.

This differs from the existing Broadcom protocol: Centauri uses these own
management commands, acknowledgments, event and receive queues. The radio
and 802.11 frame formats themselves are standard; no fixed 96-byte metadata
replay or Apple-specific beacon parser is necessary. The new Python decoder
uses the definitions already present in Linux's radiotap/802.11 headers.
Association, regulatory table flags and the Linux cfg80211 port remain open.

Connection request security and outcome
---------------------------------------

Native 181 confirms a six-byte ``<BBBBH>`` security prefix at offset 70 in
the 159-byte fixed connection header: authentication selector, firmware AKM
selector, cipher selector, flags and key length. The opaque key starts at 76;
the qualified extent is 32 bytes. The reference SAE request uses 3/8/4/0.
These offsets correct the initial private generator's one-byte shift.
The key itself and password field values are not saved as trace inputs.

``CONNECTION_START`` is 20000 and its asynchronous acknowledgment is
40020000. Outcome 80000000 has a 90-byte body, whose first 44 bytes are
``<32sI6sH>``: SSID storage, SSID length, BSSID and result status. Successful
native outcomes have status 0; failed owned probes have 1. Header word 3
can carry a more specific status detail and is kept separately. The final
46 bytes retain their wire representation; no pointer is followed.

The Python helper covers only these qualified wire pieces. Control words,
remaining packed fields, authentication prerequisites and a complete owned
association are still being investigated. No Linux connection support or
upstream-ready cold driver follows from successful request acknowledgment.

Native 185 limits both channel-list replies to channels 1..11, causing the
reference driver to connect to the test network's WPA2 BSS on channel 11.
The successful native request has security prefix 0/4/4/0 and key length 32,
while its RSN IE carries AKM suite type 2. Thus firmware PSK selector 4 must
be mapped separately from the IEEE RSN suite type. The SAE request instead
uses 3/8/4/0. ``AKM_SELECTOR_PSK`` and ``AKM_SELECTOR_SAE`` name the qualified
values; other authentication methods remain unqualified. The connection
outcome reports status 0 and the subsequent link query has a nonempty body.

TX bit17 controlled comparison187
---------------------------------

An alternating ARP probe sends the same router request using descriptor
metadata word0 values bit17, zero, bit17, zero. All four submissions complete
with the expected cookie, firmware/IPC stay2 and DART error stays0. Both bit17
requests receive matching router ARP replies; neither zero request receives
one within three seconds. Thus completion alone does not establish packet
delivery. This bit has a functional effect in the tested default Ethernet
queue, but its exact meaning is still unassigned. Keep the existing value;
do not name it an RF-enable, checksum or routing bit without further evidence.

Initial image MSI prerequisite and Linux ownership200..202
-----------------------------------------------------------

The endpoint-only comparison197 reaches initial4 with MSI controlab while
the root receiver is disabled.201 keeps its address/data/MME fields and INTx
mask identical but writesaa, clearing only MSI Enable; it stays main0,
submission1.202 uses8b, enabling a single message, and reaches4. Therefore
MSI Enable is required by the observed initial-image interface even with
polling; four messages are not required for that stage. No internal firmware
implementation is inferred from this external contract.

Linux200 allocates its vectors through PCI, receives two vector0 interrupts,
loads the image through its coherent DMA allocation and reads main4/submission0
with DART error0. The standard Apple host driver assigns CAM slot0 to
RID0100/SID1. The independently bounded capacity probe confirms16 writable
slots and RAZ/WI at slot16, restoring each invalid-entry probe and preserving
the valid slot0 value80010100. A table index is distinct from its encoded SID.
Linux's normal42-bit DART domain uses TCR9 and successfully translates the
owned initial submission; no special N1 pagetable implementation is added.
Linux203 repeats the result with one allocated vector and again receives two
vector0 events. The runtime control profile still allocates four vectors.

Runtime205 confirms main2/submission1 after successful Linux image loading;
initial203 is main4/submission0. Do not apply the initial submission0
completion interpretation to the runtime stage. Linux removal disables
PME enable even if requested before removal; the successful205 restart
sets D3hot+PME after the PCI object has been removed and before PERST.

Port0x88 request/completion handshake
--------------------------------------

``PORT_QUIESCE_CONTROL`` is relative to ADT apcie port bank8. The names describe
measured behavior, not an inferred hardware implementation:

* Bit0, ``PORT_QUIESCE_REQUEST``: a write sets completionbit4. Direct211's
  bounded observation sees completion within2ms while link UP stays set.
* Bit4, ``PORT_QUIESCE_COMPLETE``: completion status, cleared by writing1.
  Two isolated clears preserve the measured LTSSM/link/clock/reset state.
* Bit8: omitted by212/214 without preventing runtime startup. Its role is
  unqualified and it has no name or write in the new warm-reset sequence.

The former quiesce0x11 value combines a request with W1C status acknowledgment.
The former startup0x110 combines an unnecessary bit8 with that acknowledgment.
Issue an explicit request and poll completion; these are not persistent
configuration presets. Exact in-flight transaction gating is not yet proven.
Linux207 confirms MSI/RID/mask registers survive the reduced reset unchanged.
The old full initialize_port replay is unnecessary for warm restart.

UP alone is not a trained-link indication after PERST:213 has portstatus
0x9b001345 (UP|BUSY) and all configuration identities0xffffffff. Require UP
and BUSY-clear. Successful214 finishes at0x9b001001 and enumerates all three
runtime functions. PHY bit31 remains unqualified; preserve that limitation.

GPIO reservations and native reset
------------------------------------

J813 ADT GPIO0 ``no-restore-gpio-pins`` has eight bytes that decode as two
little-endian u32 indices78/203, not the parser's combined u64 mode value.
GPIO0 is0x39a000000; the old Linux raw-default cache reads offset0x138 and
aborts, exactly pin78. PERST pin192 atoffset0x300 remains accessible. GPIO0
has224 pins and7 interrupt groups. Use standard DT reserved ranges and GPIO
polarity; the register cache must not bulk-read firmware-owned pads.
Sparse flat caching retains atomic cached updates but reads controls only
on demand. Linux218/219 validate this provider and native PERST transition.

The T8142 native reset uses the measured bit0/bit4 quiesce handshake, port
0x800 clock/0x804 ready/0x82c reset, per-port PHY clock requests/acks and GPIO
PERST. It preserves Linux-owned MSI/RID tables and clears transient interrupt
events before restoring the mask. PHY bit31 remains explicitly named as an
unqualified T8142 control, not assigned an invented individual clock role.
The100ms reset hold/release delays retain the qualified prototype profile;
these tests do not establish their minimum values.

Generated PF0 IPC context and ownership220..226
-----------------------------------------------

The format0x0300 header is0x68 bytes. At+0x30/+0x32, separate u16 counts
describe one completion queue and13 source queues (control0 and data1..12).
At+0x44/+0x46, separate u16 counts give control completion/submission ring
entries:16 by default, independently qualified at8 with repeated wrap221.
The u64 ring pointers at+0x34/+0x3c are unaligned wire fields, not aligned
native Rust references. Relocation keeps all pointers within the owned16KiB.

Header+0x48 can be zero on PF0 (222) and Beta (226), each qualified through
Wi-Fi management and an actual Bluetooth link/role response/disconnect.
Its bit meanings remain unassigned. Do not retain the old0xffff0000 or give
it an invented functional name. Beta's distinct+0x50 bit26 selects32-byte
control completions and remains qualified separately.

PF0's generated queue8/2/4/3 registration frames are52 bytes. Ring descriptor
format is a u32 length/format, unaligned u64 DMA pointer and u32 cookie.
Inline capacity1024 is encoded with mantissa16 times4 times2^4, rather than
an opaque combined mode word. Registration bit22 enables in-place completion
for these queues; no separate CQ address is supplied. Transport completion
kind0x401 returns the request length and slot cookie, distinct from data
descriptor flags or an application reply.

Global event arena pointers belong only to PF0's DMA domain. Isolated223
and full-radio224 show that SID2/3 aliases are unnecessary; Linux225 uses
only its PF0 DMA API allocation. Queue8/2/4 receive targets are3/15/7, below
their4/16/16 capacities. Queue3 is inline transmit. Host cursors publish
owned slots; device cursors transfer completed slots back. Lengths and DMA
pointers must be bounded before reading or returning a completed buffer.

Independent Wi-Fi resource and IPC publication230..242
------------------------------------------------------

Wi-Fi resource publication does not require a Beta host context, an mswb
resource or a ControlApp greeting. Direct230 copies the current companion's
``msww`` bytes into a fresh PF1 allocation, publishes its config pointer and
observes firmware status+0x08 reach2. The source directory offset is640; it
is not page aligned. The destination DMA address and resource size are page
aligned. Do not impose the destination alignment on the FTAB source offset.

The resource uses PCI config+0xf88/+0xf8c address halves and+0xf90 byte length;
Unix microseconds use+0xff8/+0xffc. Direct232 additionally generates the Wi-Fi
context and twelve basic queue registrations, then queries8/10/47, all with
Beta untouched. Its DMA window contains only PF1's resource/context/pool.
The PF0 arena's address is unnecessary even as a window bound;234 repeats
the narrowed window through full Wi-Fi management and a Bluetooth link.

Wi-Fi's handshake doorbell is BAR0+0x1028, distinct from Beta's+0x107c.
State is+0x0c, context address+0x10/+0x14, window base+0x18/+0x1c and window
byte length+0x20. Write handshake1 and wait state1 before context/window
publication and handshake2. Native233's first draft uses the Beta offset
and cannot reach state1; this is corrected, not a new hardware requirement.

Endpoint MSI Enable is required at context publication. Direct237 changes
only the qualified Wi-Fi MSI profile to disabled (control0x8a); firmware
changes to3 and IPC remains1, with DART0. Direct238 restores16 messages at
the same near-4GiB addresses/window used by native235 and passes startup,
twelve registrations and queries8/10/47. Direct239 selects one enabled
message and passes the same operations. This does not yet qualify every
future data queue's MSI routing; the Linux draft owns16 handlers through PCI.

The config resource and context have different address constraints in these
tests. Direct240's resource high word1 leaves firmware0;242 repeats with the
resource exactly at4GiB and waits three seconds, still firmware0/DART0 despite
a verified SID2 translation and config readback. Direct241 keeps the resource
below4GiB and puts context/pool at4GiB: firmware2/IPC2, twelve registrations
and queries8/10/47 all pass, DART0. The resource allocator therefore uses a
32-bit coherent mask, then restores the wider context allocation mask. This
is an observed resource-interface limit, not a claim that the entire device
or DART has32-bit addressing. The context header retains the still-unassigned
qualified+0x60 profile0x00500000; its individual bits need further probing.

Native basic queue ownership245..247
-------------------------------------

The basic queue profiles are generated from dimensions, with rings at arena
+20000 in8000-byte regions and packet pools after the2MiB context arena.
Admin CQ/SQ entries are16 bytes; CQ completion401 returns frame length and
slot cookie. A separate frame per admin slot prevents pending DMA reuse.
Basic CQ flags encode source queue in the high half and401 in the low half;
its next word contains returned length and cookie. Queue6 SQ128/CQ1024 and
other asymmetric dimensions must be treated independently. Firmware SQ
consumption alone does not complete its outstanding packet buffer.

Management headers are24 bytes:16 prefix bytes, u16 total length, u16
transaction and u32 command. Requests generate a zero prefix. Replies' fourth
prefix word is command-specific: native247 confirms01430000 on inactive
commands3/20, while state/address bodies remain valid. It is not a generic
nonzero-error rule. Explicit alias rejections for scan/connection retain their
separately qualified semantics. Interface operation bodies are five zero
bytes; active state2 plus MAC readback proves interface activation only.

The current basic queue CQ word7=2 and SQ profiles still contain individually
unassigned bits. Observing MSI2 traffic does not isolate each field's role.
Do not rename unknown profile flags as interrupt selectors without a probe.

Native scan event ordering248
-----------------------------

A completed queue2 event80000005 does not imply the host has already drained
all queue4 reports. Its u16 frame count is an independent drain condition.
Native248 processes388 reports across11 matching completions; each carries
its own eight-bit cookie, independent of the management transaction. Host
cancellation detaches only the callback owner; report counting continues
until the actual event/count closes that firmware scan. This avoids reusing
cookie/state while late DMA reports remain possible. No firmware cancel
opcode is assigned by this implementation.

Disconnect and native connection lifetime250..254
-------------------------------------------------

Command20001 has a distinct40020001 acknowledgment and empty80000004
completion for the same u16 transaction. A joined interface rejects direct
command3 with01470000 until this operation completes. The qualified default
body is eight zero bytes. Native optional148/u32value30 and initial scalar8
are unnecessary. Values3/8/0 in the first word work; four bytes return
01530000 without a completion event. The two words' individual roles, any
on-air reason mapping and untested intermediate lengths remain unassigned.

Native250/254 qualify the existing all-zero five-byte interface operation
through association as well as scanning, so the earlier options11 is not
required. Active/inactive flags follow verified state readback; a rejected
deactivation must not silently mark the host interface inactive. The driver
holds each accepted cfg80211 BSS reference until reporting the corresponding
firmware result, rather than relying on a scan-cache lookup after the fact.

Native Ethernet geometry255..256
---------------------------------

The separate data region begins at context DMA+0xa00000. Its offsets are
TXCQ0/0x40000 bytes, TXSQ0x40000/0x10000 bytes, RXCQ0x50000/0x20000 bytes,
RXSQ0x70000/0x8000 bytes, RX pool0x78000/0x400000 bytes and TX pool
0x478000/0x40000 bytes. Total0x4b8000 bytes. Source entries are2048;
TX completions8192 and RX completions4096. TX descriptors and both completion
types are32 bytes; RX descriptors16 bytes. The same layout has native Linux
packet and wrap evidence, rather than only Python/TAP traffic evidence.

TX completion kind502/source25 returns the outstanding cookie. RX kind403/
source12 returns cookie and length; valid RX data begins two bytes into its
own posted2048-byte buffer. Length/source/kind/cookie/ownership are validated
before advancing host cursors or returning a cookie. Reposting uses the next
available SQ slot, not the cookie number. CQ completion can precede device SQ
cursor visibility; software defers the return rather than discarding it.

TX source notification is BAR+1030 and RX source notification+1050. CQ host
cursor publication does not ring these source doorbells. Context cursors are
aligned u16 values: source25/12, completion192/207. Native255 TX9053/RX15863
crosses every source and completion wrap without an ownership/protocol fault.
TX descriptor+16 bit17 is retained because alternating Python ARP probes187
show it is required by the qualified profile; its exact role is unassigned.
Extended registration profile words remain qualified values. Native MSI3/8
activity is correlation with those profiles, not isolation of each field's
interrupt-routing meaning.

Independent SAE password and key-slot behavior270/271
-----------------------------------------------------

The connection security prefix at70 uses authentication3, firmwareAKM8,
CCMP4 and flags0 for SAE. The key-length word at74 remains32, but a generated
zero area76..107 works: no precomputed PMK is required. TLV139 is a packed
u16 tag, u16 payload length, u16 password length and the password bytes.
An independently generated request with the scanned SAE RSN element and no
extra native TLVs completes association;270 then obtains DHCP through fresh
Ethernet queues. A deliberately wrong password271 is acknowledged but reports
connection status1/detail00730000, so acknowledgment is not authentication.
Neither experiment saves the request or password. The additional native
13c/178/17e records are unnecessary for this tested profile. Their contents
are not copied into the driver. PWE mechanism selection and active PMF attack
behavior are not established by this functional connection/data test.

SAE authentication method observation277..284
---------------------------------------------

Fresh independent Python277/278 expose ordinary 802.11 Authentication reports
on basic receive queue4 while firmware performs SAE. The ordinary24-byte
management wrapper is followed by radiotap; its declared96-byte length places
the public 802.11 header at packet offset120 in these runs. This offset is an
observation, not a parser constant. Source/destination/BSSID match the selected
station and AP. SAE algorithm3, transaction1, status126 and group19 identify an
H2E commit response; transaction2/status0 follows and connection outcome0
confirms successful authentication. This establishes that the default Pixel
path actually uses H2E, beyond merely seeing H2E advertised in the beacon.

``decode_sae_authentication`` shares the existing bounded radiotap decoder and
returns only peer-checked public transaction/status/group/method metadata.
Cryptographic scalars, elements, tokens and confirm bodies are never returned.
Four synthetic boundary tests cover method/status distinctions, wrong peers,
fragment/protection flags, malformed/truncated radio/802.11 headers and FCS.
The five prior scan decoder tests still pass after extracting the common radio
header decoder. No firmware/Apple executable inspection supplies this code.

Repeated joins in277 often report no new authentication frames, consistent
with retained *current-run* firmware/AP authentication caching. They cannot
qualify method control. Subsequent candidate tests therefore use a complete
chip reset and fresh firmware/context for each input. Optional TLV13c values0
and2 each give fresh H2E plus successful association; value1 instead returns
outcome1/detail017c0000 before an authentication header is observed. Its
meaning remains unassigned. Fresh security-prefix byte73 values1/2/4/8 each
still show H2E commit and confirm headers, then outcome1. These observations
do not justify naming any tested field as PWE control. Candidate values are
private diagnostics and are not new driver defaults.

Prototype276 initially limited queue4 to160 bytes for metadata, truncating
beacon IE decoding and accepting no BSS. It is a setup negative and does not
prove the AP was absent.277 restores full declared scan-report extents while
the method decoder retains only the bounded public authentication prefix.
All these direct Python owners stop DMA with all three busmasters disabled
and DART error0. Forced-PWE support is still being investigated separately
from this method observation.

H2E-only policy and explicit RSNXE285..298
-----------------------------------------

Standard RSNXE element244 is accepted as a connection-request TLV with the
existing u16 tag/u16 length envelope. Its one-byte payload0x20 advertises SAE
H2E (bit5); the low nibble encodes payload length minus one. The Python and
Rust encoders generate this element, rather than copying a vendor record.
The capability validator rejects absent, malformed or non-H2E peer RSNXE.
Python297 uses that explicit request after full reset and obtains a fresh
H2E commit, confirm0, association0 and DHCP ACK at the factory MAC address.

This is a standard capability/policy path, not evidence for an undocumented
exclusive firmware PWE-selector bit. Fresh285 with optional13c=64 joins the
SAE AP using H2E, but287 fails on the WPA2 AP, while286 with13c=1 joins WPA2.
Together with279, these results rule out assigning a simple PWE meaning to
those values. A bounded survey of optional180..1ff records with payload0/1
likewise finds no legacy-method discriminator; ignored unknown tags remain
a possible explanation. No such candidate enters the driver. Three unique,
read-back station addresses in290 each induce fresh H2E authentication;
cached joins without authentication reports remain inconclusive for method.

Linux298 accepts NL80211_SAE_PWE_HASH_TO_ELEMENT, requires a selected peer
advertising well-formed H2E RSNXE, appends the generated capability to its
firmware request and cfg80211 association IEs, and watches peer-matched public
SAE metadata on queue4. A legacy commit under H2E-only policy returns EPROTO
and stops Wi-Fi DMA/service instead of authorizing a mismatched connection.
The Rust test injects that mismatch in the same poll as a success event and
verifies no success callback occurs. The normal default/BOTH path remains
available; forced hunting/pecking is rejected. Cached reconnects do not need
a new authentication frame, since the current-run cache is firmware-owned.

Existing brcmfmac/mwifiex/nxpwifi firmware-offload drivers and supplicant2.11
were consulted as local primary sources for RSNXE and policy handling. Their
vendor wire protocols were not imported or treated as Centauri documentation.
Unlike those existing drivers, this bring-up path explicitly checks its
selected BSS and observes the public authentication metadata made available
by Centauri. Wider device behavior remains to be qualified on other boards.
