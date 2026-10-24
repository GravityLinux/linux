# Native object/mesh experiment on T8132

`asahi_m4.native_mesh=1` is an explicitly paired kernel/userspace experiment.
The default is **0 and must remain 0**. No capability bit or execution UAPI
is provided. The paired stream starts with complete native object/mesh state.
The decoder runs only with the option enabled; ordinary streams are identified
by their first packet, without searching their contents for native state.
In a nonzero mode, that first word is read before waiting for the render's
producers. An ordinary first word publishes the render exactly as at mode 0,
with no CPU wait. Only a native header, or an unreadable first word, waits for
the producers and then decodes the whole stream. That relies on a userspace
contract: native headers are written on the CPU before SUBMIT, and no GPU job
writes or rewrites a render's first encoder word. claude-compiler confirmed it
for every Honeykrisp emit path (`hk_apple9_mesh_command_build`, isolated at
`cs->start`; secondaries are replayed on the CPU; the GPU VDM writers emit only
ordinary words) in `tmp/claude-ct/native-header-peek-hk-audit-20261024.md`
(2026-10-24). A userspace that emits native streams any other way must use the
waiting path; the alternative is an explicit per-render flag.

The admitted class has physical T16 or T32 mesh execution, one direct object dispatch,
four-vertex storage and one triangle primitive. It supports 4–128 per-vertex
32-bit scalar components, including XYZW. The two scalar counts and output-row
count must agree: N components, N+1 rows, with no reserved upper bits. This
corresponds to 32 + 16*N local bytes (at most 2080), rounded up by the paired
Mesa launch to a power-of-two allocation (at most 4096). Task payload,
per-primitive outputs, culling, multiple draws, hardware indirect packets and wider
resource classes remain rejected. Single-sample, single-layer restrictions
also remain in the runtime.

Object dispatch local thread dimensions must be either (1,1,1) for the existing
scalar carrier, (16,1,1) for physical T16, or (32,1,1) for physical T32.
At `native_mesh=1`, total thread dimensions must equal the local dimensions.
Only `native_mesh=2` admits nonzero multiples of the local dimensions.
Thus (512,2,3) total / (16,1,1) local describes 32x2x3 parent workgroups.
The mesh execution width in the state packet does not set the object dispatch
width. Partial workgroups and other local shapes are rejected. This validates
the parent packet shape, not a child mesh grid exported by its shader, nor an
aggregate resource or workgroup-count limit. Physical T64/T128 state remains
rejected; logical workgroups lowered
by the compiler to physical T32 do not require those wider state values.
T32 and multiple-parent admission enable device qualification without changing
payload sizes, scene ownership or the default-off gate; neither is a hardware
pass by itself. Multiple-parent use needs separate ordering and lifetime tests.

A taskless API draw with N mesh workgroups can keep one object workgroup and
read its child XYZ grid from a userspace GPU buffer. A future indirect API
implementation can use that same packet with a GPU-produced grid. Neither
requires a hardware indirect object packet or CPU reads of the grid/count
buffer. Do not multiply parent dimensions by a taskless child grid. Indirect
API semantics, producer synchronization, count/stride handling and zero-child
completion still require userspace implementation and device qualification.
The current hk native API gate excludes these broader API shapes independently
of kernel admission. Hardware indirect/link packets and multiple draws in one
stream remain rejected until their native packet contract is established.

USC frame extents (for example mesh 64/64 versus Metal 16/16) are encoded by
userspace in the launch record. The kernel passes the launch handles through;
it does not read, size or validate those extents, or the local-allocation field.

This is bounded resource-class admission, not validation of arbitrary shader
programs. Launches and their local-allocation instructions remain userspace
programs; the kernel does not assume a compiler-specific instruction sequence
or fixed register assignment. The paired compiler must generate matching
allocation, output and interpolation state. Attribute admission alone is not
hardware qualification of attributes or the maximum component count.

A VM whose first render runs with the option enabled initializes native support
mode before its first publication.
The initialized 24-buffer operand pool serves both ordinary and native Work;
there is no additional native buffer-admission request. Native payload storage
is cleared, exclusive per render, and retained through completion. CPU producer
waits still reduce ordinary submission pipelining. Keep this opt-in experiment
disabled for normal use.

Position-only H1, cold initialization, slot rebind, sequential draws through
scene 7 and concurrent separate VMs passed the #82 device campaign. General
payload sizing, attribute rendering, scene wraparound and simultaneous native
renders on multiple queues in one VM require their own device qualification.

