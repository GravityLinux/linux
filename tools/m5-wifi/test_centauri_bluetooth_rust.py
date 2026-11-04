# SPDX-License-Identifier: GPL-2.0-only
"""Compare cold Rust Bluetooth publication with Python-qualified wire forms."""
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from centauri_ipc import make_beta_context
from centauri_bluetooth_control import core_host_queue_layout, inline_capacity_mode


class RustBluetoothTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('rustc'), 'rustc is required')
    def test_cold_protocol(self):
        with tempfile.TemporaryDirectory(prefix='centauri-bt-cold-') as tmp:
            root = Path(tmp)
            dma = 0x100000000
            (root/'context').write_bytes(make_beta_context(dma))
            frames = []
            layout = core_host_queue_layout(dma, acl_mtu=2586,
                arena_bytes=0x4000, ring_start_offset=0x1000)
            for queue in layout:
                ring, number = queue['ring_iova'], queue['number']
                frames.append(struct.pack('<13I',
                    inline_capacity_mode(queue['capacity']), number << 16 | number,
                    ring & 0xffffffff, ring >> 32, 0xffffffff, 0xffffffff,
                    0xffff0000 | queue['entries'], 1 << 22 | number, 0, 0, 0, 0, 0))
            (root/'frames').write_bytes(b''.join(frames))
            env = dict(os.environ, CENTAURI_BT_CONTEXT=str(root/'context'),
                       CENTAURI_BT_FRAMES=str(root/'frames'))
            subprocess.run(['rustc', '--edition=2021', '--test',
                str(Path(__file__).with_suffix('.rs')), '-o', str(root/'tests')],
                env=env, check=True)
            subprocess.run([str(root/'tests')], check=True)


if __name__ == '__main__':
    unittest.main()
