<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Scheduled G16C/G17P UAPI integration and source checks

Native FList bank correction (2026-10-27, Image271/v81): the output-qualified
three-client capture and four-phase plain opaque replay establish that request
+0x30 is the operand dispatch bank, acknowledged by RX13 +8 and support +8.
Native retained render registrations use banks3/4/5 independently of ASID.
The old code registered all owners in bank0, overwrote its directory on every
render and incorrectly rejected bank values above2. The corrected path uses
the existing host Dispatch[context-1] reservation consistently for registration
and growth, preserves other banks and wakes the work loop for every bank.
No common scheduling change or artificial resource limit is introduced.

`check_scheduled_flist_banks.py` compiles actual production constructors with
the versions macro and compares six entire native requests, replies and dispatch
records. Old source rejects native bank3; corrected source passes all six,
including wrong-bank and duplicate-reply rejection. Native/plain replay checks
six changed outputs,3000 guard bytes and18 stage retirements. The resource gate
also passes288 complete parameter/support objects and64 FList requests.

Image271 passes the Max default16 draws and both two- and nine-generation
retained large/small Vulkan tests with normal compression and both parallel
settings enabled. The old second-large-draw firmware timeout is absent.
Receipts `max-vulkan-retained-normal-v49-owned-flist-banks.json` and
`max-vulkan-retained-normal-v51-owned-flist-banks-wrap.json` include all16 CPUs,
66 actual runtime hashes, clean unbind and exact RID0 cold recovery.

Parallel Chromium v52 passes4096 hardware pixels, but subsequent WebGPU cannot
obtain an adapter after host EIO during resource setup. Diagnostic Image272/v53
identifies missing_retained_client during queue recycling. Preparation checked
both logical and selected physical clients, but Max publication checked only
the logical queue. A changed eligible pool could therefore bypass required
client preparation and retire its old queue with no incoming replacement.

Publication now revalidates both identities through the existing replacement
and retry path. The deterministic host gate compiles the actual branch/helper
and passes six interleavings; the original source fails the stale-pool oracle.
Checking only the physical client fails the opposite logical-routing case.
Image274/v84 then passes parallel Chromium4096 pixels, WebGPU16384 cells and
32768 rendered samples over32 generations, post-browser Vulkan, clean unbind
and exact cold recovery (max-browser-shared-v54-physical-client-revalidation.json).
This tests both existing parallel settings1 with all16 CPUs and240 actual runtime
hashes. Temporary failure-phase diagnostics were removed after qualification;
final queue-wrap and repeated client teardown checks remain pending. Both targets
build, but current-turn device access is Max-only under the latest supplied rules.
The monitor still explicitly disables WFxT as documented below. Image/source
hashes for182 inputs are in kernel-neo-lifecycle-v81.json. Earlier sections
below are chronological evidence; their open second-draw issue is superseded
by this bank correction, not a blanket claim that all lifecycle issues pass.

Current graphics validation (2026-10-27, Image269): the committed queue-local
stamp implementation now passes real Max Chromium and WebGPU as well as the
retained Vulkan controls. Receipt `max-browser-shared-v47-local-stamp-no-wfxt.json`
verifies4096 WebGL pixels,16384 Game of Life cells,32768 pixel samples over32
generations, post-browser Vulkan, clean unbind and exact cold recovery.

This run uses the Linux monitor's explicit `--disable-wfxt` diagnostic, adding
the kernel's existing `id_aa64isar2.wfxt=0` override. All16 CPUs and240 actual
runtime-file hashes are verified, including the actual command line and CPU
feature list. The prior Image269 browser run (v46) hit the same CPU4 process_srcu
`__delay` Oops immediately after WFIT, with all general registers reported zero,
before any graphics workload began. The underlying monitor/platform cause is
unresolved; the override avoids this instruction without changing GPU code.
It is not a production kernel restriction or a claim of long-running stability.

The optional15-pool parallel Vulkan control still fails its second large draw
with that override (v48), separating the firmware timeout from this CPU issue.
Retesting retained MMIO resume/start handling after all native packet fixes
also fails (Image270/v45); that candidate was archived and removed. Production
uses the common Neo policy without artificial pool limits or dependency disabling.

The next native reference is three retained Metal processes, one indirect
triangle each for warm and repeat, six pixel/count checks and3000 guard bytes.
`metal_g16c_three_retained_clients.m` is strictly ARM64-compiled but has never
executed on the host or target. Adapt the capture/console/plain-replay workflow
to all three groups before using it; no native GPU qualification is claimed.
The earlier three-pool diagnostic host EIO remains a separate recycling issue.
The Neo parity goal is still active.


Native completion stamp checkpoint (2026-10-27, Image269): captured Max
completion stamps advance in the physical queue, independently of the global
render ordinal. The retained pair/A packet has global ordinal3, queue item1
and stamp0x200; the old constructor emitted0x400. The existing native-data
regression now rejects that old stamp. Both packet construction and owned host
completion checks use the queue-local stamp after the correction; all eight
native descriptor/entry cases and576 ten-image comparisons pass.

Max's default16 draws plus nine retained large and nine retained small draws
pass on Image269, with all pixels, clean unbind and exact cold recovery (v44).
Neo's short parallel control also passes (v43). Max's optional15-pool parallel
control still times out on the second large draw (v42); this correction is not
claimed to resolve that issue. Chromium/WebGPU qualification above is Image267,
not a new browser run on269. The three-pool diagnostic268/v41 exposed a different
unpublished host EIO during old-owner recycling, before the main draw was
published; it did not produce the15-pool firmware timeout. Keep those failures
separate and do not infer a hardware pool limit from that diagnostic.


Native retained descriptor checkpoint (2026-10-27, Image267): eight
output-qualified single-user Max descriptors show TA descriptor ordinal zero,
FR descriptor ordinal equal to its queue-local item, and tail flag1 in both
stages. The previous multi-owner branch wrote TA ordinal1 and tail3 at native
pair/A global3/local1. `check_scheduled_render_preparation.py --retained-native`
now checks these fields in addition to the four entry fields, with descriptor
hashes in the receipt. The old code fails ordinal1 vs0; fixing only that field
then fails tail3 vs1; both corrections pass all eight native descriptor/entry
cases and576 ten-image/address comparisons.

Image267 passes Neo's short Vulkan parallel-admission control and Max's
normal draw. It also passes Max Chromium4096 pixels, WebGPU16384 cells plus
32768 pixel samples, post-browser Vulkan, clean unbind and cold recovery
(`max-browser-shared-v40-native-tail-repeat.json`). The first browser run
(v38) passed Chromium but stopped during WebGPU at a CPU4 process_srcu worker
Oops in `__delay` immediately after WFIT, reporting all general registers zero.
Its log and exact-device recovery are preserved; the cause is unresolved and
its WebGPU result is not a pass. The cold repeat uses the same Image267.

These descriptor corrections do not fix the optional15-pool Max timeout:
v36 still fails the second retained large submission, while Neo v37 passes.
A diagnostic limiting only Max to two pools with both parallel settings still
on passes (Image266/v35). This narrows the investigation but does not prove a
hardware pool limit or measure simultaneous GPU execution. The diagnostic cap
was removed; no such restriction is added to production. An owned-pool append
candidate (Image265) also failed and was archived without promotion.


