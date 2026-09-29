# G17P synchronous-port checks

These host tools compare the Rust source used by the kernel with the current
m1n1 Python shim. They do not connect to hardware or load captured memory.
From the Linux checkout, with Python 3 and `rustc` on `PATH`:

```sh
python3 drivers/gpu/drm/asahi/tests/import_g17p_constants.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/import_g17p_layout.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/import_g17p_topology.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/check_g17p_abi.py /path/to/m1n1
python3 drivers/gpu/drm/asahi/tests/check_g17p_initgraph.py /path/to/m1n1
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

The runtime currently admits 32 total synchronous compute commands with unchanged
bindings in the first submitting VM. The source-profile test covers later wrap
metadata too; it does not claim that runtime wrap/handoff is implemented. The
native `g17p-drm-compute` test changes its input arrays and poisons its output on
each submission, checks all 2048 floats and every tail guard, then verifies that
admission exhaustion returns EOPNOTSUPP without touching the output. Changed VMs
or bindings are explicitly rejected until VM handoff is connected.

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
# On a separate fresh boot (the retained transport admits 32 commands total):
g17p-drm-batch
```

`--sync` requires CONFIG_SW_SYNC and debugfs solely to create imported external
dependency fences. It checks malformed/missing/failed binary and timeline
inputs, timeout/interruption, same-file progress while waiting, output fence
waits/exports/query and preservation after rejection. Compute remains on the
real GPU. Without `--sync`, the compute test needs no software timeline device.

`--timestamps` checks ordered start/end pairs in the middle page of a caller
GEM BO, offset/alignment validation, live object unbind/rebind, all earlier
pairs and every surrounding guard byte. Timestamp aliases retain backing
until firmware shutdown; address recycling is pending, and the retained
64 MiB aperture returns ENOSPC on exhaustion.

The batch test uses four submissions of eight independently owned graphs,
checks all 2048 expected floats, each output guard, timestamp pairs and one
completion-fence pair per batch. Invalid last-command flags/barriers and a
batch exceeding remaining capacity must leave every output/fence unchanged.
It includes the sync dependency checks and timestamp object rebind.
The host VM harness also checks exact timestamp alias backing/attributes,
collision rejection and aperture/alignment boundaries.

These native tests have passed on T8140, including retained-memory audits
after file close. Render, runtime slot reuse/VM handoff and asynchronous
frontend/backend behavior remain outside this implementation stage.