### Admission modes

`asahi_m4.native_mesh=0` remains the default and bypasses native parsing.
`=1` retains the #85 one-parent class exactly: total XYZ equals local XYZ,
with scalar, T16 or T32 object workgroups and T16/T32 mesh state.
`=2` is the separate, unqualified multi-parent research mode; it admits
nonzero total XYZ divisible by the supported local XYZ. Other values reject
driver initialization. Both enabled modes initialize native support state.
Multi-parent device use requires a guarded measurement of the 16 KiB native
firmware payload's touched range versus parent count first. Host parser tests
are not device qualification.

`=3` is research admission for the native capability study
(`native-capability-20261022`). It is not qualified and is not for normal use.
It keeps mode 2's stream structure and object dispatch rules, and replaces
the fixed one-triangle state check with that study's state model. With n
vertex planes, P primitives, V vertices and C primitive components:

- n: 4–128, with word 1 = n | (n + 1) << 8.
- P: 1–512, with word 10 = (P - 1) | (ceil(P / 16) - 1) << 16. The culling bit
  stays rejected.
- V: 1–256, with word 13 = 1 + ceil(V / 4) × n.
- C: 0–128, with word 12 = P × C. Word 2 is 0xd0 exactly when C > 0. Primitive
  data adds one link plane, so n ≥ 5 then.
- Application output (vertex planes without the link plane, plus 4 × P × C
  bytes) is at most 32 KiB, the compiler's stated mesh-output limit.
- Draw headers: 0x70000000 points, 0x70000100 lines, 0x70000600 triangles.

Words 3, 4, 9 and 11 keep their mode 1/2 values. The checks use only the
13 state words already read, so command-memory reads are unchanged. The
object payload size is not a packet field. Its exclusive storage stays 16 KiB,
which is the study's largest payload. The firmware TA size and gate are the
fixed mesh values from modes 1 and 2. Whether they hold for wider shapes is a
device question, so admission here is not GPU qualification.

## Qualified classes (`native_mesh=4`)

Mode 4 admits exactly the 31 device-qualified native-index classes of Mesa's
`hk_apple9_native_index_shapes.def` and nothing else from mode 3's research
model. Those are position-only, taskless triangle mesh classes at physical T16
or T32, with the single-parent object dispatch they were qualified with:
(16, 1, 1), as Honeykrisp emits it, or the scalar (1, 1, 1) carrier of the
qualification fixtures.
- **The state check:** four position planes (`0x504`), no primitive data
  (`0x10`), no primitive components, one triangle draw (`0x70000600`), and total
  = local = (16, 1, 1) or (1, 1, 1).
- **The layout:** one of the 29 `(mesh width, primitive word, vertex rows)`
  layouts in `g16_mesh::QUALIFIED`. The kernel sees vertices only as ceil(V / 4)
  quads, so V3 and V4 share a layout at P1 and P2, and the exact class is
  Mesa's.
- **Excluded:** T16V16P16, physical W64, point and line draws, primitive data
  and multi-parent dispatch.

It is the only mode intended for a production default. That needs device
qualification at mode 4, not mode 3.

## Switching at runtime

The boot value only sets the initial state. As root, with debugfs mounted:

```sh
cat /sys/kernel/debug/asahi_m4/native_mesh       # current mode
echo 1 > /sys/kernel/debug/asahi_m4/native_mesh  # one-parent mode (2: multi-parent)
echo 0 > /sys/kernel/debug/asahi_m4/native_mesh  # back to the default
```

Values above the highest supported mode return EINVAL. Each write starts a new
generation, even when it writes the current value, and is logged as
`G16: native_mesh=N (generation G)`. Flipping is safe at any time, not only
when idle:

- **A VM latches the mode when it is created** (VM_CREATE). Its support header
  (native for modes 1–4) is built from that latch at its first render, and every
  one of its renders is decoded with it. A runtime change affects only VMs
  created afterwards.
- **A VM created at mode 0** (KWin, for example) never decodes native streams.
  Userspace must not submit any there: they would run as ordinary Work.
- **A VM created in a native mode** keeps decoding with that mode after a switch
  to 0, so its native streams never run with ordinary state.

So start a fresh process (a new VM) after changing the mode. The module
parameter is read-only in sysfs; use the debugfs file.
