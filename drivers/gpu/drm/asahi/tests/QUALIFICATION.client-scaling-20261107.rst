Shared G16C/G17P client scaling qualification, 2026-11-07
======================================================

M4 Max J616c (Mac16,5, chip 0x6041), RID 0 / JPWWJRX2N0 only.
No Neo hardware access was performed. Both ABI paths use the changed CPU
scheduler and memory helpers; the Neo comparisons below are offline tests.

Result
------

Sixteen independent native Wayland Vulkan cubes presented through Sway/Zink,
DMA-BUF and the internal DCP output at approximately 120 frames/s each with
all sixteen CPUs online and all three CPU-frequency policies on schedutil.
The final mixed workload added muted Firefox/YouTube playback and twelve
window resizes. After those resizes, a separate twenty-second measurement
reported 119.843--119.897 frames/s per client, with 38--39 complete samples
of sixty frames each. Individual half-second samples occasionally measured
about 118 frames/s; this is not a promise that every deadline is met.

The video was actually 1280x720 at 25 frames/s, despite its title mentioning
4K. Playback advanced normally. The dropped-frame counter stopped increasing
after startup/resizing. A compositor screenshot contains the browser and all
sixteen visible cubes; a screenshot alone cannot exclude transient corruption.
A separate DCP work trace measured 359 commits in approximately three seconds
in the final mixed scene. The active output mode was 3456x2160 at 120 Hz.

With the same corrected CPU topology, Mesa and compositor scheduling policy,
the old 6bb0f5e1bc25 kernel measured about 102--103 frames/s with sixteen
cubes. The earlier experimental variants are not the source being qualified.
The final version preserves the established retirement/publication ordering.

What changed
------------

* Device-owned CPU work stays on one online fast CPU, chosen from the first
  highest-frequency CPU policy. A later CPU within that policy avoids its
  boot/interrupt CPU. On this Max that selects CPU 9 (mask 0x200).
  The global workqueue mask remains 0xffff; no CPU is offlined and no governor,
  frequency floor, realtime policy or nice boost is applied to GPU workers.
  Existing per-job work items and concurrency remain; this is not an ordered
  workqueue with max_active=1. Last-owner destruction is deferred to another
  workqueue, so a callback never destroys the queue executing it.
* Warm admission does not repeat client preparation unless scratch/template
  construction could have changed the selected owner. Ordinary scheduled
  admission retains immutable snapshots without building unused private roots.
* Pool selection classifies live GPU, CPU-completion and preparation leases
  once per selection. No availability result survives that selection.
* Ownership/table lookup hints cache checked indices, never PTE contents or
  firmware counter values. Every translation still reads the live PTE.
  Independent clean firmware-counter reads share a cache-maintenance barrier
  pair. There are no intervening stores in the converted read groups.
* Retained render capacity uses all thirty-three disjoint event pairs: the
  original fifteen identities remain unchanged; extra pairs use grids 92--127.
  Independent compute keeps grids 32--91 and legacy compute keeps its own
  handling. Terminal decoding now observes all 128 report bits instead of
  truncating the report to its low word. Backing is still allocated on demand.
  Native cold-boot logs include pool 32, exercising the highest pair.

The separate T6041 device-tree change describes the existing three CPU/cache
clusters and capacities. The test DTB adds only those CPU properties to the
previously working DTB: all 1,565 original properties, including all 1,435
non-CPU properties, were preserved. Shared-cache sizes were checked against
ADT metadata, and capacity ratios use the existing base-M4 description.

Native correctness
------------------

After the visible mixed workload, with all sixteen cubes and video still
running:

* Two independent DRM clients shared three DMA-BUF images for 256 frames.
  All 1,048,576 checked pixels were exact; native server waits were used.
* Two native Vulkan programs ran concurrently in each of fence and timeline
  modes. Each ran 64 rounds of shared-image resize, SHA256, texture sampling,
  discard, render and readback, checking 196,608 hashes per program with up to
  eight pending operations. All four passed.
* No asynchronous submission failures, GPU faults, firmware timeouts, kernel
  warnings or BUGs were present in the final boot's kernel log.

Host checks execute production Rust methods or compare full serialized DATA:

* 260 whole render descriptors and four independent native capture images;
  every generated layout assertion matched the recovered ABI.
* 288 complete parameter/support objects, 64 FList requests and 128 complete
  growth replies matched the qualified source constructors.
* All 33 event pairs, 20,000 coalesced report combinations, foreign/unpaired
  bits, compute isolation and exact owner counts.
* 20,000 lease-classification comparisons with the previous availability gate;
  1,024 original selector cases and 32,768 affine selector cases.
* 200,000 checked allocation spans, changing live PTEs, reordered/stale hints,
  and 10,000 warm ownership hits without another tree lookup.
* 8,128 stale-cache counter batches and 10,000 changing firmware epochs,
  including discontinuous mappings and invalid addresses.
* 64 admission snapshot cases on both ABIs, including validation failures;
  20 cold/warm support cases and 1,000 blocked rebind retries.
* Actual queue configuration across CPU-frequency/hotplug/cpumask configs,
  concurrent first submissions, offline CPUs, missing policies and allocation
  failures. Actual pinned destructors cover active callbacks, construction
  unwind and 1,000 exactly-once deferred queue releases.
* 12,544 control publications cover outstanding cleanup, both independent
  consumers, wraparound, full-ring rejection and data/barrier/tail ordering.
  The accompanying control-append fix has its own preceding commit.

Reproduction and artifacts
--------------------------

Host artifact directory, relative to the bootloader-m4maxmbp checkout::

  build/m4max-devices/gpu-runtime-batching-20261106/

Final kernel: Image-local-steady-v13, build #173, SHA256::

  6290e2b8f006c9a452d70c871d00143879c5bba4dd1b4e76608355938c48de3f

manifest-local-steady-v13.json and builder-source-local-steady-v13.json
verify all 171 production source files against the actual builder. The DTB
is test-cpu-topology-v1.dtb. boot-local-steady-v13.json records its boot.
The final scene, steady interval and native checks are recorded in::

  test-local-steady-v13.log
  visual-local-steady-v13.log
  soak-local-steady-v13.log
  functional-local-steady-v13.log
  final-native-state-v13.log

The validated preceding candidate was also cold-booted twice. The final
candidate omits that candidate's unnecessary retirement-order experiment.

Mesa was unchanged: the compositor and tests mapped the qualified Vulkan
DSO (SHA256 3b2a11187893e784cfc0c2a4b9092f1df2441047e150b25c1afbcfe1fec986c0)
and Gallium DSO (SHA256 cb98868460be706a67ce96aa7c283c764bc929553a96da70561f620a289dfa59).
The private-library test launcher restores Sway's normal main-thread RR
priority 1 after loading those libraries; all baseline comparisons use the
same policy. GPU workers keep ordinary scheduling priority. Audio remained
muted throughout.

Limits
------

This qualifies the visible sixteen-client workload, not arbitrary sustained
context churn or a demanding game. An earlier repeated 64-client churn test
stalled on both an experimental kernel and the exact old 6bb0f5e1bc25 control
image. That separate pre-existing failure has not been established fixed by
this qualification. Neo/M4 Pro native regression tests remain outstanding.
