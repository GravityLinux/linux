# Two-client latency diagnosis and fixes, 2026-10-31

This supersedes the performance conclusions in the earlier shared-performance
qualification. The M4 Max was not limited to 8.7 fps rendering.

Driver commit: `5a8a86426728`, on `experimental/gpu-performance-7.3` in
`/Users/user/gravity/linux-gpu-performance-7.3`, following the shared VM work
`2592a4064892` and DCP completion fix `01024cfbaaab`.
Changes apply to the common G16C/G17P scheduler and VM. Hardware tests were
**M4 Max RID0 only**, Mac16,5/J616c, chip 0x6041, serial JPWWJRX2N0.
All 16 CPUs remained enabled. No Neo device access or Apple disassembly occurred.
No firmware layouts, constants, or required receipt sequences were changed.

## Causes established by profiling

1. A compute-copy submission prepared and discarded the same page-table
   rebind tree roughly nine times while its eight ownership-checked control
   receipts retired. Each plan cost 5–12 ms. One recorded submission spent
   81.065 ms in compute preparation before publication at 91.205 ms. Rendering
   itself published in about 3–4 ms. Ioctl admission was only tens of us.
2. The minimal lab kernel had `CONFIG_HIGH_RES_TIMERS` disabled at HZ=250.
   The requested 200 us hrtimer retry therefore ran at roughly 4 ms, adding
   about 32 ms for an eight-receipt handoff once duplicate preparation was
   removed. These costs interact; do not add 81 and 32 ms as independent costs.
   Standard arm64 defconfig already enables high-resolution timers.
3. The one remaining compute plan still used an owned table copy, about
   6.7 ms, while render already shared immutable leaves. The compute copy used
   for framebuffer export exercised that slower path on every client switch.

The shared scheduler now advances nonblocking admission before allocating the
rebind plan, then repeats all admission/seed checks before publication. Compute
uses the same immutable-leaf switch as render when its private save area stays
in place; moves and primer aliases retain the original owned-copy transaction.
Compute addresses remain absolute, including addresses below the render base.
The measured compute plan is now about 0.2–0.5 ms. Receipt-only retries use
20 us; ordinary pending GPU work retains the 200 us fallback and external
input fences remain callback-driven. All eight required controls remain.

## Benchmark corrections and buffering

The old short benchmark included its final complete CPU scan of each 30 MB
image in the timing window, although the first full scan was in warmup. This
was substantial fixed CPU validation overhead, not production rendering work.
The corrected window excludes validation-only tail passes. Those passes still
render, present and check all pixels/guards before PASS can be printed.
`timed_full_oracle=0` identifies the corrected measurement.

The serial benchmark also waits for DCP presentation before starting another
render. `PERF_PIPELINE=1` provides compositor-style overlap using two DMA-BUFs
per Vulkan client (four total), rather than writing a current scanout. GPU work
for a retired spare runs while the previous flip completes. Only one DCP flip
is pending; its event is consumed before another is submitted. The last timed
flip is drained before stopping the clock. Both buffers have complete first
and last CPU oracles; sparse color and guard checks remain on measured frames.

These are explicit metric/buffering changes. The FPS increase is not a pure
comparison of driver code alone. The original executable is preserved for
matched comparisons; phase timings establish the driver improvements.

## Results

All results use Linux 7.3 under the existing 16-CPU hardware-passthrough monitor,
production Mesa Vulkan, native GPU execution and DCP scanout. The preferred
mode was selected, 3456x2160 at nominal 120 Hz. No CPU framebuffer copy,
software renderer, debug substitution or KWin workaround was used.

| Test | Result | Mean record/submit through Vulkan fence |
| --- | --- | --- |
| Original benchmark, before this investigation | 8.3–8.7 fps | 98–107 ms |
| Original executable after gate/compute sharing/high-res fixes, 200 us retries | 42.736 fps | 6.370 ms |
| Corrected window, two clients, offscreen, final code | 205.866 fps | 4.813 ms |
| Corrected window, serial DMA-BUF/DCP, final code | 75.790 fps | 4.308 ms |
| Pipelined DMA-BUF/DCP, 32 frames | 114.779 fps | 4.600 ms |
| Pipelined DMA-BUF/DCP, 256 frames / 255 flips | **119.514 fps** | 4.459 ms |

