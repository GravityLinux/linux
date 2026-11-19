# Shared scheduled GPU multi-client performance plan

Requested by the operator on 2026-11-03. Applies to G16S (M4 Pro), G16C
(M4 Max), and G17P (A18 Pro/Neo). Hardware access remains M4 Max RID 0 only,
serial JPWWJRX2N0, all 16 CPUs. Camera extraction is deferred.

Baseline: Linux image commit `582beabe96db`; GPU Rust sources are identical
to `replacement/210-gpu` at `178cbe6e2bd2`. Initial audit and actual-selector
witness are in the bootloader checkout's
`build/gpu-multiclient-audit-20261103/`.

## Implementation order

Operator steering: investigate on-demand backing growth before increasing
retained context counts further. The 20 x 8 MiB operand pool, 16 MiB
directory and 256 preemption save slots are eagerly backed reference sizes,
not established minimums. Account allocator rounding (about 193 MiB for the
Max compute private layout), preserve a working-size fallback, and qualify
firmware growth/retirement separately from initial-size experiments.

1. **Retain independent compute queues for active clients.** The common
   generation selector currently reassigns an unrelated idle queue before
   allocating a free one. Prefer an exact generation, then this logical
   queue's retired generation, then a fresh owner within a memory budget,
   then an unrelated retired owner. Preserve a reconfiguration already in
   progress. Use least-recently-used selection for eviction. The budget is
   for preferential retention, not a new concurrency or API queue limit:
   when every owner is busy, existing capacity remains available. Reuse must
   retain all firmware cleanup, transport, dependency and CPU visibility
   leases. Max's private compute backing is about 193 MiB per owner before
   transport/page tables, so retention must not grow without a bound.

2. **Retain render queues and execution contexts across client switches.**
   Separate logical queue/context identity from exclusive parameter-buffer
   pool use. Preserve each active client's queue records, executable root,
   ASID and private state within a bounded cache. Begin with serialized pool
   use; additional rings alone do not permit overlap. Pool/growth state must
   have explicit ownership through retirement and context close. Keep the
   known-failing `partial_independent_owner` mode disabled, including on Pro.
   Queue ownership/selection belongs in shared code; wire differences stay
   in existing versioned encoders. Do not add a Max-only scheduling policy.

3. **Carry immutable snapshot identities.** The frontend already tracks
   mapping generations, while retained backend checks repeatedly compare
   all bindings and BO pointers. Propagate an identity tied to owner, engine
   view and mapping generation. Invalidate on bind, unbind and replacement,
   including PRIME; never use owner identity alone as proof of equality.

4. **Reduce wake storms and repeated global polling.** Replace the IRQ-side
   quadratic search for queue heads with explicit per-queue head/ready state.
   Poll device firmware once per event, then target affected queues using a
   fair ready list. Preserve external fence callbacks, error propagation and
   the bounded fallback for coalesced/lost firmware notifications.

5. **Avoid unchanged CPU-map membership work.** Cache validated BO-set
   membership by immutable identity rather than removing/reinserting every
   tree node on each preparation/retirement pass. Keep memory ordering and
   map pruning/BO reclamation. Coherent caller mappings already avoid a
   per-submit data-cache sweep; that optimization is already implemented.

6. **Gate normal switch logging.** Successful retained-root handoffs should
   follow `submission_log`; errors remain visible.

## Qualification

- Test actual selector/ownership methods, including exact reuse, pending
  maintenance, busy owners, budget exhaustion, eviction, failure and close.
- Build both shared wire variants; verify generated layout assertions remain
  intact. Do not infer Neo hardware validation from host tests or Max runs.
- Use short tests during implementation. Measure two, four and eight clients
  with production Vulkan and Sway/DCP DMA-BUF presentation, including the
  compositor as a GPU client. Start with fixed mappings, then bind/unbind and
  close/reopen churn. Preserve the existing known-good native boot assets.
- Record setup separately from steady state: CPU time per submission,
  queue-wait percentiles, runtime mutex wait/hold time, rebind/control counts,
  retained memory and missed 120 Hz deadlines. Display FPS alone can hide
  CPU and latency gains. Exclude full-image validation scans from timing.
- Require correct pixels/guards, dependency ordering, bounded memory,
  isolation and clean error handling. Run sustained churn only after short
  cases pass and immediately before declaring the changes qualified.

