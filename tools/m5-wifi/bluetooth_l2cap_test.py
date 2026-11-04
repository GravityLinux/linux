#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Echo own test data from only the operator-selected Bluetooth peer.

For LLMs: standard Linux sockets, with pairing/encryption required. No keys or
packet bodies are logged. Run on the machine whose hardware driver is tested.
"""
import argparse
import hashlib
import socket
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer', required=True, help='Exact authorized Bluetooth address')
parser.add_argument('--psm', type=lambda value: int(value, 0), default=0x1001)
parser.add_argument('--connections', type=int, default=1,
                    help='Bounded sequential channels on the same listening socket')
args = parser.parse_args()
if not 1 <= args.connections <= 16:
    raise ValueError('Invalid test connection count')
if not 0x1001 <= args.psm <= 0xffff or args.psm & 0x0101 != 1:
    raise ValueError('Invalid test PSM')
server = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP)
# Linux UAPI SOL_BLUETOOTH=274, BT_SECURITY=4, BT_SECURITY_MEDIUM=2.
server.setsockopt(274, 4, struct.pack('BB', 2, 0))
server.bind(('00:00:00:00:00:00', args.psm))
server.listen(1)
print(f'LINUX_L2CAP_READY psm={args.psm:#x} security=paired-and-encrypted', flush=True)
for channel_index in range(1, args.connections + 1):
    connection, peer = server.accept()
    if peer[0].upper() != args.peer.upper():
        connection.close()
        raise RuntimeError('Unexpected Bluetooth peer')
    security = connection.getsockopt(274, 4, 2)
    if security[0] < 2 or security[1] < 7:
        raise RuntimeError('Link did not meet requested encryption/key-size policy')
    print('LINUX_L2CAP_SECURITY', dict(connection=channel_index, level=security[0], key_size=security[1]), flush=True)
    digest = hashlib.sha256()
    packets = total = 0
    while True:
        data = connection.recv(4096)
        if not data:
            break
        if not data.startswith(b'Gravity Bluetooth data proof '):
            raise RuntimeError('Unexpected test data')
        if connection.send(data) != len(data):
            raise RuntimeError('Short Bluetooth send')
        packets += 1
        total += len(data)
        digest.update(data)
        if packets % 128 == 0:
            print('LINUX_BT_DATA_PROGRESS', dict(packets=packets, bytes=total), flush=True)
    if not packets:
        raise RuntimeError('Peer closed before any test data arrived')
    print('LINUX_BT_DATA_RECEIVED', dict(connection=channel_index, packets=packets, bytes=total,
          sha256=digest.hexdigest(), echoed=True), flush=True)
    connection.close()
server.close()
