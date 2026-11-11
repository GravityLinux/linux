#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host tests of the G16 progress-based drain policy (g16_drain.rs).

Compiles the production module with drain-progress.rs. rustc must be on PATH;
no device or kernel build is needed.
"""
from pathlib import Path
import os
import subprocess
import tempfile

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_drain.rs').exists())
source = f'''#![allow(dead_code)]
#[path = "{root / 'drivers/gpu/drm/asahi/g16_drain.rs'}"] mod g16_drain;
''' + here.with_name('drain-progress.rs').read_text()

with tempfile.TemporaryDirectory(prefix='asahi-drain-') as temp:
    temp = Path(temp)
    (temp / 'test.rs').write_text(source)
    subprocess.run([os.environ.get('RUSTC', 'rustc'), '--edition=2021', '--test', '-Dwarnings',
                    str(temp / 'test.rs'), '-o', str(temp / 'test')], check=True)
    subprocess.run([str(temp / 'test')], check=True)
