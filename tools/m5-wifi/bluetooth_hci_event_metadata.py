#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Observe Linux HCI metadata after driver normalization, without secret bodies.

This is kernel-visible HCI, not direct firmware event-field evidence.
"""
import argparse
import json
import socket
import struct
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--index', type=int, default=0)
parser.add_argument('--seconds', type=int, default=60)
args = parser.parse_args()
if not 0 <= args.index <= 15 or not 1 <= args.seconds <= 360:
    raise ValueError('Invalid bounded controller observation')
events = {
    3: 'connection_complete', 4: 'connection_request', 5: 'disconnect',
    8: 'encryption_change', 14: 'command_complete', 15: 'command_status',
    22: 'pin_request', 23: 'link_key_request', 49: 'io_capability_request',
    50: 'io_capability_response', 51: 'confirmation_request',
    52: 'passkey_request', 53: 'oob_request', 54: 'pairing_complete',
}
# Explicitly exclude Link Key Notification, passkey notification and every
# vendor event. Even selected events print only the named fields below.
mask = sum(1 << number for number in events)
with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI) as sock:
    sock.bind((args.index,))
    sock.setsockopt(0, 2, struct.pack('<IIIH2x', 1 << 4,
        mask & 0xffffffff, mask >> 32, 0))
    sock.settimeout(1)
    end = time.monotonic() + args.seconds
    print('HCI_METADATA_READY', flush=True)
    while time.monotonic() < end:
        try:
            packet = sock.recv(260)
        except TimeoutError:
            continue
        if len(packet) < 3 or packet[0] != 4 or packet[2] + 3 != len(packet):
            raise RuntimeError('Malformed HCI event extent')
        code = packet[1]
        if code not in events:
            continue
        result = {'event': events[code], 'bytes': len(packet)}
        if code == 14 and len(packet) >= 7:
            result.update(opcode=hex(struct.unpack_from('<H', packet, 4)[0]),
                          status=packet[6])
        elif code == 15 and len(packet) == 7:
            result.update(opcode=hex(struct.unpack_from('<H', packet, 5)[0]),
                          status=packet[3])
        elif code == 4 and len(packet) == 13:
            result['link_type'] = packet[12]
        elif code == 3 and len(packet) == 14:
            result.update(status=packet[3], link_type=packet[12], encrypted=packet[13])
        elif code in (5, 8, 54) and len(packet) >= 4:
            result['status'] = packet[3]
        if code == 8 and len(packet) == 7:
            result['enabled'] = packet[6]
        print(json.dumps(result, sort_keys=True), flush=True)
