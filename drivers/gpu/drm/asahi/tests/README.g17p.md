# G17P UAPI integration and source checks

The frontend accepts copied commands and publishes pending output fences before
returning. Firmware events and DMA-fence callbacks advance the backend; pending
external inputs have no GPU execution timeout. Jobs retain their file, VM and
GEM references until completion. Published backing pages remain retained.

The completion bar is hardware integration through the Asahi UAPI; unit or
source serialization checks are not required. Run consecutively in one driver
session, using the same complete output/guard/timestamp/fence oracles:

```sh
g17p-drm-memory && g17p-drm-retained-wave &&
g17p-drm-render-vm --cleanup && g17p-drm-compute-vm --cleanup &&
g17p-drm-mixed-vm && g17p-drm-mixed-batch && g17p-drm-memory
g17p-drm-async --backend && g17p-drm-async --close
g17p-drm-compute-stress --compute-count 4096
g17p-drm-render-vm --stress
```

The independent-pool fixture submits fifteen separate queues in one VM, with
120 independent full-image oracles, per-job output fences, four GPU timestamps,
and timestamp guard regions. Generate its caller data from the same authored
payloads as the retained-render tests (opaque executable pages stay unchanged):

```sh
python3 drivers/gpu/drm/asahi/tests/make_g17p_multi_pool_workload.py /path/to/m1n1 drivers/gpu/drm/asahi/tests/g17p_drm_multi_pool_workload.h
gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -Iinclude/uapi/drm -Iinclude/uapi -o g17p-drm-multi-pool drivers/gpu/drm/asahi/tests/g17p_drm_multi_pool.c
g17p-drm-multi-pool 1 --require-overlap
g17p-drm-multi-pool 4096 && g17p-drm-multi-pool 4096
```

The short overlap mode requires at least three simultaneous TA-start through
FR-end intervals. The long runs verify every image and fence through repeated
descriptor, context, status, pool-record, scratch and transport reuse, including
the shared TA249..255 / FR0..1 descriptor-address collision. They also cross
the global u16 optional-ordinal wrap. Each finite storage slot remains leased
until its own render retires; a conflicting slot is skipped when another is
free. Physical backing remains retained.

Build compute-stress from g17p_drm_retained_wave.c with
`-DG17P_TIMESTAMP_CAPACITY=4096`; this increases the integration fixture's
caller timestamp storage. The driver uses normal resource leases and retired
transport rotation. Both compute profiles recycle two finite pointer/item
backing banks after the preceding wave retires.

`g17p_drm_wait.h` adds explicit output-fence waits to the older integration
programs. `g17p-drm-async` bypasses that adapter for submission and proves
unsignaled acceptance, copied commands, same-queue ordering, independent-queue
progress, binary/timeline completion and dependency error recovery. Its backend
mode holds 16 accepted jobs beyond two seconds, destroys their queues, closes
output GEM handles, checks pending VM_UNBIND returns EBUSY, interrupts a UAPI
wait, then verifies every output and independent completion fence. The close
mode closes the DRM file before signaling the accepted job's imported input.

Run owned-fault, pool-limit/recovery and native C/R/C cases on separate boots
with their documented diagnostic parameters below. A fault fence must carry
its own error; a healthy peer's image is never a substitute for that check.

The remaining sections retain source-port comparisons and earlier hardware
measurements for reference. Their historical synchronous-stage limits and
unfinished-stage statements describe those recorded revisions.


These host tools compare the Rust source used by the kernel with the current
m1n1 Python shim. They do not connect to hardware or load captured memory.
From the Linux checkout, with Python 3 and `rustc` on `PATH`:

```sh
python3 drivers/gpu/drm/asahi/tests/import_g17p_constants.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/import_g17p_layout.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/import_g17p_topology.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/check_g17p_abi.py /path/to/m1n1
python3 drivers/gpu/drm/asahi/tests/check_g17p_initgraph.py /path/to/m1n1
python3 drivers/gpu/drm/asahi/tests/check_g17p_render.py /path/to/m1n1
python3 drivers/gpu/drm/asahi/tests/check_g17p_render_graph.py /path/to/m1n1
rustc --edition=2021 drivers/gpu/drm/asahi/tests/g17p_vm.rs -o /tmp/g17p-vm-test
/tmp/g17p-vm-test
```

