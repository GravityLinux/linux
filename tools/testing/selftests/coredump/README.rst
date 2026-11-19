coredump selftest
=================

Background context
------------------

`coredump` is a feature which dumps a process's memory space when the process terminates
unexpectedly (e.g. due to segmentation fault), which can be useful for debugging. By default,
`coredump` dumps the memory to the file named `core`, but this behavior can be changed by writing a
different file name to `/proc/sys/kernel/core_pattern`. Furthermore, `coredump` can be piped to a
user-space program by writing the pipe symbol (`|`) followed by the command to be executed to
`/proc/sys/kernel/core_pattern`. For the full description, see `man 5 core`.

The piped user program may be interested in reading the stack pointers of the crashed process. The
crashed process's stack pointers can be read from `procfs`: it is the `kstkesp` field in
`/proc/$PID/stat`. See `man 5 proc` for all the details.

The problem
-----------
While a thread is active, the stack pointer is unsafe to read and therefore the `kstkesp` field
reads zero. But when the thread is dead (e.g. during a coredump), this field should have valid
value.

However, this was broken in the past and `kstkesp` was zero even during coredump:

* commit 0a1eb2d474ed ("fs/proc: Stop reporting eip and esp in /proc/PID/stat") changed kstkesp to
  always be zero

* commit fd7d56270b52 ("fs/proc: Report eip/esp in /prod/PID/stat for coredumping") fixed it for the
  coredumping thread. However, other threads in a coredumping process still had the problem.

* commit cb8f381f1613 ("fs/proc/array.c: allow reporting eip/esp for all coredumping threads") fixed
  for all threads in a coredumping process.

* commit 92307383082d ("coredump:  Don't perform any cleanups before dumping core") broke it again
  for the other threads in a coredumping process.

The problem has been fixed now, but considering the history, it may appear again in the future.

The goal of this test
---------------------
This test detects problem with reading `kstkesp` during coredump by doing the following:

#. Tell the kernel to execute the "stackdump" script when a coredump happens. This script
   reads the stack pointers of all threads of crashed processes.

#. Spawn a child process who creates some threads and then crashes.

#. Read the output from the "stackdump" script, and make sure all stack pointer values are
   non-zero.

NT_FILE file-offset regression (manual)
--------------------------------------

``nt_file.py`` maps a retained backing file at process-page offsets
0, 1, 2, 3, 4, 5 and 7, saves their byte offsets from ``/proc/self/maps``
in a JSON manifest, and terminates itself with SIGSEGV. Its ``check`` mode
reads the kernel-generated ELF64 core's NT_FILE and compares every test
mapping's ``file_ofs * page_size`` with the saved byte offset. Keep the
manifest and backing file until analysis finishes. Run in a scratch directory
with enough space for a Python core; core collection must be enabled and the
hard RLIMIT_CORE must permit the dump.

On a system using systemd-coredump, from this directory::

    testdir=$(mktemp -d)
    python3 nt_file.py crash "$testdir/native.json"
    # Expected SIGSEGV. Use the PID printed by the helper below.
    coredumpctl --output="$testdir/native.core" dump PID
    python3 nt_file.py check "$testdir/native.json" "$testdir/native.core"
    eu-unstrip -n --core="$testdir/native.core"
    gdb -nx -batch -c "$testdir/native.core" -ex 'bt' -ex 'x/i $pc'

For a local-file core_pattern, use the kernel's resulting core file instead
of coredumpctl's export. Do not use GDB's ``generate-core-file``: that would
test GDB's note writer rather than the kernel's.

On a P3S 16K kernel, first run the above natively (expect page_size=16384),
then use the existing P3S launcher to run the same Python interpreter in
the 4K personality::

    make -C ../arm64/p3s exec-4k
    ../arm64/p3s/exec-4k python3 nt_file.py crash "$testdir/p3s.json"
    # Expect page_size=4096. Export the printed PID's core as above.
    coredumpctl --output="$testdir/p3s.core" dump PID
    python3 nt_file.py check "$testdir/p3s.json" "$testdir/p3s.core"
    eu-unstrip -n --core="$testdir/p3s.core"
    gdb -nx -batch -c "$testdir/p3s.core" -ex 'bt' -ex 'x/i $pc'

The launcher must itself have native-compatible ELF alignment (its Makefile
already supplies this). The interpreter and its libraries must support the
4K personality. With matching binaries available, GDB should resolve the
faulting PC to libc's ``kill`` wrapper and show its syscall return path;
eu-unstrip should identify the interpreter and its loaded libraries. Optional
debuginfo provides further source/line detail. Compare with a live GDB run
on the same binaries if instruction-level confirmation is needed.

The 4K test includes offsets 4, 8, 12, 20 and 28 KiB, so it detects both
missing slice bits and an unconverted 16K page index. Before the fix, e.g.
20 KiB is recorded as vm_pgoff=1 and interpreted as 4 KiB; after the fix it
is recorded as 5 and interpreted as 20 KiB. The native 16K test must keep
the original offsets and advertised page size.
