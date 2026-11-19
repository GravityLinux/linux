# SPDX-License-Identifier: GPL-2.0-only
"""Configure an ordinary wpa_supplicant through its Unix control socket.

A PMK arrives only on non-echoing terminal stdin. No credential is placed in
argv, a config file or output. The supplicant owns scanning and nl80211 setup.
"""
import argparse
import json
from pathlib import Path
import socket
import sys
import tempfile
import termios
import time


def read_pmk():
    fd = sys.stdin.fileno()
    previous = termios.tcgetattr(fd)
    attributes = previous.copy()
    attributes[3] &= ~(termios.ECHO | termios.ECHONL)
    termios.tcsetattr(fd, termios.TCSANOW, attributes)
    try:
        print('PMK_INPUT_READY', flush=True)
        encoded = bytearray(sys.stdin.buffer.readline(66)).strip()
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, previous)
    if len(encoded) != 64 or any(value not in b'0123456789abcdefABCDEF' for value in encoded):
        encoded[:] = bytes(len(encoded))
        raise ValueError('Expected a32-byte PMK as hexadecimal terminal input')
    return encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control', required=True)
    parser.add_argument('--ssid-hex', required=True)
    parser.add_argument('--scan-frequency', type=int)
    parser.add_argument('--passive-scan', action='store_true',
                        help='Use the supplicant passive_scan setting for a passive-only device')
    args = parser.parse_args()
    ssid = bytes.fromhex(args.ssid_hex)
    if not 1 <= len(ssid) <= 32:
        parser.error('SSID must contain1..32 bytes')
    pmk = read_pmk()
    try:
        with tempfile.TemporaryDirectory(prefix='wpa-pmk-') as directory:
            with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as control:
                control.settimeout(5)
                control.bind(str(Path(directory)/'client'))
                control.connect(args.control)

                def request(command):
                    control.send(command)
                    return control.recv(4096).strip()

                def configure(command, label):
                    if request(command) != b'OK':
                        raise RuntimeError('Supplicant refused '+label)

                if args.passive_scan:
                    configure(b'SET passive_scan 1', 'passive scan mode')
                network = request(b'ADD_NETWORK')
                if not network.isdigit():
                    raise RuntimeError('Supplicant did not allocate a network')
                prefix = b'SET_NETWORK '+network+b' '
                try:
                    configure(prefix+b'ssid '+ssid.hex().encode(), 'SSID')
                    for field, value in ((b'key_mgmt', b'WPA-PSK'), (b'proto', b'RSN'),
                                         (b'pairwise', b'CCMP'), (b'group', b'CCMP')):
                        configure(prefix+field+b' '+value, field.decode())
                    if args.scan_frequency is not None:
                        configure(prefix+b'scan_freq '+str(args.scan_frequency).encode(), 'scan frequency')
                    message = bytearray(prefix+b'psk ')
                    message += pmk
                    try:
                        configure(message, 'PMK')
                    finally:
                        message[:] = bytes(len(message))
                        pmk[:] = bytes(len(pmk))
                    configure(b'SELECT_NETWORK '+network, 'select network')
                    deadline = time.monotonic()+30
                    last_state = None
                    while time.monotonic() < deadline:
                        status = dict(line.split('=', 1) for line in
                                      request(b'STATUS').decode().splitlines() if '=' in line)
                        state = status.get('wpa_state')
                        if state != last_state:
                            print('SUPPLICANT_STATE', state, flush=True)
                            last_state = state
                        if state == 'COMPLETED':
                            print('SUPPLICANT_CONNECTED', json.dumps({key:status[key] for key in
                                ('wpa_state','freq','bssid','pairwise_cipher','group_cipher','key_mgmt')
                                if key in status}, sort_keys=True), flush=True)
                            return
                        time.sleep(.2)
                    raise TimeoutError('Supplicant did not complete association')
                except BaseException:
                    request(b'REMOVE_NETWORK '+network)
                    raise
    finally:
        pmk[:] = bytes(len(pmk))


if __name__ == '__main__':
    main()
