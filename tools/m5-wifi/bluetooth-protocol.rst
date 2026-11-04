.. SPDX-License-Identifier: GPL-2.0-only

Audience: For LLMs; working evidence pending human review.

Centauri Bluetooth Python transport observations
================================================

For LLMs. Experimental hardware observations, not a completed specification.
No Apple executable or firmware instruction contents were inspected. Standard
HCI frames and typed host queue metadata are the sources for this transport.

Python is the development and probing implementation. Do not advance the Rust
Bluetooth draft until pairing/data work here and the used fields are understood.
The Python driver directly accesses the device under Linux. VHCI connects its
HCI packets to the existing Linux stack; it does not emulate controller replies.

The current cold Linux PCI adapter generates its own context and four inline
rings in normal coherent DMA. Native257/258 qualify standard HCI initialization,
fresh pairing and encrypted SDP through the Rust transport. It no longer
accepts a CENB handoff, nonce page or previous controller context. See the
cold checkpoint below and ``COLD-START.rst`` for the remaining firmware input
sources and other limits. The reserved-memory and platform-adapter tests
below remain historical comparisons.

Earlier verified Python state (boot 98p)
-----------------------------------------

A newly generated single 16-KiB SID3 page replaces the original 121 Bluetooth
host pages (1,982,464 bytes). One reusable control frame at context+0xf80 and
four two-slot inline rings fit in this page. Commands/events retain capacity
260; ACL rings retain full reported MTU 2586 plus the standard four-byte header,
rounded to 2592 bytes. Ring starts are context+0x1000/0x1240/0x1480/0x2900.
No auxiliary queues are registered. A frame is reused only after completion.

A fresh Linux/Mac Mini pair passed 2,000 changing-payload 512-byte encrypted
echoes (1,024,000 bytes), with exact Mac comparisons and Linux SHA256
36df7634c61e042902561d3dd30fc8232f3ecc576a91c45b7ab594ba94fee499.
Each packet contains its sequence number and a changing byte pattern, so a
stale packet cannot satisfy the next comparison. Security level was 2, key
size 16. Firmware/IPC remained 2 and DART error zero. Wi-Fi HTTPS returned 200
in the same boot. Named JSON metadata is 722 bytes; CPU access including the
nonce page is 20,480 bytes. Linux reserves 1,966,080 fewer bytes than the
original mapping. Shared firmware/control/Wi-Fi domains remain retained.

Bluetooth MSI remains disabled: Python polls. Factory address comes from ADT;
FF0B has no declared parameters. After every successful Reset the driver sends
its own FF77=0 before releasing Reset Complete. No captured startup parameter
blob is replayed. The earlier long-idle Wi-Fi firmware error remains an
outstanding stability observation; this is not a whole-chip cold-boot claim.

On the preceding 32-KiB layout, cached pairing after VHCI recreation also worked
with the explicit, bounded BlueZ compatibility probe below. Encrypted
Linux-initiated SDP returned a valid response. Both vendor connection codes
F0/F1 mean ACL. F0 occurs in both directions and both roles; controlled FF77
zero gives F0, while nonzero 1/2/255 gives F1. These are neutral enumerated codes,
not a direction or role bitfield. The Python baseline was then ported to the
native Rust transport below.

Native Linux Rust transport (boot 98p)
----------------------------------------

After the Python data proof, the native Rust driver repeated fresh pairing and
2,000 changing-payload encrypted 512-byte echoes, with exact Mac comparisons.
Linux SHA256 was
e6162c8d832534401f92d6c0112dfd3380b9ae270907e09efe335c6913e5c5d4.
The Python hardware backend was stopped. ``hci0`` resolved to platform driver
``centauri-bluetooth``; native controller counters showed zero RX/TX errors.
Rust Wi-Fi returned HTTPS200 after the Bluetooth transfer.
The final build also passed native module unload/reload. With the explicit
BlueZ 5.72 stored-key compatibility helper, a kernel-owned outgoing encrypted
SDP socket sent 20 bytes and received a valid 129-byte response, security level
2 and key size 16, without re-pairing. The final build then passed another
128 changing-payload encrypted echoes using the in-tree Mac client. HTTPS
again returned 200 after reload.
The 96 Python tests pass; the native module builds against the same Linux 7.3
Image/toolchain, and the HCI C adapter passes checkpatch without warnings.

The Rust protocol adopts the exact Python-tested one-page/full-MTU layout.
It validates the T8142 platform, current-boot nonce, reserved host RAM,
BAR context/window, index-vector extents, queue identities and idle TX cursors.
It generates inline descriptors and maintains RX credits rather than replaying
registrations. Each successful HCI Reset is withheld until the driver's scoped
FF77=0 completes. The original Reset Complete and unrelated HCI frames retain
their contents. Only observed successful connection types F0/F1 translate to
standard ACL1. Vendor event bodies remain unread. Polling uses no Bluetooth MSI;
SCO and a complete BLE/audio implementation are outside this verified path.

``pack_bluetooth_handoff.py`` encodes ``CENB0002``. Header bytes 0..135 describe
format/length/counts, serial length, context IOVA, BAR0, DART, nonce physical,
DRAM base/size, the 32-byte nonce and a zero-padded 32-byte serial. The serial
field is informational and is not compared with the running device. The next
24 bytes are one ``<QQQ>`` IOVA/physical/length mapping. Four ``<HHIQ>`` records
encode number, entry count, capacity and ring IOVA. Total: 224 bytes. No queue
registration words, packet contents, firmware bytes or pairing keys are encoded.
The earlier platform adapter loaded this metadata as
``gravity/centauri-bluetooth-handoff``. The new PCI/DT adapter instead reads the
immutable metadata from its ``memory-region``. Wi-Fi metadata stays separate.
Do not run the Python hardware backend while the native driver owns the queues.

