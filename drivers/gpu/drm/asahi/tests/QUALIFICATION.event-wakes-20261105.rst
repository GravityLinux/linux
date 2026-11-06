Shared firmware event wake reduction, 2026-11-05
================================================

With sixteen independent Vulkan Wayland clients, firmware notifications woke
all waiting logical queue heads even though each worker polls the whole GPU
session. This amplified runtime-mutex contention and repeated preparation.

The scheduler now wakes preferred admission queues and one rotating ready
head. Direct receipt callbacks wake the exact owner on both publication and
GPU completion. Publication has a separate internal fence: Neo-style control
handshakes may finish before execution. Callback storage is allocated before
ioctl commitment; late registration against an already-signalled fence
explicitly requeues its owner. Fatal errors still wake every accepted job.
Only the elected session poller repeats a raced firmware notification and
arms scheduled retry/watchdog work; legacy execution retains its watchdog.

Firmware encodings, pool counts, queue assignment, default overlap policy,
CPU configuration and userspace completion/visibility rules are unchanged.

Validation
----------

Actual-source host gates pass:

* ``check_event_heads.py``: 20,000 membership/retirement transitions,
  file-scoped FIFO heads, preferred ready owners, bounded normal wakes, fair
  rotation, promotion, fatal broadcasts, callback identities and transactional
  allocation failures.
* ``check_receipt_wakes.py``: stable preallocated callback storage, exact
  wake, signal-before-registration recheck, callback removal and teardown.
* ``check_publication_wakes.py``: publication state precedes notification,
  publication does not complete execution, staged/unstaged receipt selection
  and unconditional late-registration coverage.
* Existing actual-source admission-view and cold-rebind gates pass.

Native Linux Image builds compile both G16C and G17P backends. Hardware
validation is M4 Max RID 0 / JPWWJRX2N0 only, all sixteen CPUs online. Neo
hardware access remains prohibited. Native RAM Image #99 SHA256 is
``e688096794e7e84d94a1086fae5cc64bf8cea589f3571fd923adea884b52966d``.
Installed/default kernel assets were not replaced.

The display path is real Vulkan Wayland vkcubes -> private fixed Mesa ->
Sway/Zink -> DMA-BUF -> internal DCP panel, 3456x2160 at 120 Hz, scale 2.
Audio remains muted. Same-log-level short three-second traces observed:

============================= ============== ==============
Measurement (sixteen cubes)    Before         Candidate #99
============================= ============== ==============
DCP commits/second            29.0           35.0
Job/watchdog callbacks/second  16910          8779
System CPU ticks / 3 seconds  911            372
Lock contentions / 3 seconds  177828         51841
============================= ============== ==============

One cube still produces 120 DCP commits/second. The two-cube observation was
117; a separate boot with submission_log=0 measured 110, and sixteen cubes
remained at 35. Short measurements vary with CPU frequency and include trace
overhead. These are presentation-update rates, not each client's render FPS.

With all sixteen cubes active, two independent Zink clients exchanged a
three-image DMA-BUF ring using native server fences: 256 frames and 1,048,576
exact output pixels passed. The earlier completion-only candidate also
ran muted YouTube alongside sixteen visible cubes; that is not evidence of
perfect video pixels or satisfactory frame pacing. No new GPU timeout or
kernel failure was observed in the final short tests.

Limitations and rejected controls
---------------------------------

The overall multi-client performance problem remains OPEN. This change
reduces CPU overhead; it does not restore 120 Hz with sixteen clients.
No replacement branch was pushed on the basis of these partial results.

A runtime try-lock candidate starved admission (3.3 DCP commits/second) and
was rejected. Preparing Max descriptors before caller rebind had no useful
measured benefit and was restored. Temporary aggregate timing instrumentation
was also removed before building the qualified candidate.

Diagnostic CPU controls (all sixteen CPUs remained online): a performance
frequency governor raised a sixteen-cube observation from 35.7 to 50.7;
restricting unbound worker placement to P cores raised 36.7 to 44.0. Both
settings were restored immediately. These controls used a rejected
preparation candidate and are diagnostic, not production qualification.
The Max DT also lacks CPU capacity properties (Linux reports every CPU as
1024); this omission needs separate validation. No CPU DT/policy fix is
included here.

An unchanged historical ``check_scheduled_execution.py`` assumes compute
ID 4 and render ID 3 are invalid. The existing encoder supports IDs through
127 and rejects that old assertion. No submission binding encoder changed
in this patch; do not describe the entire historical test suite as passing.

Evidence is retained in the bootloader checkout under
``build/m4max-devices/gpu-multiclient-followup-20261105/``. ``LIVE.json``
records boot receipts, source hashes, controls and the pending regression
investigation. This is separate from the shared Zink race hypotheses: their
forced-interleaving tests are not an end-to-end upstream-driver reproducer,
and the presentation slowdown also reproduces without Zink applications.
