.. SPDX-License-Identifier: GPL-2.0-only

P3S research smoke tests
=======================

This downstream kernel series gives an explicitly selected process 4K
translations on a native 16K arm64 kernel. It requires
``CONFIG_ARM64_PER_PROCESS_PAGE_SIZE``, 47-bit native VA and disabled
AArch32 ``COMPAT``. 4K processes use a 39-bit address space. Keep the boot
default ``p3s_4k=off`` and select only the test process.

The new personality flag ``ADDR_PAGE_SIZE_4KB`` (0x40000000) requests 4K
geometry at the next exec. It does not convert the current mm. Fork
inherits the parent's geometry. Exec reevaluates the personality once
the system has reached ``SYSTEM_RUNNING``. This is a downstream research
interface, not an upstream ABI commitment.

Build the static tests explicitly::

  make -C tools/testing/selftests/arm64/p3s CC=aarch64-linux-gnu-gcc

The launcher has 64K ELF alignment so the native 16K loader can run it.
The memory smoke has 4K ELF alignment to exercise P3S ELF loading.
The futex test has 64K alignment so its mixed-mode parent can start natively.
They are extended programs, excluded from automatic native test execution.
On a 16K host, run::

  getconf PAGE_SIZE
  ./exec-4k ./test-4k
  getconf PAGE_SIZE

The host must remain at 16384. The selected process must report sysconf
and AT_PAGESZ=4096. It checks single-quarter R/NONE protection with isolated
fault probes, partial unmap/replacement, fork COW, incongruent file/VA
slice offsets, shared aliases and kernel I/O. A passing run ends with
``PASS: warthog 4K process smoke``. The tests disable core dumps.

The separate ``test-futex-slice`` program checks shared futex keys::

  ./exec-4k ./test-futex-slice
  ./test-futex-slice --mixed ./exec-4k

It issues a shared FUTEX_WAKE on a lone 4K file slice with a PROT_NONE
native-aligned neighbor. Correct Linux semantics return 0. A forked waiter
uses a read-only alias with a different virtual slice offset. The waker
must match the same backing byte and must not wake a different slice of
the same native page. The mixed-mode case starts a native parent and
executes a 4K child, checking that their inode-based keys agree. Each wait
has a timeout. The original pinned warthog series returns EFAULT on the
lone slice; the downstream futex fix uses the backing page and offset.

The separate ``test-exec-slice`` program checks executable file slices::

  ./exec-4k ./test-exec-slice

It maps file quarter 1 at virtual quarter 3 with RX permission and executes
a small function. It changes the slice to RW, triggers private COW, restores
RX, and checks that an adjacent private alias and its protection do not
affect the code. Removing execute permission must fault in an isolated
child; restoring it must allow execution again. The backing file must retain
its original bytes. This covers ELF-like file/VA incongruence and protection
changes without depending on an emulator or graphics library.

The same test-4k.c can be built statically for x86-64 and launched through
FEXInterpreter using exec-4k. No muvm, KVM, or preload adapter is involved.
Dynamic x86 applications additionally need FEX's read-only mounted or
extracted rootfs. Keep FEX settings on a writable temporary directory when
the host root is read-only. Avoid mixing page modes through an existing
FEXServer while diagnosing initial launches.

These tests do not cover memory pressure/swap, multithreaded revocation,
userfaultfd, generic device pinning or graphics. THP/KSM/hugetlb are disabled
for 4K processes; fixed mremap requires matching offsets modulo 16K.
Success is an initial process milestone, not a production-kernel result.