The reproducible data test uses ``bluetooth_l2cap_test.py --peer
BC:74:EA:B6:3B:D5`` on Linux and our public-SDK Mac client::

    clang -fobjc-arc -framework Foundation -framework AppKit -framework IOBluetooth \
        macos_bluetooth_data_test.m -o macos-bluetooth-data-test
    ./macos-bluetooth-data-test C0:C7:DB:C7:D7:86 2000 512

Only the selected M5 address is accepted by the client. Start with a disconnected
ACL; the public Mac API's new-process channel creation on an existing ACL was
unreliable in separate experiments. ``bluetooth_sdp_test.py`` independently tests
encrypted Linux-initiated standard SDP data and logs response counts only.
Stored pairing on the test BlueZ 5.72 requires the bounded compatibility helper
after controller recreation, as explained below. Cold firmware/Wi-Fi association
and long-idle stability remain separate from the verified native packet path.

Boot 98 empty startup, polling and stored-pair verification
-----------------------------------------------------------

After isolated PF2 reset, Python published a newly zeroed 32-KiB domain with
four two-slot rings. No Bluetooth MSI routing was restored. Factory address,
zero-parameter FF0B and HCI Reset were sufficient for subsequent normal Linux
initialization and 2,000 encrypted 512-byte echoes. Linux SHA256 was
8a0e828e7792db1fd3c3451ad9f2972f94922450e43df996acf69effc847453d;
Mac exact comparison passed. Firmware/IPC stayed 2 and DART error zero.

The optional ``bluetooth_restore_classic_pair.py`` probe reads the existing
private BlueZ store and sends standard MGMT Load Link Keys with BDADDR_BREDR.
It checks the prototype identity and selected controller, refuses multiple
stored peers, and never logs/persists key values. This isolates BlueZ 5.72's
stored public-address-type mismatch from the radio driver. After VHCI recreation,
reloading the existing pair allowed an encrypted Linux-owned SDP socket with
security level 2 and a 16-byte key, without another pairing exchange. Our generated
20-byte Service Search Attribute request received a validated 129-byte response
(121 attribute bytes, no continuation). Only counts were retained.

With standard Create Connection role-switch permission forced to zero, Role
Discovery returned central (0) on an F0 link. Earlier F0 trials returned peripheral
(1). F0 therefore does not directly encode the current role; it also occurs on
incoming links. FF77 values 1, 2 and 255 produced F1 in controlled incoming-link
trials, while zero produced F0. Value 1 additionally passed 128 exact encrypted
512-byte echoes after stored-pair restoration. The exhaustive creation test still
establishes inhibition for all 255 nonzero parameter values. Restoring the unmodified driver to zero again produced F0 and passed the
encrypted Linux-initiated SDP request, in peripheral role (1). Values 2 and
255 producing F1 show that this is not a raw parameter-byte copy.

The Mac custom L2CAP test service still reports a missing UUID association after
receiving our request; changing SDP key spelling did not fix it. This does not
contradict the successful Linux-initiated standard SDP data transaction. No Apple
executable contents were inspected to investigate that peer-service behavior.

Boot 95 outgoing Classic connection control
--------------------------------------------

A controlled FF77 one-byte parameter sequence 1, 0, 1, 0 reproducibly rejected
Create Connection with status 0x0c at 1 and completed real connections at 0.
Each successful temporary link was explicitly disconnected. HCI Reset restored
the rejection, so the Python VHCI backend withholds successful Reset Complete,
sends its own FF77=0, and releases the original reset reply only after setup
succeeds. Unrelated events and ACL packets remain forwarded. The command is
named Classic Create Inhibit for this observed behavior; other effects remain
unproven. No captured parameter block is replayed.

The initial outgoing trials reported vendor type 0xf0; earlier incoming trials
reported 0xf1. Boot 98 also reports 0xf0 on incoming connections, so the earlier
direction-based constant names were too specific. Both values are confirmed
ACL. Role Discovery on 0xf0 returned peripheral; the role relationship requires
controlled testing. Nibble masking would incorrectly turn 0xf0 into SCO. The optional
Linux-boundary translation accepts exactly the two observed successful codes.
Failed and unrelated events remain unchanged; unknown successful vendor codes
are rejected.

After an isolated function reset, the same FF77=0 test completed a real outgoing
connection using the newly allocated 32-KiB domain and four two-slot rings.
Factory address, generated ZERO16 FF0B, Reset, then FF77=0 were sufficient for
that connection test. Encrypted Linux-originated application data with this
setup remains the next verification; do not infer it from bare connection success.

Boot 95 exhaustive byte and startup-extent probes
--------------------------------------------------

Python on Linux tested every FF77 parameter byte from 0 through 255, followed
by zero again. Every byte was accepted with no return parameters. Both zero
trials completed actual outgoing links and explicit disconnects; all 255
nonzero values rejected Create Connection with status 0x0c. Reset preceded
each value. This establishes Boolean behavior across the complete byte range
for the tested operation, rather than independent named flag bits.

The updated reset setup also passed fresh Linux pairing and 2,000 exact
encrypted 512-byte round trips from the Mac with four two-slot rings and the
32-KiB domain. Linux SHA256 was
5cbd01f6c72bea89c2814f1584220cc7dd3463cd2452f9f79b89442f3f09f9f2.
A recreated VHCI controller exposed a separate BlueZ 5.72 key-restoration issue:
user space submits the stored public address type for the Classic link key,
and this kernel skips it. A fresh pair then passed another 128 round trips.
No key contents were logged. The upstream user-space path is src/adapter.c
load_link_keys; no BlueZ implementation was imported into this driver.

