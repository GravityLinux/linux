#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Read named host-support fields on the exact prototype controller.

For LLMs: Linux HCI raw-socket UAPI and standard HCI commands only. Use while
controller initialization is idle. Filters exclude pairing/key/data events.
"""
import argparse
import json
import os
import socket
import struct
import time


def exchange(sock, opcode, parameters=b''):
    # struct hci_filter: packet types, two event bitmaps, command opcode.
    sock.setsockopt(0, 2, struct.pack('<IIIH2x', 1 << 4,
                                    (1 << 0x0e) | (1 << 0x0f), 0, opcode))
    packet = bytes([1]) + struct.pack('<HB', opcode, len(parameters)) + parameters
    if sock.send(packet) != len(packet):
        raise RuntimeError('Short HCI command send')
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        sock.settimeout(deadline - time.monotonic())
        response = sock.recv(259)
        if len(response) < 3 or response[0] != 4 or response[2] + 3 != len(response):
            raise RuntimeError('Invalid standard HCI event extent')
        if response[1] == 0x0f and len(response) == 7:
            if struct.unpack_from('<H', response, 5)[0] == opcode and response[3]:
                raise RuntimeError(f'HCI {opcode:#x} command status {response[3]:#x}')
        elif response[1] == 0x0e and len(response) >= 7:
            if struct.unpack_from('<H', response, 4)[0] != opcode:
                continue
            if response[6]:
                raise RuntimeError(f'HCI {opcode:#x} completion status {response[6]:#x}')
            return response[7:]
    raise TimeoutError(f'No completion for {opcode:#x}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--index', type=int, default=0)
    parser.add_argument('--expected-address', required=True)
    args = parser.parse_args()
    if os.uname().machine != 'aarch64':
        raise RuntimeError('This prototype probe requires an arm64 device')
    expected = bytes.fromhex(args.expected_address.replace(':', ''))
    if len(expected) != 6:
        raise ValueError('Invalid controller address')
    with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI) as sock:
        sock.bind((args.index,))
        address = exchange(sock, 0x1009)
        if address != expected[::-1]:
            raise RuntimeError('Controller address does not match the selected target')
        result = {}
        for opcode, names in ((0x0c55, ('simple_pairing_enabled',)),
                              (0x0c6c, ('le_host_supported', 'simultaneous_le_host_supported')),
                              (0x0c79, ('secure_connections_host_supported',))):
            value = exchange(sock, opcode)
            if len(value) != len(names):
                raise RuntimeError('Unexpected host-support return extent')
            result.update(zip(names, value))
        print('BT_HOST_SUPPORT_READBACK', json.dumps(result, sort_keys=True), flush=True)


if __name__ == '__main__':
    main()
