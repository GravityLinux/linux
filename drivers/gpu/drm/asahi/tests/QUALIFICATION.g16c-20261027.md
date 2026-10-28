<!-- SPDX-License-Identifier: GPL-2.0-only -->
# M4 Max port qualification, 2026-10-27

The outstanding parallel-render and client-recycling failures are resolved.
The qualified driver revision is `56548d275`, Image275
(`Image-neo-lifecycle-v85`). Both existing parallel settings are enabled in all
final hardware runs, with compression enabled and all16 Max CPUs online.
The existing common Neo scheduler, leases and preparation machinery are used.
Max submission-entry/context encodings and its single-ASC boot path retain
their capture-backed differences from Neo.

Two functional corrections close the outstanding failures:

- `74e3dde82`: bind FList registration and operand growth to the owner's
  reserved dispatch bank. Native request +0x30 selects the bank; RX13 +8 and
  support +8 identify the same bank. Previously every render owner registered
  bank0 and replaced that bank's directory, aliasing independent owners.
- `56548d275`: validate both the logical caller and the selected physical
  render client before publication. Off-lock preparation already checks both.
  Checking only the logical queue allowed a changed eligible pool to bypass
  client preparation and retire its old queue without an incoming replacement.
  The correction uses the existing replacement/handoff/retry path.

Temporary failure-phase diagnostics and experimental pool caps are absent
from the qualified source. No firmware dependency is replaced by a CPU wait.

## Reference and source evidence

The minimal three-client Metal reference ran under single-user macOS, with
all16 CPUs, and captured full UAT images at three sequential warm submissions
and the combined retained repeat. Plain m1n1 opaque replay, without a macOS
guest, verifies six changed outputs,3000 guard bytes and18 stage retirements.
The original failed host-validator/replay attempts remain preserved alongside
the corrected ownership-bounded validation and passing replay.

`check_scheduled_flist_banks.py` compiles production constructors through the
actual versions macro and compares six complete native FList requests/replies
and six dispatch records. Old source rejects native bank3. Wrong-bank and
duplicate replies are rejected by the corrected source. The resource gate
also passes288 complete parameter/support objects and64 registration cases.

`check_scheduled_render_replacement.py` compiles the actual publication branch
and replacement helper. Six deterministic selection/identity interleavings
pass; the old publication code fails the stale-pool case. A selected-only
trial fails the opposite logical-routing case and was never run on hardware.

The existing source-projection check passes35 tagged Neo Session defaults,
probe, CPU scheduler, identity/GET_PARAMS and submit-admission comparisons.
Current-turn hardware access was restricted to RID0; no new Neo run is claimed.

## Final hardware results

Evidence paths below are relative to the bootloader checkout's
`build/dual-gpu-20261026/` directory.

| Receipt | Verified behavior |
| --- | --- |
| `max-vulkan-retained-normal-v55-final-queue-and-client-recycling.json` | Default16 draws; nine retained1280x720 images;257 retained40x20 images;17 fresh Vulkan clients, checking every output pixel. Crosses scratch8, transport128 and descriptor256 boundaries. |
| `max-browser-shared-v56-final-owned-banks-and-clients.json` | Chromium WebGL2:4096 hardware pixels. Conditional WebGPU Game of Life:32 generations,16384 cell checks and32768 rendered samples. Post-browser Vulkan also passes. |
| `max-vulkan-retained-normal-v57-concurrent-retained-clients.json` | Two independent Vulkan clients launched concurrently, first two and then nine retained generations each, at1280x720 and40x20. Every output pixel and both process results pass. |

All three receipts require clean driver unbind and exact RID0 reboot/raw
chainload/identity verification. Runtime verification covers66 actual files
for Vulkan controls and240 for Chromium/WebGPU. The concurrency test checks
functional progress and outputs; it does not measure GPU interval overlap or
throughput. The Vulkan implementation remains the operator-selected WIP Mesa
driver; these workloads are not a full Vulkan conformance run.

The final image manifest verifies183 exact source inputs and the Image/object
hashes. The sparse checkout's UAPI header is read from its tracked Git object,
as in the build. The aggregate receipt is:

`build/m4max-gpu/neo-lifecycle-restoration-20261025/final-image275-qualification-v1.json`

It hashes the three final hardware receipts, native replay and host controls.
The complete tested source archive is retained beside the Image and manifest.
No final GPU monitor/controller remains active; Max is cold-recovered to proxy.

## Separate platform limitations

The monitor explicitly uses Linux's `id_aa64isar2.wfxt=0` override. The observed
WFIT register-loss cause remains unresolved and occurred before graphics as
well as during an earlier workload. The final browser controller also retried
one confirmed pre-GPU spin-table startup stall, preserving all16 CPUs. These
monitor/startup issues remain recorded separately from GPU port qualification.
No Apple executable instructions were inspected or decoded for this work.