FF0B accepted every generated all-zero parameter extent from 0 through 32,
each followed by a successful standard version query. Only our inactive TX
slot was zeroed before each trial, without reading old command contents.
After a further isolated function reset and a new zeroed 32-KiB host domain,
factored initialization sent factory address, FF0B with *zero* HCI parameters,
then Reset. Page scan remained ready for 12 seconds, followed by a successful
outgoing connection and disconnect with FF77=0. Firmware/IPC stayed 2, DART
error stayed zero. This removes the sixteen parameter bytes from that verified
warm-function RF path. An encrypted application-data check for the empty
startup command remains pending; the 128-bit mask hypothesis is not established.

Linux-originated application data remains unverified. Standard L2CAP tracing
and the Mac's runtime log confirm receipt of our request at its custom PSM,
then the Mac service reports a missing service-UUID association. An RFCOMM
probe reached an encrypted socket but the peer closed before echo data. These
are recorded counterexamples, not successful outgoing data proofs. During a
long idle run, Wi-Fi separately entered firmware state 3; Bluetooth remained
usable. A new reference launch is required before rechecking both radios.

Earlier own-context baseline (boot 88)
--------------------------------------

After an isolated PF2 reset, Python zeroed the host context and all thirteen
rings, generated their registrations, restored the factory address from ADT,
and generated the all-ones FF0B mask plus standard host-support/buffer fields.
Both page scan and inquiry scan stayed ready. Linux then paired to the exact
authorized Mac Mini and exchanged 2,000 exact 512-byte echoes (1,024,000 bytes),
with pairing/encryption and 16-byte key size. Linux's received-data SHA256 was
3c6cb3db80562aeb2cb439f07947b1494b9c1f748f0a2aea4a6504e68ab73b76.

This used owned/generated queue contents, with running firmware, allocations
and DART mappings retained. It did not replay the captured 24-byte FE44 setup
record, but that is not proof the record is unnecessary for a whole-chip cold
boot. The 128-bit FF0B record was generated as an integer mask. Both zero and
all ones were accepted on a reference context, but its role and independent
necessity remain unproven. The successful compound host-support setup must be
reduced by further Python experiments before naming individual requirements.

Data used unmodified ACL PB=0, default ERTM availability and default RSWITCH/SNIFF
link policy. The opt-in Connection Complete link-type F1-to-1 translation remains
necessary in the tested Linux boundary. A subsequent same-ACL channel reopen
still failed with Mac API error 0xe00002bc before data. Rust Wi-Fi on cen0 also
passed fresh HTTP301/219-byte and HTTPS200/87,382-byte requests in this boot.

A second cold function-initialization trial omitted FF0B entirely while keeping
all other setup the same. Page scan failed with firmware state 3. Sending FF0B
with sixteen generated ZERO bytes instead passed page scan and a second actual
Linux pair plus 2,000 exact 512-byte echoes. Linux SHA256 for this zero-record
run was a14d3e4b85091c51952397a4e0fd9009746c546a7c143cda389fa76cea29f049.
No enabled parameter bit, captured all-ones value, or FE44 body is needed for
this verified warm-function data path. The command has an initialization side
effect; its detailed parameter semantics are still unresolved. The helper name
``probe_ff0b_mask`` records the original mask hypothesis, not a proven event-mask
meaning. The boot 89 page-scan reduction below tests the remaining standard setup calls.

Standard readback under the working Linux stack returned Simple Pairing=1,
LE Host Supported=1, Simultaneous LE Host Supported=0 and Secure Connections
Host Supported=1. A continuously asserted simultaneous-host bit is therefore
not necessary for this verified data run; startup side effects are not isolated.
Bluetooth remains Python. SCO is not implemented, and the Rust Bluetooth port
is deferred while required initialization and remaining fields are investigated.

Boot 89 Python initialization reduction
----------------------------------------

After a fresh reference boot, own-function initialization with all thirteen
queues passed page scan for 12 seconds using only the factory address, FF0B
with sixteen generated zero bytes, and HCI Reset. No explicit Host Buffer Size,
Controller To Host Flow Control, Simple Pairing, LE Host Support or Secure
Connections Host Support command was sent by the bare Python setup. This
isolates those extra setup calls as unnecessary for page scan; it is not yet a
pairing/data result for the reduced setup.

A further function reset registered only queues 12, 7, 1, 8 and 2. The same
minimal setup again passed page scan for 12 seconds, firmware/IPC both 2 and
DART error zero. Queues 3 through 6, 9 through 11 and 13 are therefore not
required for this page-scan test. Queue 12 bodies remain unread. Named metadata
for this five-queue setup is 1,992 bytes; physical allocations are still retained.

Clearing registration word 7 bit 22 in that five-queue setup failed during the
very first queue-12 registration: global host submission index became 1 while
both device control indices stayed 0, and firmware changed to state 3 without
a DART error. No HCI command or scan test ran in that trial. This does not
establish an interrupt/notification meaning for bit 22, or necessity on other
queues. The previous ``notification_bits`` name is an experimental hypothesis.
A restoration trial stopped safely at PCI configuration CRS before BAR access;
no successful restored-baseline result is claimed yet. A subsequent fresh
reference launch lost its USB proxy while starting CPU 8, before Bluetooth
initialization; that launch is not evidence about the queue field.

Bounded queue-size probing
---------------------------

The Python metadata minimizer accepts explicit queue_entries overrides only
when the queue is selected and 2 <= requested slots <= original slots. Its
translation/access set is recalculated from the smaller geometry; out-of-range,
boolean and unselected-queue requests are rejected. The runtime accepts a
smaller auxiliary ring only with the already-confirmed packet capacity.
These controls enable experiments; they do not claim a smaller slot count has
passed hardware tests yet. For example, a 16-slot queue 12 would reduce this
boot's CPU-accessible range from 217,088 to 102,400 bytes, without freeing DMA.

