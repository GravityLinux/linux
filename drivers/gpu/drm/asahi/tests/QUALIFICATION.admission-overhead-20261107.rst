Shared G16C/G17P admission CPU overhead, 2026-11-07
================================================

The scheduler keeps its existing per-job preparation and publication model.
This change removes repeated CPU work without changing the firmware ABI,
GPU address space, retained-owner lifecycle, or publication/retirement order.

Changes
-------

* Firmware notifications have a distinct poll epoch. CPU fence callbacks and
  queue promotion still participate in the original lost-wake handshake but
  no longer force another full firmware poll within the coalescing interval.
* A pending scheduled head uses its existing high-resolution retry timer
  instead of additionally queuing a 10 ms watchdog. The legacy watchdog and
  the slow fallback remain when no high-resolution retry was armed.
* Successful completed Points avoid locking their receipt again. Receipt
  callbacks are not rebound after their result has reached the Point, and a
  retiring job does not bind callbacks that would immediately requeue it.
  A receipt becoming published while its Point is still pending still binds
  the underlying fence and retains the existing late-registration wake.
* Warm render and compute preparation hold one runtime guard across sizing
  and capture, rather than selecting and locking twice. Cold allocation stays
  off-lock; admission and seed checks are repeated after reacquisition.
* G16C pool selection classifies retained owners and queue leases once per
  selection. G17P similarly classifies cached caller membership once. Exact
  and related caller preference, private-window checks, preparation/CPU/GPU
  leases, ASID limits and eviction ordering remain unchanged.

Native environment
------------------

Only M4 Max Mac16,5 / J616c, chip 0x6041, RID 0 / serial JPWWJRX2N0 was used.
Neo device access remains revoked; its production selector was tested offline.
All 16 CPUs stayed online, all three policies used schedutil, and the global
workqueue mask remained ffff. GPU workers retained ordinary priority.

The final candidate retains the preceding device-workqueue affinity default.
Broader placement was tested, not silently accepted as a performance fix.
No CPU-frequency floor, governor override or realtime GPU-worker boost is
part of this change. The corrected T6041 CPU/cache device tree from the
preceding qualification was used for every comparison.

The visible workload uses native Wayland Vulkan cubes, Sway's Zink renderer,
DMA-BUF and the internal DCP panel at 120 Hz. Sway's normal main-thread RR1
policy was restored after the private-library launcher loaded Mesa. Firefox
and the OS audio sink were muted. The final mixed scene includes sixteen
visible cubes, YouTube playback and twelve Firefox window resizes.

Final RAM-boot kernel #179 (boot attempt 97), image Image-final-v8, SHA256::

  23d3e103a58d49a0312feedc076a71ab7347a829a555ba57b14580fa8c44dc74

All 171 production source files matched the actual builder before copying
the image. The builder's two operator wire-control WIP files were restored
after that capture. The installed fallback kernel was not replaced, and the
original development checkout's four operator WIP files were untouched.

Mesa was unchanged: Vulkan DSO SHA256
3b2a11187893e784cfc0c2a4b9092f1df2441047e150b25c1afbcfe1fec986c0,
Gallium DSO SHA256
cb98868460be706a67ce96aa7c283c764bc929553a96da70561f620a289dfa59.

Presentation control
--------------------

The final 16-client plain scene presented at 119.987--119.994 fps per client;
DCP committed at 119.99 fps. It generated 10,247 GPU job callbacks/s versus
11,293/s in the preceding same-affinity control at 120 fps, about nine percent
fewer. The old redundant watchdog contributed approximately 2,000 callbacks/s;
none occurred in the final trace. Other job callbacks vary with retry timing.
Three-second system CPU totals are noisy and do not establish a corresponding
percentage reduction in total CPU usage.

After twelve Firefox resizes, the separate twenty-second mixed measurement
reported 118.930--119.036 fps per cube. Individual sixty-frame samples dipped
to about 105 fps, so this does not establish perfect deadline delivery. The
video was 1280x720 at 25 fps, despite its title mentioning 4K. Playback advanced
normally; dropped frames stopped increasing after startup/resizing. A
compositor screenshot showed the browser and all sixteen cubes without obvious
corruption; it cannot exclude transient artifacts.

With the sixteen cubes and video running, two independent DRM clients
exchanged three DMA-BUF images for 256 frames with native server waits:
all 1,048,576 checked pixels were exact. Two concurrent native Vulkan programs
each ran 64 asynchronous resize/SHA256/texture/discard/render/readback rounds
in each of fence and timeline modes. All four runs checked 196,608 hashes
with up to eight pending operations and passed. The final kernel log contained
no GPU fault, timeout, asynchronous submission failure, WARNING or BUG.

Host checks
-----------

* 20,000 actual admission/retirement/wake transitions and 10,000 separate
  CPU/firmware epoch transitions, including counter wraparound.
* 48 actual pending-job retry branches: elected head, 20/200 us retry,
  legacy fallback and lost-wake requeue; at most one timer is armed.
* 30,000 old/new G16C pool selections, including duplicate/dormant owners,
  invalid or busy queues, lease pressure and private-window overflow. In the
  fully resident late-match case owner examinations fall from 1,122 to 33.
* 60,000 old/new G17P selections, including active/cached/duplicate callers,
  geometry compatibility, legacy/native modes and ASID pressure.
* 1,344 actual old/new Point refresh states; successful terminal history
  skips its receipt lock, while pending and failed-history joins are preserved.
* 96 actual compute preparations and the render cold/warm/retry gate: one
  warm guard, two cold guards, off-lock allocation/encoding, error attribution,
  changed-owner revalidation and 1,000 blocked retries without table allocation.
* 396 actual receipt-binding branches, plus publication and late-registration
  callback gates. Retiring jobs never register a new self-wake.
* Whole serialized render/support/growth and compute preparation checks passed
  before native qualification. No firmware-wire layout was changed.

Rejected experiments and limits
------------------------------

Restricting the workqueue to the whole first fast frequency domain lets its
callbacks execute across all six P0 CPUs. Plain sixteen-cube runs approached
120 fps, but mixed-video runs ranged roughly 102--117 fps. A same-scene
single-CPU control still reached 120 fps. Five-core placement and cache,
system and strict-cache workqueue scopes did not provide a stable replacement.
The final patch therefore does not change the preceding affinity default or
claim that cross-CPU scaling has been solved.

Two additional query/lease-scan shortcuts passed semantic host gates but
performed poorly in the mixed native scene (about 64 fps). The preceding
candidate was cold-booted as a control and those shortcuts were removed.
These observations do not establish the mechanism of that slowdown.

An earlier repeated 64-client churn stall reproduced on the old baseline too.
This change does not qualify that failure as fixed, arbitrary context churn,
or demanding-game performance. Neo and M4 Pro native testing remain outstanding.

Exact scripts, candidate manifests, build/boot receipts and rejected controls
are retained in the bootloader checkout under::

  build/m4max-devices/gpu-admission-overhead-20261107/

Principal final receipts are manifest-final-v8.json, builder-source-final-v8.json,
boot-final-v8.json, test-final-v8.log, visual-final-v8.log, soak-final-v8.log,
functional-final-v8.log, final-health-v8.log and desktop-final-v8.png. Whole
wire-image checks are under final-scheduled_render-v5,
final-scheduled_render_resources-v5, final-scheduled_parameter_growth-v5 and
final-scheduled_compute_preparation-v5; the serialized constructors were
unchanged between that run and the final image.