## Progress

- [x] Audit current shared code and preserve this plan.
- [x] Step 1 implementation and host qualification.
- [x] Step 1 M4 Max execution and performance comparison.
- [x] Step 2 implementation and host qualification.
- [x] Step 2 M4 Max production qualification (operand-bank fix and committed-source default boot; see 2026-11-04 qualification).
- [x] Steps 3–6 implementation, host checks and M4 Max integration qualification.

## Current receipts

- Plan commit `aeed0605ef6a`; compute retention commit `70790a6c02ac`.
- Actual selector: 524290 original cases plus retained-owner/LRU/budget and
  busy-capacity cases pass. Actual compute admission test passes. Encoder
  comparison passes 1056 tagged-Neo and 528 Max cases, all bytes/addresses.
- Native kernel build succeeded (both scheduled backends compiled), candidate
  Image SHA256 `cffcb33342e299beb77aa02800281d87b6296d5657b40ccf9ba413658bbea906`.
  Builder source overlay was restored after build; baseline image retained.
- M4 Max baseline: two independent Vulkan processes, 64 KiB GPU copies,
  16 timed iterations each after warmup: 2.29/2.30 ms mean submit-to-fence;
  complete output checks pass, dmesg confirms both use physical bank 0.
  This is a compute microbenchmark, not display performance qualification.
- Builder VPN became unavailable before candidate download. Device remains
  on the known-good native image, all 16 CPUs. No candidate hardware claim.
- On-demand growth investigation: tested reactive render PB growth is not
  independent compute operand growth. The quiescent operand-extension method
  currently has no production call sites. Live extension needs qualification;
  directory backing and save-slot demand are separate opportunities.

Step 2 foundation commit: the queue event/completion namespace is now
explicit rather than inferred from the PB pool slot, in both render encoders.
Existing Neo output compares byte-identically over 3459953 bytes; 30
independent event/pool cases pass. Max compares 576 complete encoder cases.
This does not yet enable persistent render queues or concurrent pools.

## On-demand backing investigation

Prioritize this before increasing retained owner counts. The independently
accounted Max allocation is 201932800 bytes (192.578125 MiB): 160 MiB operand
buffers, 16 MiB directory, 16 MiB rounded preemption slots and 0.578125 MiB
other state. These are reference allocation sizes, not minimum requirements.

The source-built shim's `ComputeOperands.grow()` already implements explicit
quiescent extension: allocate and map additional 8 MiB buffers, append table
entries, register opcode 0x20, then wait for the owned reply, control retirement
and firmware-applied count. Preserve the live directory and cursors. Historical
20-to-64 pressure execution used newly allocated buffers and checked exact
shader results and guards. This supports reusing the extension transaction;
it does not qualify an automatic demand trigger or concurrent extension.
The current kernel `scheduled_operand_runtime::grow_operands` has no callers,
and independent queue owners need their own integration with the same leases.

The directory is a recycling ring, not just an initial page list. Historical
source pressure advanced its active window from entries 0..40960 to
290008..330968 without adding operand buffers. Mapping only the initial
320 KiB prefix while advertising the existing 16 MiB capacity would fault.
Reducing ring capacity is a separate firmware-geometry experiment; sparse
backing requires ahead-of-cursor publication or a qualified fault protocol.

Evidence in the bootloader checkout:

- `build/m4max-gpu/compute-operands-runtime-pressure-v3-independent-audit-20261019.json`
  records 20-to-64 explicit growth and actual use of fresh buffers.
- `build/m4max-gpu/compute-operands-default20-v1-independent-audit-20261019.json`
  records private-memory pressure with only completion events and no automatic
  growth/limit request. Hardware throttling is an inference, not established.
- `build/m4max-gpu/compute-directory-reuse-frontier-v2-independent-audit-20261019.json`
  records the recycled directory entries and controlled tail-restoration test.
- `build/m4max-gpu/m4max-sampler-v19-production-python-20261021/m1n1/g16c/compute_operands.py`
  is our preserved source-built extension implementation, with its inventory.

Investigation sequence:

1. Test initial counts 20 (control), then 1 in the isolated source shim, all
   16 CPUs. Reuse the independently checked 256/16384-thread private-memory
   pressure shader, compare the complete output page and guards, record all
   owned firmware events and teardown. Restore native Linux afterward. This
   measures sizing behavior, not the new Linux scheduler.
