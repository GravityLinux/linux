#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Export unchanged source-owned render and mixed-batch GEMs; no Apple client pages."""
import argparse,json,hashlib,struct
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('source',type=Path);p.add_argument('output',type=Path);p.add_argument('--mixed',action='store_true');a=p.parse_args()
m=json.loads((a.source/'provenance.json').read_text())
if a.mixed:
 assert m['own_mesa_resources'] and m['apple_binary_pages']==0 and m['render_commands']==m['compute_commands']==1
 hashes=m['files']
else:
 assert m['own_Mesa_client_images']and m['opaque_Apple_client_pages']==0 and m['commands_per_ioctl']==1
 hashes=m['sha256']
a.output.mkdir(parents=True,exist_ok=False)
for name,digest in hashes.items():
 b=(a.source/name).read_bytes();assert hashlib.sha256(b).hexdigest()==digest,name;(a.output/name).write_bytes(b)
h=struct.unpack_from('<8I',(a.output/'inputs.bin').read_bytes());assert h[0]==0x3242434d and ((a.mixed and h[7]==4096)or(not a.mixed and h[4]==280 and h[7]==4))
(a.output/'provenance.json').write_text(json.dumps(dict(source=str(a.source),source_manifest_sha256=hashlib.sha256((a.source/'provenance.json').read_bytes()).hexdigest(),own_Mesa_client_images=True,Apple_client_pages=0,all_objects_and_commands_unchanged=True),indent=2)+'\n')
print('Exported unchanged source-only', 'mixed render/resolve' if a.mixed else '1x1 red-render', 'fixture')