The ABI check compares 452 whole objects with `g17p_initdata.py`, including
non-aligned address fields, both root sizes, control-only channels, optional
hardware fields, and overlapping status/configuration writes. The graph check
executes the original `build_initdata` function with a bounded in-memory arena.
It compares 48 complete allocations, both channel tables, low/high aliases and
all final page attributes at four base addresses and performance-table inputs.
Malformed storage, unaligned bases and address overflow are rejected before
the graph builder changes output bytes.

The graph oracle extracts only the two builder function definitions from the
experiment's AST. It never imports that script's hardware setup or entrypoint.
The import tools copy named source constants from `g17p_initdata.py` and
`g17p.py`; omitting `--check` regenerates the two Rust constant files.

These checks prove serialization parity. They do not prove firmware startup,
GPU execution, rendered output, or the later asynchronous stages.

The render oracle requires Python 3.10 or newer to import the source dataclass.
It compares the 73/89 ordered main programs, all three partial programs,
class-2/class-4 control objects and full descriptor bodies by executing the
actual `G17PWorkBuilder.item` constructor against an in-memory allocator.
Its 192 parameter sets vary dimensions, layers, samples, every address,
pipeline low bytes, status namespaces, duplicate registers, timestamps,
pool selection, alias relocation and structural-tail options. The kernel's
integer-only merge calculation matches Python for all 16384 supported
dimensions. Unsupported USC bases fail before any output mutation.

The render graph oracle compares complete pool arrays, leaf pages, packed
shared/optional records and operand directories. Its 512 queue-context cases
cover ordinary, partial-opening, dependency and extended-context forms,
explicit points/completion values, locator relocation and retained bytes on
slot reuse. Event initialization is limited to the host's 64-byte record;
adjacent bytes remain intact. These constructors are used by the synchronous render runtime. Native output
checks are separate from the serialization tests below.

The topology import also generates the T8140 source-memory DT include. It
reserves the union of prescribed RAM leaf/table pages, including the second
firmware root, while excluding MMIO and the ADT-owned L2 page. This is an
address-only import from `g17p_source_topology.py`. Linux validates the full
reservation list before any future physical placement or firmware publication.

The VM harness runs the actual kernel constructor against bounded synthetic
RAM. It checks 3203 firmware, context-0 alias and MMIO leaf translations, every
initdata byte after physical placement, both distinct private firmware roots,
and all three context tags. A pre-existing shared-L2 mapping must fail before
context publication. It does not simulate firmware execution or cache coherence.

Retained direct-compute metadata (source-only, no device access):

```
python3 drivers/gpu/drm/asahi/tests/check_g17p_compute_lifecycle.py /path/to/m1n1
```

The direct runtime reserves the source bootstrap's 258 logical command
placements. It retains the queue identity while switching pointer/item backing
at ordinals 128 and 256, using a two-slot pool with ownership/readback and
inactive-storage integrity checks. Descriptor/context/scheduler storage follows
the source's 240/256/36-record reuse rules. Completed caller
bindings can be replaced transactionally, including a handoff between logical
VMs/files over the retained shared root in native slots 2/3. The source-profile test covers later wrap
metadata too; it does not claim that runtime wrap/handoff is implemented. The
native `g17p-drm-compute` test changes its input arrays and poisons its output on
each submission, checks all 16512 floats and every tail guard, then verifies that
admission exhaustion returns EOPNOTSUPP without touching the output. The `--rebind` option replaces the output GEM at the same DVA after every
completed dispatch and retains/checks all older CPU images and guards.
`g17p-drm-compute-vm` alternates two independent DRM files with colliding DVAs,
separate timestamp/fence ownership and distinct noncolliding sentinels.

Native integration tests run from a RAM initramfs on the explicitly selected
development target. Generate the caller workload headers from pure Python
constructors (no proxy imports or captured firmware objects):

```sh
python3 drivers/gpu/drm/asahi/tests/make_g17p_compute_workload.py /path/to/m1n1 /tmp/g17p_drm_workload.h
python3 drivers/gpu/drm/asahi/tests/make_g17p_compute_workload.py /path/to/m1n1 /tmp/g17p_drm_batch_workload.h --batch-count 32
```

Cross-compile `g17p_drm_compute.c` and `g17p_drm_batch.c` with the installed
kernel UAPI headers, `-I/tmp`, `-O2 -Wall -Wextra -Werror -static`. The tests
share the memory helpers and optional sync/timestamp checks in this directory.
On the target:

```sh
g17p-drm-compute --sync --timestamps
# On a separate fresh boot:
g17p-drm-batch
```