2. Compare pressure throughput and occupancy as well as correctness. A smaller
   pool may execute correctly by limiting concurrency. Do not infer that a
   passing copy or pressure shader establishes an appropriate production size.
3. Port quiescent extension into independent queue ownership, preserving root,
   ASID, bank, support identity and original cursor state. Preallocate off-lock;
   reject cleanly before publication and retain backing on uncertain failure.
4. Determine the demand signal experimentally. Do not reinterpret render PB
   event 6 as an operand request, or blindly enable the opaque `notify` field.
   If exhaustion throttles rather than requests growth, use measured workload
   pressure and bounded between-job growth instead of inventing a handler.
5. Back preemption slots at software admission and reuse only after full
   retirement. Initial lazy mapping alone will fill all 256 slots over time;
   steady-state reduction also needs a qualified smaller live slot set.

The retention accounting no longer sums every allocation twice per stage.
`Memory` tracks rounded physical bytes at allocation/ownership transfer; host
tests execute its actual methods through prepared reuse, detached transfer,
allocation/transfer failures and overflow. Kernel build/hardware performance
qualification of this follow-up remains pending the builder connection.

### New M4 Max sizing and growth results

Actual source-built shim execution on RID 0, serial JPWWJRX2N0, all 16 CPUs.
Preserved v19 source inventory verifies unchanged. The diagnostic overrides
only the initial operand count; all original fixture manifests, output bins,
full output pages, guards, stamps, syncs and retirement checks remain enabled.
These are not Linux candidate tests or display performance measurements.

| Initial buffers | Small: 256 threads | Large: 16384 threads | Growth |
| --- | --- | --- | --- |
| 20 | Pass | Pass twice | None |
| 10 | Pass | Pass twice | None |
| 8 | Pass | Firmware timeout | None |
| 5 | Pass | Firmware timeout | None |
| 1 | Firmware timeout | Not reached | None |
| 5 | Pass | Pass twice | Quiescent extension to 10 before large shader |

The 5-to-10 case was repeated with initially zero samples at the beginning,
middle and end of every new buffer. GPU execution subsequently changed samples
in all five new buffers (indices 5..9). The owned FList reply reports applied
count 10; queue recreation preserves the same pool, and teardown invalidates
the roots and frees every allocation exactly once. Operand backing therefore
starts at 40 MiB and reaches 80 MiB for this case, versus the reference 160 MiB.
No automatic sizing or arbitrary-workload minimum is established by this test.

Insufficient pools produce kind-4 timeout events without growth requests.
A controlled diagnostic setting support word +0x28 to one (the field currently
named `notify`) with five buffers still times out on the large shader without
a growth request. This bit alone does not establish a recoverable demand
protocol. It remains unchanged in production.

An earlier attempted pre-first-submit extension was rejected by the host's
live-owner check because the VM root was not published. It is not a firmware
growth failure; the corrected between-job tests above use the ordinary live
root and unchanged ASID/cursors.

Current UAPI compute submissions expose an opaque CDM stream and no explicit
scratch-demand hint. Mesa already knows shader private-memory extents. The
next integration should either qualify an actual firmware pressure protocol
or carry a validated resource-demand hint and grow before admitting the
larger workload. Waiting for the observed timeout is not a qualified retry
strategy. Preserve default allocations until the new path passes Linux GPU
and compositor/DCP DMA-BUF tests.

Exact commands, gates, logs, event records, fresh-buffer samples, results and
native restore receipts are in the bootloader checkout's
`build/gpu-multiclient-audit-20261103/operand-sizing*`. The phase-4 orchestration
records the repeated growth/use case and the diagnostic notification case.
Host accounting fix is commit `df858d36610d`; the prepared ten-file kernel
overlay includes this and render namespace commit `d189a2bbd8a9`.

Independent audit `operand-sizing-audit.json` rebuilds the C CPU oracles and
checks their agreement with the independent JavaScript results, then compares
every complete GPU output page, the expected negative cases, source inventory,
growth receipt, fresh-buffer samples and exact teardown. All assertions pass;
automatic sizing and Linux candidate qualification remain false.