Current hardware checkpoint (2026-10-27, Image264): Max now follows Neo's
existing render-pool count and admission policy. Previously Max always selected
from fifteen pools and admitted overlapping render owners even when Neo's
`partial_independent_owner` and `alternate_queue_pairs` settings were zero.
The shared default uses one pool and waits for the previous render admission;
the existing cleanup and native-owner guards now apply before either backend.
No firmware dependency is replaced by a CPU wait mode, and no Mesa production
change is needed for this correction.

On this kernel, with production Vulkan and compression enabled, both Max
(all16 CPUs) and Neo (all6 CPUs) pass the original16-draw sampler test plus
nine retained command-buffer submissions at1280x720 and nine at40x20. Every
pixel is checked after poisoning readback again; the ninth submission crosses
the eight-slot scratch reuse boundary. Both cleanly unbind and cold-recover.
Max also passes real Chromium WebGL2 (4096 pixels) and the conditional WebGPU
Game of Life workload (128x128 cells,32 generations,16384 cell checks and32768
rendered pixel samples), followed by another Vulkan draw and clean unbind.
ANGLE/Honeykrisp identifies the actual M4 Max Vulkan device; software fallback
is disabled. The hardware Vulkan DSO is unchanged from the earlier CDM fix.

Evidence is under the bootloader checkout's `build/dual-gpu-20261026/`:
`max-browser-shared-v26-common-policy.json`,
`max-vulkan-retained-normal-v28-common-policy-wrap.json`, and
`neo-vulkan-retained-normal-v30-common-policy-wrap.json`. Exact181-file source
and Image/object hashes are in `neo-lifecycle-restoration-20261025/kernel-neo-lifecycle-v74.json`.
The existing shared render-wave host test also passes23 ownership/failure cases.

This establishes the default production path, not full Neo parity. Enabling
both existing parallel settings still fails the ordinary second retained draw
on Max (v29), while Neo passes even nine repeats with these settings (v32).
Max still selects another idle pool where Neo can append to an already-owned
pool with disjoint finite slots. Continue comparing that ownership lifecycle;
do not describe the timeout as a required firmware difference or mark the
port goal complete. Two-pool admission without pipelining also passes Max's
short control (v31), separating this from a simple prohibition on multiple pools.


Max compute sampler admission now accepts nonzero logical heaps through the
shared parser and existing generated descriptor. G16C permits representable
count+1 values (through0xfffffffe), with full checked count*8 readable VM
coverage, alignment and pointer/count agreement. G17P keeps its existing1024
limit and the source gate compares the entire parameter parser at that limit.
This change does not add kernel shader parsing or client-root rewriting.

`g16c_drm_sampler.c` consumes wholly source-owned shader/texture/GEM inputs
and builds native32 descriptor table DATA in the caller. Default runs are
short; reserve the wide count/pointer sequence for final qualification:

```sh
/g16c_drm_sampler /sampler-fixture
/g16c_drm_sampler /sampler-fixture --raw=17
/g16c_drm_sampler /sampler-fixture --wide
```

The short cases use counts1/17/513. The wide sequence includes65535/65537,
500000 at heapA/B/A, then shrink to1. Every positive case checks two RGBA
samples, the complete output/guards and every non-output BO byte, including
shader, root, logical heaps and native table. Five malformed input/range cases
precede each run. The raw diagnostic leaves the original caller table/root
unchanged and records output, rather than claiming heap conversion: at count17
that table's slot44 is nearest while logical heap entry16 is linear. Raw gives
white; constructing the native table gives gray. Firmware sampler fields alone
do not replace this shader's table. The shim's Mesa-preamble-specific root
adapter is not part of the native kernel interface. Integrating equivalent
bindless table upload into the real Mesa client remains separate work; do not
claim that integration or all sampler semantics from these direct C controls.

Reproduce the fixture with `prepare_g16c_sampler_fixture.py SOURCE OUTPUT`,
where SOURCE is the qualified sampler-wide-nonzero-compute-v248-20261021-inputs
directory. The exporter checks source provenance and every source BO hash,
copies shader bytes unchanged and adds only an empty caller-owned table BO.
It imports no Apple client pages or firmware objects.

Native Max checkpoint (2026-10-22): the shared Rust DRM driver now executes
compute through the existing CPU scheduler, immutable snapshots, ClientLease,
GEM/fence and CPU completion owners. Max supplies its actual context/operand
pool, queue, Work15/SKSM construction and single-ASC publication/retirement.
No second CPU scheduler or fabricated Neo boot image is used. Both ABI profiles
retain their own real initdata types in RuntimeImage.

The focused native tests are:

```sh
/g17p_drm_memory /dev/dri/renderD128 --g16c
/g16c_drm_compute /g16c-fixture
/g16c_drm_compute /g16c-fixture --timestamps
/g16c_drm_compute /g16c-fixture --timestamps --file-close
/g16c_drm_compute /g16c-fixture --queues=2 --kernel-base=0x7100000000
/g16c_drm_compute /g16c-fixture --replace=3 --queues=2 --destroy-accepted
```

Build the first two executable names from g17p_drm_memory.c and
g16c_drm_compute.c using the cross compiler, installed UAPI headers and
`-Wno-unused-function` for shared C helpers. The default Neo memory invocation
and expected parameters are preserved. The compute fixture consists entirely
of our Mesa-generated constant32 GEM resources and one command; it supplies no
firmware object or Apple binary page. Its source manifest and checksums are
preserved in the Max checkpoint fixture-v2/provenance.json.

On the final native Image, the memory test passes6303 assertions. Each compute
mode submits twice, resets the poisoned output, checks all16384 deadbeef words
and every other byte in the81920-byte output GEM. The timestamp mode also
checks two ordered caller-owned start/end pairs, invalid object IDs/offsets
and the complete timestamp BO guards. Two iterations do not exercise the
shared helper's iteration16 timestamp-object rebind. All16 CPUs remain online;
firmware stays healthy, and subsequent platform unbind completes.

Max admits compute and nonzero sampler fields through this path. Render
is still rejected. Hardware admission serializes
Max's shared operand view/engine rebind; native concurrency, queued firmware
dependencies, FList growth and render/private-pressure recycling remain separate ports.
Published Max contexts now close through seven maintenance/tick records and
one close record per used queue, using the live firmware-assigned prefix
indices. Every record must retire before exact owned TTBR invalidation and
dropping Client/root/BO references. Deferred closes are reaped by the common
scheduler after CPU completion. Uncertain controls poison the session and
retain backing. Neo's existing permanent BO retention remains unchanged.

The final short native lifetime gate passes six submissions across three
successive application lifetimes: two explicit VM destroys and one DRM file
close. All three have kernel release receipts and reuse firmware context1.
The serialized compute engine also reuses one five-page work slot, checking
all exact mappings before clearing it after GPU and CPU retirement. This
removes the work-generation arena and total-context-lifetimes limits. There
are still at most63 concurrent firmware context namespaces.

