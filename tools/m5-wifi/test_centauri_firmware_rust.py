# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the kernel FTAB parser with synthetic Python-produced inputs."""
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest

from centauri_ftab import make_boot_allocation, prepare_image


class RustFirmwareTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('rustc'), 'rustc is needed for kernel parser host checks')
    def test_kernel_parser_against_python_encoder(self):
        def header(count, kind=0):
            return struct.pack('<8I8sII', 0, 1, kind, 0, 0, 0, 0, 0, b'rkosftab', count, 0)
        layout = (header(2, 0x42) + struct.pack('<4sIII', b'msww', 0x4000, 0x8000, 0) +
                  struct.pack('<4sIII', b'mswb', 0xc000, 0x4000, 0))
        vendor = header(1) + struct.pack('<4sIII', b'2ftb', 64, len(layout), 0) + layout
        image = prepare_image(vendor, b'synthetic manifest, not a signature',
                              {b'nvrm': b'synthetic nvrm', b'_cal': b'synthetic cal', b'_apb': b'synthetic apb'})
        with tempfile.TemporaryDirectory(prefix='centauri-rust-ftab-') as directory:
            root = Path(directory)
            image_path, allocation_path = root/'image.bin', root/'allocation.bin'
            image_path.write_bytes(image)
            allocation_path.write_bytes(make_boot_allocation(vendor))
            env = dict(os.environ, CENTAURI_FW_TEST_IMAGE=str(image_path),
                       CENTAURI_FW_TEST_ALLOCATION=str(allocation_path))
            for name in ('test_centauri_firmware.rs', 'test_centauri_bootstrap.rs'):
                subprocess.run(['rustc', '--edition=2021', '--test',
                                str(Path(__file__).with_name(name)), '-o', str(root/'test')],
                               env=env, check=True)
                subprocess.run([str(root/'test')], check=True)


if __name__ == '__main__':
    unittest.main()
