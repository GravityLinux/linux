# SPDX-License-Identifier: GPL-2.0-only
"""Cross-check the kernel IPC implementation against qualified Python geometry."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace as NS
import unittest
from centauri_ipc import make_context
from centauri_queues import DataQueue, PROFILES

class RustControlTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('rustc'), 'rustc is required')
    def test_kernel_control_protocol(self):
        with tempfile.TemporaryDirectory(prefix='centauri-control-') as directory:
            root=Path(directory)
            (root/'context').write_bytes(make_context(0x112340000))
            frames=[]
            arena=NS(u=NS(proxy=None),physical=0,iova=0x123000000,
                     ipc=NS(submit=frames.append))
            for number,profile in PROFILES.items():DataQueue(arena,number,profile).configure()
            (root/'frames').write_bytes(b''.join(frames))
            env=dict(os.environ,CENTAURI_CONTROL_CONTEXT=str(root/'context'),
                     CENTAURI_CONTROL_FRAMES=str(root/'frames'))
            source=Path(__file__).with_name('test_centauri_control.rs')
            subprocess.run(['rustc','--edition=2021','--test',str(source),'-o',str(root/'test')],env=env,check=True)
            subprocess.run([str(root/'test')],check=True)

if __name__=='__main__':unittest.main()