Retired Max private pool chunks, support/high-root pages, queue records/rings,
save/robustness pages and context/queue slots now reuse retained backing. All
old TTBR words and support mappings are checked before pool reuse. Fresh
application root paths are prepared before clearing and remapping the private
chunks; directories and counters are rebuilt for the requested VA base. Queue
reset checks every exact high mapping and ctx0 alias before clearing even one
byte. Engine SKSM arrays have independent Session-wide backing and aliases;
reset never clears them. Physical backing stays in common Memory until stopped
teardown. Cached slots track the high-water mark of firmware-retained contexts
and queues. Standalone compute queue destruction now registers an accepted-job
activity owner, including jobs blocked before any firmware queue construction.
The existing async Job increments it at acceptance and decrements it only at
unique retirement. A close request survives until all those jobs and GPU/CPU
leases finish. Max reaps it before the public completion signal, using eight
retired maintenance/tick/close controls and checking every installed VM's exact
roots remain unchanged. Neo allocates no activity owner and keeps its original
async behavior. No second scheduler or cleanup worker is introduced.

Within the same live root, a retired queue's save mappings are checked exactly
and reused without advancing the VA cursor. A queue retained by another live
owner cannot be reused; VM release detaches its save slot after root invalidation.
Once every queue closes independently, detached VM release invalidates owned
roots and performs six post-root maintenance controls if no survivor remains,
matching the qualified source sequence.

The final compute-only native recycling gate passes138 checked submissions
across69 application lifetimes:68 use two queues and the last returns to one.
Thirty-four explicit VM destroys and35 file closes produce69 kernel receipts,
620 retired controls and reuse of context1. The extra66-lifetime loop exceeds
the old63-total-lifetimes cap and crosses the256-entry control ring twice.
Retained driver backing is flat after the first two-queue VM:181 allocations,
196820992 bytes, one pool slot and two queue slots. Private VA bases alternate
between0x7000000000 and0x7100000000; the final single-queue case uses0x7200000000.
There are68 guarded ordered caller timestamp pairs. All16 CPUs remain online;
subsequent unbind removes DRM nodes without reported errors. This qualifies
serialized compute close/recreate recycling, not simultaneous application VMs,
render, private-pressure/growth or fault-path reuse.

The subsequent live-VM queue gate passes528 checked submissions in five VM
lifetimes. Its final run keeps one VM and a survivor queue open for512 jobs,
destroying256 other queues while their first accepted job waits on an imported
software fence. Each blocked output remains poisoned until the input signals;
old queue IDs reject new submissions. Then both the accepted job and survivor
produce correct complete output. All266 actual queue retirements preserve
roots and retire eight controls;259 occur after imported-fence-blocked queue
destruction. Five VM release receipts include the detached and file-close
paths, for2161 total retired controls. After warm-up backing stays181
allocations/196820992 bytes, one pool/two queue slots. The post-run6303 memory
assertions and platform unbind pass with all16 CPUs online.

Keep normal development runs short (`--replace=3`). `--replace=256 --queues=2
--destroy-accepted` is the final boundary run, not a routine smoke test.
This still does not qualify concurrent application VMs, render/private-pressure
or fault-path recycling. Retiring obsolete binding-generation roots/BOs after
BO rebind within one still-live VM also remains a separate port; unchanged
binding snapshots are what this queue-replacement gate reuses.
`--destroy-accepted` uses CONFIG_SW_SYNC only to hold
an imported input fence; GPU execution remains the native driver.

The `--file-close` banner alone does not prove context retirement: require
the corresponding kernel close receipt before unbind. The timestamp helper's
generic banner mentions object rebind, but two iterations never reach that
case. Post-compute platform unbind passes with all16 CPUs still online.
No optional Max feature bits are advertised yet. Normal board DTS still keeps
the GPU disabled pending permanent PMGR ownership; hardware tests use the
explicit passthrough monitor and loader-populated typed FDT.

`scheduled_operands.rs` defines Max's compute FList Default template, including
the compute owner's explicit kind/phase overrides. The short gate compares32
whole pages with the qualified source constructor and asserts size, alignment
and every field offset:

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_operands.py /path/to/max-m1n1 \
    --evidence /path/to/fresh-evidence-directory
```

`check_scheduled_session_shape.py` compares35 original Neo Session defaults,
probe, complete CPU scheduler/async worker, queue create/destroy,
identity/GET_PARAMS and ioctl admission with
the preserved tag. Only immutable G16C-only branches and typed image access
are excluded. This is source-preservation evidence; live Neo GPU regression
is still required. Passing layout, compile or memory tests alone does not
qualify GPU output or retirement.

`check_scheduled_queues.py` compares all48 fresh backing images and four complete
ctx0 alias images against qualified source QueueSet.create. It also poisons
each queue and verifies a reset returns every byte to its template with zero
new allocations, preserves independent canonical engine arrays, and rejects
a changed final alias before any zero. Run with the typed-memory FDT fixture:

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_queues.py /path/to/max-m1n1 \
    --inputs /path/to/python-platform-inputs/input.json \
    --fdt-template /path/to/typed-memory-fdt-fixture.dtb \
    --evidence /path/to/fresh-evidence-directory
```

`scheduled_lifetime.rs` uses source Default templates with every field offset,
size and alignment asserted. Its short differential gate compares71 complete
64-byte maintenance/tick/close records with the qualified source encoders,
including varying firmware-assigned indices and seven malformed inputs:

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_lifetime.py /path/to/max-m1n1 \
    --evidence /path/to/fresh-evidence-directory
```

The G16C/G17P integration starts at the `neo-known-good-2026-10-21` tag.
`AGX_SCHEDULED` is a separate versions-macro group with one axis, `G`, and
the concrete profiles `G16C` and `G17P`. Firmware is fixed to macOS 26.6.2.
The older `AGX` group retains its existing generation/firmware combinations.
The operator-approved boundary keeps scheduled-family initdata definitions and
construction separate from the older Asahi initdata. Within this newer family,
use versioned layouts for stable meanings with changed placement/presence and
explicit construction for different roles or handshakes. Share a leaf record
across families only when both its layout and meaning are established.

For each converted wire structure, first check every field offset, total size
and alignment against the RE layout, then compare the complete encoded image
with the established encoders and available captured protocol DATA. Constant
blocks use source-defined `Default` templates; constructors override variable
fields. Owned staging values must never become mutable references to live
firmware storage. Do not claim GPU qualification from layout or compile checks.

The first converted objects are `scheduled_wire::QueueRecordG16C/G17P` and
`QueuePointersG16C/G17P`; the ordinary Neo queue builder uses both generated
G17P structures. Neo's 0xac queue wire record stays separate from its 0xc0
allocation stride. With Python 3.10+
and `rustc`, the paired layout/reference check is:

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_wire.py /path/to/max-m1n1 \
    --g17p-layout /path/to/neo-m1n1/proxyclient/m1n1/agx/g17p.py \
    --native-queue /path/to/captured/stage-0-queue.bin \
    --native-queue /path/to/captured/stage-1-queue.bin \
    --native-queue /path/to/captured/stage-2-queue.bin
```

Native queue inputs are optional DATA-only comparisons; source/layout checks
always run. The Neo oracle is loaded from the exact preserved commit via Git,
so this check does not compare the new implementation against itself.
`host_versions.py` builds the production macro for the standalone host tools.

