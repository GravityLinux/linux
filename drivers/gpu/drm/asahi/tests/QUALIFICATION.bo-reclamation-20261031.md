# Shared caller BO reclamation, 2026-10-31

The reported leak reproduces in the shared G16C/G17P driver. The session's
deduplicated backing vector never released caller BOs, and inherited CPU-map
caches retained every previous client's vmaps. This change removes those two
unbounded holders. Firmware/private `Memory.allocations` has a separate policy.

Base: `72d50df7b8ee`, branch `experimental/gpu-performance-7.3`, worktree
`/Users/user/gravity/linux-gpu-performance-7.3`. Compiled remote source is
`/home/lab/linux-gpu-performance-7.3`; build is
`/home/lab/linux-gpu-performance-7.3-build` on `lab@192.168.170.255`.

Hardware qualification is **M4 Max RID0 only**, Mac16,5/J616c, chip 0x6041,
serial JPWWJRX2N0. All 16 CPUs stayed online under the existing Linux hardware
passthrough monitor. Neo hardware was not accessed. The changes apply to the
common paths; firmware layouts, control sequences and device identity are
unchanged. No Apple binary disassembly occurred.

## Ownership changes

Each successful cache-maintenance pass keeps only its current buffer set in
`CpuMaps`. Existing RBTree nodes move without allocation; stale mappings and
their GEM references drop after releasing the tree mutex. A different lease
can recreate a missing map. Executable clients and completion/preparation
owners hold their BO references independently of this cache.

The common scheduler reaps backing after at least 64 new BO enrollments,
under the publication mutex and after CPU completion signalling. It requires a
Running session with no pending render/compute/native work, independent ticket,
active preparation/CPU lease, unconsumed cleanup/maintenance receipt, or live
wrapped transport flight. Compute reconfiguration is checked separately from
the queue's execution fences: an idle execution queue can still have pending
maintenance controls. Failed/experimental native sessions retain backing.

Only GEM objects whose reference count is exactly one can be released. Their
only owner is the retained vector; handles, DMA-BUF exports/imports, executable
clients, leases and vmaps prevent eligibility. A global UAT invalidation precedes
the first release. Identity keys are removed before final GEM puts. No
invalidation is issued for a scan with no eligible objects. The module option
`asahi_neo.release_backing=0` retains backing permanently for comparison.

## Reproduction and results

`bo-reclaim.c` is the supplied allocation/copy probe with error checking, a
native Zink renderer check, explicit context teardown and a full independent
word-by-word copy oracle. Each copy uses a PID/iteration-dependent pattern.
Separate processes prevent Mesa's per-context BO reuse from hiding the leak.
The actual Vulkan binary is the previously qualified production DSO with SHA256
`c597d37bd2684873a66ac620eb5ca91f3f643f648e82a00f0d4660d270ad5fcf`.
The existing Mesa 26.3 Zink frontend reports git `50cfaf889f`; this string is
not the version of that separately installed Vulkan binary.

Before the fix, copying two 16 MiB buffers left 46,864 KiB unevictable after
exit. Further clients each added approximately the same amount. The no-copy
control left zero unevictable memory. Four 16 MiB copy clients and one 64 MiB
copy client left 326,624 KiB pinned. GPU unbind released all but 192 KiB.
The baseline used a larger runtime image; only within-boot deltas are comparable.

Final kernel: `Image-reclaim2`, SHA256
`969007b4f66b65cd59c0eaaf34e2cf4df6e43ce71f43270c5e0a473a1bc7c76d`.
Linux 7.3, LLVM 21, Rust 1.96, 16 KiB pages, high-resolution timers enabled,
HZ=250. All seven modified production source files match the compiled remote
source hashes in `reclaim2-source.json`.

| Final test | Result |
| --- | --- |
| 32 separate clients, two fresh 64 MiB buffers each | 4 GiB of unique large-buffer allocations; Shmem samples 633,040–634,704 KiB, spread 1,664 KiB |
| 60 subsequent 1 MiB clients | Shmem samples 381,168–415,376 KiB; no accumulating trend |
| 30 alloc/copy/delete cycles in one context | Every complete copy passed; subsequent small clients reclaimed the batch |
| Total GL processes / fully checked copies | 123 / 151, including concurrent presentation and drain clients |
| Two Vulkan clients, GPU → DMA-BUF → DCP | 256 frames / 255 flips, 3456x2160, 119.514 fps |
| Same presentation while 12 GL clients allocate/copy/exit | 256 frames / 255 flips, 120.000 fps; all copies/pixels/guards passed |
| Native 200000-instance PB pressure | Four frames, full color/arguments/query/294884 guards/six timestamps/prior poison; explicit and file-close VMs at 0x7000000000 and 0x7100004000 |
| Native memory UAPI | 6313 checks passed |
| Shutdown and kernel log | GPU unbind RC0, CPUs0–15 online; no GPU fault, failed asynchronous submission, firmware crash or flip timeout |

The display tests use normal native GPU DMA-BUF export/import and physical DCP
scanout. They are a presentation/lifetime regression benchmark, not a demanding
game, compositor or Vulkan conformance result. The game task was cancelled
before launching any game or compositor.

The actual-method host test covers 14 busy/batch/disabled gates, live-reference
preservation, identity removal, invalidation before final puts and deferred retry.
It also executes the actual compute/transport idle predicates. Existing shared
leaf switching, compute-admission and historical identity/constructor checks pass.

## Limits and evidence

Reclamation remains conservative and batched. It requires whole-session idle;
continuous GPU work can postpone release. Up to a batch and retained owners'
current working sets can remain pinned. Memory did **not** immediately return
to its boot baseline: after the final mixed workload and small compute drain,
Shmem was 492,896 KiB versus 342,384 KiB at boot, then 347,056 KiB after GPU
unbind. This qualification establishes bounded churn in these workloads,
not immediate release of every BO after file close or reclamation under every
continuously busy schedule. Per-owner reclamation is still future work.

The safety argument assumes firmware does not continue accessing caller backing
after its tracked executable roots and leases are gone. These tests support the
quiescent/invalidation rule; they do not prove all possible firmware behaviour.

Artifacts:
`/Users/user/gravity/bootloader-m4maxmbp/build/gpu-bo-reclaim-20261031/`.
`audit-final.py` validates receipts, memory plateau, source/binary hashes and
`results.json`. `baseline-host.log` preserves the unfixed comparison;
`reclaim2-host.log`, `final-leak-suite.log`, `memory-samples.json` and
`final-gpu-dmesg.log` preserve the final run. Intermediate `reclaim1` includes
one incorrectly invoked memory test using its default Neo identity assertion;
the corrected `--g16c` invocation and the final kernel both passed all 6313 checks.

The final runtime includes `/qual/run-leak-suite`, `/qual/run-gl`,
`/qual/g17p_drm_memory`, `/qual/pb-tests` and `/qual/dcp-present-pipeline`.
`package-qualification.py` records the packaged programs and hashes.
Use the authorized Max-only boot procedure in the bootloader checkout; do not
run historical dual-device/Neo controllers.
