#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Vary two own-Mesa loop uniforms for the native cross-VM handoff test.

The input is an output-checked compute-overlap fixture from our offline Mesa
exporter. Shader, launcher, bindings and command bytes remain unchanged.
Version3 tells g16c_drm_compute_cross_vm to verify each loop against its own
uniform and require actual GPU overlap during --drain-arrival.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--incoming-iterations", type=int, default=2_000_000)
    parser.add_argument("--old-iterations", type=int, default=1_000_000)
    args = parser.parse_args()
    steps = (args.incoming_iterations, args.old_iterations)
    if not all(0 < count <= 16_000_000 for count in steps):
        parser.error("iteration counts must be between1 and16000000")
    if args.output.exists():
        parser.error("output directory already exists")

    provenance = json.loads((args.source / "provenance.json").read_text())
    assert provenance["own_mesa_resources"] and provenance["apple_binary_pages"] == 0
    images = {}
    for path in args.source.iterdir():
        if path.is_file() and path.name != "provenance.json":
            data = path.read_bytes()
            assert sha(data) == provenance["files"][path.name], path
            images[path.name] = data
    original = images.copy()
    header = struct.unpack_from("<8I", images["inputs.bin"])
    assert header[0] == 0x384f4347 and header[1] in (1, 2)
    assert header[4:7] == (144, 10, 100)
    inputs = bytearray(images["inputs.bin"])
    struct.pack_into("<I", inputs, 4, 3)
    images["inputs.bin"] = bytes(inputs)

    # Independent CPU arithmetic; no instruction decoder or GPU runs here.
    rust = """use std::io::Write;
fn main() {
    let mut result = Vec::new();
    for steps in [%du32, %du32] {
        for lane in 0u32..16 {
            let mut value = 0x5a110000 ^ lane;
            for i in 0..steps {
                value = value.rotate_left(7).wrapping_add(0x9e3779b9).wrapping_add(i);
            }
            result.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::io::stdout().write_all(&result).unwrap();
}
""" % steps
    with tempfile.TemporaryDirectory(prefix="g16c-handoff-oracle-") as temporary:
        temporary = Path(temporary)
        source = temporary / "oracle.rs"
        binary = temporary / "oracle"
        source.write_text(rust)
        subprocess.run(["rustc", "--edition=2021", "-O", str(source), "-o", str(binary)], check=True)
        expected = subprocess.check_output([str(binary)])
    assert len(expected) == 128
    for job, (uniform, output, iterations) in enumerate(zip((11, 101), (10, 100), steps)):
        name = f"gem-{uniform}.bin"
        uniform_bytes = bytearray(images[name])
        assert len(uniform_bytes) >= 4
        struct.pack_into("<I", uniform_bytes, 0, iterations)
        images[name] = bytes(uniform_bytes)
        data = bytearray(images[f"gem-{output}.bin"])
        assert len(data) == 16384 and data[:128] == bytes([0xa5]) * 128
        data[:64] = expected[job * 64:(job + 1) * 64]
        images[f"expected-gem-{output}.bin"] = bytes(data)

    allowed = {"inputs.bin", "gem-11.bin", "gem-101.bin",
               "expected-gem-10.bin", "expected-gem-100.bin"}
    changed = [name for name, data in images.items() if data != original[name]]
    assert set(changed) <= allowed
    assert images["inputs.bin"][:4] == original["inputs.bin"][:4]
    assert images["inputs.bin"][8:] == original["inputs.bin"][8:]
    for name in ("gem-11.bin", "gem-101.bin"):
        assert images[name][4:] == original[name][4:]
    args.output.mkdir(parents=True, exist_ok=False)
    for name, data in images.items():
        (args.output / name).write_bytes(data)
    result = dict(version=3, own_mesa_resources=True, apple_binary_pages=0,
                  source_fixture=str(args.source), iterations=list(steps),
                  independent_CPU_oracle=True,
                  all_shader_launcher_binding_and_command_bytes_unchanged=True,
                  modified_files=sorted(changed),
                  files={name: sha(data) for name, data in images.items()})
    (args.output / "provenance.json").write_text(json.dumps(result, indent=2) + "\n")
    print("PASS version3 uniform-only handoff fixture", json.dumps(list(steps)))


if __name__ == "__main__":
    main()
