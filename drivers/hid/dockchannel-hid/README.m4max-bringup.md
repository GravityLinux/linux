# M4 Max MTP qualification

M4 Max J616c keyboard and trackpad now work with the shared M4 DockChannel
HID transport. The DT uses the Max's resolved physical resources, rather than
raw arm-io bus addresses: MTP at `0x514600000`, DART at `0x514800000`, and
DockChannel IRQ registers at `0x514b14000`. The mailbox IRQ pairs are ordered
as empty/not-empty, and SID aliases 1/2/3 share canonical translation SID 0.

The transport comes from `niklassheth/linux`, `bits/090-spi-hid` at
`91544d13c2c7154ed5dde9b8287a0351b979a4f5`. DART SID remapping comes from
`bits/020-dart` at `f2fefc6b963f059979fff346f90cd2353afb818d`.
The existing GPU RTKit extensions are retained. No separate Max HID protocol
implementation was needed.

The trackpad requires `apple/tpmtfw-j616c.bin`. The tested package was generated
with Asahi's standard multitouch converter from the J616c restore component
in macOS 26.6.2 build 25G83, after checking its BuildManifest SHA-384 digest.
Personality `C1FD2,2` matches this board. The Linux firmware file SHA-256 is
`7eb84312cb4ea74833ce9567ba8c92bc42a0c26c5c34d6bdfaa5cb65abfe4b26`.
Firmware bytes are kept in the local test artifacts, outside this source tree.
No Apple binary was disassembled.

## Hardware results

Linux boots all 16 CPUs and opens both real input devices. The operator's
keyboard sweep produced 77 presses and 77 releases over 76 distinct keycodes,
with balanced pairs. After adding trackpad firmware, the operator's test
produced 314 touch reports, activity in slots 0 and 1, contact endings, three
two-finger sequences and eight click/release pairs. Subsequent keyboard input
also passes. There are no dropped-input notifications or kernel/DART faults.
These tests qualify raw input; compositor gesture/scroll policy was not tested.
Suspend/resume remains outside scope.

Image SHA-256: `8f1fbf5dc66e0a99b93227b9141d3b8a265746ab153b06d6ca7cb525451812cb`.
The configuration enables DART, DockChannel, RTKit helper, input evdev, Apple
HID and magicmouse, including the Apple HID LED dependencies.

## Test reproduction

The userspace reader is `tests/mtp-input-watch.c`, built with:

```sh
aarch64-linux-gnu-gcc -static -Os -Wall -Wextra -Werror \
  mtp-input-watch.c -o mtp-input-watch
```

Install it in an initramfs with the board trackpad firmware under
`/lib/firmware/apple/tpmtfw-j616c.bin`. It reads MTP evdev devices and prints key,
touch, click and dropped-event records. It does not grab input exclusively.

Bootloader-checkout evidence is indexed by
`references/m4max-mtp-evidence-20261028.json`. Complete images, initramfs,
firmware provenance, input logs and monitor source are retained under
`build/m4max-devices/mtp-boot-v1/`. The successful run is `boot-4`; the earlier
keyboard-only test is `boot-3`. Earlier failed launches remain retained.

The monitor initializes only the MTP DMA filter, reserves opaque MTP firmware
segments and places the FDT after the Image header's full memory size,
including BSS. Production loader integration must preserve firmware memory.
The test uses all CPUs, spin-table startup and the existing WFxT override.
Device access remains Max RID 0 only; Neo access is revoked.