The last run timed 246 frames over 2058.344 ms. Short pipelined runs include
pipeline-boundary overhead; sustained cadence approaches the mode's 120 Hz.
These are benchmark flip completions, not Chromium/game/conformance scores or
a bare-metal performance claim. CPU submit-to-fence time is not a GPU hardware
timestamp. Neo hardware performance has not been measured.

## Correctness and reuse

The final kernel passed 384 on-screen frames with 377 DCP events, including
four independent pipelined client restarts after native growth/private-window
movement, plus the two-client offscreen control. There are no hidden retries
for EBUSY in the test. Pixels and guards must pass before a result is printed.

Four native 200000-instance pressure frames passed across explicit-close and
file-close VMs at 0x7000000000 and unaligned 0x7100004000, exercising the owned
private-move fallback after shared compute roots. The fixture checks complete
arguments/color/query data, 294884 guards, six timestamps and prior-bank poison.
The memory-UAPI test passed 6313 checks. Final GPU unbind returned zero and CPUs
0-15 stayed online; no GPU fault, failed asynchronous submission or flip-done
timeout was found. The target was then returned to the development proxy.

Host tests execute the actual scheduler preparation method: eight blocked
admissions allocate no table/descriptor/rebind plans; one ready admission builds
one plan, and errors remain owned. Actual UserVm tests include absolute compute
addresses, mixed private leaves, snapshot drop, copied fallback, stale/cancelled
plans, bounded repeated switches, allocation failures and sorting. Existing
cold/warm preparation, admission-view, physical-selection, growth, receipt and
historical identity/queue checks also pass. Source layout assertions remain.

## Reproduction and handoff

Artifacts: `/Users/user/gravity/bootloader-m4maxmbp/build/gpu-switch-profile-20261031`.
`results.json` and `audit-final.py` record/check final receipts and hashes.
`profile1/dmesg.log` and `profile1-breakdown.json` preserve the slow-path evidence;
timestamps deduplicate repeated dmesg prints. Intermediate gate-only and
high-resolution-timer results remain in their own directories.
`final-source.json` verifies all 145 relevant production sources against the
remote build; `final-source.tar.gz` preserves them. Final image is
`Image-control20`; configuration is `kernel-final.config` with high-res timers.

Remote build source/output remain on `lab@192.168.170.255` at
`/home/lab/linux-gpu-performance-7.3` and
`/home/lab/linux-gpu-performance-7.3-build`. Nothing was pushed remotely.
Use the exact RID0/raw-chainload instructions in AGENTS.md. From the bootloader
checkout the final launch used:

```sh
M1N1DEVICE=/dev/cu.usbmodemJPWWJRX2N01 .venv/bin/python -u \
  build/m4max-devices/hdmi-linux-v1/monitor-stage1.py \
  build/gpu-switch-profile-20261031/Image-control20 \
  build/gpu-performance-20261031/t6041-j616c.dtb \
  build/gpu-performance-20261031/initramfs.cpio.gz \
  --disable-wfxt --enable-gpu --bootarg asahi_neo.submission_log=0 \
  --evidence build/gpu-switch-profile-20261031/control20
```

After readiness/all16 verification, use the benchmark build instructions in
README.md, preserving production Mesa and the test shaders, then run:

```sh
PERF_PIPELINE=1 PERF_FRAMES=128 /opt/mesa-vulkan-apple9/run-feature \
  /run/dcp-present-pipeline \
  /opt/mesa-vulkan-apple9/bin/apple9-render-draw.vert.spv \
  /opt/mesa-vulkan-apple9/bin/apple9-render-draw.frag.spv
```

Final Image SHA256: `778abf8faa31b02dceff0134d33e3eefb1189a1891f90ce2fb31f0bcf67d30a2`.
