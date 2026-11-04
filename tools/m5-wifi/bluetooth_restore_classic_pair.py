#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Reload our sole test peer's Classic key using the standard MGMT address type.

For LLMs: optional BlueZ 5.72 interoperability probe after VHCI recreation.
This is user-space pairing restoration, not a controller protocol workaround.
Key material is sent from the existing private store to the kernel, never logged
or persisted by this tool.
"""
import argparse
import configparser
import ctypes
import os
from pathlib import Path
import re
import socket
import struct
import time
from bluetooth_hci_host_probe import exchange

HCI_DEV_NONE, HCI_CHANNEL_CONTROL = 0xffff, 3
MGMT_EV_COMMAND_COMPLETE, MGMT_EV_COMMAND_STATUS = 1, 2
MGMT_OP_LOAD_LINK_KEYS = 0x0012
BDADDR_BREDR = 0


def classic_key_payload(peer, info):
    try:
        address = bytes.fromhex(peer.replace(':', ''))
        key = bytes.fromhex(info['LinkKey']['Key'].removeprefix('0x'))
        key_type = int(info['LinkKey']['Type'])
        pin_length = int(info['LinkKey'].get('PINLength', '0'))
    except (ValueError, KeyError, configparser.Error):
        raise ValueError('Invalid stored Classic pairing metadata') from None
    if len(address) != 6 or len(key) != 16 or not 0 <= key_type <= 8 or not 0 <= pin_length <= 16:
        raise ValueError('Invalid stored Classic key geometry')
    # Debug disabled, one key. Link keys always use the Classic address type;
    # General/AddressType describes the peer's separate LE identity.
    return struct.pack('<BH6sBB16sB', 0, 1, address[::-1], BDADDR_BREDR,
                       key_type, key, pin_length)


def bind_control(sock):
    # Python 3.12's HCI tuple has no channel member; use the Linux sockaddr_hci.
    class Address(ctypes.Structure):
        _fields_ = [('family', ctypes.c_ushort), ('device', ctypes.c_ushort),
                    ('channel', ctypes.c_ushort)]
    address = Address(socket.AF_BLUETOOTH, HCI_DEV_NONE, HCI_CHANNEL_CONTROL)
    library = ctypes.CDLL(None, use_errno=True)
    library.bind.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint]
    library.bind.restype = ctypes.c_int
    if library.bind(sock.fileno(), ctypes.byref(address), ctypes.sizeof(address)):
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))


def load_key(sock, index, payload):
    packet = struct.pack('<HHH', MGMT_OP_LOAD_LINK_KEYS, index, len(payload)) + payload
    if sock.send(packet) != len(packet):
        raise RuntimeError('Short MGMT command write')
    end = time.monotonic() + 3
    while time.monotonic() < end:
        sock.settimeout(end - time.monotonic())
        event = sock.recv(4096)
        if len(event) < 9:
            continue
        kind, controller, length = struct.unpack_from('<HHH', event)
        if controller != index or length + 6 != len(event):
            continue
        if kind not in (MGMT_EV_COMMAND_COMPLETE, MGMT_EV_COMMAND_STATUS):
            continue
        opcode, status = struct.unpack_from('<HB', event, 6)
        if opcode != MGMT_OP_LOAD_LINK_KEYS:
            continue
        if status:
            raise RuntimeError(f'Classic key load status {status:#x}')
        return
    raise TimeoutError('No Classic key load completion')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer', required=True)
    parser.add_argument('--expected-address', required=True)
    parser.add_argument('--index', type=int, default=0)
    args = parser.parse_args()
    peer, address = args.peer.upper(), args.expected_address.upper()
    if not all(re.fullmatch(r'(?:[0-9A-F]{2}:){5}[0-9A-F]{2}', a) for a in (peer, address)) or not 0 <= args.index <= 15:
        raise ValueError('Invalid selected address/index')
    if os.uname().machine != 'aarch64':
        raise RuntimeError('This pairing probe requires an arm64 device')
    with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI) as raw:
        raw.bind((args.index,))
        current = exchange(raw, 0x1009)  # Standard Read BD_ADDR, filtered reply only.
    if current != bytes.fromhex(address.replace(':', ''))[::-1]:
        raise RuntimeError('Wrong controller')
    store = Path('/var/lib/bluetooth') / address
    peers = sorted(p.name for p in store.iterdir() if p.is_dir() and (p / 'info').is_file())
    if peers != [peer]:
        raise RuntimeError('This bounded probe requires exactly the selected stored peer')
    info = configparser.ConfigParser()
    try:
        info.read(store / peer / 'info')
    except configparser.Error:
        raise ValueError('Stored pairing metadata is invalid') from None
    payload = classic_key_payload(peer, info)
    with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI) as sock:
        bind_control(sock)
        load_key(sock, args.index, payload)
    print('BT_CLASSIC_PAIR_RESTORED', dict(peer=peer, index=args.index,
          address_type=BDADDR_BREDR), flush=True)


if __name__ == '__main__':
    main()
