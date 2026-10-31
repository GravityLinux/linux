# Shared G16C/G17P render performance, 2026-10-31

> Performance conclusions superseded by
> [the two-client latency investigation](QUALIFICATION.two-client-latency-20261031.md).
> The original lab used coarse timers and a serial benchmark whose final CPU
> validation scan was timed. The current pipelined DMA-BUF/DCP test reaches
> 119.5 fps; the historical receipts below are preserved unchanged.

Code is on `experimental/gpu-performance-7.3`, based on unified kernel
`6c6a2497cac2` (its GPU source matches `replacement/210-gpu`).

- `2592a4064892`: shared GPU scheduler, VM reads, immutable leaf switching,
  owned-copy fallback, bounded Max PB growth placement, tests and benchmark.
- `01024cfbaaab`: DCP commit tail waits for firmware flip completion.

The changes are shared by Max and Neo. Hardware access and validation were
**M4 Max only**: RID 0, Mac16,5/J616c, chip 0x6041, verified serial
JPWWJRX2N0. All 16 CPUs stayed enabled with the existing spin-table boot path.
No Neo hardware was accessed. No Apple binaries were disassembled.
No firmware wire layout, identity or userspace variant was changed.

## Measurements

This is Linux 7.3 under the existing 16-CPU monitor with physical GPU and DCP
passthrough, without optional MMIO tracing. These are not bare-metal desktop,
KWin, glmark2, Vulkan-conformance or game measurements.

The identical benchmark renders through production Mesa Vulkan, GPU-copies
into exported DMA-BUFs, imports those same allocations into DCP, and waits for
page-flip events. Two independent Vulkan clients alternate at the selected
preferred 3456x2160 mode (pixel clock 1042130 kHz). There is no CPU framebuffer
copy. Initial/final images and guards have complete CPU oracles, with sparse
fresh-background checks between them. Three warmup passes are excluded.

| Kernel | Frames / flips | Measured fps | Mean CPU record/submit-to-fence |
| --- | --- | --- | --- |
| Baseline | 32 / 31 | 5.279 | 177.150 ms |
| Optimized copying | 32 / 31 | 10.297 | 83.798 ms |
| Shared leaf switching, original DCP tail | 32 / 31 | 11.366 | 76.265 ms |
| Final GPU + corrected DCP tail, first run | 32 / 31 | 8.287 | 107.410 ms |
| Final GPU + corrected DCP tail, repeat | 32 / 31 | 8.691 | 98.139 ms |
| Final kernel, longer run | 128 / 127 | 9.899 | 94.115 ms |

The GPU-only change was 2.15 times the baseline in the comparable short run.
The final kernel, including correct display retirement, measured 1.57–1.65
times the baseline in its short runs. Do not silently attribute the faster
pre-DCP-fix number to the final kernel. The longer run has a different sample
count and is not a directly matched baseline comparison. Timing includes IPC,
oracle checks and event waits; the submit-to-fence column is not a GPU hardware
timestamp duration. There is substantial monitor/scheduling overhead.

## Correctness and lifecycle

The final kernel passed eight independent presentation invocations:
272 rendered frames and 264 page-flip events total, including four consecutive
client restarts, a 128-frame run, and presentation after grown resources moved
between private windows. Every completed test required correct pixels/guards.
No retry loop hides page-flip errors in the benchmark.

The native compute-to-indirect-render fixture passed four 200000-instance
pressure frames across two VM/file lifetimes. Real firmware requests grew the
PB from 23 through 33/43/53 to 63 blocks. Complete argument/color/query outputs,
294884 guards per frame, six ordered timestamps and prior-bank poison checks
passed. The grown pool was reused at the unaligned private origin
0x7100004000 after starting at 0x7000000000, without another growth sequence.
Published block IDs keep their original backing across window moves; growth
remains bounded by its retained arena. Subsequent display presentation passed.
Retained backing was 691453952 bytes after growth and stayed at that value at
the next private-window reuse, with one pool and one queue slot.

The native memory-UAPI test passed 6313 checks with the real G16C identity.
GPU driver unbind returned zero; CPUs 0-15 remained online. The monitor was
then stopped and Max rebooted/chainloaded back to the development proxy.

Host actual-source tests passed:

- Complete UserVm and leaf-switch code: frozen caller data, private/mixed
  leaves, borrowed backing surviving snapshot drop, new top-level ranges,
  cancelled/stale/colliding plans, 60 switches, 40 copy-fallback/share cycles,
  115 injected allocation refusals and 85 successful preparations, bounded
  tables, batched reads, cache hints, 20000-record sorting and allocation-failure
  heapsort fallback.