`--sync` requires CONFIG_SW_SYNC and debugfs solely to create imported external
dependency fences. It checks malformed/missing/failed binary and timeline
inputs, timeout/interruption, same-file progress while waiting, output fence
waits/exports/query and preservation after rejection. Compute remains on the
real GPU. Without `--sync`, the compute test needs no software timeline device.

`--timestamps` checks ordered start/end pairs in the middle page of a caller
GEM BO, offset/alignment validation, live object unbind/rebind, all earlier
pairs and every surrounding guard byte. Timestamp aliases retain backing through every recorded fence. Unbound ranges
become reusable after successful completion; pending or failed owners remain
retained. The 64 MiB aperture returns ENOSPC on exhaustion.

The batch test uses four submissions of eight independently owned graphs,
checks all 2048 expected floats, each output guard, timestamp pairs and one
completion-fence pair per batch. Invalid last-command flags/barriers and a
batch exceeding remaining capacity must leave every output/fence unchanged.
It includes the sync dependency checks and timestamp object rebind. Its final
rejection checks use malformed commands/barriers; the old 32-command transport
limit is gone. The 258-command compute test checks a three-command batch when
only two reserved command placements remain, then executes those two commands
and verifies refusal of further publication.
The host VM harness also checks exact timestamp alias backing/attributes,
collision rejection and aperture/alignment boundaries.

These native compute tests have passed on T8140, including retained-memory
audits after file close. The 258-command run passes both transport handoffs, descriptor/context/channel
wrap, all 516 timestamps, 258 binary/timeline fences, input dependency errors
and waits, exact results and post-close retained-memory audit. Extending beyond
the source bootstrap storage window, simultaneous native contexts and
asynchronous frontend/backend behavior remain unfinished.

The synchronous render path accepts render-only and mixed batches on one retained owner.
`g17p-drm-render` performs 32 pressure renders, replacing eight guarded output
GEMs at the same DVAs on each draw and preserving every earlier image.
`g17p-drm-render-batch` tests the current 64-command owner limit using 32 pairs
of independent 131072/65536-triangle workloads, each with eight exact full-image
oracles. It checks 256 timestamps, barriers, binary/timeline output fences and
whole-batch refusal before capacity is exceeded. Generate its header with
`make_g17p_render_workload.py --batch-pair`; use `--help` for the explicit
caller resource bundle and triangle-count arguments. The generator uses the
identified own compiler/resource inputs, never firmware object templates.

Render completion requires both retired queues, both changed status records,
validated reports and a new terminal since that publication. Completed linked
scheduler heads are quiesced before return/rebind. The source's 128-job test
used two physical pairs (64 each); do not equate its harness limit with a
qualified 128-job lifetime for this sole pair. Further reuse, a second render
owner, native dependency waves and general source parity remain unfinished. The 64-job
retirement/boundary run passes on T8140 with 512 full-image comparisons,
256 timestamps, 64 new terminals, exact credits, an empty scheduler head
and a post-close retained-memory audit. Raw evidence is recorded in the
local m1n1 EXPERIMENT_LOG.md. These results do not complete source parity.

The ordinary render-first transition now retains the render root, pool and
control history while constructing the source's two post-render compute queue
owners. It allocates fresh compute status storage and keeps private robustness
state in the reserved VM aperture, preserving caller program pages. One shared
report reader routes non-render masks to the active compute command once;
compute terminals do not satisfy a render terminal baseline. Returning to
render preserves its existing queue lifetime. Compute-first startup now owns
a dormant render graph with both producers withheld. Its first render adopts
that graph, installs caller mappings with break-before-make, replaces the
placeholder descriptors, and preserves compute roots and history. Alternating mixed
command buffers now use this ordinary synchronous path. Prepublished native
dependency waves still need porting; consecutive computes use the bounded
publication path described below.

Generate `g17p_drm_mixed_workload.h` with the compute generator's
`--batch-count 2 --mixed`, and `g17p_drm_render_workload.h` with the render
generator's `--triangles 131072`. Compile `g17p_drm_mixed.c` as above and run
`g17p-drm-mixed` on a fresh boot. It runs R/C/R/C/R on one VM, keeping disjoint
caller graphs resident at the fixed USC base. T8140 passes 4605071 checks:
three pressure renders, two independent add3 outputs, all output tails,
16 ordered timestamps and five binary/timeline fence pairs. Post-close audit
confirms retained outputs, separate roots, both compute queues retired, render
queues retired, three render and two compute report records, exact credits,
and six online CPUs. This qualifies the transition; it does not complete the
synchronous source port or asynchronous plan stages.