`scheduled_initdata.rs` separately defines the scheduled-family initialization
roots, UAT levels, channels, main configuration, MMIO and hardware-region records.
`FirmwareRole::{Primary, Secondary}` identifies the boot owner in host metadata;
it is not another macro axis or an added wire discriminator. G16C constructs one
primary instance, while G17P constructs both roles. Main defaults/channel subsets
and root form follow the role. Each ASC peer publishes the root selected by its
role, independently of its position in a list. Kind words remain ABI-specific:
G16C's primary main object and G17P's secondary main both contain kind four.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_initdata.py /path/to/max-m1n1 \
    --g17p-initdata /path/to/neo-m1n1/proxyclient/m1n1/agx/g17p_initdata.py \
    --native-root /path/to/captured/root-data.bin \
    --native-main /path/to/captured/main-data.bin
```

This checks RE offsets before source bytes, complete Max template output, the
tagged Neo encoders and whole initialization graph, role defaults and rejection
before mutation. Native inputs are bounded DATA objects only. The historical
graph test's stale mailbox expectation is corrected in its baseline test adapter;
the baseline production builders remain untouched. Layout/source agreement and
an ARM64 object compile do not qualify native Max firmware startup or GPU work.

`scheduled_hwdata.rs` describes both complete 0x3db4-byte hardware-data
layouts and the shared clock/voltage table. Max's 0x2596 word stays explicitly
unknown (value 46); it is not assumed to mean Neo's twelve work channels.
Constant-heavy objects have source `Default` templates. Production construction
copies these from static storage, then assigns fields in exclusive initialized
host staging memory, avoiding a whole-object kernel-stack temporary. Neo uses
the shared transport defaults and voltage-table constructor; its final policy
overlay and explicit opaque-field API keep their existing ordering/behavior.
Max supplies four cluster voltage columns and sixteen CS-state slots through
explicit board inputs. This does not yet connect Max to native startup.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_hwdata.py /path/to/max-m1n1 \
    --inputs /path/to/saved-adt-platform-inputs.json \
    --g17p-initdata /path/to/neo-m1n1/proxyclient/m1n1/agx/g17p_initdata.py \
    --native-hwdata /path/to/captured/hardware-data.bin
```

This checks independently parsed RE offsets and complete Max images, varying
all cluster columns, MMIO pointers and CS counts 1 through 16. Negative geometry
checks require an untouched output buffer. The optional native comparison
decodes understood variable pointers/region fields only; board fields, policy,
reserved bytes and every other byte must be rebuilt from source. Run the
initdata gate above as well to compare all Neo graph images against the tag.

`scheduled_board.rs` consumes typed FDT properties and operating-point
nodes, following the existing Asahi boundary. m1n1 selects the original ADT
rows, calibrates the board, and fills `t6041-gpu-config.dtsi`: eleven GPU OPPs
and the eight CS OPPs present on J616c. Linux does not parse or group raw ADT.
Each GPU OPP carries standard `opp-hz` and four `opp-microvolt` cells, plus
`apple,alternate-opp-hz`, four `apple,sram-microvolt` cells, two
`apple,perf-index` cells and two `apple,relative-power` cells. The CS table
uses the same `apple,cs-opp` phandle as Asahi. Scalar configuration uses normal
big-endian `apple,*` cells, including IEEE-754 bit patterns for power gains.

`scheduled_power.rs` holds the separate G16C controller wire layout and static
Default template. Typed `SettingsG16C::from_fdt` reads Asahi-style power names;
derivation validates properties and computes software-float values in compact
host storage before encoding changes the destination. The 0x63d4-byte wire
object is copied from static storage, then assigned through named fields in
exclusive host staging. Its graph pointers remain explicit constructor inputs.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_fdt.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-g16c-config-fixture.dtb \
    --native-hwdata /path/to/captured/hardware-data.bin
python3 drivers/gpu/drm/asahi/tests/check_scheduled_power.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-g16c-config-fixture.dtb \
    --native-controller /path/to/captured/controller-data.bin
```

These execute the actual m1n1 C population function with libfdt against a
compiled copy of the shipped DTS fragment, then compile the production Rust
OF readers and serializers. Expected firmware images come from the independent
qualified Python builders. The host OF adapter decodes normal big-endian FDT
cells, not ADT. The hardware gate varies frequencies, all cluster columns, CS
counts and GPC/sampling configuration and rejects malformed FDT cells/topology.
The power gate traces the independent encoder's field offsets/widths, covers
controller properties and rejects exceptional gains/overflow. Native DATA
comparisons cover each complete object. This configuration path is not yet
connected to native Max probe/startup; it does not qualify GPU execution.

`scheduled_boot_records.rs` shares the versioned Region C and primary-status
layouts, using separate source Default policies for G16C/G17P. Neo's normal
graph now uses these templates; complete graph output remains identical to
the preserved tag. The old duplicate constant runs have been removed.

`scheduled_initgraph.rs` composes Max's hardware/controller/main bundle and
all 36 boot RAM spans. Its eight context-zero views must alias the same physical
backing as their firmware views. The opening control slot overlaps main+0x4c0;
it belongs to the bundle allocation. `ImageG16C` owns unpublished CPU staging.
The bundle names the additional observed scalar locations after entropy, but
keeps the qualified source default of zero: their meaning/producer has not been
established, so observed calibration words are not universal constants.

`scheduled_mmio.rs` builds Max's 25 sparse register records and device mappings
from SoC aperture constants plus two typed FDT inputs populated by m1n1:
`apple,firmware-sw-interrupt` (u64 physical address) and `apple,active-mcc-mask`
(u32). MCC windows and the four cluster windows have separate physical strides.
The hardware-status alias uses its observed shared-NC attributes; none of these
device mappings are RAM allocations or objects to clear.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_initgraph.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-g16c-config-fixture.dtb
```

This executes the actual qualified Python `Bootstrap.build_graph` in bounded
host RAM, the m1n1 C FDT population function and the production Rust constructors.
It compares every byte of four whole graphs, every RAM permission and alias,
all channels and MMIO maps. Extra cases compare each possible MCC bit and the
full 32-window mask with the source builder. Bad storage/configuration must fail
before graph storage changes. No target is accessed. Reservations, UAT and
single-ASC startup are now wired below; native work submission remains pending.
Source-image agreement and compile success do not establish execution.

`t6041-gpu-memory.dtsi` supplies the five standard reserved-memory nodes and
phandles for TTBS, firmware page tables, shared L2, handoff and firmware image.
m1n1 validates the complete geometry/target shape before publishing normal
big-endian `reg` cells, enabling those reservations and setting the private VM
range. The GPU node stays disabled pending native hardware qualification.
`scheduled_resources.rs` validates the resulting Linux resources; the new Max
Memory constructor maps only the four RAM regions needed for host access.

`scheduled_vm_g16c.rs` constructs Max mappings with the existing common `Vm`
walker. G16C allocates its own backing instead of using Neo's fixed physical
placement tables. Contexts zero/one have independent low/high roots; firmware
keeps its existing high root and boot-code mappings. The constructor reads
bounded page-table DATA, not firmware executable pages, and never accesses
MMIO through CPU RAM operations. It retains original boot-table DATA until
stopped cleanup can detach all links to owned pages.

