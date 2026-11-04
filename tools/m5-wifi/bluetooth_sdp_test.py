#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Prove encrypted Linux-originated data using a standard SDP transaction.

No peer attributes, addresses beyond the selected peer, or key contents are
logged. This tests a kernel-owned socket, independently of raw HCI link setup.
"""
import argparse
import hashlib
import os
import socket
import struct


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer', required=True)
    parser.add_argument('--requests', type=int, default=1)
    args = parser.parse_args()
    if os.uname().machine != 'aarch64':
        raise RuntimeError('This data probe requires an arm64 device')
    address = bytes.fromhex(args.peer.replace(':', ''))
    if len(address) != 6 or not 1 <= args.requests <= 4096:
        raise ValueError('Invalid selected peer/request count')
    with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP) as link:
        link.setsockopt(274, 4, struct.pack('BB', 2, 0))
        link.settimeout(15)
        link.connect((args.peer, 1))
        level, key_size = link.getsockopt(274, 4, 2)
        if level < 2 or key_size < 7:
            raise RuntimeError('Link does not meet paired/encrypted policy')
        # Service Search Attribute Request for Serial Port, all attribute IDs.
        service_pattern = bytes.fromhex('3503191101')
        attribute_ids = bytes.fromhex('35050a0000ffff')
        parameters = service_pattern + struct.pack('>H', 0xffff) + attribute_ids + b'\x00'
        digest = hashlib.sha256()
        request_total = response_total = 0
        for transaction in range(1, args.requests + 1):
            request = struct.pack('>BHH', 6, transaction, len(parameters)) + parameters
            if link.send(request) != len(request):
                raise RuntimeError('Short SDP request')
            response = link.recv(4096)
            if len(response) < 8:
                raise RuntimeError('Short SDP response')
            code, reply_id, length, attribute_bytes = struct.unpack_from('>BHHH', response)
            continuation_offset = 7 + attribute_bytes
            if (code != 7 or reply_id != transaction or length + 5 != len(response)
                    or continuation_offset >= len(response)):
                raise RuntimeError('Invalid SDP response envelope/transaction')
            continuation_bytes = response[continuation_offset]
            if continuation_offset + 1 + continuation_bytes != len(response):
                raise RuntimeError('Invalid SDP continuation extent')
            digest.update(response)
            request_total += len(request)
            response_total += len(response)
            if transaction % 128 == 0:
                print('LINUX_BT_SDP_PROGRESS', dict(requests=transaction,
                      response_bytes=response_total), flush=True)
        print('LINUX_BT_SDP_RESULT', dict(security=level, key_size=key_size,
              request_bytes=len(request), response_bytes=len(response),
              attribute_bytes=attribute_bytes, continuation_bytes=continuation_bytes,
              requests=args.requests, total_request_bytes=request_total,
              total_response_bytes=response_total, sha256=digest.hexdigest()), flush=True)


if __name__ == '__main__':
    main()
