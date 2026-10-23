# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Run the qualified source startup's actual build_graph against owned host RAM.

No target setup, device access, captures or Apple executable content is loaded.
"""
import ast
from pathlib import Path
import struct
import sys
from types import SimpleNamespace as NS


def build(m1n1,platform,entropy):
    m1n1=Path(m1n1)
    sys.path.insert(0,str(m1n1/'proxyclient'))
    from m1n1.g16c.startup import Bootstrap
    from m1n1.g16c.memory import AddressSpace,PAGE_SIZE
    from m1n1.g16c.arena import Arena
    from m1n1.g16c.firmware import Root,Main,bundle,hardware_regions,HardwareRegion
    from m1n1.g16c.abi_defaults import apply
    # Import only the existing independent RAM fixture's class definition;
    # never execute that experiment's tests or its setup code.
    source=m1n1/'proxyclient/experiments/agx_g16c_validate_private_lifetime.py'
    nodes=ast.parse(source.read_text()).body
    cls=next(n for n in nodes if isinstance(n,ast.ClassDef) and n.name=='RAM')
    env={'struct':struct}
    exec(compile(ast.Module(body=[cls],type_ignores=[]),str(source),'exec'),env)
    ram=env['RAM']();ram.data=bytearray(64*1024*1024)
    ram.ba=NS(phys_base=ram.base,mem_size_actual=len(ram.data))
    b=Bootstrap.__new__(Bootstrap)
    b.u=ram;b.p=ram;b.iface=ram;b.platform=platform;b.started=False
    b.firmware=AddressSpace(ram,0,ram.memalign(PAGE_SIZE,PAGE_SIZE),ram.memalign(PAGE_SIZE,PAGE_SIZE))
    b.client=AddressSpace(ram,0,ram.memalign(PAGE_SIZE,PAGE_SIZE),ram.memalign(PAGE_SIZE,PAGE_SIZE))
    b.arena=Arena(b.firmware,0xfffffc2000000000,0xfffffc218100c000);b.arena.next=b.arena.end
    b.client_arenas=[];b.asc=NS(work=lambda:None);b.progress=lambda *args:None
    b.heap_views=b.primary_status_policy=b.hardware_status_mmio=b.initial_control_policy=b.scheduler_regions=True
    b.native_opening_pool=False;b.single_user_opening=True
    b.root=Root.bootstrap(max_layout=True,primary_status_policy=True,hardware_status_mmio=True)
    b.main=Main.bootstrap(max_layout=True,heap_views=True)
    b.regions=(HardwareRegion(0x100,0x7001848000,0xfffffc20c4000000,0x800,0x40,0),
               HardwareRegion(0,0x7001870000,0xfffffc20c4028000,0x800,0x40,2))
    b.scheduler_state=b.main.regions[2]-0x8000
    image=bytearray(hardware_regions(apply(bundle(platform,entropy,max_layout=True,
        heap_views=True,power_policy=True),hardware_status_address=0xfffffc218100c000),b.regions))
    struct.pack_into('<Q',image,0x26b8,b.scheduler_state);b.image=bytes(image)
    b.region_c_image=platform.region_c(max_policy=True)
    Bootstrap.build_graph(b)
    return b,ram


def spans(b,ram):
    pages=[r for _,r in sorted(b.firmware.mappings.items()) if ram.base<=r.physical<ram.base+len(ram.data)]
    spans=[]
    for page in pages:
        if spans and page.address==spans[-1][0]+spans[-1][1] and page.attributes==spans[-1][2]:
            spans[-1][1]+=page.size
        else:spans.append([page.address,page.size,page.attributes])
    return spans


def aliases(b,ram):
    high=[r for r in b.firmware.mappings.values()
          if ram.base<=r.physical<ram.base+len(ram.data)]
    pairs=[]
    for low,r in sorted(b.client.mappings.items()):
        for offset in range(0,r.size,0x4000):
            physical=r.physical+offset
            owner=next(h for h in high if h.physical<=physical<h.physical+h.size)
            peer=owner.address+physical-owner.physical
            address=low+offset
            if pairs and address==pairs[-1][0]+pairs[-1][2] and peer==pairs[-1][1]+pairs[-1][2] and r.attributes==pairs[-1][3]:
                pairs[-1][2]+=0x4000
            else:pairs.append([address,peer,0x4000,r.attributes])
    return pairs
