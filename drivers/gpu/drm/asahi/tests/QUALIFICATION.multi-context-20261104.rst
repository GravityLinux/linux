Retained render contexts: operand bank selection, 2026-11-04
===========================================================

Qualification passed on the M4 Max, including a second boot of the exact
committed source with two retained contexts enabled by default. That kernel
is installed; the previous single-context kernel remains available as rollback.

Problem and correction
----------------------

Each retained G16C render root registers a separate operand bank, but the
embedded tiling and fragment programs kept selecting bank zero. Switching
between roots therefore used shared cursor state for unrelated private
backing. Firefox video showed transient white rectangles and eventually a
firmware timeout. Two retained contexts still failed with copying rebind,
lower ASIDs, or permanently retained BO backing; the matched one-context
control passed the same client-switch sequence and video past 420 seconds.

Registers 0x14308 (tiling) and 0x14048 (fragment) select the operand bank,
not a boolean completion mode. The runtime now supplies the reserved bank
to both program arrays. Before first registration, the render support's
bank at +8 matches that selection, and the dispatch entry contains the
render directory and 23 existing buffers before adding five more. The
previous initialization used the owner's separate 20-buffer compute shape.
Already-admitted banks retain their firmware-owned cursors. Pool, UAT
context and completion namespace remain separate identities.

The source uses the existing host policy of one bank per retained physical
root (ASID minus one); native firmware does not require that relationship.
There is no scheduler-policy or wire-layout change. The two-context default
is restored, while the separate overlapping-pool experiment stays disabled.

Independent native evidence
---------------------------

Six saved macOS single-user captures have support-bank values 1/0/1 for
warm A/B/C and 5/4/3 for the later triple A/B/C. In every case both program
registers equal that bank. ASIDs remain 1/2/3 and PB slots 0/1/2, so the
bank changes are independent of those identities. Only captured descriptor
and support DATA were inspected; no Apple instructions were disassembled.

The executable regression checks those native values, both register-array
shapes, and actual runtime lowering for ASIDs 1/4/63 on G16S and G16C.
The old lowering fails with bank zero instead of three for ASID four; the
corrected lowering passes all 48 bank checks. It also compares ten complete
images/addresses in 960 preparation cases. Support layouts pass 32 complete
FList-page comparisons with every size/alignment/field offset asserted.

The separately corrected queue-local stamp/index rotations remain: native
submission 65536 retains secondary stamp 0x100 and fragment index one even
when another queue advances the global ordinal. The old encoder fails this
native DATA regression. The earlier Work15 +0x56 identity trial alone did
not fix the hang; its host ASID63/PB1 assertion is a policy regression, not an
independently captured 62. Restoring the original PB-slot value alone also
failed. Failed checkpoints and raw receipts remain preserved.

First native qualification
--------------------------

M4 Max RID0 only, Mac16,5/J616c, chip 0x6041, serial JPWWJRX2N0; all 16 CPUs.
Neo hardware was not accessed. Normal native Linux runs production
Honeykrisp/Zink under Sway with DCP DMA-BUF presentation.

Image SHA256: 957aced23c1f89cbc6006a1c4384704e44bc7b17234a1b2c47d2880dfa75eae1.
This first run explicitly selected two cache slots. Unmuted YouTube playback
ran through uptime 629.98 seconds, decoded-frame count 30 to 14628, with
repeated full-clip playback and quality changes between 720p, 1080p and
1440p. No firmware timeout or failed asynchronous submission occurred.
The operator reported no recurrence of the white video rectangles during
the corrected run.

Eight discard clients passed 512 frames / 2097152 exact pixels. Eight clients
with distinct, changing PBO texture uploads passed 1024 frames / 4194304
exact pixels. One/two/four concurrent windowed glmark2 clients and the
terrain/refract/shadow scenes completed through the normal compositor path.
These are correctness/stability receipts, not Vulkan conformance or a
claim that video or every demanding scene runs at panel refresh rate.

Evidence is in the bootloader checkout at
``build/m4max-devices/multicontext-fix-20261104/``; first-run results are in
``program-bank-v14-qualification.json``. Original failures and earlier
single-context controls are in the adjacent ``av-freeze-20261104/``.
The installed fallback remains backed up under
``/boot/mctx-debug-backup-20261104/``.

Final committed-source qualification and installation
---------------------------------------------------

Code commit: ``3c24a423b67debeb8a0877970ccfd3dae52caa00``. The isolated builder
checkout was clean at that revision. Kernel #39 Image SHA256:
``098fc6ba8b3d89559fa9314428bb6af49047902a444681863535d3a593f8d497``.

