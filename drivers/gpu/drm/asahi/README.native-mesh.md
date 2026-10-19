# Native object/mesh experiment on T8132

`asahi_m4.native_mesh=1` is an explicitly paired kernel/userspace experiment.
The default is **0 and must remain 0**. No capability bit or execution UAPI
is provided. The paired stream starts with complete native object/mesh state.
The decoder runs only with the option enabled; ordinary streams are identified
by their first packet, without searching their contents for native state.

The admitted class has physical T16 or T32 mesh execution, one direct object workgroup,
four-vertex storage and one triangle primitive. It supports 4–128 per-vertex
32-bit scalar components, including XYZW. The two scalar counts and output-row
count must agree: N components, N+1 rows, with no reserved upper bits. This
corresponds to 32 + 16*N local bytes (at most 2080), rounded up by the paired
Mesa launch to a power-of-two allocation (at most 4096). Task payload,
per-primitive outputs, culling, multiple draws, indirect draws and wider
resource classes remain rejected. Single-sample, single-layer restrictions
also remain in the runtime.

Object dispatch total and local thread dimensions must match, and must be
either (1,1,1) for the existing scalar carrier, (16,1,1) for physical T16,
or (32,1,1) for physical T32.
The mesh execution width in the state packet does not set the object dispatch
width. Mismatched dimensions, other shapes and multiple workgroups are rejected.
This bounds the parent object dispatch, not a child mesh grid exported by its
shader. Physical T64/T128 state remains rejected; logical workgroups lowered
by the compiler to physical T32 do not require those wider state values.
T32 admission enables device qualification without changing payload sizes,
scene ownership or the default-off gate; it is not a hardware pass by itself.

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
