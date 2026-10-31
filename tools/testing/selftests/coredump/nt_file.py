#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Manual NT_FILE regression: see README.rst for core collection instructions."""

import argparse
import json
import mmap
import os
from pathlib import Path
import resource
import signal
import struct


def crash(manifest):
    page = os.sysconf("SC_PAGESIZE")
    backing = manifest.with_suffix(".backing").resolve()
    mappings = []
    with backing.open("w+b") as file:
        file.truncate(8 * page)
        for index in (0, 1, 2, 3, 4, 5, 7):
            file.seek(index * page)
            file.write(bytes([index]) * page)
        file.flush()
        for index in (0, 1, 2, 3, 4, 5, 7):
            mapping = mmap.mmap(file.fileno(), page, flags=mmap.MAP_PRIVATE,
                                prot=mmap.PROT_READ, offset=index * page)
            assert mapping[0] == index
            mappings.append(mapping)
        entries = []
        for line in Path("/proc/self/maps").read_text().splitlines():
            fields = line.split(maxsplit=5)
            if len(fields) == 6 and fields[5] == str(backing):
                start, end = (int(x, 16) for x in fields[0].split("-"))
                entries.append([start, end, int(fields[2], 16)])
        assert len(entries) == 7, entries
        manifest.write_text(json.dumps(dict(page_size=page, pid=os.getpid(),
                                           backing=str(backing), entries=entries)))
        _, hard = resource.getrlimit(resource.RLIMIT_CORE)
        resource.setrlimit(resource.RLIMIT_CORE, (hard, hard))
        print(f"pid={os.getpid()} page_size={page} manifest={manifest}", flush=True)
        os.kill(os.getpid(), signal.SIGSEGV)


def file_notes(core):
    with core.open("rb") as file:
        header = file.read(64)
        assert header[:5] == b"\x7fELF\x02", "ELF64 core required"
        endian = {1: "<", 2: ">"}[header[5]]
        assert struct.unpack_from(endian + "H", header, 16)[0] == 4, "not ET_CORE"
        phoff = struct.unpack_from(endian + "Q", header, 32)[0]
        phsize, phnum = struct.unpack_from(endian + "HH", header, 54)
        for index in range(phnum):
            file.seek(phoff + index * phsize)
            phdr = struct.unpack(endian + "II6Q", file.read(56))
            if phdr[0] != 4:  # PT_NOTE
                continue
            file.seek(phdr[2])
            notes = file.read(phdr[5])
            pos = 0
            while pos < len(notes):
                namesz, descsz, kind = struct.unpack_from(endian + "III", notes, pos)
                pos += 12
                name = notes[pos:pos + namesz].rstrip(b"\0")
                pos += (namesz + 3) & ~3
                desc = notes[pos:pos + descsz]
                pos += (descsz + 3) & ~3
                if kind == 0x46494c45 and name == b"CORE":
                    count, page = struct.unpack_from(endian + "QQ", desc)
                    paths = desc[16 + 24 * count:].split(b"\0")
                    entries = [struct.unpack_from(endian + "QQQ", desc, 16 + 24 * i)
                               for i in range(count)]
                    yield page, list(zip(entries, paths))


def check(manifest, core):
    expected = json.loads(manifest.read_text())
    matches = []
    for page, entries in file_notes(core):
        assert page == expected["page_size"], (page, expected["page_size"])
        for (start, end, offset), path in entries:
            if os.fsdecode(path) == expected["backing"]:
                matches.append([start, end, offset * page])
    assert sorted(matches) == sorted(expected["entries"]), (matches, expected["entries"])
    print(f"PASS: {len(matches)} NT_FILE offsets match /proc/self/maps, page_size={page}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("crash", "check"))
    parser.add_argument("manifest", type=Path)
    parser.add_argument("core", type=Path, nargs="?")
    args = parser.parse_args()
    if args.mode == "crash":
        crash(args.manifest)
    elif args.core is None:
        parser.error("check requires a core file")
    else:
        check(args.manifest, args.core)
