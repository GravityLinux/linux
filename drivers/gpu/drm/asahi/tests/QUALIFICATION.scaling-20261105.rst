Shared retained-render scaling qualification
==========================================

The M4 Max now uses independent retained render pools during an ordinary
boot. The shared scheduler preserves cold construction leadership, but warm
pool waits do not reserve the entire render engine. Retained-context selection
prefers exact snapshots and then the same logical VM; an aging frequency cache
limits eviction of the working set when clients outnumber the fifteen pools.

Implementation
--------------

* Transfer fallback poll ownership when a queue head retires or an admission
  preference is released. The old code could leave a different queue asleep
  after the last IRQ. The actual mutation test fails on the old implementation
  (expected a wake for job 42, received none) and passes 20,000 transitions with
  the fix. Commit 48e3c99e729a contains this independently qualified repair.
* Test caller identity before expensive queue/lease availability walks and
  preserve logical-VM affinity across immutable binding generations.
* Keep a prepared work slot when another queue advances the global ordinal.
  Patch the four ordinal-dependent fields through the generated wire types;
  validate all remaining identities before publication.
* Clean firmware page tables after mutations, and preserve retry on a failed
  clean. Skip scans of owned private-only leaf ranges that neither caller uses.
  Borrowed leaves, caller overlap and private collisions keep their full checks.
* Enable the existing parallel-pool implementation automatically for chip 6041
  in the ordinary profile. ``parallel_render_auto=0`` or
  ``render_cache_slots=1`` selects the serialized diagnostic fallback. Explicit
  native/legacy controls retain their previous behavior.

The policy, cache, scheduler, VM and ownership changes are shared source. Pro
6040 and Neo 8140 retain their existing default execution profiles. The Pro's
known independent-pool/discard restriction remains enforced. Neo device access
was not used for this qualification.

Native environment
------------------

M4 Max MacBook Pro Mac16,5 / J616c, chip 6041, RID 0, serial JPWWJRX2N0.
All sixteen CPUs remained online. macOS 26.6.2 firmware / 25G83. Native Linux
7.3.0-gravity-max-20261103+, RAM boot #119. Internal DCP panel, 3456x2234 at
120 Hz, real Sway composition, native Wayland Vulkan clients and DMA-BUF
scanout. Audio remained muted throughout.

The final boot has no partial_independent_owner or alternate_queue_pairs
arguments. The boot log reports automatic independent retained pools enabled.
The Rust module parameters do not expose readable sysfs parameter files; the
boot arguments, initialization log, source manifest and host policy tests
establish the selected default.

Image SHA-256::

  431d04e66fc3b1ea1c67b0a99ee6bfacb0ff3ffbc6571348a87c4d02a7734e74

All 168 production GPU Rust files on the builder matched the qualification
checkout when this image was built. The builder's two unrelated wire-control
WIP files were saved, excluded from this build and restored afterwards.

Mesa was the fixed private build used in preceding qualification:

* Vulkan library SHA-256:
  3b2a11187893e784cfc0c2a4b9092f1df2441047e150b25c1afbcfe1fec986c0
* Gallium library SHA-256:
  cb98868460be706a67ce96aa7c283c764bc929553a96da70561f620a289dfa59
* Sway itself loaded both fixed libraries; its renderer was Zink.
* Source fixes are already published to ADevWithAnIdea/mesa,
  integration/g16c-vulkan: fd55afedef7 and 0d6dc28ca62. Qualification of this
  combination is not an upstream reachability claim about the Zink race tests.

Presentation and fairness
-------------------------

The short scaling probe places sixteen visible cubes in a 4x4 grid. Each case
uses a one-second settle and a three-second workqueue trace. DCP commit_work
counts are checked alongside per-process vkQueuePresentKHR counters. The
per-process probe also wraps resolver self-lookups, as required by Volk.
The DRM delivered-vblank tracepoint is not emitted on this driver and is not
used as the frame-rate metric.

With the normal schedutil governor and normal cache_shard workqueue policy:

========  ======================  =========================
Clients   DCP commits per second  Per-client presents/second
========  ======================  =========================
1         120.0                   120.0
2         120.0                   120.0
4         120.0                   120.0
8         120.0                   120.0
16        102.7                   102.2--105.0
========  ======================  =========================

With the CPU performance governor, sixteen visible clients produced 120.0
DCP commits/second and 119.97--120.00 presents/second in every client. This
checks fairness as well as aggregate output; faster presentation did not
come from leaving some clients asleep. The driver does not force a CPU
frequency policy. CPU governors were restored to schedutil after each control.
The cache_shard workqueue policy was also restored after its separate control.

Correctness under concurrent visible load
----------------------------------------

* Two independent Zink/DRM clients exchanged three DMA-BUF images using native
  server waits. Passed 256 frames and 1,048,576 exact pixel comparisons while
  the sixteen visible cubes were running.
* Two concurrent native Vulkan processes each ran 64 asynchronous generations
  of resize, SHA-256, texture/discard rendering and readback, with up to eight
  pending generations. Repeated with both fence and timeline waits: all four
  runs passed 196,608 hash comparisons each.
* Muted Firefox YouTube playback continued through twelve rapid window resizes
  followed by sixty seconds with all sixteen visible cubes. The stream changed
  from 480p to 720p. Video time advanced from 6.64 to 68.39 seconds and decoded
  frames from 166 to 1710. Dropped frames rose from 10 to 32 around the resize
  and quality switch; the final fifty seconds added no dropped frames.
* Each cube completed 7140 presentations, 114,240 total. Recent per-client
  rates with the browser also active were 102.8--104.1 presents/second.
  A desktop screencopy was inspected. This snapshot alone does not prove the
  absence of every possible transient video artifact.
* No GPU timeout, failed asynchronous submission or kernel fault was observed.

Host checks
-----------

* 2560 actual automatic-policy cases, including the Pro/Neo defaults, native
  controls, single-slot override and actual frontend dependency gate.
* 1024 original selector cases and 32,768 affine/cache cases; lease exclusion,
  frequency aging, inactive victims and bounded counters.
* Actual cold/warm scheduler handoff and rebind preparation ownership.
* Actual firmware VM dirty publication, failed-clean retry and DATA writes.
* Actual UserVm/leaf-switch implementation: mixed/private preservation,
  borrowed backing lifetime, cancelled/stale/colliding plans, allocation
  failures, bounded table ownership, private-only ranges and boundary overlap.
* 23 actual shared-wave retirement/CPU-lease/error cases.
* 960 original render encoder cases and 4800 late-ordinal cases: all ten
  complete object images and store addresses match fresh actual encoding.
* The tagged Neo walker and current cold VM builder produce identical
  50,864,376-byte RAM images and 2267 allocations. This is a host comparison,
  not a Neo hardware test.

Evidence is local to the bootloader checkout under
build/m4max-devices/gpu-scaling-fix-20261105/. Principal receipts are
final-default-source.json, boot-final-default.json, final-default-test-v2.log,
final-functional.log, final-visual.log and final-desktop.png. The artifact
scripts preserve exact commands and own their test processes.