The qualified ordering is cold validation/VM preparation/context clearing,
RTKit management boot, then graph population. Retain `Vm::prepare_g16c` in the
session before RTKit starts, and use that same VM for any management buffer
mappings before calling `populate_g16c`. Failure must preserve its backing until RTKit stops.
The common session destructor then invalidates Max's contexts, performs the
global UAT invalidation, restores inherited tables and releases ownership.
Any shutdown/restore failure retains backing. `build_g16c` is a convenience
transaction for a stopped ASC, including the host tests; it is not a replacement
for that management-running ownership sequence.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_resources.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-config-and-memory-fixture.dtb
python3 drivers/gpu/drm/asahi/tests/check_scheduled_vm.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-config-and-memory-fixture.dtb
python3 drivers/gpu/drm/asahi/tests/check_scheduled_vm_neo.py
```

The memory fixture includes both shipped Max DTS fragments. Resource tests run
actual C/libfdt publication and Rust OF parsing. VM tests run production image
construction and the common walker against bounded owned RAM, comparing every
boot object with actual Python startup output and checking all RAM permissions,
aliases, MMIO PTEs, isolated roots, inherited code mappings, failures and stopped
cleanup. The Neo differential check runs the original tagged walker and current
walker with the unchanged Neo graph, comparing the complete RAM/physical-address
image and allocation count. These are source/host checks, not native GPU tests.
The full Max SoC/board DTS and native work remain pending; single-ASC startup
is wired below but still needs hardware qualification.

`scheduled_boot_g16c.rs` now wires single-ASC startup into the common `Session`
owner and platform driver (`apple,agx-g16c` / `apple,agx-t6041`). It checks the
cold ASC/GPU identity, prepares UAT, applies AXI/gate/handoff/context state,
releases ASC RUN and the gate with adjacent stores, waits for RTKit, installs
the graph and publishes context zero/initdata. Max accepts through status-A+0x14;
it then retires the 0x16 opening and six 0x1b maintenance messages. The application
context stays unpublished until work admission. This path does not fabricate a
Neo graph or register unsupported DRM capabilities: native Max command lowering
and full SoC/board DTS are still pending, and the loader keeps the GPU disabled.

`scheduled_bootstrap.rs` implements the source DATA transactions and bounded
boot receive-ring servicing. Unknown/unowned events stop bring-up; independent
consumer/retired/producer positions must all match before retirement is accepted.
The shared Session defaults and Neo probe body remain identical to the tag.
Probe construction is split into per-profile helpers to bound stack usage;
large MMIO staging and Session defaults initialize directly in heap storage.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_bootstrap.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-adt-inputs.json \
    --fdt-template /path/to/compiled-config-and-memory-fixture.dtb
python3 drivers/gpu/drm/asahi/tests/check_scheduled_session_shape.py
```

The bootstrap gate compares complete RAM images after running the actual Python
publication/opening methods and EventService. Firmware responses and the host
ASID primitive are simulated; this does not qualify native startup or barriers.
`inspect_scheduled_native_code.py` checks only our compiled ARM64 Linux object
for the adjacent RUN/gate stores and major stack frames, without inspecting
Apple executable content. Major-frame totals exclude nested callees and are
not a substitute for hardware execution or a complete stack-depth analysis.

`scheduled_compute.rs` generates `ComputeDescriptorG16C/G17P` and
`ComputeProgramG16C/G17P` from the same hardware ABI axis. Neo's active compute
builder now delegates to them; its descriptor interface and forty-write program
remain unchanged. Max has its distinct scratch offsets and optional 36-write
Metal / 40-write queue-state forms. USC execution base remains fixed.

Both descriptor profiles copy source-defined Default templates from static
storage into exclusive CPU staging. The Max native packed object is 4 KiB;
a standalone allocation may be 16 KiB with a zero tail. The unaligned zero-page
pointer overwrites the high byte of the preceding ordinal word, represented by
nonoverlapping byte fields. Neo's fixed header counts and first-match scalar
lookup are preserved. Max computes header counts, takes scalar fields from
explicit inputs, and selects last matches for its secondary register array.
These source policies differ even though the final wire shape matches.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_compute.py /path/to/max-m1n1 \
    --evidence /path/to/new-evidence-directory
```

This compiles the actual production macro/layouts and the independently tagged
Neo encoder. It checks 256 complete Neo pages/programs, 195 complete Max pages/
programs against current source, two native ready descriptor DATA images,
packed and full-page output, malformed inputs before mutation and every declared
field's offset/size/alignment. No executable capture content is used. Max's
queue/scheduler records, work publication and native UAPI wiring remain pending;
these byte comparisons and native object compilation do not qualify GPU work.

`scheduled_work.rs` now generates the compute Work15 wrapper (0xc0),
SKSM/context record (0x200), and scheduler record (0x100) for G16C/G17P. All
fields, reserved spans, sizes and alignment have compile-time assertions.
Constructors populate source Default templates. Neo's active Optional, Context
and Scheduler encoders delegate to them; dependency-list writing and reuse of
firmware-owned bytes keep their existing behavior. Max computes its compact
register locators from the descriptor heap and preserves independent mixed-work
header/control and wrapper flag inputs. Only the source-qualified Max scheduler
construction is exposed; Neo's node/phase policy is not inferred for Max.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_work.py /path/to/max-m1n1 \
    --evidence /path/to/new-evidence-directory
```

The gate compares 388 complete Max records, including four independent native
protocol DATA objects and 64 actual ComputeWork.prepare cases, with 1024 complete
Neo wrapper/context/scheduler/reuse images against the original tag. It exercises
packed locator/count boundaries and near-limit item generations without adding
temporary runtime caps. The earlier descriptor/program gate must also pass after
these shared encoder changes. This does not qualify native queue allocation,
producer publication, event ownership or GPU execution.

`scheduled_queues.rs` prepares a G16C three-lane queue reservation through the
existing common `Memory` and `Vm`. Each lane has a private header, shared cursor
page, private work-ring page and shared scratch allocation with read-only
context-zero aliases. The source 0x500 cursor geometry remains separate from
the allocated ring page. `QueueContextG16C/G17P` now has a versioned Default
layout for the 0x40 control object. Session requires Running phase and a Max
VM graph, then invalidates UAT before returning the prepared queue storage.
No producer or application root is published by allocation.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_queues.py /path/to/max-m1n1 \
    --inputs /path/to/saved-loader-inputs.json \
    --fdt-template /path/to/compiled-config-and-memory-fixture.dtb \
    --evidence /path/to/new-evidence-directory
```

The gate executes actual Python QueueSet.create with private rings, compares
48 complete RAM owners and four complete alias images, and checks all PTE
permissions/physical identity, independent roots and unchanged TTBS. It covers
all four queue priorities, custom aliases, invalid geometry, collisions and
partial allocation refusal without replacing/freeing earlier backing. These
pages remain Session/Memory-owned until stopped cleanup. Per-queue retirement,
reclamation and public UAPI allocation still require integration; the prepared
storage is not a claim of a usable native DRM queue. Privileged invalidation
and GPU execution are not exercised by this host gate.

`scheduled_transport.rs` adds Max's publication transaction to the common
Session. It retains common ClientLease owners before the first store, enforces
both-consumer half-ring bounds, emits fragment/tiling/compute in source order,
publishes outer producers after a barrier, and sends the per-stage plus final
doorbells. Retirement requires owned stamp/counter progress, both consumers of
each ring, notifiers and control drains. Uncertain stores/sends poison the Max
session and retain leases. The common shutdown failure path retains this state.
Neo leaves the new transport field None and retains its existing publication.

```sh
python3 drivers/gpu/drm/asahi/tests/check_scheduled_transport.py /path/to/max-m1n1 \
    --evidence /path/to/new-evidence-directory
