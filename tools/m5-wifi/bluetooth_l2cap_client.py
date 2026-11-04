#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Verify generated L2CAP data against the exact authorized paired echo peer.

For LLMs: normal Linux Bluetooth sockets; no controller keys or payloads logged.
Use with our transient Mac SDK echo service. This checks actual round trips.
"""
import argparse
import hashlib
import os
import socket
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer', required=True)
parser.add_argument('--psm', type=lambda value: int(value, 0), required=True)
parser.add_argument('--packets', type=int, default=2000)
parser.add_argument('--size', type=int, default=512)
args = parser.parse_args()
if not 1 <= args.packets <= 10000 or not 65 <= args.size <= 672:
    raise ValueError('Invalid bounded data test geometry')
with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP) as link:
    link.setsockopt(274, 4, struct.pack('BB', 2, 0))
    link.settimeout(10)
    link.connect((args.peer, args.psm))
    if link.getpeername()[0].upper() != args.peer.upper():
        raise RuntimeError('Unexpected peer')
    level, key_size = link.getsockopt(274, 4, 2)
    if level < 2 or key_size < 7:
        raise RuntimeError('Link is not paired and encrypted')
    print('LINUX_L2CAP_CLIENT_CONNECTED', dict(level=level, key_size=key_size), flush=True)
    prefix = b'Gravity Bluetooth data proof ' + os.urandom(16)
    digest = hashlib.sha256()
    for sequence in range(args.packets):
        payload = prefix + struct.pack('<Q', sequence)
        payload += bytes((i + sequence) & 255 for i in range(args.size - len(payload)))
        if link.send(payload) != len(payload):
            raise RuntimeError('Short L2CAP write')
        response = link.recv(4096)
        if response != payload:
            raise RuntimeError('Echo data mismatch')
        digest.update(response)
        if (sequence + 1) % 128 == 0:
            print('LINUX_L2CAP_CLIENT_PROGRESS', dict(packets=sequence + 1,
                  bytes=(sequence + 1) * args.size), flush=True)
    print('LINUX_L2CAP_CLIENT_PASS', dict(packets=args.packets,
          bytes=args.packets * args.size, sha256=digest.hexdigest(), exact_echo=True), flush=True)
