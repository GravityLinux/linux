# SPDX-License-Identifier: GPL-2.0-only
"""Test an SAE-only network with an ordinary supplicant, without a key file.

The password arrives on non-echoing terminal stdin. The supplicant owns SAE;
this tool never substitutes a WPA2 PMK for SAE authentication.
"""
import argparse
import json
from pathlib import Path
import socket
import sys
import tempfile
import termios
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control', required=True)
    parser.add_argument('--ssid-hex', required=True)
    parser.add_argument('--scan-frequency', type=int)
    parser.add_argument('--sae-pwe', choices=('both', 'hash-to-element', 'hunt-and-peck'),
                        default='both', help='Require a mechanism or allow firmware negotiation')
    args = parser.parse_args()
    ssid = bytes.fromhex(args.ssid_hex)
    if not 1 <= len(ssid) <= 32:
        parser.error('SSID must contain 1..32 bytes')
    previous = termios.tcgetattr(sys.stdin.fileno())
    attributes = previous.copy()
    attributes[3] &= ~(termios.ECHO | termios.ECHONL)
    termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, attributes)
    password = bytearray()
    try:
        print('SAE_PASSWORD_INPUT_READY', flush=True)
        password = bytearray(sys.stdin.buffer.readline(65)).rstrip(b'\r\n')
    finally:
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, previous)
    try:
        if not 1 <= len(password) <= 63 or any(value < 32 or value > 126 for value in password):
            raise ValueError('Expected 1..63 printable ASCII password bytes')
        with tempfile.TemporaryDirectory(prefix='wpa-sae-') as directory:
            with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as control:
                control.settimeout(5)
                control.bind(str(Path(directory) / 'client'))
                control.connect(args.control)

                def request(command):
                    control.send(command)
                    return control.recv(4096).strip()

                def configure(command, label):
                    if request(command) != b'OK':
                        raise RuntimeError('Supplicant refused ' + label)

                configure(b'SET passive_scan 1', 'passive scan mode')
                pwe = {'both': b'2', 'hash-to-element': b'1', 'hunt-and-peck': b'0'}[args.sae_pwe]
                configure(b'SET sae_pwe ' + pwe, 'SAE PWE mode')
                network = request(b'ADD_NETWORK')
                if not network.isdigit():
                    raise RuntimeError('Supplicant did not allocate a network')
                prefix = b'SET_NETWORK ' + network + b' '
                try:
                    configure(prefix + b'ssid ' + ssid.hex().encode(), 'SSID')
                    for field, value in ((b'key_mgmt', b'SAE'), (b'proto', b'RSN'),
                                         (b'pairwise', b'CCMP'), (b'group', b'CCMP'),
                                         (b'ieee80211w', b'2')):
                        configure(prefix + field + b' ' + value, field.decode())
                    if args.scan_frequency is not None:
                        configure(prefix + b'scan_freq ' + str(args.scan_frequency).encode(), 'frequency')
                    message = bytearray(prefix + b'sae_password "')
                    for value in password:
                        if value in (34, 92):
                            message.append(92)
                        message.append(value)
                    message.append(34)
                    try:
                        configure(message, 'SAE password')
                    finally:
                        message[:] = bytes(len(message))
                        password[:] = bytes(len(password))
                    configure(b'SELECT_NETWORK ' + network, 'select network')
                    deadline = time.monotonic() + 30
                    last_state = None
                    while time.monotonic() < deadline:
                        status = dict(line.split('=', 1) for line in
                                      request(b'STATUS').decode().splitlines() if '=' in line)
                        state = status.get('wpa_state')
                        if state != last_state:
                            print('SAE_SUPPLICANT_STATE', state, flush=True)
                            last_state = state
                        if state == 'COMPLETED':
                            print('SAE_SUPPLICANT_CONNECTED', json.dumps({key: status[key] for key in
                                ('wpa_state', 'freq', 'bssid', 'pairwise_cipher', 'group_cipher', 'key_mgmt')
                                if key in status}, sort_keys=True), flush=True)
                            return
                        time.sleep(.2)
                    raise TimeoutError('Supplicant did not complete SAE association')
                except BaseException:
                    request(b'REMOVE_NETWORK ' + network)
                    raise
    finally:
        password[:] = bytes(len(password))


if __name__ == '__main__':
    main()