After the last sizing test, known-good native Linux was restored and verified
with CPUs 0..15. Two 240-frame Vulkan Wayland clients plus two alternating
Vulkan copy clients pass together in Sway. The screenshot shows both rendered
cubes; DCP state and DMA-BUF attachments confirm the panel presentation path.
No GPU fault/timeout is present in this boot's dmesg. This is the retained
baseline, not a performance gain claim. Receipts are in `post-sizing-baseline/`.

### Single-user macOS firmware-demand investigation

Operator explicitly requested a hypervisor pressure experiment. New native
reference `hv-operand-demand-v7` runs macOS 26.6.2 in single-user mode, verifies
all 16 CPUs, and executes our minimal fill followed by 16384 invocations of
the mutable 4096-word private-array hash shader. Complete outputs, guards and
driver stamps 256/512 pass. The host allows macOS to execute during the heavy
dispatch, so a firmware request can be serviced; the old capture hook instead
held macOS stopped while polling GPU completion and was unsuitable for this.

Trace coverage includes AP control publications, ASC mailbox reads/writes,
event records when first visible to the AP and when acknowledged, and operand
support accesses after the first FList publication. Full stable UAT images
were captured before initialization and at both kicks. The final image has
2107.8 MiB of mapped DRAM. Header snapshots also cover first-use and extension
boundaries. No Apple executable instruction was decoded or changed.

Observed chronology:

1. First fill completes; firmware event 1 becomes visible at trace sequence 53.
2. macOS publishes opcode 0x20 at sequence 55: context 1, bank 0, class 2,
   previous 20 buffers, added 240 buffers, support +0x28 zero.
3. Heavy kick is released at sequence 65. Firmware has applied count 260 in
   the captured pre-kick state. Its matching FList acknowledgement (event 13)
   is later consumed by macOS.
4. Heavy output/stamp 512 completes. The complete receive trace contains two
   event-1 completions and one event-13 receipt; no allocation/growth request
   appears. There is no additional FList extension during the heavy work.

This shows pre-submission host extension in this workload. It does not prove
the firmware lacks another pressure mechanism. In particular, header tracing
begins at the first FList control, so earlier shared-state reads are not covered
by that part of the trace. Event and mailbox tracing are active before it.

To test actual shortage, independently reran the previously qualified full
native Metal capture from 2026-10-19, including its complete pre-FList image.
The original opaque 20-to-260 replay again passes both outputs, all guards,
stamps and retirement in plain m1n1, all 16 CPUs. This is the earlier capture;
the new v7 capture has not itself been independently replayed.

Then construct valid initial directory/support/dispatch geometry from source
before first queue use, retain that live owner through the later AP restores,
and withhold the 240-buffer registration. An observed ordinary tick advances
the control slot; no FList receipt is forged, and all original backing/UAT
mappings remain present. GPU execution continues with the restricted count:

| Registered buffers | First fill | Heavy native shader | Firmware events |
| --- | --- | --- | --- |
| 20 | Pass | Exact output/guards, stamp 512 | Completion 1 |
| 5 | Pass, stamp 256, fully retired | Timeout, remains at stamp 256 | Timeout 4 |
| 5, candidate support +0x28 set to 1 | Pass, stamp 256, fully retired | Timeout, remains at stamp 256 | Timeout 4 |

The support count and dispatch capacity remain 20 or 5 respectively: the
firmware does not replenish them in these cases. Neither grow event 6 nor
limit event 7 appears. The candidate word's semantics are not established;
this experiment is not a production enablement of a notification flag.

The 20-buffer runner hit a host-only post-execution sampling exception because
the old audit read an unpublished table entry. Its complete saved output,
guards, stamp 512 and completion were independently verified. The sampling
lookup is corrected in the next harness version. Do not count either deliberate
5-buffer timeout as a passing workload or infer universal absence of a request
path. The independent audit records these distinctions explicitly.

Evidence under the bootloader checkout's `build/gpu-multiclient-audit-20261103/`:

- `hv-operand-demand-v7/demand-audit.json` and `demand-protocol.jsonl`.
- `hv-operand-demand-v7/first-boundary/uat/` and `hv-operand-demand-v7/uat/`;
  both independent capture validators pass checksums, mappings and outputs.
