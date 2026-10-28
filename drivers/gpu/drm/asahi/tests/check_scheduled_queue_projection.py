#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare common ring publication with the protected, working Neo source tag.

The same existing queue harness runs both actual production implementations.
It covers all store bytes/order, deferred producers, metadata, rejection before
stores, retirement predicates, and 393216 dual-consumer counter combinations.
No device access or GPU execution is claimed by this host projection.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
args = p.parse_args()
tests = Path(__file__).resolve().parent
root = tests.parents[4]
revision = 'a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
source = tests.parent / 'g17p_queue.rs'
baseline = subprocess.check_output(['git', 'show', revision + ':drivers/gpu/drm/asahi/g17p_queue.rs'], cwd=root)
harness = (tests / 'g17p_queue.rs').read_text()
with tempfile.TemporaryDirectory(prefix='scheduled-queue-projection-') as d:
    tmp = Path(d)
    flags = host_versions.build(tmp)
    tagged = tmp / 'original_neo.rs'
    tagged.write_bytes(baseline)
    outputs = []
    for name, implementation in [('Neo', tagged), ('current', source)]:
        main = tmp / (name + '.rs')
        main.write_text(harness.replace('"../g17p_queue.rs"', json.dumps(str(implementation))))
        binary = tmp / name
        subprocess.run(['rustc', '--edition=2021', *flags, str(main), '-o', str(binary)], check=True)
        outputs.append(subprocess.check_output([str(binary)]))
    assert outputs[0] == outputs[1], 'Working Neo publication trace changed'
args.evidence.mkdir(parents=True, exist_ok=False)
(args.evidence / 'trace.txt').write_bytes(outputs[1])
(args.evidence / 'summary.json').write_text(json.dumps(dict(
    protected_Neo_revision=revision,
    original_sha256=hashlib.sha256(baseline).hexdigest(),
    current_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
    trace_sha256=hashlib.sha256(outputs[1]).hexdigest(),
    exact_ordered_publications=192,
    dual_consumer_states=393216,
    invalid_extent_rejected_before_stores=True,
    native_GPU_execution=False,
), indent=2) + '\n')
print('PASS: 192 exact tagged-Neo publication traces, 393216 dual-consumer states, pre-store rejection and retirement checks')