- Rebind planning: 259 allocation refusals, 41 successful plans, 15 private
  relocation cases, guards, ownership, break-before-make and ASID invalidations.
- Growth tables: 93 allocation refusals and 35 successful plans.
- Cold/warm root/descriptor preparation; both engines' borrowed-to-owned
  preparation race; physical-owner replacement; growth admission; producer
  receipt leases; 128 complete growth-reply byte/layout comparisons.
- Historical Session/probe/identity/ioctl/queue checks remain intact. Its
  scheduler freeze explicitly allows the shared performance methods now
  requested for both GPUs; actual-method tests cover those changes.

## Failures preserved during development

A prototype sorting large tuples with the standard sort overflowed the 32 KiB
kernel stack. The committed implementation sorts small indices with cached
keys and falls back to constant-stack heapsort on allocation failure. The real
full-panel runs qualify that replacement.

Restricting the 200 us retry to unstaged work reduced throughput. The final
shared predicate also covers unresolved published work whose growth/retirement
notifications can coalesce. Input fences still use callbacks, not periodic
GPU polling. Retirement wakes other pool owners immediately.

The original DCP tail passed sustained presentation but an immediate restart
failed with PageFlip EBUSY. GPU-only client restarts remained healthy. DCP has
firmware swap-complete callbacks and no vblank IRQ; waiting for vblank can skip
waiting for an actual outstanding flip. The separate DCP commit preserves the
runtime-PM order but waits for flip_done before cleanup. Four consecutive
on-screen restarts then passed. KWin's safety-margin and timestamp heuristics
were not changed or tested here.

Several boots stalled at SMP startup before the GPU ran. They are not workload
results; standard RID0 reboot and the same 16-CPU chainload/boot procedure
succeeded. The inefficient first benchmark prototype is also excluded from
performance comparisons. Complete failure logs remain alongside successes.

## Reproduction and artifacts

Use `tests/README.md` for benchmark compilation, runtime requirements and the
host regressions. Mesa is the existing production G16C/G17P build at 7e6ee44;
no Mesa source modification, renderer spoof, debug substitute, nocompress
switch or KWin configuration workaround is used for these results.

Artifacts are under:

```
/Users/user/gravity/bootloader-m4maxmbp/build/gpu-performance-20261031/
```

`performance-results.json` records measurements and hashes; `audit-final.py`
checks actual final receipts. `runtime.json` inventories guest Mesa/runtime
files; `final-source.json` verifies all 145 relevant production source files
against the remote build and `final-source.tar.gz` preserves them. The final
configuration and build log are `kernel.config` and `kernel-final-build.log`.
The reviewed benchmark rebuild differs only in unstripped file symbols: its
stripped binary SHA256 matches the original measured binary exactly.

From `/Users/user/gravity/bootloader-m4maxmbp`, after the authorized RID0 reboot
and raw chainload procedure in AGENTS.md, the final Linux launch was:

```sh
M1N1DEVICE=/dev/cu.usbmodemJPWWJRX2N01 .venv/bin/python -u \
  build/m4max-devices/hdmi-linux-v1/monitor-stage1.py \
  build/gpu-performance-20261031/Image-candidate7 \
  build/gpu-performance-20261031/t6041-j616c.dtb \
  build/gpu-performance-20261031/initramfs.cpio.gz \
  --disable-wfxt --enable-gpu --bootarg asahi_neo.submission_log=0 \
  --evidence build/gpu-performance-20261031/candidate7-1
```

Wait for MAX_GPU_PERF_READY and CPUs 0-15 before running in the guest:

```sh
PERF_FRAMES=16 /opt/mesa-vulkan-apple9/run-feature /dcp-present-bench \
  /opt/mesa-vulkan-apple9/bin/apple9-render-draw.vert.spv \
  /opt/mesa-vulkan-apple9/bin/apple9-render-draw.frag.spv
```

Remote build source: `/home/lab/linux-gpu-performance-7.3` on
`lab@192.168.170.255`; output: `/home/lab/linux-gpu-performance-7.3-build`.
The source changes are committed locally for review; no remote branches were
pushed by this task. Existing production and experimental worktrees were
preserved. AGENTS.md was not modified.

Final Image SHA256: `b7e038349bc6ce026a358da907f8418e698cb6cf9ff20a417dc204449ca6b4c5`.
