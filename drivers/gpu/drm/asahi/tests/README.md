<!-- SPDX-License-Identifier: GPL-2.0-only -->
Native Max compressed-color producer and consumer:

```sh
python3 prepare_g16c_color_compression.py /path/to/m1n1 /path/to/own-Mesa-layered-depth-submit /path/to/fixture
/g16c_drm_render /fixture --phase-pair --reject-flags
```

This source-owned32x32x2 RGBA8 case stores red/blue through a compressed PBE
into BO9 with metadata BO124. A second submission uses the compressed TFU
input and writes a separate uncompressed BO127. Its complete32768-byte image
must match2048 CPU-generated red/blue pixels and untouched padding. The
producer leaves decoded output untouched; the consumer must preserve every
byte of compressed input and metadata. Scissor DATA suppresses foreground
fragments while retaining the source VDM draw; shader and launch bytes do not
change. Compressed depth main/metadata BOs125/126 also retain exact guards.

Version6 fixtures explicitly bound four opaque payload regions. They require
positive changes from the original seeds,138752 exact guard bytes per draw,
all other caller BOs unchanged, four ordered timestamps and complete timestamp
history/guards. Two `phase1-preserve-gem-ID.bin` one-byte declarations require
whole-BO preservation against the actual preceding GPU output; primary decoded
color is always compared exactly and cannot be masked.

`--alternate-input` generates a separate known-input control: the same consumer
uses a source-format uncompressed TFU descriptor pointing to a CPU-authored
green/white texture. It must produce green/white, leaving the earlier compressed
red/blue image intact. The control texture is explicitly attached in both
variants. This validates input selection without inventing expected opaque
compression codes. Old depth BO10 is no longer hardware depth in this fixture
and supplies that declared control texture. Source-gate metadata handle128 is
renamed124 only to fit the test fixture's local ID array; native DRM handles
have no such128-ID restriction.

Native Max passes both pairs, three compressed-color VM lifetimes with old
exported decoded-output/timestamp poison, relocated origins and both close
paths. Ten full host readbacks independently check decoded CPU images, opaque
region guards and bit-identical compressed inputs. Production kernel90 and all
firmware structs/Defaults/scheduler code remain unchanged.

Diagnostic prototypes tried treating a zero scissor as empty tiler work; both
PPP202 and203 still reloaded the image. Removing the consumer draw also stopped
the positive reload. Those failed assumptions and unchanged expected buffers
are preserved in the evidence and excluded from qualification. No claim is
made for arbitrary layered zero-draw rendering or general compression formats.
The final control uses known source data and full CPU expectations. It does not
weaken or relabel the failed prototype assertions.

Native Max timestamp object ranges and retirement:

```sh
/g16c_drm_timestamp_ranges /g16c-fixture
/g16c_drm_timestamp_ranges /g16c-render-fixture --file-close
```

The Max timestamp-object UAPI accepts nonempty8-byte-aligned offsets/ranges,
matching the source-qualified shim. The alias registry reserves whole16KiB
pages, retains the logical base/size for reference bounds and cache ownership,
and unmaps the rounded reservation after all accepted work and visibility
fences retire. Neo keeps its existing page-aligned admission policy. No
firmware ABI layout or scheduler implementation changes.

The short native client checks compute and render using simultaneous separate
BO aliases at offset/range8/8,16376/16,16392/16400 and49144/8 in49152-byte BOs.
It checks both sides of page boundaries, the last legal word, whole timestamp
BO guards, ordered stamps, every caller BO and actual output. References that
fit mapped pages but exceed the logical8-byte range are rejected before work;
zero/misaligned/overflowing/outside bindings are also rejected.

Each engine then accepts input-blocked work, proves its output fence is still
pending, unbinds the old timestamp tokens and closes their GEM handles. New
8-byte aliases remain untouched while the input is blocked and while the old
job completes. Submitting to replacements and then newly recycled aliases
changes only their intended words; old stamps/guards remain intact. Old CPU
mappings deliberately retain backing, so this tests alias/fence ownership and
stale translations, not physical-page reuse after the last CPU mapping closes.
Seven jobs per engine pass; compute explicitly destroys its VM, render also
checks file-close cleanup after its jobs complete. Full DRM-file close while
these new biased ranges are input-blocked is not claimed by this test.

`tests/g17p_timestamp_span.rs` imports the production page-span helper. Four
host tests cover both sides of a boundary, tiny/exact-end ranges, invalid and
overflowing ranges, and136 page-aligned offset/size pairs preserving the old
mapping exactly. Compile with `rustc --edition=2021 --test` and run the result.
Native new-kernel regressions also pass empty-tile controls, compressed depth,
legacy page-aligned timestamps, mixed work, memory UAPI and unbind with all16
CPUs. Live Neo GPU testing remains separate.

Native Max causal empty-tile controls:

```sh
python3 prepare_g16c_mesa_render_fixture.py /path/to/m1n1 /path/to/source-empty33 /path/to/fixture --oracle=rgba8-empty-tiles --flags=12 --ppp-ctrl=0x203 --control-triplet
/g16c_drm_render /fixture --control-triplet --reject-flags
```

The source workload has a wholly clipped triangle and a source-owned red
background fragment. A version-5 fixture submits flags12/14/12 on the SAME
VM/queue, holding PPP0x203 fixed. It restores the A5 output seed before each
phase. Disabled processing must preserve the entire16KiB BO; enabling it must
write all1089 red pixels in the33x33 Morton layout and preserve12028 guard bytes.
The second disabled phase checks that the previous enabled flag does not stick.
CPU addressing and source colors generate the expectations before any GPU
readback. Shader/launch and all other GEM bytes are unchanged.