- `operand-native-replay-control/`: original whole native replay passes.
- `operand-native-restricted-v2-20/`, `operand-native-restricted-v2-5/`,
  `operand-native-restricted-v3-5-notify1/`: controlled pressure observations.
- `restricted-native-pressure-audit.json`: independent output/guard, source
  geometry, count, event and negative-result checks pass.
- `hv-demand-tools-v6/` and `operand-demand-replay-tools-v2/` / `-v3/`:
  exact commands, source and launchers. Native reference command:
  `DEMAND_ATTEMPT=hv-operand-demand-v7 .venv/bin/python -u build/gpu-multiclient-audit-20261103/hv-demand-tools-v6/run-reference.py`.

Conclusion: no usable firmware-requested compute-operand growth mechanism is
established. CPU-driven quiescent extension remains qualified. Pre-admission
growth informed by workload requirements is the demonstrated route to pursue;
do not implement timeout-as-retry or reduce production defaults on this evidence.

After testing, restored the original native Linux image and verified CPUs 0..15,
two Wayland Vulkan cubes plus two GPU copy clients, Sway DMA-BUF presentation
through DCP and clean GPU dmesg. Receipt: `post-firmware-demand.json`.


## Completed shared implementation and M4 Max qualification

Final source is on `experimental/gpu-multiclient-20261103`. The build contains
all production changes in the plan, using the existing versioned wire builders:

- Compute retains hot generations within `compute_cache_mb=512`; foreign idle
  eviction is LRU. Actual rounded allocation accounting is constant time.
- `render_cache_slots=2` retains two executable render roots, ASIDs, queue
  records and private/growth state. Both backends use the common exact-match /
  LRU pool selector. The cache is a context-count bound; it does not shrink the
  reference operand allocations. `render_cache_slots=1` restores immediate
  single-context reuse. Explicit larger values are capped at the existing
  15-slot namespace.
- Cached contexts acquire one exclusive render lease. A different PB slot
  cannot publish until the previous GPU wave and CPU visibility leases retire.
  `partial_independent_owner=0` and `alternate_queue_pairs=0` remain the
  defaults; caching does not turn on their overlapping-pool experiment.
- Admission snapshots allocate a unique Arc identity for their owner, engine
  view and mapping generation. It travels through deferred/materialized
  clients, templates, rebind commits and completion leases. Mapping mutations
  invalidate it; unqualified legacy clients retain structural comparison.
- Validated CPU-map membership uses that same immutable identity. An unchanged
  set needs only ordering; a changed set still validates, prunes old vmaps,
  drops their BO references outside the mutex and permits backing reclamation.
  Tokens themselves retain no BOs or VM roots.
- IRQ wakes iterate an explicit list of queue heads, rotating the starting
  queue. Admission reserves both membership collections before publication;
  retirement promotes only the actual head. Input/dependency callbacks and
  fatal wakes retain their existing ownership rules.
- Workers share a device-wide receipt poll per wake epoch under the runtime
  mutex. The existing 20 us control / 200 us execution fallback remains. A
  notification racing a poll is not consumed by recording a later epoch.
- Successful root, caller and transport handoff logging obeys `submission_log`.
  Errors remain visible. No firmware ABI or UAPI constant was guessed.

### Final build and execution

The exact final Image SHA256 is
`84d7a96209ba278ff7a6837997ce8655041257435ae9a5936a0eae36869edcfa`.
Linux reports build #20, all CPUs 0..15. Direct native m1n1 kboot used the
unchanged installed DT and initramfs, with root on the installed Btrfs system.
Both wire backends compile. The original installed boot assets/default remain
available; this candidate is running from the explicitly loaded development
Image. The shared build host's 19 overlaid sources were hash-verified restored
and their dependency timestamps invalidated for its next build.

Host qualification executes actual production methods: snapshot equality,
CPU-map pruning/failure/release; 20,000 interleaved queue-head admission,
retirement and wake transitions; 1,024 render selector combinations plus actual
cross-pool GPU/CPU lease gates and native/legacy policy isolation. These pass
on the local ARM host and the x86 Linux builder (Rust 1.94). The final tests also
check cold/warm preparation and changed-selection replacement. Encoder
comparisons pass 576 complete Max render cases, 1,056 tagged-Neo compute cases
and 528 Max compute cases, including complete bytes and write addresses.