The final boot has no cache-size override and allocates PB0/ASID1 and
PB1/ASID63. All 16 CPUs remain online. The second unmuted video run reached
uptime 470.15 seconds, decoded-frame count 36 to 10901, including full-clip
repeats. Eight discard clients again passed 512 frames, and eight changing
PBO-upload clients passed 1024 frames / 4194304 exact pixels. Two simultaneous
native Vulkan windows completed 720 application frames. Both sustained runs
and the final fresh kernel log have no GPU firmware timeout or failed
asynchronous submission. Neo hardware was not accessed.

All window workloads used the normal Sway display path. DCP state confirms
3456x2160 at 120 Hz, with active DMA-BUF-backed scanout shared with the GPU.
With video paused, the short on-screen build/texture/phong scenes scored
868 for one client and 370/368 for two. Individual two-client scene rates
were 288--422 fps. These short glmark2 measurements do not imply that every
complex scene reaches panel refresh rate; terrain, for example, was much
slower during the earlier competing-video test.

The exact final Image replaced ``/boot/vmlinuz-7.3.0-gravity-max-20261103+``
after hash verification, using an atomic rename. The original #27 Image,
SHA256 ``515a6b21ea4bb8dfd7dbe759a22502d326b1f9073a11163fa4092f3b5488700c``,
remains in ``/boot/mctx-debug-backup-20261104/``. Boot entry, stage 2 and
initramfs hashes were verified unchanged. Video was resumed after timing.
Final receipts are ``qualification.json``, ``final-source-build-identity.log``,
``bank-final-display-state.log`` and ``install-bank-final.log`` in the private
evidence directory. Earlier failed candidate #32's qualification remains
withdrawn; only the corrected-bank runs qualify this change.

Resize and scheduler follow-up: qualification limits
---------------------------------------------------

The operator subsequently reported jerky motion after resizing Firefox and
occasional stale frames in the video with 16 native Wayland vkcubes. The
sustained runs above therefore do not establish that multi-client video
presentation is fully correct or performant.

Commit ``012da5193343`` avoids render preparation while admission is blocked,
removes publication broadcasts to unrelated resource waiters, and elects one
queue head for fast global-session polling. Exact dependency callbacks,
retirement broadcasts, per-queue watchdogs and successor promotion remain.
Host tests execute 20,000 ready-list transitions, including poll ownership
handoff, and actual cold/warm preparation and allocation-failure cases.

M4 Max kernel #47 was RAM-booted with the original configuration matching the
installed modules. Image SHA256:
``7083243fc6b55932f49396fbd93a38b4ccdd188285af330cbdc0f150fbc80ca8``.
All 16 CPUs were online. System output and browser video were muted throughout
these follow-up tests, as requested by the operator. No Neo access occurred.
The installed #39 kernel and installed Mesa libraries remain unchanged.

With the same Sway/DCP, 720p YouTube and 16-cube layout, a three-second trace
measured 14,050 normal scheduler callbacks/s versus 31,113 on #39. System CPU
fell from approximately 5.9 to 2.3 cores. Display commits remained around
25/s versus 26/s: this is a CPU improvement, not a frame-pacing fix. Pausing
the cubes restored approximately 120 display commits/s. A boot-only trial of
15 retained contexts used more CPU and memory without a material frame-rate
gain; the default remains two serialized contexts.

Two concurrent native Vulkan processes passed 64 frames each with eight
frames pending, split command buffers, dynamic rendering, shared-image resize,
discard and image readback. Immediate output snapshots checked 393,216 SHA256
digests and 3,145,728 rendered pixels, using fence and timeline waits. These
checks do not establish that all Zink or browser readback paths are correct.

The new ``render_upload_queued_resize.c`` probe intermittently maps the pack
buffer's initial 0xcd bytes after a successful GL fence wait, even on the
unchanged #39 baseline. A diagnostic observed those bytes becoming correct
within about a millisecond without another GL call. Mapping can itself enqueue
a staging copy, so this alone does not prove that the original render fence
signalled early. Ring-one and software controls pass; synchronization
validation also passes while changing the timing. Native Vulkan controls
including GPU-initialized readback and longer compute dispatches pass.

Two independently demonstrated Zink batch-state races are under investigation
in a separate Mesa worktree. Applying both did not eliminate this reproduction;
neither is installed. The readback failure and compositor frame pacing remain
open. Do not treat the driver's overall multi-client goal as complete.

Follow-up receipts, source snapshots and original failures are preserved in
``build/m4max-devices/resize-latency-20261104/`` in the bootloader checkout.
``early4-matching/result.json`` records the clean measurement and scope.

The subsequent ``map-diagnostic-eight.log`` captures a failing direct mapping,
not a staging copy: ``staging=0``, coherent/cacheable memory flags ``0xf``, and
BO write batch43 with Vulkan ``last_finished=43``. Bytes become correct
without another GL call. Both experimental Zink race corrections were active
in that diagnostic. Disabling the deferred fragment-barrier optimization also
reproduces the issue with the original library. These observations further
narrow the failure but do not yet assign it to the kernel or Vulkan driver.