All three submissions check four ordered timestamps, prior timestamp history,
the whole49152-byte timestamp BO guards, every input BO and caller-output bytes.
The native Max triplet passes, including three VM lifetimes/nine submissions
with old exported color/timestamp poison, relocated origins and both close
paths. Entire output readbacks independently match CPU expectations with
0/4356/0 changed bytes. Existing two-phase compressed/uncompressed cases and
normal/hold paths pass the generalized test client. This adds test coverage of
the existing port; no production GPU struct or scheduler change is required.

Native Max compressed depth/stencil store and load:

```sh
python3 prepare_g16c_zls_pair.py /path/to/m1n1 /path/to/own-Mesa-offline-submit /path/to/fixture --kind=depth --compressed
/g16c_drm_render /fixture --phase-pair --reject-flags
```

Version-4 fixtures use the same source state/draw groups and unchanged shader
and launch bytes as the uncompressed pairs. They place the compressed main BO
at 0x6000050000 with a 32KiB layer stride, and metadata at 0x6000070000 with a
256-byte layer stride. The source-qualified commands and seeds match the
previous shim cases byte for byte except for requested render flags12.

Color remains an exact whole-BO CPU oracle. Compressed main and metadata codes
are opaque: `opaque-writes-gem-ID.bin` explicitly marks bounded writable bytes.
Every other byte must match its source seed, and both BOs must change from
their seeds. Main writable spans are4096 bytes/layer for depth or1024 for
stencil; metadata spans are128 bytes/layer, including a guarded128-byte hole
between layers. No GPU payload bytes are promoted to CPU expected data.
Masks are restricted to version4 phase pairs and cannot mask primary color.
Phase readback supports the smaller16KiB metadata BO.

Native Max passes both compressed store/load pairs and their `--no-load`
controls with exact2048-pixel color/guards, main/metadata guards, other inputs,
timestamp history, invalid-flag rejection and cleanup. The differing consumer
images establish use of prior ZLS state. This qualifies these32x32x2 one-sample
profiles, not arbitrary compression formats, strides or compressed-color
surfaces. Production driver code and the existing qualified Image are unchanged.

Native Max uncompressed depth/stencil store and load:

```sh
python3 prepare_g16c_zls_pair.py /path/to/m1n1 /path/to/own-Mesa-offline-submit /path/to/fixture --kind=depth
/g16c_drm_render /fixture --phase-pair --reject-flags
```

Each version-3 fixture runs two submissions in one VM/queue. Phase 0 stores
depth 0.25/0.75 or stencil 7/9 across two 32×32 layers. Phase 1 preserves the
GPU-written color and ZLS BOs, updates only the caller VDM DATA and command,
then loads and tests those values. Full CPU oracles require red/green color
with depth 0.25/0.5 or stencil 8/10. All color/ZLS padding and other BOs are
compared, alongside timestamp history, guards and explicit cleanup.

`--no-load` generates causal controls: depth clears to 1 then becomes 0.5/0.5
with green/green color; stencil clears to zero, rejects both comparisons and
retains red/blue color. These controls and the load cases pass on native Max.
The second stencil control changes no color bytes but changes its checked ZLS
BO, so progress is checked across declared outputs. A three-lifetime depth
pair regression also checks old exported color/timestamp poison and both close
paths. It does not retain old depth BOs as poisoned guards.

The exporter selects complete state/draw groups from our hash-verified source
VDM stream; shader and launch bytes remain unchanged. It preserves the source
`isp_bgobjvals`, including opaque upper bits. Zeroing 0x300 in an initial test
fixture cleared the first depth-tested color layer despite correct depth
load/store; restoring the source value fixes the pair with the original oracle.
No GPU production code change was needed for these profiles.

Phase files are `phase1-command.bin`, optional `phase1-input-gem-ID.bin`, and
`phase1-expected-gem-ID.bin`. Mutable seeds reset only before phase 0. The client
waits for completion before changing VDM DATA, and preserves phase-0 timestamp
history through phase 1. Phase readbacks are saved under `/g16c-readback-*`.
These tests cover uncompressed D32F/stencil8 at page strides. Broader formats
and strides remain separate qualification work.

Native Max complete render images:

```sh
python3 prepare_g16c_mesa_render_fixture.py /path/to/m1n1 /path/to/own-Mesa-offline-submit /path/to/fixture --oracle=rgba8-three-layer-colors --flags=12 --utile 32 16
/g16c_drm_render /fixture --expected-images --reject-flags
```

The CPU oracle uses the source fixtures' POT Morton layout, page-aligned array
layers and explicit per-sample RGBA8 words. Shader-defined colors and public
Mesa addressing determine every expected byte, including untouched padding;
GPU readbacks never supply the expected image. Full 1×/2×/4× expected buffers
and pixel locations match the existing independent CPU source oracles.

Expected-image fixtures use version 2 and require `--expected-images`.
`expected-gem-ID.bin` describes the entire BO. Other BOs must stay byte-identical
to their seeds. The client can also load expected images for additional BOs;
depth/stencil writes are qualified by the phase-pair tests described above.
Mutable BO seeds are restored before each submission. Timestamp histories and
guards retain the existing checks. Default version-1 pixel fixtures still work.

Native short cases pass 33×33 and 65×65 red images, distinct colors across three
33×33 layers at 1×/2×/4× MSAA, and both 32×16 and 16×16 utiles. Every image and
all other caller BOs are compared in full. These cases include flags 12,
nondefault PPP controls, reserved-bit rejection, relocated VM memory and file
close. Three further layered VM lifetimes check exported old output/timestamp
poison and both close paths. Production GPU code and its qualified Image remain
unchanged: these tests add real native coverage of the existing port. This does
not close general depth/stencil/compression, partial-rendering or growth work.

