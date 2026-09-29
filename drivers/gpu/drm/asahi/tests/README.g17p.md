# G17P synchronous-port checks

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
pairs and every surrounding guard byte. Timestamp aliases retain backing
until firmware shutdown; address recycling is pending, and the retained
64 MiB aperture returns ENOSPC on exhaustion.

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

These are construction and report-service prerequisites. The native opt-in
still needs its execution-root join, fresh scheduler backing and deferred
publication sequence wired into Linux before hardware qualification. Ordinary
synchronous submission remains the active runtime path.

Kernel#73 regression of the graph-selected growth service passes the existing
`g17p-drm-retained-wave` workload (13227558checks) and memory UAPI (6234checks).
R/Cx258/R output, all56 compute pages, all eight final images,516 timestamps,
aggregate fences, invalid suffixes and exhausted-capacity rejection pass.
Post-close queue/status/descriptor/context/report audit passes on all six CPUs;
Linux resumes. This validates the ordinary path after the refactor, while the
native dependency runtime remains unqualified.