Boot 92 reduced Python data verification
----------------------------------------

A fresh reference boot and isolated function reset initialized only queues
12, 7, 1, 8 and 2, with queue 12 reduced from 128 to 16 slots. Python generated
and zeroed the context/rings, set the factory address, sent sixteen generated
zero bytes with FF0B, then HCI Reset. No extra bare host-buffer/support setup
was sent. Page scan passed for 12 seconds with repeated queue-12 wraparound.

Linux then used that five-queue transport through Python VHCI, completed its
normal standard HCI initialization, paired the authorized Mac Mini, and passed
2,000 exact 512-byte round trips: 1,024,000 bytes. Security level was 2 and key
size 16. Linux SHA256 was
58bb3cae340c72dcadf1f0418d7e8326f5573413566db0ce74cdb424801ec54b.
The tested Python source SHA256 was
d38a960fb1939d299467784122adaa4b89d5d8cc252a20d7030519e45d2e18f9.
ACL PB=0, ERTM availability and RSWITCH/SNIFF remained at normal defaults. The
F1-to-1 Connection Complete probe was enabled. Rust Wi-Fi concurrently returned
HTTP301/219 bytes, HTTPS200/87,147 bytes, then another HTTP301/219 after the
Bluetooth bulk exchange.

The reduced named Bluetooth metadata is 1,431 bytes and the CPU access set is
102,400 bytes, versus 3,332 and 274,432 for the thirteen-queue baseline. All live
DMA/page-table reservations still total 35,241,984 bytes; none were freed. This
proves the reduced owned-host warm-function path, not a whole-chip cold boot.
The registration bit-22 failure and F1 high-bit meaning remain unresolved.

Same-ACL reopen control on boot 92
-----------------------------------

A Mac test that stays in one public IOBluetooth API client process, waits for
its channel-closed callback, then opens another channel succeeded. Both sides
then completed sixteen sequential encrypted channels on one ACL connection,
128 exact 512-byte echoes per channel: 2,048 packets and 1,048,576 bytes total.
The Mac log has sixteen exact-match results and only one baseband open. Linux
reported all sixteen channel completions with exact echo and 16-byte key size;
firmware/IPC remained 2 and DART error zero. No controller field or link policy
was changed for this control. Same-ACL channel reopening therefore works in
this tested sequence. The failure across separate Mac test processes remains
an API/session-lifetime investigation, not a demonstrated hardware transport
requirement. The Linux server now accepts a bounded --connections count.

Four-queue Bluetooth data on boot 92
-------------------------------------

A further isolated function reset omitted every auxiliary ring, including queue
12. Only command TX 1 (5 slots), event RX 7 (17), ACL TX 2 (9), and ACL RX 8 (9)
were registered. The same factory-address / zero FF0B / Reset sequence passed
page scan. Fresh Linux Python VHCI initialization, pairing and 2,000 exact
512-byte echoes then passed: 1,024,000 bytes, security level 2, key size 16.
Linux received SHA256 was
b03320bd0acb59c9c52c5de2cad4d44aad64b9c7cb020d89999d2053a6110681.
No auxiliary queue was registered, read or drained, and its packet counter
remained zero. Firmware/IPC stayed 2 and DART error zero. Rust Wi-Fi again
returned HTTP301 and 219 bytes after the transfer.

The four-queue named metadata is 1,189 bytes; permitted CPU RAM is 81,920 bytes.
This eliminates auxiliary data from the working transport. Firmware, original
physical allocations and DART mappings remain retained; no memory was freed.

Control vectors and in-place completion probe (boot 93)
--------------------------------------------------------

A context declaring two completion queues and keeping 22 transfer queues was
accepted with relocated transfer-index vectors. A 44-byte completion-create
message registered CQ1 in zeroed host-context RAM. The initial Python control
helper incorrectly treated CQ0's device index and SQ0's device index as
adjacent: the actual CQ0 head and relocated SQ0 tail both reached 1, while the
old adjacent read saw (CQ0 head, CQ1 head) = (1, 0). The helper now follows all
four published index-vector pointers and both control-ring pointers, with
bounds checks before writes. Offline tests deliberately relocate and separate
these fields, and reject out-of-context pointers before writing.

Command queue 1 with CQ1 selected and registration bit 22 clear registered, but
its first Read Local Version command failed: firmware 3, IPC 2, DART zero,
command producer 1/device consumer 0, CQ1 unchanged and no completion marker.
Keeping that same two-CQ layout and restoring only bit 22 passed five standard
queries. Every submitted command slot gained tag bit 26; CQ1 remained zero.
This supports bit 22 selecting the working in-place completion mode rather
than merely enabling notifications. The source now names this observed mode
QUEUE_IN_PLACE_COMPLETIONS. A functioning alternative CQ mode is not claimed.
A follow-up adding registration bit 19 initially stopped at configuration CRS.
The identifier later recovered during the guarded 30-second polling phase;
configuration was restored before BAR access. The trial then registered all
queues but its first Read Local Version command again failed with firmware 3,
IPC 2, DART zero and no CQ1 completion. That additional flag did not make this
alternative mode functional.

The completion-create packing experiment consulted only the local primary
Linux v7.3 drivers/bluetooth/hci_bcm4377.c field definitions, by Sven Peter,
Copyright The Asahi Linux Contributors (GPL-2.0-only OR MIT). No Apple binary
or firmware instructions were read, and no Broadcom driver implementation was
imported into the N1 transport.

