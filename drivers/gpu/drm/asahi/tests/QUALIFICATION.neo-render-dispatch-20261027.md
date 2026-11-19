<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Neo render dispatch correction, 2026-10-27

The Neo failures in Vulkan instancing, WebGPU Life and the Three.js scenes came
from an inherited render-parser heuristic. The parser selected auxiliary
framebuffer flag bit 0 (few-primitives tile dispatch) when the fragment attachment
count was zero or one. Attachments describe residency; they cannot bound the
number of primitives across draws and instances. This backend does not advertise
or accept the downstream few-primitives extension. All accepted renders now use
general dispatch, preserving the independent integer-depth-bias flag at bit 18.
The correction applies to both G17P and G16C through the existing shared parser.
Mesa and the scheduler are unchanged.

## Causal hardware evidence

The reference was a 128-instance, 1x1 Metal draw under single-user macOS on Neo,
with all six CPUs enabled. Its full UAT capture replayed successfully in plain
m1n1. Source Rust descriptors and the application's own Mesa buffers also passed
on that graph. The final controlled pair changes only the few-primitives bit:

| Replay | Auxiliary flags | Result |
| --- | --- | --- |
| `opaque-replay-v17-mesa-n128-deflake` | `0xc000` | Exact output and padding pass |
| `opaque-replay-v18-mesa-n128-auxflag` | `0xc001` | Zero output; firmware type-4 timeout |

The tiling descriptors are identical. Only fragment descriptor bytes at offsets
`0x1a0` and `0x1f40`, the two copies of this flag, differ. Buffers, pointers and
the opening control prefix are unchanged. No Apple executable was disassembled.
The legacy Neo reference also contains the bad heuristic; this is not a Max
firmware-topology difference.

Artifacts are in the bootloader checkout under
`build/neo-heavy-draw-20261027/`. The persistent index is
`references/neo-render-dispatch-evidence-20261027.json`.

## Parser regression

Run from the bootloader checkout, substituting the Linux checkout path:

```sh
.venv/bin/python /Users/user/gravity/linux-g16c-g17p/drivers/gpu/drm/asahi/tests/check_g17p_render_dispatch.py \
  . --evidence build/neo-heavy-draw-20261027/dispatch-parser-regression-v2 \
  --baseline b4ac8c3a39435495a49d0901fb901100d32c2f3f
```

The test executes the actual production parser and versions macro, checks the
240-byte UAPI payload and generated field offsets, and covers 16 attachment/flag
combinations plus three rejected flags. The same test fails on the old parser.
The shared-session shape test retains its strict legacy comparison with an exact,
documented projection for this correction.

## Normal Linux qualification

The normal Linux 7.3 image has SHA-256
`d0e5ad48470520d0013960edaf918511a3d2afbb07bc02ba90818e0ad78ec572`.
Its 849 tracked source inputs were verified against the baseline plus this
single-file correction at build time. All 688 non-test inputs still match the
committed source; subsequent edits affect host tests and documentation only.
There are no capture hooks or preloads in this kernel.
The production Mesa Vulkan DSO remains unchanged at `50cfaf889f`.

Neo first passed 128 and 256 one-pixel instances, 4096 instances at 37x19, and
256 indexed instances with D32 depth. Each case checks three complete readbacks
and guard bytes. Receipt:
`build/dual-gpu-20261026/neo-vulkan-instances-v105-aux-dispatch-linux73.json`.

Both devices then passed the identical real-workload matrix:

- Chromium hardware rendering: 4096 checked pixels.
- WebGPU Life: 32 generations, 16384 cell checks and 32768 pixel samples.
- Three.js: 4096 textured instances and 64 PBR objects with shadows. Each scene
  checks 27 complete 256x256 frames, 25 distinct frames and the restored hash.
- Subsequent native Vulkan workload, clean driver unbind, exact-device cold
  chainload and identity verification.

Receipts are `build/dual-gpu-20261026/{neo,max}-real-workloads-v106-linux73-aux-dispatch-all.json`.
Each verifies 246 actual runtime hashes and all CPUs (Neo six, Max sixteen).
Neo uses the 24 separately reserved source RAM ranges required by Linux 7.3.

There are no failing GPU workloads in this matrix. Max needed one cold retry
for the already known all-CPU startup stall before userspace or GPU execution;
that platform issue remains unresolved. These workloads do not establish full
Vulkan conformance.