Native M4 Max tests use production Honeykrisp Vulkan 1.4, with Zink for GL,
and Sway at 3456x2160, scale 2, nominal 120 Hz. Two glmark2 windows are visible
in screenshots. DCP has a Sway-allocated scanout framebuffer, and its DMA-BUFs
are attached to `288000000.gpu`: this is the regular GPU/compositor/display
path, not an off-screen performance test.

Short performance comparison (same three scenes, 2 s each, 1600x900 windows,
same Mesa and compositor; `submission_log=0` in both kernels):

| Measurement | Original image | Final image |
| --- | ---: | ---: |
| One on-screen client, short glmark2 score | 796 | 972 |
| Two on-screen clients, respective short scores | 193 / 197 | 413 / 410 |
| Two alternating copy clients, mean submit-to-fence | 2.829 ms | 1.146 ms |
| Four alternating copy clients, mean submit-to-fence | 2.741 ms | 2.743 ms |
| Eight alternating copy clients, mean submit-to-fence | 2.668 ms | 2.620 ms |

The on-screen two-client gain is about 2.1x. These are short application
throughput scores, not a full glmark2 suite or measured panel presentation FPS.
The four/eight-client copy results intentionally exceed the conservative
compute retention budget; no steady-state speedup is established there.
Single-client copy timing varies (1.039 ms original, 1.178 ms final in this
pair); do not infer a gain from it. Aggregate system busy time during the
presentation test fell from 1,717 to 1,608 USER_HZ ticks while application
throughput increased. No exclusive driver CPU or mutex latency claim is made.

Correctness/recycling qualification on this exact final build:

- Two independent Wayland Vulkan cubes plus two alternating Vulkan copy clients
  finish together through Sway/DCP with correct displayed geometry.
- One, two, four and eight independent Vulkan copy clients pass full output
  comparisons after every timed copy.
- `discard_texture.c` runs two concurrent Zink clients using exactly the
  texture-value-live-after-discard pattern. All 512 frames / 2,097,152 pixels
  match the CPU oracle. This qualifies the serialized cache on Max, not the
  legacy overlapping-pool experiment or another SoC.
- Eight close/reopen render clients run alongside four copy clients each:
  2,048 timed copies pass, crossing finite queue storage boundaries. Shmem
  oscillates rather than accumulating. After 33 small reclamation clients,
  Shmem is 269,168 kB, below its 351,872 kB pre-churn baseline.
- The final boot's complete GPU dmesg has no fault, timeout or rejected retire.

Independent audit: bootloader checkout
`build/gpu-multiclient-audit-20261103/qualification-final.json` and
`audit-final.py`. Raw native receipts are hash-verified in
`qualified-final/`; archive SHA256
`dc691e3cfd29533975b63510eb957aabc1cb522f9d1c68ec3f47de022d8bc363`.
Exact direct-boot controllers, build manifests/scripts and host test receipts
are alongside them. The earlier `identity-ready` boot-entry attempts selected
the original kernel and are excluded from candidate claims. Earlier churn
logs were lost across a hard reboot and are superseded by the final archived
churn receipts. Build timestamp reuse was detected and corrected before
candidate execution.

Neo/G16S hardware was not accessed. Their shared source and wire builders are
covered by compile/host comparisons; their hardware qualification remains
separate. Automatic firmware-requested operand growth remains unestablished;
all reference operand/directory/save-slot sizes are preserved. Explicit
quiescent growth and the earlier pressure evidence remain documented above.

## Native Firefox regression, 2026-11-04

Real desktop playback invalidates the earlier Step 2 qualification: the
serialized two-context cache can hang the GPU even though overlapping pools
remain disabled. Keep `render_cache_slots=1` as the shared default. The
immutable snapshot, CPU-map, fair wake/poll and compute-retention changes stay
in place. Explicit larger cache sizes remain diagnostic opt-ins, not qualified
production settings. No Neo hardware was accessed.