Capacity encoding equivalence on boot 94
----------------------------------------

Changing only the ACL TX/RX capacity word from 0x30510001 to 0x20a20001 kept
all four queue geometries and flags unchanged. The first uses mantissa 81 and
exponent 3; the second uses 162 and 2. Both evaluate to 2592 bytes. Registration,
page scan, fresh Linux pairing and 2,000 exact 512-byte echoes passed with the
alternate encoding, wrapping both nine-slot ACL rings repeatedly. Firmware/IPC
stayed 2 and DART error zero. Linux SHA256 was
7c7b1da206c94f11915bf9473b396b06cad282bbb5662a8c6cc28115c1fd524b.
Rust Wi-Fi returned HTTP301/219 bytes after the test.

This hardware control confirms the exponent/mantissa capacity formula and its
unchanged 2608-byte ring stride for both ACL directions, rather than merely
matching several captured examples. The Python encoder can generate that field
from a byte capacity; the encoding no longer needs to be replayed as a raw
constant. Full controller MTU payload testing remains separate from this
512-byte repeated-slot/stride test.

Fresh 32-KiB domain and MTU-derived geometry (boot 94)
-----------------------------------------------------

After quiescing PF2 and resetting its IPC, Python allocated/zeroed a fresh
32-KiB arena and two new 16-KiB DART page-table pages. Only SID3 was replaced;
SID1/SID2 root/configuration registers stayed unchanged. The new domain maps
only the arena, and the PCI DMA aperture is narrowed to those same 32 KiB.
No original Bluetooth DMA buffer or table is needed by SID3. Linux's updated
reservation map excludes the old, disjoint Bluetooth allocation pages.

Generated two-slot command/event rings use the standard maximum HCI packet
extents rounded to four bytes. A bootstrap transport accepts only those two
queues, reads standard Read Buffer Size, then derives ACL capacity from the
reported MTU 2586 plus four-byte HCI header, rounded to 2592. Generated ACL
rings fit after the initial rings without relocating them. The full transport
still requires all four rings. The owned global control ring uses 16-byte
completions, confirming its format selector is independent of packet queues.

Page scan and fresh Linux Python VHCI initialization, pairing/encryption and
2,000 exact 512-byte echoes passed with this new domain. Linux SHA256 was
efd7193b3fa75287696f1b4096bb3e4fd1ee521518086d39c0e2cd40bf4224a0.
Security level was 2 with a 16-byte key. Tested Python source SHA256 was
76b6918c60d162087c3ffdae1df6e2ee45c37f5de40cbf5c25064cc174017cd6.
Firmware/IPC stayed 2, DART zero, no auxiliary RX, and no ACL PB rewriting.
Rust Wi-Fi returned fresh HTTP301/219 bytes after the exchange.

Named Bluetooth metadata is 722 bytes and permitted CPU RAM is 36,864 bytes
(arena plus nonce page). SID3 mapped bytes fell from 1,982,464 to 32,768.
Total live DMA/page-table reservations fell from 35,258,368 to 33,308,672 bytes.
The remaining firmware/control/Wi-Fi domains are still retained; this is not a
whole-chip cold-boot claim. The debug launcher needed an explicit stage-2 map
for the arena inside its own heap; a stopped Linux access exposed the omission,
which was fixed and retried before the successful data test.

Exact ACL enum and repeated tiny-ring channels (boot 94)
--------------------------------------------------------

The VHCI boundary now translates only the whole observed successful Connection
Complete value 0xf1 to standard ACL value 1. It does not strip upper bits from
other values or reinterpret undefined fields in failed connection events.
Unknown successful vendor codes are rejected in translation mode. The old
--probe-link-type-low-nibble spelling is an alias for this narrower behavior;
no individual meaning is assigned to the upper nibble.

The updated bridge passed sixteen sequential encrypted channels on one ACL
using the owned 32-KiB domain and four two-slot rings: 128 exact 512-byte echoes
each, 2,048 packets / 1,048,576 bytes total. This control used a generated
all-ones FF0B record before VHCI registration. Source SHA256 was
c09e064770c89d1b1654c63813382a5659bb632f765dbd966186b3b78ea91417.
Both zero and all-ones records have real data evidence; all ones is not made
a driver requirement. The opt-in --probe-startup-mask control sends only a
generated 128-bit record before a kernel HCI owner exists.

Linux-initiated Create Connection returned Command Status 0x0c (Command
Disallowed), despite being advertised in Supported Commands. Host Buffer Size /
flow-off, scan-off, simultaneous LE-host support, all-ones startup record and
zero default link policy did not fix the observed rejection. The link-policy
control was repeated after a confirmed empty connection list; an immediate Mac
disconnect request can leave the ACL active. Outgoing initialization/role work
continues; no outgoing connection or data success is claimed.

PF2 registers
-------------

================== ============================================================
BAR0 offset        Current interpretation and evidence
================== ============================================================
0x00               IPC state; 2 throughout successful owned HCI queries
0x04 / 0x08        Published host context IOVA, low/high halves
0x0c / 0x10        DMA address window base, low/high halves
0x14               DMA window extent
0x18               Firmware startup state; 2 ready
0x1000 + 4*q       Queue doorbell: publish next host ring index
================== ============================================================

The context points to device queue-index u16 values at +0x18 and host queue-index
u16 values at +0x28. These are u64 pointers, not embedded indices. The u32 at
+0x30 has queue count 22 in its upper half. Context offsets +0x7a/+0xa8 happened
to be the vector locations on the tested boot; code follows the pointers instead.
No register mentioned here is an Apple program or firmware instruction pointer.

Inline records
--------------

