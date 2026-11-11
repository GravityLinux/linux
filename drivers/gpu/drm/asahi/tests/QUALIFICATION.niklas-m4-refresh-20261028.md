<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Niklas M4 refresh and Max/Neo qualification

Production source `2c51e2f09762e343e640dfd31718e9def42d82b7` imports:

- `niklassheth/linux`, `bits/210-gpu` at
  `f0c5e08072e98c33a1ea56010d55b6b422aefe03`.
- The eleven additional `bits/219-g16-native-mesh` commits through
  `e5b4c3520e04d415b33c92f8f007696ccb9ab36e`.

The main branch was rewritten upstream. The previous remote tip and the local
pre-import snapshot are retained. The mesh follow-ups were based on the older
monolithic driver; they are merged into the new runtime/frontend modules so
neither set of changes is lost. `bits/211-gpu-debug` at `0ec6cdc20e2d` is already
an ancestor of the refreshed main branch. All three remote tips were reverified.

The refresh includes deferred GEM-close unmapping, progress-based close drains,
client-scoped destroy/unbind waits, queue-failure propagation, preallocated
submission bookkeeping, TVB growth without reclaim, per-submission shared
notifiers and publication accounting, firmware-managed idle/power transitions,
the extra policy object and thermal sensor mapping, and diagnostics cleanup.
The split frontend and runtime preserve upstream's module boundaries.

Mesh additions cover the T32 profile, bounded multi-parent/research modes,
qualified native-index classes, runtime mode switching, VM-latched mode and the
read-only sysfs advertisement. Native mesh remains disabled by default.

The existing RTKit Region view, imported GEM initialization and registered DRM
file callbacks remain as narrow adapters to this tree's shared Rust interfaces.
The mesh selftest reads all split modules. Imported executable selftests retain
their upstream modes.

## Fragment-barrier compatibility

Upstream moves the downstream feature and render flag to bit 29. The M4 path
and public header use that allocation exactly. Neo/Max additionally accept
their shipped render flag at bit 3, and Neo advertises both feature bits, so
existing production Mesa binaries continue to work. The actual render parser
tests both flags and their combination. The shared-session preservation test
projects only this explicit alias before comparing scheduling and admission.
Few-primitives remains unadvertised and rejected on Neo/Max. Their firmware
lifecycle, scheduler and retained tiler reservation admission fix are unchanged.

## Validation

All fourteen imported M4 host suites pass. These include 26 native-mesh tests,
ordinary render byte equivalence against the real pre-mesh parent, 80 scene
offsets, deferred unmapping, drain progress, failed queues, fixed bookkeeping
bounds, publication statistics, TVB growth and downstream UAPI allocation.
The four existing shared host checks pass: render parsing, retained-growth
admission, root rebind guards and the 35-field Neo session comparison. The
parser checks 16 attachment/flag combinations, three barrier compatibility
cases and three rejected flags.

Linux 7.3 builds with both `DRM_ASAHI_M4=y` and `DRM_ASAHI_NEO=y`.
All 894 source overlay paths were hash-verified against the committed source.
The normal image has no temporary diagnostic modifications:

`6157a6761a188921443e8e2a5846c3e1f2d3aa52004d9f288eaa3d3b43f446dc`

Both Max and Neo pass the complete real-workload matrix with the unchanged
Mesa runtime at `50cfaf889f`: Chromium checks 4096 hardware-rendered pixels;
WebGPU Life checks 16384 cells and 32768 pixel samples over 32 generations;
Three.js checks 4096 instances and a 64-object PBR/shadow scene. Neo runs the
instancing scene three consecutive times before PBR. Each scene verifies 27
complete 256x256 frames, 25 distinct frames and the restored image hash.
Both pass subsequent native Vulkan, clean driver unbind and exact-device cold
recovery. Each verifies 246 runtime file hashes and all CPUs: six Neo and
sixteen Max, using spin-table startup.

Receipts relative to the bootloader checkout:

- `build/dual-gpu-20261026/neo-real-workloads-v116-linux73-niklas-refresh-all.json`
- `build/dual-gpu-20261026/max-real-workloads-v116-linux73-niklas-refresh-all.json`
- `references/niklas-m4-refresh-evidence-20261028.json`

M4/T8132 hardware validation remains with the operator's M4 tester. These are
output and lifecycle regression checks on Max and Neo, not Vulkan conformance.