Native Max vertex scratch and nonclustered rendering:

```sh
python3 prepare_g16c_mesa_render_fixture.py /path/to/m1n1 /path/to/own-Mesa-offline-submit /path/to/fixture --flags=5
/g16c_drm_render /fixture --check-inputs --reject-flags --kernel-base=0x7100004000
```

The exporter checks every original Mesa producer hash, preserves all shader
and launch bytes, omits only an entirely zero unused shader-aperture tail and
coalesces page-equivalent bindings. Output poison and requested UAPI flags are
the only other DATA changes. It supports independently specified contiguous
output extents; the native client still needs a correct pixel oracle.

Max accepts VERTEX_SCRATCH and NO_VERTEX_CLUSTERING through an explicit
per-platform admission policy. Neo keeps its original mask. The Max runtime
already waits for all GPU jobs and CPU completion leases, retains its owned
28-tranche queue operand pool at the VM origin and emits nonclustered TA
control bit 0. A new explicit origin check protects that invariant before
render admission. The classic scratch word is not part of this Max ABI.

Native tests pass dense 4,096- and 8,192-word private-array hash workloads,
including scratch/no-clustering/empty-tile/fragment-barrier combinations,
relocated VM origins and file-close teardown after compute/mixed work. The
original shader compares its private-array result against CPU-generated hash
uniforms; the pixel observes the triangle's provoking vertex, not every vertex
independently. A separate 32×32 nonclustered render checks all 1,024 pixels.
Each case uses two draws with full output/timestamp guards and unchanged input
BO checks. `--reject-flags` also submits reserved bit 4, requires EINVAL with
all input/output/timestamp bytes intact, then proves valid work still executes.
These profiles do not qualify general partial rendering, growth, geometry or
overlapping firmware execution. Use the normal short cases during development.

Native Max render sampler counts and caller tables:

```sh
python3 prepare_g16c_render_sampler_fixture.py /path/to/m1n1 /path/to/qualified-source-vertex-sampler /path/to/fixture --count=65535 --heap=B --empty-tiles --fragment-barrier
/g16c_drm_render /fixture --pixel=0xff808080 --check-inputs
```

The exporter verifies the qualified source input hashes and preserves every
source shader/launch byte. It constructs only caller-owned logical heaps,
native compact descriptor tables and their root pointers. The native kernel
does not decode or rewrite the client shader. Automatic Mesa table generation
and integration are separate from this fixture transport test.

Max admission now applies its existing wider sampler policy to render as well
as compute; Neo still uses 1,024 for both. Render's UAPI count is u16, so 65,535
is valid and its firmware count-plus-one is 65,536. The native short matrix
checks 515, 1,025 and 65,535 samplers, real white/gray filtering and a red-texture
control. The two larger counts combine empty-tile processing with the fragment
barrier. Every draw checks the complete 16 KiB output/guards, all 21 input BOs
(including shader/root/tables), four ordered timestamps, timestamp history and
the rest of a three-page timestamp BO. Each invocation performs two draws and
explicit cleanup. These profiles do not establish general scratch/clustering,
geometry, partial rendering or overlapping firmware execution.

Native Max dormant parameter-buffer sharing:

```sh
/g16c_drm_pb_sharing /g16c-fixture /g16c-render-fixture --holders=2
/g16c_drm_pb_sharing /g16c-fixture /g16c-render-fixture --active-render
```

The first test keeps compute VMs and queues open while fresh render VMs borrow
the dormant render cache. Every handoff checks every surviving compute output
and guard, submits new compute work, then checks it again. Reverse teardown
also resubmits all survivors. Use two holders during development and sixteen
for closure across the fifteen-PB namespace limit. The native sixteen-holder
run passes 288 compute jobs and 34 draws, with seventeen transfers of PB 0 and
no fresh PB allocation inside that run. Seventeen compute pools/queue slots
remain cached at the high-water mark; this does not mean zero memory growth
while creating additional live VMs.

The active-render variant retains a rendered VM, draws in a peer VM, then
resubmits the retained renderer and checks its output, timestamp history and
guards. The native run allocates PB 1 for the peer instead of stealing the
attached PB 0. Transfer is allowed only at global GPU/CPU quiescence when the
cached resource has no root lease, admission or live sequence. It preserves
firmware-updated metadata and cumulative counters, then uses the existing
validated rebind path. All GPU jobs remain serialized; live VMs alone are not
proof of overlapping firmware execution. The `--hold` modes in the compute and
render clients implement this test's stdin/stdout protocol and retain normal
explicit cleanup on exit.

Native Max live binding generations:

```sh
/g16c_drm_generations /g16c-fixture --generations=3 --deferred-old
/g16c_drm_generations /g16c-render-fixture --generations=3 --deferred-old
/g16c_drm_generations /g16c-render-fixture --generations=3 --destroy-deferred-old
```

This source-only Mesa fixture test keeps one VM and two logical queues open.
It replaces the output BO at the same GPU VA, using nonzero BO offsets and
outer guards. Old physical pages stay alive as poisoned read-only bindings;
each job verifies its entire output/guard allocation and all older poison.
The delayed-input variant accepts A against the original bindings, rebinds to
B and executes B on another queue before releasing A. A must then write only
its captured original pages, preserve B, and allow B to execute again. The
queue-destroy variant also destroys A's logical queue after acceptance, rejects
new work on that ID, and recreates it only after the accepted A has completed.