The header is 16 bytes: little-endian u32 length/flags, u64 pointer, u32 tag.
Length occupies bits 31:8; the pointer is all ones for inline packets. The body
starts immediately after the header. Observed transmit low byte 2 and posted
receive low byte 0 work; individual flag semantics remain to be probed. A tag
initially equals physical ring slot. Whether firmware requires that relation is
not yet tested; do not describe it as an arbitrary cookie until hardware proves it.

========== ================= ============ ========== ==========================
Queue      Role              Capacity     Slots      Evidence
========== ================= ============ ========== ==========================
1          HCI command TX    260 bytes    5          Owned standard HCI queries
7          HCI event RX      260 bytes    17         Matching command completions
2          ACL data TX  2592 bytes   9          Paired L2CAP echo on RF
8          ACL data RX  2592 bytes   9          Paired L2CAP echo on RF
========== ================= ============ ========== ==========================

Stride is header length plus capacity, including the 260-byte queue whose
stride is 276. One ring slot stays unused. RX completions occur in the posted
slot; replenish the next host producer slot, which is generally a different
slot. Reusing only the completed slot would destroy the ring's free-credit
invariant. Standard queries have wrapped both command and event rings.

The queue configuration's first word appears to encode capacity as mantissa
bits 23:16 shifted left by two plus exponent bits 31:28. The 0x00410001 mode
therefore yields 260 bytes; 0x30510001 yields 2592. The 0x00b50001 posted header
observed elsewhere yields 724. This is a cross-size inference; equivalent mode
encodings have not yet been tested through owned queue registration.

HCI and Linux integration
-------------------------

Standard Read Local Version (0x1001), Read BD_ADDR (0x1009), Read Local Supported
Commands (0x1002), Read Local Supported Features (0x1003), and Read Buffer Size
(0x1005) all returned status zero using Python on the M5. Firmware/IPC remained
ready and DART error status zero. Only the two-byte vendor event discriminator and frame extent are recorded;
the rest of each vendor event body is discarded unread.
No vendor startup command is replayed. No link keys or pairing secrets are logged.

Linux VHCI unconditionally advertises synchronous-flow-control transport support.
The command/ACL prototype does not implement SCO; a default-preserving adapter
parameter, hci_vhci.sco_flow_control=0, omits that transport quirk. Without it,
HCI Write Synchronous Flow Control Enable (0x0c2f) returns Command Disallowed
(0x0c), aborting Linux controller initialization. This result does not establish
that the physical chip lacks SCO or that the queue transport failed.

Captured and retained state
---------------------------

The boot-82 minimal private handoff contained named queue geometry and only
translations for context, four inline rings and our boot nonce: 1,303 JSON bytes
and 69,632 bytes of allowed CPU RAM access. It contains no packet bodies,
firmware workspaces, keys, or replay command streams. The larger firmware/DART
allocations remain reserved; this metadata reduction does not reclaim them.

Connection-type extension
-------------------------

On multiple incoming Classic links, Connection Complete (event 0x03) carried
status zero, handle 1 and link_type 0xf1. Linux expects standard ACL type 0x01;
with 0xf1 it leaves the pending connection handle unset (0xf00), then drops ACL
frames for actual handle 1. The opt-in Python --probe-link-type-low-nibble
experiment translates only this standard event field at the VHCI boundary.
It caused pairing to succeed and a 65-byte encrypted L2CAP challenge to echo
exactly between the M5 Linux system and the local Mac Mini. Hardware queue
payloads are not modified. Upper nibble semantics remain UNKNOWN; do not claim
this is padding or delete it from protocol documentation.

The reference request to accept the connection asked for central role (0),
then Role Change returned status 0x21 and peripheral role (1). Connection
Complete succeeded afterward. This role result is independent of the link-type
translation; it is not the observed cause of dropped ACL packets.

Incomplete stability experiments
--------------------------------

The first paired link exchanged an exact 65-byte L2CAP challenge. Reopening a
custom L2CAP channel subsequently completed signaling configuration on both
sides, but the Mac initiated disconnection before application data. Changing
Linux's disable_ertm option removed the optional RFC field and did not resolve
this. Classic L2CAP echo control requests got five replies, but those replies
contained zero payload bytes and are not application integrity proof. RFCOMM
connected at security level 2 using the Mac's advertised Serial Port service;
the attempted stream echo timed out. Boot 85 later passed 2,000 exact 512-byte
L2CAP round trips after a normal ACL reconnect, as recorded below; reopening
under the same ACL remains unresolved.

A handover from the running Python driver left the Bluetooth function at IPC
state 2, firmware state 3, DART error zero; Wi-Fi traffic still worked. The exact
cause is unknown. The initial tag experiment failed its ready check before any
modified tag was submitted. A later controlled bit-16 trial did execute and
failed; its evidence is recorded in the descriptor-tag section below.
PCIe advertises function-level reset support, but reset clears the function's
memory decode enable: config restoration must precede BAR access. A failed
post-reset BAR read caused a host bus fault during recovery. Do not blindly
retry reset or infer a firmware/program cause from this result.

Auxiliary receive queue
------------------------

A fresh handoff showed queue 12 with equal host/device indices and no posted
credits. Its observed inline geometry is 1,024-byte capacity, 128 slots. A
body-free Python maintenance path refills only free slots, discards completions
without reading any auxiliary body, and reposts fresh typed headers. During
50 successful standard HCI queries on the bare proxy, it handled 79 auxiliary
records and both firmware/IPC remained ready with DART error zero. This proves
an active auxiliary RX stream. It does not yet prove that neglecting this
queue caused the previous firmware failure or explain its record bodies.

Including this queue expands the private named handoff to 1,990 bytes and its
CPU RAM access set to 204,800 bytes. All firmware/DART allocations still remain
reserved. No auxiliary record body is retained or replayed.