The current direct-compute transport run passes 16533676 native checks and
6234 memory UAPI checks on T8140. Its audit retains the original transport at
384/384/384, pool slot zero at 384/384/384, and pool slot one at 6/6/6;
CL2 wraps to 2/2/2 with report credits 2/2. The 240 descriptor slots contain
the correct most-recent logical ordinals and timestamp destinations. Private
robustness pages now live in the reserved VM aperture for ordinary compute as
well as the render-first path. Reuse of both pool slots is implemented from the source ownership protocol;
this hardware run exercises two handoffs.

Run `g17p-drm-mixed --compute-first` on a fresh boot for C/R/C/R. Kernel #68
passes 3474569 native checks: two pressure renders, two independent add3
outputs, 12 guarded timestamp values and four binary/timeline fence pairs.
The post-close audit checks shared caller physical pages across the distinct
retained roots, both engines' 6/6/6 inner cursors, the unused compute owner's
zero cursors, report order (compute, growth, render, compute, render) and exact
5/5 credits. All six CPU contexts are collected and Linux resumes. This path
retains the same logical VM/file across the engine transitions.

The same #68 kernel also passes the full 258-command direct-compute regression
with the dormant graph withheld: 16533676 native checks, 6234 memory checks,
both transport switches, all timestamp/fence checks, and the post-close audit
of all retained ring and descriptor placements. This does not complete the remaining synchronous source behavior.

For mixed buffers within one ioctl, generate the render header with
`--triangles 131072 --batch-pair` as `g17p_drm_render_batch_workload.h`, retain
the mixed compute header above, and compile `g17p_drm_mixed_batch.c` with the
same UAPI headers/options. On separate fresh boots run:

```
g17p-drm-mixed-batch
g17p-drm-mixed-batch --compute-first
```

Kernel #69 passes R/C/R/C (3245407 checks) and two consecutive C/R/C/R
buffers (4326776 checks). Every command has a separate output: each render
checks eight complete images, and each compute checks all 64 floats plus its
tail. The two render programs use 131072 and 65536 triangles. The timestamp
oracle checks order within and across buffers, all old values and guard bytes;
binary/timeline fences cover each aggregate ioctl.

Trailing invalid flags, future barriers, missing timestamp handles and a valid
VM binding overlapping render-private storage all reject before any prefix.
Kernel #69 also rejected a third compute against its then-unported later
post-render lifetime. That artificial two-command limit and its obsolete test
assertions are removed by the retained path below. Rejections
preserve all outputs, timestamps and aggregate fences, including on the live
compute-first runtime. Both runs subsequently pass 6234 memory UAPI checks.

Post-close audits check all final images/results, retained roots and shared
caller pages, retired inner cursors, all timestamp guards, and complete report
sequences/credits (5/5 for render-first, 9/9 for compute-first). Six CPU contexts
are collected. These are ordinary synchronous alternating-engine buffers;
prepublished native dependency waves remain
separate unfinished source paths.


Consecutive compute commands use source publication waves of at most 36,
bounded further by CL2 channel credits and the 128-command transport interval.
Every staged command owns a distinct status pair and a saved queue target;
mutable descriptor/scheduler/context resources cannot alias within a wave.
Both producers are released per command, then one CL2 notification publishes
the wave. The ioctl waits for every command, invalidates caller outputs after
the last, and only then completes aggregate fences. A fault retains the whole
session's GPU-visible storage. Shared render reports use a FIFO of active
compute owners. This remains synchronous at the userspace boundary.

Generate and cross-compile the larger native test using the same flags above:

```sh
python3 drivers/gpu/drm/asahi/tests/make_g17p_compute_workload.py /path/to/m1n1 /tmp/g17p_drm_wave_workload.h --batch-count 64
# Compile g17p_drm_wave.c, then run on a fresh boot:
g17p-drm-wave
# On a separate fresh boot, exercise the shared report FIFO:
g17p-drm-mixed-batch --compute-wave
```

The first test submits 64/64/64/64/2 commands with 258 distinct input sets,
64 independent output pages, 516 guarded timestamps, a timestamp rebind after
128 commands and five aggregate binary/timeline fence pairs. It checks earlier
outputs and timestamp history, malformed suffix rejection and admission when
only two commands remain. Kernel #70 passes 8632642 native checks plus 6234
memory UAPI checks. Its ten notification groups include full 36-command waves,
both transport handoffs, descriptor/context/channel wrap and exact post-close
retention of all 64 output pages, 258 status pairs and timestamp aliases.
The status allocator is the source's relocatable per-command completion-page
mechanism, also used for retained direct commands rather than reusing their
old shared completion pair.

