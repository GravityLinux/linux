# Native object/mesh experiment on T8132

`asahi_m4.native_mesh=1` is an explicitly paired kernel/userspace experiment.
The default is **0 and must remain 0**. No capability bit or execution UAPI
is provided. The paired stream starts with complete native object/mesh state.
The decoder runs only with the option enabled; ordinary streams are identified
by their first packet, without searching their contents for native state.

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

All VMs initialize native support mode before first publication when opted in.
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
