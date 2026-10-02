

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