The mixed test mode uses R/C/C/R with separate output for every command and
cross-engine barriers. It retains the same whole-buffer rejection, timestamp,
image and aggregate-fence checks as the alternating modes. Kernel #70 passes
3245407 checks plus 6234 memory checks. Post-close audit verifies sixteen full
images, two compute pages, twelve timestamps, independent status pairs, both
compute queues retired, render queues retired and exact FIFO report order
(growth, render3, compute16, compute32, render3) with credits 5/5. All six CPUs
are inspected and Linux resumes. The full source
port still needs additional owner/lifecycle paths and native dependency waves;
these tests do not establish asynchronous or Mesa parity.


Ordinary post-render compute now covers the source's 258-command window.
Ordinal zero uses the opening owner, ordinal one starts queue/grid five, and
later commands retain that queue with fresh descriptors, two ring entries
(descriptor/event) and no further optional item. Allocate only the current
command's scheduler/event/descriptor storage and preserve queue, support,
job-list and context-page history. Emit the source's initial context-two 0x2e
control tick before ordinal two, without a separate control doorbell. The first
two commands have independent dispatch storage and can share a notification;
later ordinary commands reuse queue one's dispatch locations, so retire their
leases before staging the next. The ioctl remains synchronous.

The expanded lifecycle oracle compares 396 full objects / 1103264 bytes with
pure Python constructors, including post-render descriptors, event placements
and all context/descriptor wrap boundaries. Generate the usual two-graph mixed
header and pressure-render header, compile `g17p_drm_retained_mixed.c`, and run
`g17p-drm-retained-mixed` on a fresh boot. Kernel #71 passes 146475917 checks:
R/Cx258/R, changing inputs, all eight full images throughout, every compute
output/guard, 524 timestamps with complete history, 260 aggregate fence pairs,
capacity rejection before a prefix and exhaustion. Rendering succeeds again
after compute. Subsequent memory UAPI checks pass 6234.

For consecutive buffers on this same owner, generate
`g17p_drm_retained_wave_workload.h` with `--batch-count 56 --mixed`, compile
`g17p_drm_retained_wave.c`, then run `g17p-drm-retained-wave` on a fresh boot.
The first eight caller graph namespaces belong to render, leaving 56 disjoint
source compute graphs. It submits 56/56/56/56/34 commands between two pressure
renders: 258 distinct input sets, 56 independent output pages, 516 timestamp
values with live alias rebind and five aggregate binary/timeline fence pairs.
Kernel #71 passes 13227558 checks plus 6234 memory checks. Invalid suffixes,
input wait errors and a 35-command buffer with only 34 placements remaining
preserve every prefix output, timestamp and fence.

Both post-close audits verify final render and compute outputs, independent
completion words, the latest 240 descriptors, all 256 retained context records,
the full 515-entry queue-one ring, channel and report wrap, timestamps and exact
report credits. Queue zero is 3/3/3, queue one 515/515/515, render queues6/6/6,
primary credits5/5, secondary0/0. All six CPU contexts are inspected and Linux
resumes. Remaining render-owner paths, native C/R/C dependency waves and general
source parity are still open; no asynchronous or Mesa parity is claimed.

The native C/R/C profile now has a separate unpublished object constructor in
`g17p_dependency.rs`. It includes four fresh queues, UUID0x16, compact36-register
CL descriptors, distinct scheduler/completion owners, moved render pools and
status leaves, native render descriptors and dependency context points. The
opening and closing CL descriptors both restart their local ordinal at zero.
The class registration receipt matches exactly its owned0x28-byte prefix, once
on primary; malformed fields, secondary receipts and duplicates remain unknown.
The release transition preserves the source's render cursor/inner-state writes
and partial primary/secondary index updates. Growth storage is selected by its
owned graph, so the native graph cannot overwrite the repurposed compute support.

Run `check_g17p_dependency.py /path/to/m1n1` with Python3 and Rust. The source-only
oracle executes the actual shim topology/register methods over synthetic owned
RAM and compares362 objects /936944bytes, including32 complete CL descriptors,
32 render descriptors, variable CDM/samplers/timestamps/completion values,
registration receipts and rejection cases, and the source-authored host index
transition. No target, firmware binary or captured page is imported.