On M4 Max, exact Image SHA256
`84d7a96209ba278ff7a6837997ce8655041257435ae9a5936a0eae36869edcfa`,
the operator's Firefox/YouTube desktop froze with firmware timeout at uptime
185.812s (context 1, PB 0). Linux/SSH remained responsive. Rebooting the same
image with `asahi_neo.render_cache_slots=1` played the same YouTube video at
2560x1440 through Sway/DCP for 90s (2257 video frames, 9 dropped) with no GPU
errors. Re-enabling only `render_cache_slots=2` reproduced a firmware timeout
at uptime 25.347s (context 63, PB 1) as YouTube opened. Both captures have a
tiling start timestamp and no fragment timestamp; the precise missing
firmware lifecycle rule is not established. This is a tested containment of
the regression, not a claim that the multi-context lifecycle is repaired.

Preserved evidence is in the bootloader checkout under
`build/m4max-devices/av-freeze-20261104/`: original and comparison dmesg,
browser playback counters, kernel/boot labels and audio control captures.

### Multi-context correction, 2026-11-04

The single-context containment above is superseded by the isolated G16C
Work15 context-index correction. The second retained pool has PB/event slot1
and ASID63; field +0x56 needs UAT context index62, not PB slot1. The existing
compute wrapper already distinguishes these identities. Both render wrappers
now derive the field from the selected context; bank initialization and
scheduler policy are unchanged. The two-context default is restored.

See `QUALIFICATION.multi-context-20261104.rst` for the failing old-source
control, both generated ABI checks and native Firefox/client-turnover results.
The fix retains the multi-client performance improvement; Neo hardware was
not accessed. Other firmware/lifecycle experiments remain separate opt-ins.

The continued #32 playback run subsequently timed out at uptime383.492s in
PB0/ASID1. The wrapper correction is therefore insufficient; the production
qualification above is withdrawn and the one-context default/installed #27
kernel are restored while duration-matched controls and lifecycle diagnosis
continue. Do not treat checkpoint05a462b99401 as a completed fix.


## Completed retained-context qualification, 2026-11-04

Code commit `3c24a423b67d` fixes the context-private operand bank selection.
Native capture DATA proves registers `0x14308`/`0x14048` select the support's
bank; the old runtime left them at zero. Support, render directory/capacity,
registration and both programs now agree, without a separate scheduling path.
The two-context default is restored, while overlapping pools remain disabled.

Two native M4 Max runs passed sustained unmuted video through uptime 630 s
and 470 s, independent changing-upload and discard pixel checks, and normal
Sway/DCP DMA-BUF presentation. The final committed-source kernel #39 has no
cache override, passed two native Vulkan windows and on-screen one/two/four
client workloads, and is installed with the #27 fallback preserved. The
operator reported no recurrence of the transient white video rectangles during
the corrected run. See `QUALIFICATION.multi-context-20261104.rst` and private
`multicontext-fix-20261104/qualification.json`. Neo hardware was not accessed.

## Further multi-client resize findings, 2026-11-04

The operator's later 16-vkcube/YouTube resize test still shows delayed motion
and occasional stale video frames. The earlier corrected-bank qualification
covers that specific hang fix; it does not close these newer observations.

Commit `012da5193343` reduces redundant admission preparation and elects one
fast session poller. Clean kernel #47 on M4 Max reduces normal scheduler
callbacks from about 31k/s to 14k/s and system CPU from 5.9 to 2.3 cores. DCP
commits still run at about 25/s with 16 cubes, returning to 120/s when they pause.
Keep the two-context default: a boot-only fifteen-context trial did not solve
frame pacing and increased CPU/memory use.

The queued framebuffer/PBO resize probe reproduces an intermittent stale
readback on both original #39 and the newer scheduler. Native Vulkan output
controls pass. Two Zink state races have deterministic source-level repros,
but correcting them does not eliminate this readback failure; their changes
remain separate and uninstalled. Investigate buffer-map staging-copy waits
and the emitted submission dependencies before attributing this to a single
layer. See the qualification document's follow-up section and private
`resize-latency-20261104/` receipts. These open issues prevent marking the
multi-client goal complete. All follow-up tests stay muted; Neo remains
restricted.

A later low-overhead map diagnostic captured a failing readback with no
staging allocation: both original and mapped memory have flags `0xf`
(host-visible, coherent and cached), and the BO's write batch equals the
completed Vulkan timeline value. The original mapped bytes then become
correct without another GL call. This rules out a missing map-time staging
wait for that occurrence; investigate the actual GPU completion/visibility
and submission graph. The fragment-barrier opt-out also reproduces the
failure, so that optimization alone does not explain it.
