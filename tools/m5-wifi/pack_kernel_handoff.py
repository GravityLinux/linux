#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Encode our typed current-boot metadata for the Rust kernel driver.

Input is the private hardware metadata export, never an Apple image. No
packet payload or firmware workspace bytes enter this file. The driver
uses these addresses to adopt live queues; it does not replay queue configs.
"""
import argparse
import json
import pathlib
import struct

HEADER = struct.Struct('<8s4I10Q6s2x32s32s')
MAPPING = struct.Struct('<QQQ')
QUEUE = struct.Struct('<III15I')


def pack(state):
    serial = state['system_serial'].encode()
    nonce = bytes.fromhex(state['handoff_nonce'])
    mac = bytes.fromhex(state['station_mac'])
    mappings = sorted(state['streams']['2']['mappings'], key=lambda item: item['iova'])
    queues = state['queue_configs']
    if not 0 < len(serial) <= 32 or len(nonce) != 32 or len(mac) != 6 or mac[0] & 1:
        raise ValueError('Invalid identity or current-boot nonce')
    if not 0 < len(mappings) <= 8192 or len(queues) > 426:
        raise ValueError('Unexpected mapping or queue count')
    body = bytearray()
    previous_end = 0
    for item in mappings:
        iova, physical, length = (item[key] for key in ('iova', 'physical', 'length'))
        if iova < previous_end or length <= 0 or (physical | length) & 4095:
            raise ValueError('Invalid translation range')
        body += MAPPING.pack(iova, physical, length)
        previous_end = iova + length
    for item in queues:
        words = item['words']
        if len(words) > 15:
            raise ValueError('Unsupported queue metadata geometry')
        body += QUEUE.pack(item['kind'], item['queue'], len(words), *(words + [0] * (15 - len(words))))
    header = HEADER.pack(b'CENH0001', HEADER.size + len(body), len(mappings), len(queues), len(serial),
                         state['context_iova'], state['context_physical'], state['bar0_physical'],
                         state['dart_base'], state['owned_tx_arena_iova'], state['owned_tx_arena_physical'],
                         state['owned_tx_arena_size'], state['handoff_nonce_physical'],
                         state['dram_base'], state['dram_size'], mac, nonce, serial)
    return header + body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('metadata', type=pathlib.Path)
    parser.add_argument('output', type=pathlib.Path)
    args = parser.parse_args()
    encoded = pack(json.loads(args.metadata.read_text()))
    args.output.write_bytes(encoded)
    print(f'Encoded {len(encoded)} bytes of typed address/queue metadata; no device payloads')


if __name__ == '__main__':
    main()
