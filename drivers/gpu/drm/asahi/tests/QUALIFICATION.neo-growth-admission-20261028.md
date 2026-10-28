<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Neo retained tiler reservation admission

Commit `44609dc2027921be4d5dc85465ec711db56e22e1` corrects the intermittent
Neo caller-rebind `EBUSY` seen during Chromium/Three.js workloads. A retired
pool still owns its tiler backing and the virtual range reserved for later
growth. A caller's encoder buffer can grow into that range after the pool
was created. Previously, selection considered retirement and caller identity,
then the root planner correctly rejected the conflicting binding.

Selection now also checks whether the caller's bindings overlap the retained
growth reservation. It chooses another compatible pool through the existing
selector. The check covers the entire reservation, including unallocated space
needed by future firmware growth requests, and uses the same address projection
as render rebinding. A new pool remains eligible because its reservation is
chosen against the incoming root. Ordinary busy owners retain the existing
deferred-admission behavior.

No PTE collision guard, backing ownership or in-flight lifetime was weakened.
No tiler IDs or firmware objects are reset, and no CPU wait replaces GPU work.
The previous scheduler, firmware ABI, object layouts and Mesa remain in use.

## Exact causal evidence

The v114 diagnostic kernel reproduces the failure after three successful
instancing scenes, during the PBR scene. Its allocation and rejection records
identify the same retained object:

| Field | Value |
| --- | --- |
| Pool / ASID | 1 / 55 |
| Tiler allocation VA | `0x1008048000` |
| Tiler allocation physical address | `0x1009ba00000` |
| Rejected old PTE | `0xc001009ba00c8b` |
| PTE physical address mask | `0x000003ffffffc000` |
| Rebind call site | `PoolRebindSeed::prepare`, `g17p_render_context_session.rs:829` |

Masking the PTE produces exactly the logged tiler allocation physical address.
The root planner was protecting actual private backing. This establishes a
pool eligibility error, rather than missing GEM ownership or corrupt tables.
Earlier runs reproduced the same address collision on both the integrated
M4 tree and the preceding qualified kernel. Failed runs remain retained.

## Regression and hardware qualification

`check_g17p_growth_admission.py` compiles the actual production selector,
growth compatibility method and reservation check. It tests the measured
expanded-encoder overlap, alternate compatible owners, temporary exhaustion,
range adjacency, future growth space, relative bindings, bounds and ordinary
matching-owner preference. The same expanded-encoder test fails on revision
`0fe205373dc0fcab97e13d1adee349bbe29a6ca7`.

```sh
python3 check_g17p_growth_admission.py --evidence /tmp/neo-growth-current
python3 check_g17p_growth_admission.py --revision 0fe205373dc0 \
  --expect-failure --evidence /tmp/neo-growth-before
```

The existing actual root-planner test also passes: 259 allocation refusals
preserve live tables and retry successfully, 41 replacement/removal/addition
plans succeed, and 15 private-relocation cases preserve ownership and guards.
The shared-session preservation checks pass as well.

The normal Linux 7.3 kernel, without temporary diagnostics, passes Chromium,
WebGPU Life, Three.js instancing/PBR, subsequent native Vulkan and clean driver
unbind on both Neo and M4 Max. Neo runs the instancing scene three consecutive
times before PBR. Each scene verifies 27 complete 256x256 images, 25 distinct
frames and the restored hash. Chromium checks 4096 hardware pixels; Life checks
16384 cells and 32768 pixel samples over 32 generations. Each target verifies
246 runtime file hashes and all CPUs: six on Neo and sixteen on Max. Both are
cold-recovered with exact serial, model and chip identity verification.

Image SHA-256:
`75cf9261cbbf878375a78c75f9483657faf0a3505d7ccb51e0546cb03b94a652`.
Its source manifest verifies 860 inputs against the committed fix. Mesa remains
unchanged at `50cfaf889f`; the M4/T8132 object hash is also unchanged.

Evidence paths are relative to the bootloader checkout:

- `references/neo-growth-admission-evidence-20261028.json`
- `build/dual-gpu-20261026/neo-ebusy-20261028/causal-proof-v1.json`
- `build/dual-gpu-20261026/neo-real-workloads-v115-linux73-growth-admission-all.json`
- `build/dual-gpu-20261026/max-real-workloads-v115-linux73-growth-admission-all.json`

The deterministic admission regression and exact backing identity establish
the correction; the hardware campaign additionally verifies real outputs and
lifecycle behavior. It is not a full Vulkan conformance run.
