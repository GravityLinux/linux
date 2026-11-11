<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Niklas M4 integration and Max/Neo regression checks

The M4 driver now incorporates `niklassheth/linux` branches `bits/210-gpu`
at `58670e17a3fa2c202d90c8ab8b7ae2af17c10f1e` and
`bits/219-g16-native-mesh` at `1c33687689f5c0f595f4f9861cdbd4c527fc985c`.
Both remote tips were reverified after integration.

The import includes VM and timestamp lifetime fixes, per-VM draining, mapping
snapshots, bounded admission, asynchronous pump improvements, firmware diagnostics
and debugfs, explicit few-primitives support, and removal of retired experiments.
The four mesh commits retain their upstream authorship. Mesh execution remains
an opt-in experiment with `asahi_m4.native_mesh=0` by default.

Of 25 imported driver, UAPI and selftest files, 23 match the upstream tip exactly.
Only `g16.rs` and `g16_drm.rs` need frontend adapters for this tree's existing
kernel contracts:

- RTKit buffers use the bounded `Region` memory view.
- DRM files open with registered-device identity and retain their raw-file
  callbacks.
- Imported GEM objects use `create_imported`, initializing the same shared
  owner/VM state as upstream's `import_args`.

The modern shmem helper retains this tree's full Rust allocation callback and
borrows attachment-owned imported scatterlists without mapping them twice.
The `system_dfl` workqueue interface and `Owned<T>: Unpin` implementation are
additive. The existing legacy shmem helper remains separate. G16's explicit
few-primitives capability is not advertised or accepted by G17P/G16C.
No Max/Neo runtime source, scheduler or Mesa source changed in this integration.

## Host and build validation

- All 11 upstream native-mesh tests pass, including complete ordinary-render
  byte comparison against the actual pre-mesh parent in this checkout.
- Scene tests pass 80 unique offsets, register/cursor agreement, exhaustion and
  reuse at two placements and three cycles.
- The production render-parser regression passes 16 combinations and three
  unsupported-flag checks; the previous parser fails the same test.
- The shared-session comparison preserves 35 tagged Neo defaults plus the
  documented probe, scheduler, identity and admission checks.
- A complete Linux 7.3 Image build succeeds with both `DRM_ASAHI_M4=y` and
  `DRM_ASAHI_NEO=y`. All 860 overlay inputs were hash-verified.

Qualified production source: `5a46593840656eb245f25698e33847ddbbf51220`.
Image SHA-256:
`ae767bcb6dd3a15b4771d043e0d6fcb9d188a56126e9a6f429f713bb11327625`.
The image has no temporary rebind diagnostics. Mesa remains at `50cfaf889f`.

## Hardware results and existing intermittent failure

Both targets pass the full normal-image matrix: Chromium with 4096 checked
hardware pixels; WebGPU Life with 32 generations, 16384 checked cells and 32768
pixel samples; Three.js with 4096 instances and 64 PBR objects/shadows; then a
native Vulkan workload. Each Three.js scene checks 27 full 256x256 frames,
25 distinct frames and the restored image hash. Each target verifies 246 runtime
file hashes, all CPUs, clean driver unbind and exact-device cold recovery.

Receipts, relative to the bootloader checkout:

- `build/dual-gpu-20261026/max-real-workloads-v107-linux73-niklas-m4-all.json`
- `build/dual-gpu-20261026/neo-real-workloads-v110-linux73-niklas-m4-all.json`

The first integrated Neo run, v107, failed Three.js instancing with an occupied
caller PTE at `0x1008048000`; the asynchronous submission returned `EBUSY`.
The previous qualified kernel, without this import, reproduces the same failure
at the same address in v108. A diagnostic-only integrated run passes in v109;
the normal integrated image then passes in v110. Failed runs and diagnostic
artifacts are retained. This intermittent Neo rebinding issue was unresolved at
this qualification; the passing repeat did not fix it. Commit `44609dc20279`
subsequently corrects the retained tiler reservation admission error; see
[the causal evidence and fixed-kernel qualification](QUALIFICATION.neo-growth-admission-20261028.md).
No regression from
the M4 import was identified in this matrix, which is not Vulkan conformance.

Detailed provenance, image manifests, controls and logs are indexed by
`references/niklas-m4-integration-evidence-20261028.json` in the bootloader
checkout. M4/T8132 hardware validation is left to the operator's M4 tester.