The Max backend retires obsolete executable generations only at its serialized
GPU/CPU quiescence boundary, through the existing exact control/root retirement
path, then reuses cached pools, queues and physical backing. Caller VM state and
logical queues remain valid; accepted jobs keep immutable snapshots and BOs and
can reconstruct an earlier generation. Max now revalidates the selected snapshot
under the publication lock and retries preparation if another worker changed it.
Neo's entire CPU scheduler remains unchanged outside explicit Max branches.
Use three generations during development; reserve a larger run for closure of
this profile. Both kinds pass 128 generations in a still-open VM, including
accepted old snapshots and logical queue destruction. The 260 final root
retirement receipts stay at 337 allocations / 452,165,632 bytes, one pool and
one queue slot, with ASID 1 reused. The complete probe also passes mixed work,
timestamps, three VM lifetimes, native memory assertions and unbind on all 16
CPUs. This is separate from actual overlapping firmware execution.

Native Max render output and engine assignment:

```sh
python3 prepare_g16c_render_fixture.py /path/to/render-source-fixture /path/to/render-fixture
python3 prepare_g16c_render_fixture.py /path/to/mixed-source-fixture /path/to/mixed-fixture --mixed
python3 check_scheduled_execution.py /path/to/m1n1 --evidence /tmp/max-execution
python3 check_scheduled_lifetime.py /path/to/m1n1 --evidence /tmp/max-close
```

`g16c_drm_render /g16c-render-fixture` is a short native test: two draws,
expected red output, every byte of the 16 KiB output/guard GEM, four ordered
TA/fragment timestamp pairs and the remaining bytes of a three-page timestamp
GEM, followed by explicit queue/GEM/VM cleanup. The source-only 1×1 fixture
preserves every Mesa object and command byte and verifies the producer hashes.
This test passes through the existing Linux DRM scheduler on Max. The first
harness omitted GEM cleanup and correctly received EBUSY at VM destruction;
the corrected test also passes explicit VM destruction.

The common scheduler selects the existing Max runtime for both command types.
Render uses source-built parameter resources, high-water geometry storage,
programs and work pairs. It requires both shared driver stamps, the event
counter and all transport consumers before CPU completion; all four caller
timestamps and the binding snapshot remain owned until then. Render queue
retirement uses a runtime tick plus close, rather than the compute-only close
prelude. The engine plan fixes physical event slots at the first publication:
compute-first uses CDM/TA/fragment slots 0/1/2; render-first uses 2/0/1.
The gate checks all 4,608 ordered writes in 256 complete source plans, including
fragment-before-tiling setup, and rejects 23 malformed bindings. These host
checks alone do not prove GPU execution.

`g16c_drm_mixed /g16c-mixed-fixture` exercises a real render/resolve dependency
in a single ioctl and VM. It checks freshly rendered MSAA samples, resolve
pixels, guards, six timestamps per batch, two timestamp object generations,
and explicit close. Hardware results and exact Image/initramfs hashes belong
in the bootloader evidence receipts; do not infer broader feature completion
from the presence of this test. Both compute-first and render-first hardware
runs pass this two-generation MSAA render/resolve batch, then standalone work.
The initial render checkpoint passed 22 checked hardware commands in 18 ioctls
(10 render, 12 compute), including four dependent mixed batches and native
memory/unbind checks on all 16 CPUs. This is GPU passthrough with a PSCI/VUART
monitor; bare-metal qualification remains separate.

Native Max render lifetime reuse:

```sh
python3 check_scheduled_render_allocation.py /path/to/m1n1 --inputs /path/to/platform/input.json --recycle --evidence /tmp/render-rebind
/g16c_drm_render /g16c-render-fixture --lifetimes=3
```

The host gate compares 31,230 complete virtual pages and all DATA/attribute
bytes across allocation and relocated reattachment. It requires identical
physical backing, no new backing allocation, preserved PB metadata mutations,
fresh root ownership, and one idempotent admission per lifetime. A foreign last
metadata PTE or geometry guard rejects before clearing cached backing. RAM/PTE
APIs and replies are models; these checks are separate from GPU qualification.

On VM release, the native runtime keeps logical PB identity and its own
firmware-updated metadata, drops the retired executable root lease, and caches
private, compact/geometry and support backing. A later VM verifies exact invalid
TTBRs and retained PTEs, preflights fresh mappings and guards, relocates and
rebuilds private operand tables/support from source, and readmits the pool.
Per-VM scene counters reset while cumulative PB counters remain monotonic.
Dormant cached render resources do not select the render close protocol for a
compute-only VM. Render snapshots now retain the VM's requested private-window
base instead of assuming the initial fixture's placement.

The native lifetime test alternates explicit VM destruction and file close,
and cycles three minimum 512 MiB windows, including a non-4-GiB-aligned base.
It retains exported old output/timestamp pages and poisons them. Separate DRM
file mappings keep the guard pages inspectable without pinning old VM files.
Each new VM imports those pages as read-only coherent bindings; native ownership
and cache validation includes them, and every new draw checks all older poison.
The lifetime profile creates an exportable writeback output BO; its Mesa bytes
and shader remain unchanged. The ordinary fixture still uses VM-private output.
Use three lifetimes while developing; reserve `--lifetimes=128` for final closure
of this profile, covering old namespace limits and work/control-ring wrap.
The final native run passes 128 lifetimes / 256 draws. Every retirement receipt
stays at 606 allocations / 893,714,432 bytes after warming two pools and two queue
slots; context 1 is reused throughout. It retires 1,024 close/maintenance controls,
checks 512 guarded TA/fragment timestamp pairs and all retained output/timestamp
poison, then passes native memory assertions, unbind and all 16 CPUs online.
The complete probe also passes two mixed batches and standalone compute.

Remaining qualification includes cache transfer under simultaneous VM/PB pressure, broader geometry/control/sampler
profiles, native firmware dependencies and concurrency, growth and faults,
unaligned timestamp object ranges, bare-metal power management and live Neo
regression. The older wire/admission gates below have narrower scopes.


