#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Encode current-boot addresses and proven Bluetooth queue geometry, not replay.

The one-page Python layout is the only geometry supported by this first Rust
port. No controller payload, key, opaque command, or queue registration words
are retained. This metadata describes already initialized host-owned queues.
"""
import argparse
import json
from pathlib import Path
import struct

HEADER = struct.Struct('<8s4I6Q32s32s')
MAPPING = struct.Struct('<QQQ')
QUEUE = struct.Struct('<HHIQ')
CORE_QUEUES = {1: (260, 0x1000), 7: (260, 0x1240),
               2: (2592, 0x1480), 8: (2592, 0x2900)}


def pack(state):
    serial = state['system_serial'].encode('ascii')
    nonce = bytes.fromhex(state['handoff_nonce'])
    context = state['bluetooth_context_iova']
    mappings = state['streams']['3']['mappings']
    queues = state['bluetooth_queues']
    if not 0 < len(serial) <= 32 or len(nonce) != 32:
        raise ValueError('Invalid serial metadata or malformed current-boot nonce')
    if len(mappings) != 1 or len(queues) != 4:
        raise ValueError('Only the verified one-page/four-ring layout is supported')
    mapping = mappings[0]
    if mapping['iova'] != context or context & 0x3fff or mapping['physical'] & 0x3fff or mapping['length'] != 0x4000:
        raise ValueError('Expected one aligned 16-KiB host arena')
    dram = state['dram_base']
    end = dram + state['dram_size']
    nonce_physical = state['handoff_nonce_physical']
    if not dram <= mapping['physical'] <= end - 0x4000 or not dram <= nonce_physical <= end - 32 or nonce_physical & 4095 > 4064:
        raise ValueError('Host memory outside DRAM or nonce straddles page')
    if any(not 0 < state[name] < 1 << 64 or state[name] & 4095
           for name in ('bluetooth_bar0_physical', 'dart_base')):
        raise ValueError('Invalid Bluetooth MMIO resources')
    if {q['number'] for q in queues} != set(CORE_QUEUES):
        raise ValueError('Missing or duplicate core ring')
    body = bytearray(MAPPING.pack(mapping['iova'], mapping['physical'], mapping['length']))
    for q in sorted(queues, key=lambda q: q['number']):
        capacity, offset = CORE_QUEUES[q['number']]
        if q['entries'] != 2 or q['capacity'] != capacity or q['ring_iova'] != context + offset:
            raise ValueError('Ring differs from tested full-MTU Python layout')
        body += QUEUE.pack(q['number'], q['entries'], capacity, q['ring_iova'])
    return HEADER.pack(b'CENB0002', HEADER.size + len(body), 1, 4, len(serial),
                       context, state['bluetooth_bar0_physical'], state['dart_base'],
                       nonce_physical, dram, state['dram_size'], nonce, serial) + body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('metadata', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    encoded = pack(json.loads(args.metadata.read_text()))
    args.output.write_bytes(encoded)
    print(f'Encoded {len(encoded)} bytes of named host metadata; no device payloads')


if __name__ == '__main__':
    main()
