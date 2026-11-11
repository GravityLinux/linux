#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host tests of the G16 queue-failure cancellation (g16_queue_fail.rs).

Compiles the production module with queue-fail.rs. rustc must be on PATH;
no device or kernel build is needed.
"""
from pathlib import Path
import os
import subprocess
import tempfile

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_queue_fail.rs').exists())
source = f'''#![allow(dead_code)]
#[path = "{root / 'drivers/gpu/drm/asahi/g16_queue_fail.rs'}"] mod g16_queue_fail;
''' + here.with_name('queue-fail.rs').read_text()

with tempfile.TemporaryDirectory(prefix='asahi-queue-fail-') as temp:
    temp = Path(temp)
    (temp / 'test.rs').write_text(source)
    subprocess.run([os.environ.get('RUSTC', 'rustc'), '--edition=2021', '--test', '-Dwarnings',
                    str(temp / 'test.rs'), '-o', str(temp / 'test')], check=True)
    subprocess.run([str(temp / 'test')], check=True)