Descriptor tag experiments
--------------------------

Baseline host-side Python completed 50 standard queries while maintaining
queue 12. The next query changed only the submission tag's bit 16 from zero
to one, preserving payload, ring slot and all indices. It emitted a vendor
error event (51 bytes total; only discriminator bytes 0x0e,0x0a inspected) and
then failed the ready check. No successful tag-variation result exists; upper
tag bits must not be treated as an arbitrary cookie. The failed probe did not
record the individual state register values before reset, so the exact failing
state value on that run is not established.

Completed command, event and auxiliary headers gained bit 26 (0x04000000)
after device processing, while host-posted headers had that bit clear. This
supports a descriptor completion marker, not an HCI packet-type interpretation.
Its full semantics and the remaining tag field widths are still being probed.

An isolated PCIe function reset was repeated with config-only inspection first.
It reset the command register to zero and BAR0's address to zero (type bits
0x0c remained). Restoring saved BARs and enabling memory decode, with bus
mastering still disabled, is required before attempting interface MMIO.

Own context recovery and address programming
--------------------------------------------

Restoring PCI configuration after an isolated reset made PF2 report firmware
ready (2), IPC unpublished (0), while Wi-Fi stayed ready and DART error remained
zero. Python then zeroed the host context and five ring extents, published a
new header, and generated queue registrations for only 12/7/1/8/2. All five
registrations and the five standard HCI queries completed successfully. These
host structures are initialized by our code; old queue records are not replayed.
The existing reserved allocations and DART mappings are still retained.

Reset clears the controller address. ADT bluetooth/local-mac-address matches
the address originally returned by standard Read BD_ADDR. A controlled FC06
command with six generated address bytes changes standard readback. FC06 uses
display/network byte order, while Read BD_ADDR returns little endian. Trying
both byte orders and restoring the actual factory address established the
meaning and order of all six parameter bytes. No reference vendor body is copied.

Healthy-function reset can initially return PCIe CRS (vendor ID 0x0001), unlike
the immediate ready response seen after firmware failure. Poll configuration
until the real identity returns; never read a BAR while CRS or decode is absent.
Reset from a healthy active context has not yet been made reliable; recovery
from the subsequent failed state and context-word minimization remain ongoing.

Control completion format field resolved
----------------------------------------

With context word +0x50 cleared, global control indices still advanced, while
completions appeared at base+0 and base+16, not base+0 and base+32. Both records
had the expected 0x401 format and length/slot fields. Using a 16-byte stride
completed all remaining queue registrations, address programming and ten more
standard queries with firmware/IPC ready and DART error zero. Setting bit 26
selects the previously observed 32-byte global-control completion stride.
The common builder now takes completion_entry_bytes rather than an unnamed
word. The new owned controller can use 16-byte control completions.
Inline packet descriptor tags still gain bit 26 with the context field clear;
that tag status bit is independent of the global control completion format.

Reference-context all-RX baseline
---------------------------------

Boot 85 preserved reference initialization and maintained receive credits on
queues 7, 8, 9, 10, 11, 12 and 13. Auxiliary geometries are respectively
724 bytes/9 slots, 256/17, 268/9, 1024/128 and 260/17 for queues 9 through 13.
Only queue 12 produced auxiliary records during standard queries, pairing and
initial application traffic. All auxiliary bodies remain unread. Fifty bare
Python standard queries passed, then Linux Python VHCI initialized, paired the
exact authorized Mac Mini, and received/echoed one generated 512-byte packet.
The initial Mac helper did not report the echo, so that test was not a verified
round trip. Firmware remained 2 and DART error zero through over ten minutes.
This baseline does not establish that the additional RX queues fixed the own
context radio failure; reference vendor initialization is another difference.

The named handoff is 2,678 JSON bytes and permits CPU access to 245,760 bytes of
context/rings/nonce RAM. All live DMA and page-table reservations still total
35,291,136 bytes. These are distinct quantities; no allocation has been freed.
At initial context publication, the first 64 bytes of the auxiliary host
context region at +0x3d8 were zero. Known BAR initialization writes were 107c=1,
window/context publication, 107c=2, matching the own Python handshake. Many
reference vendor command opcodes remain unexplained; only their headers and
parameter lengths were captured, with no vendor bodies retained or replayed.

ACL packet-boundary probe
--------------------------

Standard Linux ACL sends on the Classic link used PB=0 (first packet,
non-automatically-flushable); incoming Mac packets used PB=2 (first packet,
automatically flushable). Linux v7.3 hci.h defines LMP_NO_FLUSH as feature byte
6 bit 6, and l2cap_core.c selects PB=0 when that capability is advertised.
The controller's observed feature byte 6 is 0x7b, which includes this bit.

An opt-in, owned JSON probe file lets Python change only start-packet PB bits
13:12 of outgoing ACL headers to 0 or 2. Handles, broadcast bits, length and
payload are preserved; continuation fragments are never changed. Setting PB=2
before a fresh pair produced over 1,100 generated 512-byte packet round trips
verified by the Mac helper. Its 60-second harness deadline ended the run before
2,000 packets. A later 2,000-packet run and same-context A/B comparison are
recorded below; paired channel reopening remains unresolved. Do not yet claim a hardware
capability correction or clear LMP_NO_FLUSH by default based on this experiment.
The new Python control is experimental and does not alter the default path.

Boot 85 bulk verification and counterexample
---------------------------------------------

After a normal peer ACL disconnect/reconnect, both endpoints completed 2,000
512-byte challenge/echo exchanges: 1,024,000 bytes, Mac exact comparison true,
Linux SHA256 e00a4a2656e272da1f2ab1da68e1d009f46e5c97619c9c2634f3274a08eccba3.
The live PB override was changed from 2 back to null during that same transfer.
ACL submissions continued with the override disabled, while the override-change
counter stopped advancing. PB=0 is therefore usable for application data;
rewriting it is not required and no advertised capability should be cleared.

