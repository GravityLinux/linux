Shared runtime contention qualification
=======================================

Concurrent per-client CPU preparation remains in place. Shared RuntimeRef
now uses the kernel's sleeping priority-inheritance rt_mutex instead of an
optimistically spinning mutex. Both G16C and G17P construct this same lock.
The Rust lock backend uses the standard kernel operations and a small Rust
helper for the lockdep assertion; Kconfig selects RT_MUTEXES.

Warm scratch/template queries and backing/client selection share read passes.
Cold template sizing still follows scratch publication. Allocation, encoding
and CPU cache completion remain off-lock. An empty CPU-completion pass keeps
its guard through publication rather than releasing and reacquiring it.

Retained-pool eviction weights aged frequency by the installed backing-object
count, a bounded scalar estimate of mapping/cache-warming cost. Exact callers
and logical-VM affinity still win first. GPU/CPU/control leases still exclude
busy pools; inactivity still clears protection. This metadata retains no BO
references. The change applies to the shared Pro/Max/Neo paths and introduces
no firmware-wire layout or command-encoding changes.

Environment and source
----------------------

M4 Max Mac16,5 / J616c, chip 0x6041, RID 0, serial JPWWJRX2N0. All 16 CPUs online,
spin-table boot, macOS 26.6.2/25G83 firmware. Native Linux 7.3.0-gravity-max-
20261103+, RAM boot #137. Only the Max was accessed; Neo hardware was not used.

Real native Wayland Vulkan clients, Sway's Zink renderer, DMA-BUF and the
internal 3456x2234 120 Hz DCP panel. No experimental render flags. Audio muted.
Normal controls use schedutil, global workqueue mask ffff and cache_shard.
The separate performance-governor control restores schedutil in finally.
The driver sets no CPU-frequency or worker-affinity policy.

Qualification image SHA-256::

  543275c52708d21a34f25dfa3b60b32de824d306f2dec5489be858200c5254c4

All 168 production GPU Rust files, Kconfig and rust/helpers/mutex.c matched the
builder when this image was captured. The builder's two operator wire-control
WIP files were excluded from the build, then restored. The installed fallback
kernel was not replaced. The original development tree's four WIP files were
preserved.

Fixed private Mesa remained identical to the preceding qualification:

* Vulkan: 3b2a11187893e784cfc0c2a4b9092f1df2441047e150b25c1afbcfe1fec986c0
* Gallium: cb98868460be706a67ce96aa7c283c764bc929553a96da70561f620a289dfa59

Sway's mapped libraries were verified, and native probes identify Apple M4 Max
G16C B1. HK_RENDER_AVAILABILITY=1 and ASAHI_MESA_DEBUG=nocompress match the
preceding control. No Mesa change is part of this kernel commit.

Presentation and CPU cost
-------------------------

The paired baseline reboots the previously qualified image 119, SHA-256
431d04e66fc3b1ea1c67b0a99ee6bfacb0ff3ffbc6571348a87c4d02a7734e74.
Both boots use the same 1/2/4/8/16-client sequence: one-second settle and a
three-second workqueue trace, with all clients visible through Sway/DCP.
Per-client vkQueuePresentKHR counters check fairness alongside DCP commit_work
counts. Lock/scheduler tracepoints are disabled during these measurements.
System CPU ticks come from /proc/stat and cover the whole kernel workload.

==========  ==============  ==============  ===============  ===============
Clients     Baseline fps    Optimized fps   Baseline sys     Optimized sys
==========  ==============  ==============  ===============  ===============
1           120.0           120.0           47               54
2           120.0           120.0           64               55
4           120.0           120.0           164              79
8           120.0           120.0           474              148
16          93.3            97.0            1269             316
==========  ==============  ==============  ===============  ===============

At 16 clients this is 75.1 percent less system CPU time at comparable throughput;
per-client presentation rates were 94.95--97.26 fps, so the result does not hide
starved clients. These are short controls, not a claim that the small FPS
increase is statistically significant or that schedutil now reaches 120 fps.

With the CPU performance governor, all 16 clients presented at 119.99--120.00 fps,
DCP committed at 120 fps, and system CPU time was 250 ticks over three seconds.
Frequency policy was restored afterwards. The normal-governor gap remains;
this change removes the measured CPU waste without forcing a governor.

Concurrent correctness
----------------------

* Two independent Zink/DRM clients exchanged three DMA-BUF images with native
  server waits: 256 frames, 1,048,576 exact pixels, while 16 visible cubes ran.
* Two concurrent native Vulkan processes each ran 64 async generations of
  resize, SHA256, texture/discard, render and readback. Fence and timeline
  modes both passed: four runs of 196,608 hashes, up to 8 pending generations.
* Eight simultaneous Vulkan processes ran 8 generations each beside the 16
  visible cubes. Both fence and timeline modes passed: 16 runs, 32,768 hashes
  each, with up to 8 pending generations per process.
* Muted Firefox/YouTube continued through 12 window resizes and a 60-second
  normal-governor soak beside 16 new cubes. Playback advanced 7.46 to 69.00 seconds,
  decoded frames 186 to 1725, and quality changed 480p to 720p. Dropped frames rose
  from 10 to 36 during resize/quality adjustment, then stayed flat for the last
  40 seconds. No video error or pause occurred.
* The cubes completed 68,040 presentations during the video test; recent rates
  were 62.10--62.54 fps each under the normal governor. This is a different load
  from the cube-only FPS table and from the preceding performance-governor
  video qualification. A screencopy was inspected, with no obvious corruption;
  one snapshot cannot exclude every transient artifact.
* No GPU timeout, rejected recycle, failed asynchronous submission or kernel
  fault was observed in these runs.

Host gates
----------

* Actual batched support preparation: 20 warm/cold/failure/stale-capture cases;
  warm reads acquire runtime once, sizing follows scratch commit, and CPU
  allocations/encoding execute off-lock.
* Actual completion guard transition: no extra acquisition for empty batches,
  every cache completion off-lock, first error retained, publication locked.
* Actual selector: 1024 original plus 32,768 affine cases, active expensive-root
  protection, inactive-root release, weight saturation and bounds, and the
  existing GPU/CPU/control/preparation exclusions.
* Actual cold/warm root/descriptor preparation and 1000 blocked retries with no
  snapshot allocations.
* 23 shared-wave retirement/CPU-lease/failure cases, and cold/warm scheduler
  handoff/error-release gates.

Implementation choices
----------------------

Whole-progress service-worker pools reduced CPU usage but reduced presentation
rate. Bound/high-priority routing and receipt-only shortcuts also failed the
native comparison. Those experiments were discarded. The final patch keeps
the original asynchronous callbacks, workqueues, election and existing shared
session-poll coalescing. Per-client preparation and GPU execution stay
concurrent; only shared runtime mutations use the sleeping lock.

Artifacts and exact scripts are under the bootloader checkout's
build/m4max-devices/gpu-coalesced-service-20261106/. Principal receipts:
final-source.json, boot-cost-cache.json, test-paired-baseline.log,
test-cost-cache.log, functional-final.log, performance-control-final.log,
visual-final.log, qualification-health.log and visual-data/. Rejected
experiments are retained separately and are not part of the published source.
