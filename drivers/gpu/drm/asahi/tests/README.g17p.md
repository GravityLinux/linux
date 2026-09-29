# G17P synchronous-port checks

These host tools compare the Rust source used by the kernel with the current
m1n1 Python shim. They do not connect to hardware or load captured memory.
From the Linux checkout, with Python 3 and `rustc` on `PATH`:

```sh
python3 drivers/gpu/drm/asahi/tests/import_g17p_constants.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/import_g17p_layout.py /path/to/m1n1 --check
python3 drivers/gpu/drm/asahi/tests/check_g17p_abi.py /path/to/m1n1
python3 drivers/gpu/drm/asahi/tests/check_g17p_initgraph.py /path/to/m1n1
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