```

This executes the production Rust transaction and actual Python Publisher with
bounded DATA/translation stand-ins and counted lease stand-ins. Traces include
22 successful publication calls, 19 failures after each write/send, nine
pre-write/live-flight rejects, independent flight release and 360 ring-boundary
checks. It does not execute native translations, cache instructions, actual GEM
owners or GPU work. Public Max DRM submission and native event/fence wiring
remain incomplete; the next milestone is a native boot/probe test.

The frontend accepts copied commands and publishes pending output fences before
returning. Firmware events and DMA-fence callbacks advance the backend; pending
external inputs have no GPU execution timeout. Jobs retain their file, VM and
GEM references until completion. Published backing pages remain retained.

Layout and source serialization checks are required during integration.
Completion also requires hardware execution through the Asahi UAPI. Run consecutively in one driver
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
transport rotation. The ordinary path keeps separate firmware queue/context/private-state owners
for public queues. At each 128-command transport boundary, it can publish the
next generation on another owner while the old generation is live. Only that
owner's own retirement permits reusing its two finite transport banks. Native
diagnostic profiles retain the previous transport path.

Independent compute queue qualification (ordinary default `compute_queues=1`):

```sh
python3 drivers/gpu/drm/asahi/tests/make_g17p_compute_workload.py /path/to/m1n1 /tmp/g17p_drm_retained_wave_workload.h --batch-count 56 --mixed
python3 drivers/gpu/drm/asahi/tests/make_g17p_render_workload.py /path/to/m1n1 /tmp/g17p_drm_render_batch_workload.h --triangles 131072 --batch-pair
gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -I/tmp -Iinclude/uapi/drm -Iinclude/uapi -o g17p-drm-compute-queues drivers/gpu/drm/asahi/tests/g17p_drm_compute_queues_render.c
python3 drivers/gpu/drm/asahi/tests/make_g17p_render_workload.py /path/to/m1n1 /tmp/g17p_drm_render_long_workload.h --triangles 524288 --batch-pair
gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -I/tmp -Iinclude/uapi/drm -Iinclude/uapi -o g17p-drm-first-compute drivers/gpu/drm/asahi/tests/g17p_drm_first_compute.c
gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -I/tmp -Iinclude/uapi/drm -Iinclude/uapi -o g17p-drm-compute-render-overlap drivers/gpu/drm/asahi/tests/g17p_drm_compute_render_overlap.c
```

On a cold driver, run `g17p-drm-compute-vm --cleanup` followed by
`g17p-drm-compute-queues 25`: three independent public queues/VMs and distinct
private apertures, 4200 computes, complete input/output/guard checks, each
batch's exported fence status, and every command's timestamp pair. The first
render is submitted after accepting the initial compute batches; eight full
images and its own fence must pass. Reverse wait order does not substitute for
checking every queue. Firmware publication logs and retained queue counters
provide the rollover/ownership evidence.

On a separate fresh boot, run `g17p-drm-first-compute`: the first compute is
submitted while a long render's fence is pending, and GPU timestamps must
show overlap. Both complete render images, compute outputs/guards, timestamp
guards, and owned fences are checked. Follow with the retained integration
suite above, mixed dependency/error/teardown tests, and the 90-image MSAA suite.
No unit tests are required. GPU executable fixtures remain untouched.
The full qualification sequence starts with render-only VM cleanup, then
first-compute overlap, repeated compute/render overlap, the three-VM compute
suite, mixed asynchronous dependencies/errors/teardown, 4,096 compute stress,
and the retained integration/90-image MSAA/close/pressure regressions.

Run `g17p-drm-compute-render-overlap 25` in the retained integration session:
4,200 computes across three public queues/VMs, and 25 repeated 131,072-triangle
renders. Each batch is accepted behind an imported input fence; after actual
TA start, release all compute queues. Every retained wave must show compute/FR
GPU timestamp overlap. Check all eight full images per wave, every compute
input/output/guard, timestamps, and each exported fence status. The initial
allocation wave checks all outputs but permits setup latency to miss overlap.
The first-compute fixture uses a separate 524,288-triangle source-generated
render to keep its measured overlap window longer than cold allocation.

Independent compute uses the observed native asynchronous profile: render
completion control zero, compute execution gate one, and support word08 one.
Completion scratch, event allocations, and preemption/robustness records are
private to every live command; operand allocator state stays private to its
queue. Context reuse preserves firmware-written words, and retired transport
snapshots use page-sized volatile reads with checked mappings/cache ordering.
An idle full-window owner rotates with its existing caller/root; it does not
materialize a replacement VM just to discard it. Retired owners are reused
before allocating another permanent owner; live owners cause growth up to the
available context capacity, rather than a global completion wait. Backing
pages stay retained.


Render address-space integration (`partial_independent_owner=1`,
`alternate_queue_pairs=1`, ordinary `compute_queues=1`):

```sh
gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -I/tmp -Iinclude/uapi/drm -Iinclude/uapi -o g17p-drm-render-roots drivers/gpu/drm/asahi/tests/g17p_drm_render_roots.c
g17p-drm-render-roots 1024
g17p-drm-render-roots 256 --geometry
g17p-drm-render-roots 32 --same-vm
```

Use the unchanged `g17p_drm_render_batch_workload.h` generated above. The first
case submits two public queues in different files/VMs at identical DVAs with
distinct GEMs, alternating submission order. Each pair checks all 16 active
output images and 16 inactive image guards, both exported sync-file statuses,
every GPU timestamp, and timestamp guards. Every pair after initial allocation
must overlap according to GPU timestamps. `--allow-serial` retains all output
checks for the old-kernel baseline; it does not qualify the new behavior.

`--same-vm` uses two queues in one VM. It unbinds the second stream's original
output mappings while idle, submits the first render, then binds those outputs
before submitting the second render. The first ioctl retains an earlier mapping
snapshot; the second must execute through its newer one while the first is
pending. Both original GPU streams remain unchanged. All 16 outputs, both own
fences and all timestamp guards must pass, with retained overlap.

Repeat both cases after a fresh compute-first startup (`g17p-drm-compute-vm
--cleanup`, then the two compute queue/overlap cases above). Adopting the dormant
render graph must create the same independent pools before its first producer;
an uninitialized pool cannot be required as the condition for creating it.

Each ordinary render pool retains its root and ASID. Only that pool's own
retirement permits mapping replacement; other pools remain installed. Completion
refresh and growth requests use the ticket/pool's exact root. Render and compute
reserve from the same 64-slot hardware ASID budget. Prefer an idle exact caller
snapshot before rebuilding mappings in another pool; a live matching owner does
not prevent using another available pool. Physical backing is retained.

Qualify these cases together with the complete compute/mixed/dependency/error/
teardown/pressure/90-image MSAA/after-compute/dirty-timestamp integration sequence.
An isolated render or queue-retirement pass is insufficient. No unit tests are
required. Validation uses all six CPUs and the existing HV setup with AGX
tracing disabled; it establishes overlap, not absolute native-EL2 performance.


`g17p_drm_wait.h` adds explicit output-fence waits to the older integration
programs. `g17p-drm-async` bypasses that adapter for submission and proves
unsignaled acceptance, copied commands, same-queue ordering, independent-queue
progress, binary/timeline completion and dependency error recovery. Its backend
mode holds 16 accepted jobs beyond two seconds, destroys their queues, closes
output GEM handles, checks logical VM_UNBIND succeeds while accepted work retains its immutable
mapping snapshot, interrupts a UAPI
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


Queued render ownership integration (the ordinary async profile above):

```sh
make ARCH=arm64 headers_install INSTALL_HDR_PATH=/tmp/g17p-uapi
python3 drivers/gpu/drm/asahi/tests/make_g17p_render_pending_workload.py /path/to/m1n1 /tmp/g17p_drm_render_pending_workload.h
aarch64-linux-gnu-gcc -O2 -Wall -Wextra -Werror -Wno-unused-function -static -I/tmp -I/tmp/g17p-uapi/include/drm -I/tmp/g17p-uapi/include -o g17p-drm-render-pending drivers/gpu/drm/asahi/tests/g17p_drm_render_pending.c
g17p-drm-render-pending 128 --require-overlap --cold
g17p-drm-render-pending 128 --require-overlap
```

The fixture submits 32 independently addressed, long renders on four public
queues before waiting. Each wave checks all 256 complete output images,
32 exported sync-file statuses, four GPU timestamps per job and timestamp
guard pages. Every wave must have at least three GPU-live renders, including
transport rollover; a successful ioctl or retired queue is insufficient. The
cold variant skips serial preflight and must run first on a fresh boot. Run
the preflight variant again after the retained compute/mixed/Mesa regression
suite. Firmware publication diagnostics additionally report how many older
uncompleted render tickets share the selected pool. All physical backing
remains retained. No unit tests are needed.

`g17p-drm-render-roots --geometry` alternates distinct width and utile height
while the other application's render remains pending. Independent pool-local
scratch leases permit the geometry change without clearing live scratch.
The same full-image/fence/timestamp and retained overlap checks apply.

### Logical unbind and CPU concurrency qualification

`g17p-drm-unbind-pending 256` keeps an accepted command behind an imported
input fence while unbinding its mapping and timestamp alias, partially unbinding
a second BO alias and rebinding the same DVA to distinct backing. Independent
work and the new mapping must complete first; the original command then writes
only its old backing. All output, guard, timestamp, binary and timeline values
are checked. `--input-error` closes the unsignaled producer instead: the old
command must fail with ENOENT and leave every old output/timestamp unchanged.

`g17p-drm-render-unbind 32` removes eight output mappings and the timestamp
alias while a render fence is pending, admits a second queue with different
backing at the same DVAs, and checks both complete old/new images and all four
GPU timestamps and guards. GPU timestamp overlap is reported per round.

`g17p-drm-cpu-parallel prepare` uses three independent queues/VMs. With boot
parameter `asahi_neo.cpu_prepare_pause_queue=2`, one queue's private preparation
is deliberately paused for two seconds outside the device mutex. Queue 3 must
complete within 1.5 seconds while queue 2's fence remains pending. `cache` uses
`asahi_neo.cpu_cache_pause_grid=32` to pause CPU visibility after GPU retirement;
another owner must complete before the first fence can signal. Both compare
complete output and timestamp/guard regions. These diagnostic parameters are
disabled by default. Render-owner and contention measurements supplement these
fixtures before the runtime-mutex checklist item can be marked complete.


`render-prepare` and `render-cache` extend the same pause test to render
owners. The first uses `asahi_neo.cpu_prepare_pause_queue=2`; the second uses
`asahi_neo.cpu_cache_pause_grid=1` for the fragment owner. Complete old/new
images, owned timestamps and guards must pass alongside independent compute
completion before the two-second pause ends.

`g17p-drm-submission-contention` runs the retained 48-job pressure oracle
without logical unbind so that a driver with broad unbind guards can run the
identical workload. Use fresh matching baseline/candidate boots with
`asahi_neo.submission_log=3`. Compare all `RUNTIME_INTERVAL` phases together
and `SUBMIT_PUBLICATION` latency; the latter includes the deliberate imported
input-fence delay. The 48-job publication span measures only the released
burst. Keep the one-time cold firmware bootstrap separate when interpreting
the bulk-work comparison. Every own output/image/guard and binary/timeline
fence must pass; timing without those checks is not qualification.

`g17p-drm-compute-reconfiguration` alternates 320 submissions between two
DRM files at colliding DVAs. It checks every own and inactive output, guard,
sentinel and retained timestamp across repeated owner cleanup/re-registration,
context reuse and finite transport rollover. It uses the same generated
`g17p_drm_workload.h` as `g17p-drm-compute-vm`; GPU programs are unchanged.

`g17p-drm-gpu-dependencies rcrc 16` and `rrcc 16`, with optional `--split`,
queue 32 long prefix renders with distinct output backing before the mixed
tail. Every complete image, owned fence/timestamp and guard is checked; an
unrelated ready compute must finish while the final producer remains pending.
With `asahi_neo.firmware_dependencies=1`, publication logs must additionally
prove native dependent publication before producer retirement. That parameter
is diagnostic and disabled by default. `error 1` requires `tvb_max_blocks=8`
and both `first_render_fragment_sync_grow=1` and `repeat_fragment_sync_grow=1`:
an actual owned type7 must fail producer/consumer fences and leave consumer
output/timestamps untouched. This cancellation oracle currently fails; do
not treat correct fence errors or normal-only passes as completion of native
dependency qualification. Independent compute context receiver slots must be distinct from render pools.
The old fixed slot2 collided with render pool2. Using compute grids32..91
as receiver slots passes fresh RRCC/RCRC combined/split runs (80 strict rounds)
and accumulated contention48 twice plus compute320 and mixed48. Engine bits,
event-record counter kind, firmware milestones and physical backing are unchanged.
This fixes the normal mixed failure; the cancellation oracle remains open.

`g17p-drm-gpu-dependencies-control` is an output-only diagnostic variant. It
records missing overlap without aborting, while preserving every data/fence/
timestamp assertion. Its passes do not qualify strict asynchronous scheduling.


G16C native hardware probing (2026-10-22): the real shared Rust driver has
booted firmware on J616c, accepted the complete source-built initdata graph,
and retired opening control producers 1 and 7. Linux reports all 16 CPUs
online. This qualifies firmware startup, not native compute/render output or
DRM registration. Those remain the next hardware milestones.

The new t6041-j616c.dts describes the minimal CPU/AIC/UART/GPU resources.
The GPU is disabled by default until permanent PMGR ownership is installed;
the probe tool explicitly powers SGX/ASC and enables the test node. The
M1N1_MONITOR build option selects PSCI for the test monitor; the normal board
uses spin-table startup. Apply tests/g16c-probe.config to the known-good Neo
kernel configuration. Native Max must not inherit its old NR_CPUS=10 limit.
Use idle=yield during this bring-up to avoid the observed M4 WFI state loss.

The clean-room bootloader checkout's experiments/g16c_linux_probe.py loads
the actual ARM64 Image and loader-populated FDT. m1n1 supplies PSCI CPU_ON and
its existing VUART; GPU MMIO, mailbox interrupts, firmware and RAM pass through
to hardware. VUART JPWWJRX2N03 is bridged to the terminal, with all proxy and
chainload control explicitly on JPWWJRX2N01. The launcher records dmesg,
populated.dtb, input hashes and the exact command. It reads symbols only from
our own matching m1n1-raw.elf and disables automatic crash disassembly.

The first hardware probe exposed a real startup-order bug absent from the
original host fixture: firmware links its reserved shared L2 after the cold
inventory. Refresh that inventory after RTKit boot, before graph mapping.
Walk only declared page-table DATA reservations and preserve pre-management
snapshots for cleanup. The short VM gate now reproduces that late link,
checks repeated refresh and rollback, and rejects newly linked out-of-range
or cyclic tables. Host equality remains supporting evidence; each subsequent
feature must also be tested through this native driver on the Max.

Native G16C active parameter-buffer growth
----------------------------------------

`check_scheduled_parameter_growth.py BOOTLOADER --evidence NEW_DIRECTORY`
compares 128 complete replies with the source-qualified shim, compiles every
wire size/alignment/field-offset assertion, and rejects malformed requests and
foreign PB metadata. Active free-list reordering and holes remain permitted.
This is a CPU/layout check, not evidence of GPU execution.

`prepare_g16c_parameter_growth.py BOOTLOADER SOURCE OUTPUT` exports the qualified
`shared-indirect-partial-load-mixed-count{2,200000}-source-v1-20261018` workload.
The commands and all shader/launch bytes remain exact; only a verified zero
shader-aperture tail is omitted and page-equivalent bindings are coalesced.
Complete argument, color and counting-query expectations come from CPU source
inputs, independently checked against the previous source fixture. Known draw
argument results are never uploaded.

Build `g16c_drm_parameter_growth.c` with the same strict flags/UAPI headers as
`g16c_drm_mixed.c`. Run the two-instance control before the 200000-instance
pressure case. Each invocation uses two batches in one VM, compares all three
complete output BOs (294884 guard bytes), six ordered timestamps per batch and
prior timestamp-bank poison. Actual outputs/timestamps are saved under
`/pressure-INSTANCES-epochN-*` for independent readback. `--file-close` tests
implicit queue/VM teardown; `--origin=0x7100004000` relocates driver-private
storage. Verify grown-pool reuse across fresh VMs using these options, and
require actual owned-growth receipts in dmesg on the first pressure draw.

The native transaction validates pending render/context/slot/token/TTBR and PB
metadata before ACK. It then retires the source's six maintenance controls,
adds fresh guarded backing, preserves manager/counter/free-list/RegionC bytes,
and publishes only appended block entries and two counts before a typed reply.
Control retirement and applied manager counts are separate conditions: require
both with bounded waits. A following growth request remains unacknowledged
until the current transaction finishes. The original render deadline is not
reset. Grown IDs/backing survive dormant-pool transfer; retired roots do not.

This case qualifies reactive growth and the one-pixel indirect partial-render
path, not all partial-render formats, maximum-capacity/physical-OOM recovery,
overlapping firmware flights, or Neo hardware behavior. Faults quarantine
published backing. Capture new unknown behavior using the single-user Metal /
full-UAT / cold-replay / source-bisection procedure. Before executing any guest
client, verify the shell, all 16 CPUs, actual executable and fixture hashes;
a failed verifier must stop dependent actions.

Native G16C parameter-memory-limit retirement
-------------------------------------------

`check_scheduled_parameter_error.py BOOTLOADER --evidence NEW_DIRECTORY`
compares128 complete type9 replies with the qualified source shim, compiles
all wire size/alignment/field-offset assertions, and rejects5632 foreign
report bytes,4096 foreign descriptor bytes,256 incorrect descriptor sizes and
five invalid reply identities. The runtime also ties every checked descriptor
identity offset to the actual macro-generated FragmentDescriptorG16C. Unknown
report words remain opaque; no report callback/cookie pointer is dereferenced.

Build `g16c_drm_parameter_oom.c` with the usual strict flags and UAPI headers.
Before GPU execution, run its REAL reader in `--preflight` mode against the
pressure/control fixtures; this supports a native host build without a DRM
node. Hardware-only barrier checks must not apply to synthetic attachment
commands, which require NONE. Query a fresh BINARY output fence for each
submission's exact errno; a timeline wrapper is not an individual error fence.

Cold-boot the test kernel with the explicit module parameter
`asahi_neo.g16c_parameter_oom_test=0` (the loader accepts
`--bootarg asahi_neo.g16c_parameter_oom_test=0`). Default0xffffffff disables
this diagnostic. Run this client before any other render so ordinal0 uses
the source-qualified one-shot TA892=0/FG2100=1 policy. All ordinary render
parameters retain their defaults. This causes a REAL firmware OOM7; it does
not synthesize an event or write completion stamps from the CPU.

```
/g16c_drm_parameter_oom /fixture-pressure200000 /fixture-pressure2 --preflight
/g16c_drm_parameter_oom /fixture-pressure200000 /fixture-pressure2
```

Three phases share one live VM/queue. First: the compute producer completes,
the pressure render retires via owned OOM7/error9, and the output fence reports
-ENOMEM. Second: barriers0 intentionally inherit that failure; both commands
reject before GPU publication and every output/timestamp seed stays unchanged.
Third: VDM barriers NONE explicitly disconnect the old failed render; changing
only the known uniform input to2 instances gives complete CPU argument/color/
query matches and six timestamps on the SAME queue. The compute dependency
within the batch remains intact. Do not claim transparent recovery with the
old failed dependencies still requested.

Every phase checks294884 output guard bytes, timestamp-bank guards and prior
bank poison. Failed-render color/query mutable prefixes and failed-render
user timestamp words are unspecified; the compute result is checked fully.
The successful retry checks complete output BOs. All twelve actual output/
timestamp banks are saved as /oom-epochN-* for independent readback. Optional
--file-close skips explicit queue/VM destruction; the qualified initial case
uses explicit close, with later regular pressure runs covering file close.

Native ownership is validated BEFORE RX ACK against the pending render,
context/slot/event/stamp/queue/manager/descriptor, installed roots and live
source descriptor fields. Only a type9 reply is published; no GPU backing or
parameter metadata is changed by this handler. Reply retirement is not render
completion: the normal stamps, event counter and queue/channel cursors must
also retire. CPU cache completion preserves the command's ENOMEM while making
its bounded writes visible; a cache failure remains a fatal device error.
Unowned/duplicate reports and uncertain replies retain the fail-closed path.

After this test, require ordinary pressure growth/reuse and memory/unbind
checks. Also cold-boot WITHOUT the diagnostic parameter and run short native
regressions. This qualifies owned OOM7 retirement, explicit dependency
attribution and independent retry; host allocation failure during growth,
maximum capacity/refusal handling, other faults, uncertain publication and
actual overlapping firmware work still require separate evidence.
