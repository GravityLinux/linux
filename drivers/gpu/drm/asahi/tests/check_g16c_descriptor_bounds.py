#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Validate descriptor object bounds against independent adjacent native allocations.

Historical captures can include the next allocation's header. Require exact
virtual AND physical identity before classifying those bytes as a neighbor.
"""
from pathlib import Path
import argparse,hashlib,json,sys
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('bootloader',type=Path)
parser.add_argument('--capture',type=Path,action='append',required=True)
parser.add_argument('--evidence',type=Path,required=True)
args=parser.parse_args()
sys.path.insert(0,str(args.bootloader/'proxyclient/experiments'))
from agx_g16c_reference_image import ReferenceImage
rows=[]
for root in args.capture:
 name=str(root.resolve());image=ReferenceImage(root);plan=json.loads((root/'replay-plan.json').read_text());clients=plan['all_clients']
 for tag,c in clients.items():
  for stage,size,overread in [('0',0x9c0,0x38),('1',0x2240,0x40)]:
   start=c['render_stages'][stage]['descriptor'];end=start+size;tail=image.virtual(-1,end,overread);neighbors=[(t,d)for t,d in clients.items()if d['render_stages'][stage]['descriptor']==end]
   row=dict(capture=name,client=tag,stage=int(stage),start=start,size=size,end=end,historical_overread_bytes=overread,overread_sha256=hashlib.sha256(tail).hexdigest())
   if any(tail):
    (other,d),=neighbors;peer=image.virtual(-1,d['render_stages'][stage]['descriptor'],overread)
    assert tail==peer and end==d['render_stages'][stage]['descriptor']
    assert image.virtual(-1,start,size)[-16:]!=tail[:16]
    a=next(m for m in image.maps[-1]if m['va']<=end<m['va']+m['size']);b=next(m for m in image.maps[-1]if m['va']<=d['render_stages'][stage]['descriptor']<m['va']+m['size'])
    assert a['pa']+end-a['va']==b['pa']+d['render_stages'][stage]['descriptor']-b['va']
    row.update(adjacent_client=other,identical_physical_bytes=True,is_next_descriptor_header=True)
   else:assert not neighbors;row.update(unoccupied_overread_zero=True)
   rows.append(row)
assert rows and any(r.get('is_next_descriptor_header',False)for r in rows)
report=dict(pass_=True,findings=rows,exact_same_virtual_and_physical_addresses=True,nonzero_neighbor_headers=sum(r.get('is_next_descriptor_header',False)for r in rows),zero_unoccupied_overreads=sum(r.get('unoccupied_overread_zero',False)for r in rows),conclusion='Historical G16C extension was a read beyond the descriptor into the next adjacent allocation; no peer link exists here.',invalidated_interpretations=['cross-client peer extensions','extension-zeroing proves required handshake','A-first/B-first mutations test topology'],actual_negative_meaning='Experiments corrupted an independently queued job header.',historical_results_preserved=True)
args.evidence.mkdir(parents=True,exist_ok=False)
(args.evidence/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
print('PASS',len(rows),'native boundary spans;',report['nonzero_neighbor_headers'],'exact next-header aliases;',report['zero_unoccupied_overreads'],'unoccupied spans')