Session now connects the native opt-in to a source-owned cold primer, checked
caller rebinding, live execution-root join, fresh scheduler backing, deferred
publication, RTKit/report service, closing/render/opening retirement and final
caller copyback. The source diagnostic and context-owner restrictions remain.
Kernel #84 passes native C/R/C on RID1: all64 primer outputs/guards,
both distinct caller compute outputs, eight complete render images, all
command timestamps, cross-engine barriers and aggregate fence success status1;
2164061 checks. Native Closing/Render/Opening retire independently. This fixes
the #82 missing primary-index span and #83 misattributed render terminal mask6.
Ordinary synchronous submission remains the default; the broader context,
lifecycle/fault and source-parity coverage still needs validation.

Kernel#73 regression of the graph-selected growth service passes the existing
`g17p-drm-retained-wave` workload (13227558checks) and memory UAPI (6234checks).
R/Cx258/R output, all56 compute pages, all eight final images,516 timestamps,
aggregate fences, invalid suffixes and exhausted-capacity rejection pass.
Post-close queue/status/descriptor/context/report audit passes on all six CPUs;
Linux resumes. This validates the ordinary path after the refactor, while the
native dependency runtime remains unqualified.

Kernel #82 regression after the whole-shim integration passes
`g17p-drm-retained-wave` (13227558 checks) and memory UAPI (6234 checks),
including R/Cx258/R outputs, 516 timestamps, fences, guards and invalid suffix
rejection. The artificial 258-command admission limit is removed; the old
exhausted-capacity rejection above describes historical kernels. Cleanup,
maintenance, relocation and fault execution callers are now connected, but
native owner/fault/lifecycle hardware coverage and the full method-parity audit
remain open. Reproduction logs and source/input hashes are under
`neo-rust-port-20260929/whole-shim-integration/` in the adjacent artifacts tree.

Native compute/render VM profiles now have separate real-hardware coverage.
Kernel #85 with `asahi_neo.native_compute_vms=1` passes
`g17p-drm-compute-vm --cleanup`: 32 alternating colliding-DVA submissions,
successful exported fences, exact-span unbind and owned slot-three cleanup
(8006125 checks). The frozen TTB shows slot three cleared and slot two retained.