Scheduled-family work wire layouts (source and native DATA):

```sh
python3 check_scheduled_work.py /path/to/m1n1 --evidence /tmp/scheduled-work
python3 check_scheduled_render_work.py /path/to/m1n1 --evidence /tmp/render-work
python3 check_scheduled_render.py /path/to/m1n1 --evidence /tmp/render-bodies
python3 check_scheduled_render_program.py /path/to/m1n1 --evidence /tmp/render-programs
```

The production versions macro generates `WorkWrapperG16C/G17P` and
`WorkRecordG16C/G17P`; compile-time assertions cover every field offset,
size and alignment. Compute aliases preserve existing callers. The first
gate compares 1,024 complete Neo objects with the known-good tag, 388 Max
objects with qualified source/native DATA, and 64 actual source preparation
cases. The render gate compares 264 complete Max wrappers/records, including
eight independent native DATA images, and rejects eleven malformed records.
These are wire checks. Native compute regression passed after this change;
the native output test above supplies separate hardware evidence.

The body gate compiles the same production macro and checks every field,
size and alignment of TA/fragment descriptors for both profiles. It compares
260 complete Max descriptors with qualified source, including four independent
native DATA images. Its 2,080 malformed-input checks require unchanged staging
on error. A separately compiled known-good Neo serializer produces 512 complete
descriptors identical to the templated implementation, including late overlay
enable combinations and overlapping caller overrides. Max primary programs
are restored last; Neo's original overlay order and byte widths are preserved.
Production Rust now lowers the common command into all five Max register
programs and descriptor body inputs. The complete descriptor gate consumes
those Rust outputs. The program gate compares 660 whole ordered programs,
132 individual TIB encodings and all 1,032 valid TIB/sample/utile combinations
with qualified source, plus 3,168 rejected requests. Native DATA from four
complete captured descriptors is checked independently. All five programs
populate source `Default` templates; no captured image is a template.
The native runtime above integrates resources, publication and output; these
program/body gates separately qualify layout and lowering bytes.

Max parameter resources and owned FList admission:

```sh
python3 check_scheduled_render_resources.py /path/to/m1n1 --evidence /tmp/render-resources
python3 check_scheduled_render_allocation.py /path/to/m1n1 --inputs /path/to/platform/input.json --evidence /tmp/render-allocation
python3 check_scheduled_bootstrap.py /path/to/m1n1 --inputs /path/to/platform/input.json --fdt-template /path/to/typed-memory.dtb --evidence /tmp/bootstrap-flist
```

The wire gate compares 288 complete parameter/support objects and 64 FList
requests. Every wire field offset, size and alignment is asserted. The allocator
gate executes actual qualified `RenderResources.allocate` and the production
Rust allocator, comparing all 15,615 virtual pages (including fitted geometry), DATA bytes and attributes.
It models RAM, PTE APIs, cache primitives and firmware replies. A last-page alias
mismatch or foreign installed root rejects before registration. Root leases and
all backing remain owned through admission; guards stay unmapped. The controller
wait requires both the exact owned kind-13 reply and all three TX cursors.
The bootstrap gate checks complete source state images, all 40 significant reply
bytes, duplicate/unowned rejection, crossing a UAT leaf, and render-status words.

A test-only kernel caller exercised the same production allocation/admission
code on Max: reply matched, cursors reached `[8, 8, 8]`, repeated admission emitted
one control, and unbind removed DRM nodes with all 16 CPUs online. Its retained
probe root and caller are artifact-only code. The native output test separately qualifies work publication and completion.
Broader geometry growth and queue/context recycling remain open; this admission
test itself is not a rendered-output result.

Native dependency roots and preparation (source-only):

```sh
python3 check_g17p_dependency_vm.py /path/to/m1n1
rustc --edition=2021 -Dwarnings g17p_dependency_prepare.rs -o /tmp/neo-dependency-prepare
/tmp/neo-dependency-prepare
/tmp/neo-dependency-prepare --pool-limit
```

The root test executes the actual shim dependency-state join over synthetic
owned leaves and compares complete resulting PTEs with the Rust join. It
includes live mappings differing from the bootstrap inventory, physical aliases
with different attributes, active/dormant collisions, source-ordered state
aliases, separate zero robustness owners, and input/overlap rejection.

The preparation harness runs production queue/graph preparation and mixed
report routing with models of kernel allocation, Memory, and user UAT APIs.
It also compiles the production Vm replacement methods against the checked
Memory API. It verifies held producers, the hidden closing context, scheduler
owner replacement, decoded late low references, the compact descriptors,
CL/growth/receipt/render/CL report interleaving, credit retention on invalid
reports, and unchanged standalone report admission. The intentional unknown
low-reference rejection prints one ownership diagnostic before PASS.

Native release and retirement (source-only):

```sh
python3 check_g17p_dependency_release.py /path/to/m1n1
python3 check_g17p_dependency_retire.py /path/to/m1n1
```

Release executes the actual shim publication tail and a bounded interpreter
for its two source-authored host ASM primitives over synthetic owned RAM. Full
store/control/mailbox traces agree for 16 seeds. Class-state failures, rejected
boundaries and all 49 access-failure prefixes retain the exact published prefix.
No firmware instructions are interpreted. Retirement executes the source
G17PQueueFence predicates, render status snapshot and finish-order assignment:
147456 combinations cover independent queue/consumer/status/growth gates and
8-bit wrap. Failure and incorrect retirement order are terminal.