A subsequent channel reopen with the same ACL still failed with Mac API status
0xe00002bc before application data, even with sniff disabled in the Linux link
policy. A normal ACL reconnect again allowed a 256-packet 512-byte test to pass
with unmodified PB=0. Disabling ERTM before that new ACL is an ongoing
interoperability probe, not yet a proven fix. All these tests used the same
Python hardware transport and preserved reference firmware initialization.
Firmware/IPC stayed ready and the Rust Wi-Fi interface still returned HTTP301
and 219 bytes from google.com. Bluetooth Rust is deferred pending stability and
remaining protocol/initialization work.

Follow-up reopen controls
--------------------------

Disabling ERTM before a new ACL gave a successful 256 x 512-byte exchange on
its first channel, but reopening still failed before data. Changing the test
PSM from 0x1001 to 0x1005 also failed on that existing ACL. Explicitly acquiring
the public Mac framework connection session again did not fix it. Local Mac
runtime logs report a successful Classic channel connection callback followed
immediately by a disconnect request from the test client session. These are
host API/runtime observations, not Apple binary inspection; the cause remains
unresolved and should not be assigned to a transport bit without a causal test.

Own-context controls on boot 86
--------------------------------

With all 13 generated queues, the reference DMA window, MSI routing and PCIe
Device Control restored, reset plus Scan Enable=0 stayed ready for 12 seconds.
Changing only to page scan (Scan Enable=2) returned Command Complete status zero
and then changed firmware state to 3 before the next query could be submitted.
Inquiry scanning is therefore not necessary to trigger this failure.

A generated standard Host Buffer Size announcement was accepted: controller
Read Buffer Size returned ACL MTU 2586, SCO MTU 208, 8 ACL buffers and 1 SCO
buffer. The reference announcement uses ACL MTU 2590 (2586+4), SCO MTU 255 and
one buffer of each kind. Sending those named fields alone, and then with
explicit Controller To Host Flow Control=0, did not prevent the page-scan
failure. These are optional Python probes, not required driver initialization.
The extra four bytes remain an observed reference choice, not a claim that the
standard MTU definition includes the HCI ACL header.

To test functionality independently of the state check, one bounded Read Local
Version query was submitted while permitting firmware state 3, retaining all
IPC/DART/context/access guards. It timed out after three seconds; command queue
host index 4 stayed ahead of device index 3. State 3 was accompanied by an
actual failure to consume a command, not merely rejection by Python's guard.

A subsequent full standard host-support trial did not run because another
function reset left PCI configuration returning CRS. The timeout guard stopped
before BAR access. No successful own-context RF test exists yet.

Named transfer-footer geometry
--------------------------------

The primary Asahi Linux hci_bcm4377.c structure identifies request byte 0 as
Create Transfer Ring, byte 1 as extra header words and byte 2 as footer words.
The N1 trials establish the scaling extension: footer capacity is
``mantissa * 4 * 2**exponent``, with mantissa in bits 23:16 and exponent in
bits 31:28. Our generated core requests reserve no extra header words. This
provides names for the verified size fields without replaying captured words.
The separate completion header's second byte remains an observed format value;
no unsupported flag or header-length meaning is assigned to it.

Controlled host-page packing probe
-----------------------------------

The control API permits an explicit bounded frame area/count, with overlap
checks against published vectors, control rings and declared data rings. A
single frame can be reused because submission waits for the exact prior
completion. Unit coverage exercises that reuse across a control-ring wrap.
On boot 98f, a single frame at context+0xf80 passed all four registrations and
standard HCI queries with the original 32-KiB arena. A controlled layout places
the core rings at/after context+0x1000, preserving the full 2586-byte advertised
ACL MTU and fitting all four two-slot rings inside one 16-KiB DART page. This
packed layout passed the fresh pairing and 2,000 changing-payload echoes
recorded at the top of this document.
No unknown auxiliary contents are read to choose the layout.

Cold native Linux checkpoint257..258
--------------------------------------

The current Rust PCI path owns a fresh16KiB coherent context/ring page and a
separate PF2 copy of the current shared mswb output. It registers command/event
rings, reads ACL MTU and generates both ACL rings before HCI registration.
Standard DT local-bd-address replaces retained identity metadata. CENB handoff,
nonce validation and PageReserved mapping are no longer Bluetooth requirements.
The generated footer encoder, one-frame packing and reset completion ordering
are ported from the earlier qualified Python work.

Native257 completes normal HCI initialization, an actual Classic connection
and remote-information replies while native Wi-Fi DHCP/curl work. Native258
pairs with an empty Linux store and a removed previous host test bond. Standard
encrypted Linux SDP sockets verify level2/key16 and validate2000 changing
transaction responses,258000 response bytes. HCI RX/TX errors remain0; native
ACL totals2071/2075 repeatedly wrap the two-slot rings. Twenty concurrent
Wi-Fi HTTP200 downloads also finish. COLD-START.rst records exact counts, digest,
teardown and qualification limits.

The Mac SDK512-byte echo and raw l2ping tests remain unsuccessful; do not
substitute exit status for verified replies or claim full-size native echoes.
Three firmware configuration inputs still lack independent sources, and chip
power/reprobe, wider payload/profile, LE/SCO and PF2 IRQ service need work.
The observer sees kernel HCI after driver normalization; its standard connection
link-type values are not raw firmware field evidence. Linux source stays clean
GPL-2.0-only and no Apple executable payload was inspected.