Kernel #87 with `asahi_neo.native_render_vms=1
asahi_neo.partial_independent_owner=1` passes `g17p-drm-render-vm`: 16 alternating
colliding-DVA renders with separate 131072/65536-triangle programs, complete
images and inactive guards/timestamps/sentinels, plus successful exported fences
(21230503 checks). Generate a second render header with `--triangles 65536`,
renaming its generated `render_`/`RENDER_` and `workload` identifiers to
`second_render_`/`SECOND_RENDER_` and `second_workload` for inclusion beside the
first header. Both headers retain their original GPU addresses. The production
preparation harness also runs the complete second partial-owner constructor
and all four index aliases; it catches the full-page versus Pool-A-size bug.

The ordinary logical render profile is connected to the source registered-root
switch without opt-in parameters. Kernel #89 passes
`g17p-drm-render-vm --cleanup` with32 alternating renders, identical DVAs and
separate program/output backing, every full image, inactive guard/timestamp,
successful exported fence and exact-span active/inactive unbind (42449544
checks). Memory UAPI then passes6234 checks. Source-owned driver/growth mappings
are retained across independent caller roots; returning roots admit only missing
owned growth leaves and reject another physical owner before edits.

On a fresh #87 boot with `asahi_neo.cleanup_diagnostics=15`,
`g17p-drm-compute-vm --relocate` verifies32 alternating submissions, logical VM
cleanup/maintenance, all four source relocation diagnostics, and eight further
surviving-file submissions (9833349 checks). Complete outputs, inactive history,
guards and exported fence success are checked after each new command. These
runs do not close the full method-coverage audit or the asynchronous plan stages.

Separate-file ordinary engine switching is checked by `g17p_drm_mixed_vm.c`.
Compile it with the existing mixed/render generated headers and run
`g17p-drm-mixed-vm` or `g17p-drm-mixed-vm --compute-first` on separate fresh
boots without opt-in flags. The compute file independently binds its authored
common code prefix at the fixed USC base, in addition to the mixed main-program
image at +64 KiB. That prefix was implicit in the shared-file fixture's resident
graphics image; omitting it creates a real command fault. No executable base
changes. RID1 #91 passes R/C/R/C/R (4850865 checks), and #92 passes C/R/C/R
(3671206 checks). Both check complete outputs, both full timestamp allocations
and histories, and exported sync-file status1 for every command. Session now
selects the logical caller in registered slot1 for post-render compute and
allows the dormant render owner to adopt another file after compute retirement.
Default Source render startup explicitly retains pair zero unless the independent
owner profile is enabled; its global alternate-pair default alone does not
admit a second owner. Source native C/R/C requires a fresh global sequence.

`g17p_drm_soft_fault.c` checks the source shader-store recovery path. Compile
with the two render headers used by the render-VM test. Boot with
`asahi_neo.owned_render_fault_kind=1` and
`asahi_neo.owned_render_fault_address=0x1000005c000`, then run
`g17p-drm-soft-fault`. The address is the first output's page containing the
actual authored nonzero pixels, rather than attachment page zero. RID1 #92
passes4064733 checks: discarded target-zero stores, seven complete correct
images, timestamps/guards and successful exported fences, exact unbind and
fresh GEMs on the same queue, preserved old backing, and a second logical VM.
`g17p_drm_fault.c` separately checks command-fetch loss with kind2/address
0x1000018000: errored aggregate fence, later EIO, and preserved inactive VM.

The independent profile is qualified on RID1 #98 with
`asahi_neo.partial_independent_owner=1 asahi_neo.alternate_queue_pairs=1` and
`g17p-drm-render-vm --pair-series`:128 alternating pressure renders,1024 complete
images,512 ordered caller timestamps,128 successful exported fences and complete
inactive image/guard/history checks (153089448 checks). The retained graph is
created before a new logical-root clone so both roots carry its private aliases;
tilemap recycling resolves the selected caller's owned backing. This does not
qualify the default sole pair; that profile is qualified separately below.

`g17p_drm_limit.c` checks owned terminal-limit attribution and recovery. Generate
the first header with --triangles400017, the second with --triangles65536 using
the renaming described above, and a recovery header with --triangles1. Rename
the recovery header's render_/RENDER_/workload identifiers to
recovery_render_/RECOVERY_RENDER_/recovery_workload and save it as
g17p_drm_limit_recovery_workload.h. Compile with these generated headers. Boot
with the independent alternating profile plus
`asahi_neo.tvb_max_blocks_pool0=8
asahi_neo.first_render_fragment_sync_grow=1
asahi_neo.repeat_fragment_sync_grow=1`, then run `g17p-drm-limit`.
RID1 #100 passes3491357 checks: four source negative replies and an owned type7
produce aggregate -ENOMEM; the other caller completes eight correct images;
the failed queue then renders eight fresh exact images from a one-triangle
encoder and fresh output GEMs. Old failed backing, timestamp history, inactive
outputs and sentinels remain unchanged. Successful work requires successful
exported fences and ordered caller timestamps. A negative reply alone does not
establish a terminal limit; the smaller131072-triangle default-fragment case
emitted ordinary completion instead.

Source TVB controls accept global/per-pool bounds8..2048. Global0 is unbounded,
per-pool0 is explicitly unbounded, and per-pool0xffffffff inherits global.
Fragment diagnostic0/1 maps the source bool; default2 leaves caller behavior.

RID1 #104 fixes the ordinary sole-pair71st missing terminal. Recycled pool-A
records retain their firmware counter, so the host scheduler slot must advance
from its retired value instead of resetting0/1/2. Both Source's production
phase callback and Rust now publish current/current+1/current+2 on reuse;
fresh records retain0/1/2 and native explicit-base behavior is preserved.
`g17p-drm-render-vm --single-series` runs128 renders on the default boot profile:
153089448 checks cover1024 complete images,512 ordered caller timestamps,
128 successful exported fences and all inactive guards/history. Memory UAPI
also passes6234 checks. #107 subsequently fixes the separate172nd failure by
keeping the context marker fixed and wrapping only the hardware stamp's low
eight bits. Full scheduler nodes remain monotonic. --extended passes all255
renders:303293792 checks,2040 complete images,1020 ordered caller timestamps,
255 successful exported fences and complete inactive guards/history. Memory
UAPI passes6234 checks. No terminal or timestamp gate is bypassed.
The production Source/Rust lifecycle comparison passes2108 objects and
3197478 bytes, including fresh/reused phase writes and overflow rejection.
The same #104 kernel also passes --pair-series with the independent alternating
boot profile:153089448 checks. Final owned queue triples are192/192/192 for
both engines on both pairs, and pool-A record-zero counter/host-slot values
are4/4 on each pair. All six CPUs and owned objects are saved before resuming.

--stress requests4096 renders in one run with two independently backed caller
VMs sharing identical DVAs. Each file has2048 timestamp records in a guarded
five-page timestamp object. Every render checks both callers' complete images,
sentinels, timestamp guards/history and successful binary/timeline/exported
fences. Check counts accumulate in64 bits. Ordinary physical storage reuses
256 retired descriptor/optional/event/status/context slots while retaining
logical identities and firmware-owned context fields. Status clear/tail,
GPU-register and completion-reader addresses all use the same retired slot.
Eight-bit control producers/consumers advance modulo256 without resetting
firmware consumers. The finite inner pointer/item storage is replaced using
two retained banks after both stages and both outer consumers retire; inactive
backing is checked before reuse. The handoff reads the pointer block's0x500
bound and retains queue identity and dependency points. Only the queue record's
local mirrored read index is initialized for new backing; other firmware
fields survive the retired handoff.
Cycle and index registers select slots inside their existing owned16KiB
scratch pages, independently of logical counts. The ordinary65535 lifetime
admission is removed; optional u16 metadata and narrow descriptor fields encode
their storage width. Real arithmetic/wire-format checks remain. Native
explicit dependency-wave admission retains its separately qualified255 bound.
Source/Rust comparison passes2791 objects/3459953 bytes across275 variations,
including physical/self-alias/scratch wraps through131072. All479 Source tests,
460-object transport comparison and production preparation pass. Kernel#116 passes4096 renders:5102761512 checks,32768 complete images,16384
ordered timestamps and4096 successful exported fences. Transcript audit checks
16 work/control/report ring wraps,15 descriptor/status/optional/event storage
wraps,16 context wraps,8 cycle-scratch wraps and1 index-scratch wrap, plus9
two-bank transport handoffs per engine. All six CPU states and owned objects
are saved before resuming Linux. The operator accepts this run; the proposed
additional8192 run was not started. Broad default-only descriptor self/status
selectors also match Source after a correction; kernel#117 builds but was not
hardware booted. Recipes18497 objects and graph3410 objects pass Source
comparisons. Full source-method/default parity certification remains open.
The growth oracle executes the source policy assignments and validation for
216 global/override combinations as part of93724 protocol/policy cases.
Production preparation runs both allocator-error and bounded-policy paths;
invoke the latter as /tmp/neo-dependency-prepare --pool-limit.

Latest combined-session qualification (2026-09-30): stage completion requires
memory, retained-wave, render-vm --cleanup, compute-vm --cleanup, mixed-vm,
mixed-batch and memory consecutively through the UAPI in one retained session.
Unit tests are explicitly unnecessary for this completion bar. Box1 remains
open: kernel#122 passes memory and258-compute retained wave, then fails the
render complete-image assertion; the later && suffix has not executed.

The runtime now publishes the Source compute allocator's complete three-page
4KiB GPU page list, rather than one page of16KiB entries. This parity correction
builds and is verified in live owned data, but does not resolve the image issue.
CPU-store tracing records no writes to output pages during a reproduced bad
submission. The --tiny diagnostic passes32 one-triangle renders after the
compute wave; --single-owner subsequently exposes a pressure failure without
alternating submitting VMs. Both keep complete-image, timestamp, fence and
inactive-owner checks. Neither substitutes for the unchanged --cleanup suite.
Artifacts: neo-rust-port-20260929/whole-shim-integration/combined-compute-directory/
and combined-tiny/. Published GEM and firmware backing remains retained until
session shutdown, as required by the M1/M2/M4 grow-only backing policy.

Further combined-session evidence (kernel#128, 2026-09-30): Source logical
activation barriers, upper-root preservation and reserved robustness aliases
build and boot but do not fix complete-image corruption. Memory and retained
wave pass, followed by render-vm submission3/owner1 failing target0 byte34880
with u32 0x2ea; later commands in the required && sequence do not execute.
Box1 remains open. Live descriptor DATA agrees with actual Python UAPI
conversion, including all three embedded fragment programs and tilemap
advancement. A frozen sparse-pattern scan finds no0x2ea in5136 unique owned
allocator/scratch pages; transient data remains possible.

The compute-only logical-context correction has separate UAPI evidence:
memory, retained-wave, compute-vm --cleanup, mixed-vm, mixed-batch, memory
pass consecutively on#126. This removes the observed compute admission EBUSY,
but that diagnostic omits render-vm and does not satisfy stage completion.
The full suite on that session still fails the later retained-wave pressure
render. Artifact-only partial-load-to-clear diagnostics do not alter the
required complete-image integration oracle or count as qualification.