The production preparation harness also runs the concrete control adapter:
seven control bodies, ten notifications, independent consumer advancement and
finite control-window admission. It reads real production snapshot destinations
in modeled RAM, then retires closing CL, render and opening CL. Neither queue
retirement nor another command's status is a completion witness. Its mixed
report checks now include allocation refusal, owned native fragment event 2,
and retained credits on wrong event or duplicate limit.
The --pool-limit invocation refuses a second allocation at the source TVB bound;
the default invocation exercises allocator ENOMEM. Both retain the prior count
and mappings and verify no new backing was allocated.

These model checks do not execute the target's CPU cache/TLBI instructions,
RTKit notifications, firmware, or GPU workloads. Session now connects the
source-owned cold primer, retained preparation, execution-root activation,
RTKit publication, report service, retirement and caller copyback to UAPI
submission under `asahi_neo.native_barriers=1`. RID1 kernel #84 verifies all
64 primer outputs, then completes native C/R/C in closing/render/opening order.
`g17p-drm-mixed-batch --native` verifies every caller output, timestamp and guard,
plus exported aggregate sync-file success status 1 (2164061 checks). Earlier
#82/#83 failures identified the missing full index view and ordinary render
terminal-mask assumption; both are fixed. This one hardware workload and the
models do not establish full stage-one parity.

Hardware coverage added after the native dependency run:

| Profile | Real UAPI workload | Verified checks |
| --- | --- | --- |
| Native compute VMs (#85) |32 alternating two-file commands and slot-three cleanup |8006125 |
| Native render VMs (#87) |16 alternating two-file pressure renders |21230503 |
| Ordinary cleanup/relocation (#87) |32 two-file commands, cleanup, four relocations,8 further commands |9833349 |
| Ordinary logical render VMs (#89) |32 alternating renders, active/inactive exact-span unbind |42449544 |
| Ordinary separate-file R/C/R/C/R (#91) |Independent engine caller roots, complete outputs/history, successful exported fences |4850865 |
| Ordinary separate-file C/R/C/R (#92) |Dormant render adoption across files and subsequent root switches |3671206 |
| Owned command-fetch fault (#89) |Errored exported fence, later EIO, inactive VM guards preserved |PASS |
| Owned shader-store fault (#92) |Discarded stores, same queue with fresh GEMs, old backing and second VM preserved |4064733 |
| Independent alternating physical pairs (#98) |128 pressure renders,1024 complete images,512 ordered timestamps,128 successful exported fences |153089448 |
| Owned pool limit and recovery (#100) |Four refusals and own type7/ENOMEM; second caller and same failed queue execute complete images with fresh GEM recovery |3491357 |
| Default single physical pair (#104) |128 pressure renders,1024 complete images,512 ordered timestamps,128 successful exported fences |153089448 |
| Independent alternating pairs regression (#104) |Same128-render full output/history/timestamp/fence checks after reuse fix |153089448 |
| Default pair hardware-tag wrap (#107) |255 renders,2040 complete images,1020 ordered timestamps,255 successful exported fences |303293792 |
| Retained render storage/transport reuse (#116) |4096 renders,32768 complete images,16384 ordered timestamps,4096 successful exported fences; repeated finite bank reuse |5102761512 |

Successful workloads check their own complete outputs, inactive guards/history,
timestamps and successful exported aggregate fences. Fault cases check the
specified error or discarded stores; failed render images are not claimed
correct. Kernel #89 subsequently
passes6234 memory UAPI checks. The production preparation harness now executes
the second partial-owner constructor and verifies sparse owned growth mirrors,
including idempotence and collision/pending rejection before modification.
See README.g17p.md for generator options and target boot flags. These are
synchronous stage-one paths; no asynchronous or Mesa parity is asserted.

Completion of this synchronous port is judged by integration through the
existing UAPI: caller-owned outputs, VM isolation, timestamps, fences, cleanup
and error behavior. A source-method/default inventory is not a completion gate. The71st missing terminal is fixed by advancing the retained
scheduler slot on reuse in both Source and Rust. The default sole pair passes
128 renders on #104. The separate172nd timestamp failure is fixed on #107 by
preserving the context marker and wrapping only the eight-bit hardware stamp
tag. All255 renders pass real output/history/timestamp/fence checks. The new
ordinary reuse path uses256 descriptor/status/context slots, modulo256 control
transport, two retired pointer/item backing banks, and page-bounded scratch
selectors. It removes ordinary lifetime-count admission while retaining actual
wire-field checks. The extended Source/Rust comparison covers275 caller
variations through131072. Kernel#116 passes --stress:4096 renders and5102761512 checks, including
16 work/control/report ring wraps and9 two-bank transport handoffs per stage.
The final default-only self/status selector correction builds as#117 and
passes the broad Source recipe comparison. The subsequent combined UAPI
regression remains failing; earlier individual passes and the accepted4096
render run do not establish that this combined sequence is correct.

On RID1 kernels#117 and#119, run these binaries consecutively in one retained
driver session:

```sh
g17p-drm-memory && g17p-drm-retained-wave &&
g17p-drm-render-vm --cleanup && g17p-drm-compute-vm --cleanup &&
g17p-drm-mixed-vm && g17p-drm-mixed-batch && g17p-drm-memory
```

Memory passes6234 checks and retained R/C258/R passes13227558 checks. The
following two-file render test fails a complete-image comparison. On#119,
submission6 (owner0, generation3) has correct intended pixels and successful
completion/timestamps/fence checks, but unexpected0x2ea words beginning at
byte39016 of target0. The remaining commands are skipped by the shell's `&&`.
The diagnostic build pauses before the original assertion so caller-owned
images and all six CPU states can be captured. No expected output is relaxed.
Artifacts are in `whole-shim-integration/final-uapi-context-reuse/`.

The backing-page policy follows the M1/M2/M4 heap: it grows without returning
pages on object retirement. Neo's `Memory.allocations` retains firmware
backing until session shutdown. Before work publication, the session also
retains deduplicated GEM owners, preserving physical pages across GEM close
and unmap; temporary submission references may retire without dropping these
owners. Failed peer shutdown retains memory. Kernel#118 includes this policy
and still fails an image check in the retained wave; it is not evidence that
page retention fixes the corruption. Kernel#119 additionally preserves
firmware-owned fields when compute context records wrap, but still fails the
combined sequence described above. The image corruption remains unresolved.


`g16c_drm_compute_private` accepts fixtures from `prepare_g16c_private_compute.py`.
The ordinary cases use sixteen dynamically indexed private arrays (256 or 4096
words each). Pressure mode uses 256 or 16384 invocations with 4096-word arrays
and atomic-XOR hash bins. The C client independently calculates every expected
hash/bin before opening DRM, verifies the complete output and all other caller
BOs, repeats twice, and checks positive sync-file status plus ordered timestamps
and all timestamp guards. `--preflight` performs only input/oracle checks.
Inputs preserve every own-Mesa GEM and command byte; no reference firmware
objects, Apple shader bytes, or captured UAT pages enter the native driver.

G16C starts with twenty operand buffers. `asahi_neo.g16c_operand_buffers=25`
requests quiescent FList extension after the queue's initial compute use. This
is an explicit capacity target, not a demand-driven allocation policy. Admission
waits for GPU retirement and CPU leases. The transaction validates the live
roots, support identity and existing table, maps fresh retained backing, appends
only the table tail, sends both native wakeups and independently requires the
owned reply, all TX cursors and firmware-applied count. Live directory, dispatch
cursors and support counters are never CPU-reconstructed. Detached owners retain
buffer offsets and backing; fresh roots reconstruct their complete grown pool.
The existing caller private-window bound still applies. An uncertain publication
retains ownership and fails closed. This does not qualify automatic growth,
forced use of every appended tranche, or all capacity/fault boundaries.

`asahi_neo.g16c_operand_probe=1` enables a bounded read-only diagnostic of the
appended operand buffers (default off). It verifies zero seed samples at pool
activation, before either engine can use a fresh/recycled pool, then records
three 16KiB pages per appended buffer after all firmware flights retire. It
never samples another pending GPU user's scratch or writes live scratch DATA.
`g16c_drm_compute_private FIXTURE --repeat=N` supports a bounded repeat count
(default2) without dropping any caller BO, guard, fence or timestamp checks.
`--kernel-size=BYTES` may be combined with it to test a larger caller-private
window (default 512 MiB). Max compute, save slots and render backing use the
accepted VM's actual `kernel_end`; dormant operand pools are reused only if
their retained extents fit the new window. Render allocation checks its full
span before mapping or allocating backing. A 1 GiB window with explicit growth to 65 buffers passes private-array output checks and shows shader-written DATA beyond
the former 512 MiB limit; alternating back to 512 MiB uses a compatible smaller
pool. Ordinary probes stay short. Automatic growth and raw allocation/fault
boundary behavior remain separate qualification requirements.

`check_g16c_operand_probe.py LOG --evidence DIR` verifies the qualified
private4096/pressure16384/selector9 case. It independently reconstructs the two
values the shader writes per private-array cell and identifies sixteen sampled
words in each of the five appended buffers. Matches must form one coherent
lane/array-index model per buffer; arbitrary word membership is insufficient.
The verifier preserves the exact native log prefix, sampled words, generated
strict host C model, matches and summary. Use `--end-marker` for a different
successful native test-return marker. It proves use of the new backing with our
shader DATA; it does not assume all backing contains the final array state.
The normal native C client still supplies the independent complete caller-output
oracle. A recycled small-workload control leaves appended samples zero; pressure
changes them again. Kernel#111 qualifies this with two pressure submissions.

### G16C unpublished capacity/allocation rejection

The private-array client accepts `--expect-error=ITERATION:ERRNO` for one
accepted, unpublished rejection, with iteration zero or greater and errno
12 (ENOMEM) or 28 (ENOSPC). It still requires all successful output bytes,
guards, positive binary fences and ordered timestamps. At the rejected
iteration it requires the exact negative binary fence and every caller BO
and timestamp byte unchanged. Later submissions retain the original command
barriers; rejected commands contribute no new GPU history.

For a short capacity probe, boot with `asahi_neo.g16c_operand_buffers=65` and
run `g16c_drm_compute_private /fixture-pressure16384-v2 --repeat=2
--expect-error=1:28` with its default512MiB window. Follow with one small
submission and then a1GiB window/two pressure submissions to qualify recovery
and actual growth. This forced target exceeds Mesa's512MiB compact window;
use a separate default boot for ordinary Vulkan regressions.

For allocation rollback, boot with `asahi_neo.g16c_operand_buffers=25` and
`asahi_neo.g16c_operand_alloc_fail_after=1`, then run the same fixture with
`--repeat=3 --expect-error=1:12`. The diagnostic rejects only once, after one
full8MiB buffer has been staged in a detached heap. The third submission must
succeed in the SAME VM/queue, with one actual20->25 registration and no leaked
8MiB in the retained heap. Optional `asahi_neo.g16c_operand_probe=1` permits
the existing independent scratch CPU model after this successful retry.
This is explicit fault injection, not physical RAM exhaustion qualification.
The failure option defaults disabled and changes no Neo path or wire layout.

Only explicitly transactional pre-publication failures are recoverable.
Ordinary errors, including ENOMEM after ownership transfer/root mapping,
retain backing and fail the session. Do not classify safety by errno alone.

The render client also accepts `--kernel-size=BYTES` and
`--expect-private-space`. In the latter mode it requires exact ENOSPC on the
accepted submission's binary fence and every caller BO and timestamp byte
untouched, then uses normal queue/VM cleanup. Boot with target65, grow a
576MiB window with two pressure submissions, reject rendering into that
retained pool, and repeat two pressure submissions in a fresh576MiB VM.
This specifically checks save-page rebinding after an UNPUBLISHED VM closes.
A following normal512MiB compressed render pair must also succeed. All
ordinary render output/compression/guard checks remain intact outside the
explicit negative mode.

### Initial G16C allocation transactions

Owner, canonical engine arrays, queue backing/save pages, and initial render
resources now prepare detached backing and empty table paths before adopting
RAM and installing resource leaves. Rejected preparation preserves existing
bindings and producers. Empty parent tables may remain for a bounded retry;
errors after adoption/installation remain fatal and retain backing.

`asahi_neo.g16c_initial_alloc_fail` defaults0 (disabled). Diagnostic values
1/2/3/4 reject once immediately before adopting owner/engine/queue/render
backing respectively. These are explicit allocation-failure injections, not
physical RAM-exhaustion tests. For1..3 run the private-array fixture with
`--repeat=2 --expect-error=0:12`, then a fresh lifetime with two successful
pressure submissions and a compressed render pair. The first command must
report exact negative ENOMEM with every caller BO/timestamp byte untouched;
the next must execute on the SAME VM/queue with the original command barriers.
For4 run the render client with `--phase-pair --expect-allocation-retry`. It
checks the negative fence and all caller BO/timestamp bytes before executing
two ordinary independently checked phases on the SAME VM/queue.

CPU queue checks retain four full comparisons to source QueueSet.create,
including48 backing objects and96 alias pages, and fail each of12 queue and
four engine backing allocation boundaries. Both missing-parent table
allocation failures leave no resource leaf; retry uses already prepared
parents without more allocation. Render CPU checks retain the complete source
allocation/rebind DATA/attribute comparison and add four early/late allocation
failures with no installed leaves or descriptor write, followed by successful
same-reservation preparation. These RAM/PTE models complement live GPU output
checks. Neo's complete tagged VM image/allocation comparison remains required
when the shared UAT implementation changes.

### G16C per-flight preparation

The initial-allocation diagnostic also supports5 (compute work pages),
6 (render work pages),7 (geometry),8 (receipt/lease/timestamp/retirement
metadata) and9 (transport after planning, before flight ownership/producers).
All default disabled. Work-page and geometry backing prepare in detached
heaps with empty table paths; no resource leaf is installed before adoption.
Geometry failure preserves its existing addresses, guards and retained blocks.
The existing prepared timestamp Cache is reused for pre-submit cache cleaning.

For5,8,9 compute, use the private pressure client with
`--repeat=2 --expect-error=0:12`, then a fresh two-submission lifetime and a
compressed render pair. For6,7,8,9 render, use the render client with
`--phase-pair --expect-allocation-retry`, then a fresh render pair and two
private pressure submissions. Every injected rejection must report exact
ENOMEM on a binary fence with all caller BO/timestamp bytes untouched; retry
uses the SAME VM/queue and preserves normal positive output/guard checks.
These are controlled failures, not a claim of physical RAM exhaustion.

Transport ENOMEM is recoverable only with Running session and explicit
unpoisoned transport state. Preparation owns the flight/leases before activation or engine arm and
publication performs no further allocation. Engine arm itself can start firmware
work, so preparation must precede it even when queue producers are untouched.
All errors after the first publication store poison the transport; errno alone never overrides that boundary. The CPU Publisher
differential gate retains19 uncertain-effect cases and adds an allocation
rejection with no stores/sends, dropped unpublished leases, unchanged heads
and a successful retry of id1 in the same transport. Geometry CPU checks fail
each of its three initial backing allocations without altering prior leaves,
addresses or retained backing, then preserve the full source image comparison.

`g16c_drm_compute_overlap /fixture-compute-overlap-v1 --dependency` warms
two distinct queues in one VM, then submits a consumer with the producer's
still-pending binary syncobj as an input. Both ioctls precede either completion
wait. Three short pairs check every BO, output, timestamp guard and actual GPU
ordering. `--dependency-close` destroys the producer queue while both commands
are accepted and the producer fence remains pending; stale queue destruction
must return ENOENT, VM destruction EBUSY, and both exported fences must succeed.
The surviving mappings permit full output readback after closing the DRM fd.

These external syncobj inputs follow the common asynchronous Job's CPU input
gate. They do not qualify a firmware cross-generation consumer lease or final
DriverFile release: the output mappings still retain the file for readback.

For a bounded new-VM handoff probe, make a version3 fixture from the existing
own-Mesa balanced compute fixture:

```sh
python3 prepare_g16c_handoff_fixture.py /fixture-compute-balanced-v1 /fixture-handoff
/g16c_drm_compute_cross_vm /fixture-handoff --separate-files --relocate --drain-arrival --return-after-arrival --jobs=12 --arrival-after=4
```

The generator changes only iteration uniforms, expected CPU outputs and the
fixture version. Shader, launch, binding and command bytes stay unchanged.
The native client independently checks both arithmetic results. Arrival after4
accepted old jobs leaves subsequent work on the original queue. Version3
requires actual GPU interval overlap, incoming work before the final old job,
and continued old work after incoming completion, followed by an explicit
old-queue return. Complete BO images and timestamp guards remain mandatory.

`--jobs=4 --arrival-after=1` is a shorter fault probe. With the preceding
render/queue-lifetime sequence it reproduces a firmware event4 and negative
old fence on the current Max driver. A clean compute-only boot can complete
all GPU outputs and the return yet reject the stricter temporal guard when
incoming work covers the final old command. Such a coverage rejection is
different evidence from a firmware timeout or negative fence. Neither closes
the full early-handoff requirement.
