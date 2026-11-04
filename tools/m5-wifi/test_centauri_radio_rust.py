# SPDX-License-Identifier: GPL-2.0-only
"""Check native radio publication against the independently tested Python context."""
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from centauri_wifi import (make_wifi_context, completion_queue_config,
                           submission_queue_config, ADMIN_NOTIFY)
from centauri_wifi_queues import PROFILES, SETUP_ORDER, RING_AREA, RING_REGION_SIZE
import centauri_regs as R
from centauri_wifi_scan import scan_request
from centauri_ethernet_owned import extended_completion_config, extended_submission_config
from centauri_ethernet import tx_descriptor
from test_centauri_wifi_scan import radio_report
from test_centauri_wifi_auth import report as sae_auth_report
from centauri_wifi_connection import (ConnectionParameters, psk_connection_request,
                                      sae_connection_request, disconnect_request)


class RustRadioTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('rustc'), 'rustc is required')
    def test_kernel_radio_protocol(self):
        with tempfile.TemporaryDirectory(prefix='centauri-radio-') as directory:
            root = Path(directory)
            (root/'context').write_bytes(make_wifi_context(0x100000000))
            (root/'handshake').write_bytes(struct.pack('<I', R.WIFI_IPC_HANDSHAKE))
            (root/'notify').write_bytes(struct.pack('<I', ADMIN_NOTIFY))
            frames = bytearray()
            for index, (kind, number) in enumerate(SETUP_ORDER):
                profile = PROFILES[number]
                builder = completion_queue_config if kind == 2 else submission_queue_config
                entries = profile.completion_entries if kind == 2 else profile.submission_entries
                frame = builder(number, 0x100000000 + RING_AREA + index * RING_REGION_SIZE, entries)
                frames += struct.pack('<I', len(frame)) + frame
            (root/'frames').write_bytes(frames)
            profiles, pool = bytearray(), 0x200000
            for number, profile in PROFILES.items():
                profiles += struct.pack('<6I', number, profile.submission_entries,
                                        profile.completion_entries, profile.capacity,
                                        profile.receive_target, pool)
                pool = (pool + profile.submission_entries * profile.capacity + 0x3fff) & ~0x3fff
            (root/'profiles').write_bytes(profiles)
            (root/'scan').write_bytes(scan_request(9, [1, 6, 11], report_frames=True,
                option_words=(0, 0), timing_words=(50, 50, 110)))
            (root/'scan-frame').write_bytes(radio_report(fcs=True, extra_radio=72))
            (root/'disconnect').write_bytes(disconnect_request())
            (root/'sae-auth').write_bytes(sae_auth_report())
            (root/'connection').write_bytes(psk_connection_request(b'test-network',
                bytes.fromhex('021122334455'),11,bytes(range(32)),
                parameters=ConnectionParameters(1,1,4,(2000,6000,6000,60000,300)),
                rsn_ie=bytes.fromhex('0100000fac040100000fac040100000fac020000')))
            (root/'sae-connection').write_bytes(sae_connection_request(b'test-network',
                bytes.fromhex('021122334455'),11,b'synthetic-test-password',
                parameters=ConnectionParameters(1,1,4,(2000,6000,6000,60000,300)),
                rsn_ie=bytes.fromhex('0100000fac040100000fac040100000fac08cc00')))
            (root/'h2e-connection').write_bytes(sae_connection_request(b'test-network',
                bytes.fromhex('021122334455'),11,b'synthetic-test-password',
                parameters=ConnectionParameters(1,1,4,(2000,6000,6000,60000,300)),
                rsn_ie=bytes.fromhex('0100000fac040100000fac040100000fac08cc00'), h2e_only=True))
            data_dma = 0x100a00000
            data_frames = [
                extended_completion_config(76,192,data_dma,entries=8192,source=0xffff,profile=0x12,service=3),
                extended_submission_config(25,data_dma+0x40000,receive=False,completion_queue=76,service=3),
                extended_completion_config(15,207,data_dma+0x50000,entries=4096,source=12,profile=0xb,service=8),
                extended_submission_config(12,data_dma+0x70000,receive=True,completion_queue=12,service=8),
            ]
            (root/'data-frames').write_bytes(b''.join(struct.pack('<I',len(f))+f for f in data_frames))
            (root/'data-tx').write_bytes(tx_descriptor(64,data_dma+0x478000,0))
            replies = bytearray()
            # Synthetic runtime messages; no captured Apple reply bodies.
            for command, body in ((8, struct.pack('<HH3sHH1s', 1, 3, b'abc', 2, 1, b'Z')),
                                  (10, b'US\x01'),
                                  (47, struct.pack('<BII4x', 2, 1, 0x124))):
                replies += struct.pack('<II', command, len(body)) + body
            (root/'replies').write_bytes(replies)
            env = dict(os.environ, CENTAURI_WIFI_CONTEXT=str(root/'context'),
                       CENTAURI_WIFI_HANDSHAKE=str(root/'handshake'),
                       CENTAURI_WIFI_NOTIFY=str(root/'notify'),
                       CENTAURI_WIFI_ADMIN_FRAMES=str(root/'frames'),
                       CENTAURI_WIFI_PROFILES=str(root/'profiles'),
                       CENTAURI_WIFI_TEST_REPLIES=str(root/'replies'),
                       CENTAURI_WIFI_SCAN=str(root/'scan'),
                       CENTAURI_WIFI_SCAN_FRAME=str(root/'scan-frame'),
                       CENTAURI_WIFI_CONNECTION=str(root/'connection'),
                       CENTAURI_WIFI_SAE_CONNECTION=str(root/'sae-connection'),
                       CENTAURI_WIFI_H2E_CONNECTION=str(root/'h2e-connection'),
                       CENTAURI_WIFI_SAE_AUTH=str(root/'sae-auth'),
                       CENTAURI_WIFI_DISCONNECT=str(root/'disconnect'),
                       CENTAURI_WIFI_DATA_FRAMES=str(root/'data-frames'),
                       CENTAURI_WIFI_DATA_TX=str(root/'data-tx'))
            subprocess.run(['rustc', '--edition=2021', '--test',
                            str(Path(__file__).with_name('test_centauri_radio.rs')),
                            '-o', str(root/'test')], env=env, check=True)
            subprocess.run([str(root/'test')], check=True)


if __name__ == '__main__':
    unittest.main()
