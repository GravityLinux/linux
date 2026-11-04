# SPDX-License-Identifier: GPL-2.0-only
"""Generated Ethernet queues in the current owned Wi-Fi context.

The minimal profile registers one default transmit queue and one Ethernet
receive route. Every ring/buffer is freshly allocated; no registry, former
driver mapping or receive descriptor snapshot is an input. Profile selectors
are qualified runtime values; their unused bit meanings remain unassigned.
"""
import struct
import time

from centauri_ethernet import (EthernetQueues, RX_DESCRIPTOR, TX_DESCRIPTOR,
    COMPLETION, tx_descriptor, TX_NOTIFY, RX_NOTIFY, PACKET_CAPACITY,
    HOST_SUBMISSION_INDICES, DEVICE_SUBMISSION_INDICES,
    HOST_COMPLETION_INDICES, DEVICE_COMPLETION_INDICES)
from centauri_regs import WIFI_DMA_WINDOW_BASE_LOW, WIFI_DMA_WINDOW_BASE_HIGH, WIFI_DMA_WINDOW_LENGTH

TX_QUEUE = 25
TX_COMPLETION_QUEUE = 76
TX_COMPLETION_CURSOR = 192
RX_QUEUE = 12
RX_COMPLETION_QUEUE = 15
RX_COMPLETION_CURSOR = 207
SUBMISSION_ENTRIES = 2048
TX_COMPLETION_ENTRIES = 8192
RX_COMPLETION_ENTRIES = 4096
EXTENDED_ENTRY_FORMAT = 4 << 16
RX_PROFILE_WORD = 0x0228000b
TX_PROFILE_WORD = 0x00080012
TX_BUFFER_SLOTS = 64
TX_BUFFER_STRIDE = 4096
DMA_ALIGNMENT = 0x4000


def extended_completion_config(queue, cursor, iova, *, entries, source, profile, service, route_word=0):
    if not 0 <= queue < 234 or not 0 <= cursor < 234 or not 0 < entries <= 0xffff:
        raise ValueError('Extended completion geometry exceeds the context')
    if not 0 <= source <= 0xffff or not 0 < iova < 1 << 64:
        raise ValueError('Extended completion source/address exceeds wire width')
    return struct.pack('<13IH', EXTENDED_ENTRY_FORMAT | 2, cursor << 16 | queue,
        iova & 0xffffffff, iova >> 32, source << 16 | entries, profile, 0, service,
        0, 0, 0, route_word, 0, 0)


def extended_submission_config(queue, iova, *, receive, completion_queue, service):
    if not 0 <= queue < 192 or not 0 <= completion_queue < 234 or not 0 < iova < 1 << 64:
        raise ValueError('Extended submission geometry exceeds the context')
    return struct.pack('<15IH', 1 if receive else EXTENDED_ENTRY_FORMAT | 1,
        queue << 16 | queue, iova & 0xffffffff, iova >> 32, 0xffffffff, 0xffffffff,
        completion_queue << 16 | SUBMISSION_ENTRIES,
        RX_PROFILE_WORD if receive else TX_PROFILE_WORD, 0, service, 0, 0, 0, 0, 0, 0)


class OwnedEthernetQueues(EthernetQueues):
    """Create new data rings without changing the active firmware or domain.

    ``host`` must be the live RuntimeHost owner. IOVA is a caller allocation
    choice inside its already published window, not a chip address constant.
    The host owner must outlive this object; its close stops all DMA.
    """

    def __init__(self, host, station_mac, *, iova=0x0a000000):
        if host.closed or len(station_mac) != 6 or not any(station_mac) or station_mac[0] & 1:
            raise ValueError('A live runtime owner and unicast address are required')
        self.owner, self.control = host, host.wifi
        self.u, self.p, self.mac = host.u, host.p, bytes(station_mac)
        self.context = self.control.iova
        self.control._require_ready()
        self.window = self.u.read(self.control.bank + WIFI_DMA_WINDOW_BASE_LOW,32) | self.u.read(self.control.bank + WIFI_DMA_WINDOW_BASE_HIGH,32) << 32
        self.window_end = self.window + self.u.read(self.control.bank + WIFI_DMA_WINDOW_LENGTH,32)
        self.tx_queue, self.tx_cursor_index = TX_QUEUE, TX_COMPLETION_CURSOR
        for array, numbers in ((DEVICE_SUBMISSION_INDICES,(TX_QUEUE,RX_QUEUE)),
                               (HOST_SUBMISSION_INDICES,(TX_QUEUE,RX_QUEUE)),
                               (DEVICE_COMPLETION_INDICES,(TX_COMPLETION_CURSOR,RX_COMPLETION_CURSOR)),
                               (HOST_COMPLETION_INDICES,(TX_COMPLETION_CURSOR,RX_COMPLETION_CURSOR))):
            if any(self.index(array,number) for number in numbers):
                raise RuntimeError('Create Ethernet queues only on unused owned indices')
        offset=0
        regions={}
        def reserve(name,size):
            nonlocal offset
            offset=(offset+DMA_ALIGNMENT-1)&-DMA_ALIGNMENT
            regions[name]=(offset,size)
            offset+=size
        reserve('tx_completion',TX_COMPLETION_ENTRIES*COMPLETION.size)
        reserve('tx_submission',SUBMISSION_ENTRIES*TX_DESCRIPTOR.size)
        reserve('rx_completion',RX_COMPLETION_ENTRIES*COMPLETION.size)
        reserve('rx_submission',SUBMISSION_ENTRIES*RX_DESCRIPTOR.size)
        reserve('rx_buffers',SUBMISSION_ENTRIES*PACKET_CAPACITY)
        reserve('tx_buffers',TX_BUFFER_SLOTS*TX_BUFFER_STRIDE)
        size=(offset+DMA_ALIGNMENT-1)&-DMA_ALIGNMENT
        if iova & (DMA_ALIGNMENT-1) or iova<self.window or iova+size>self.window_end:
            raise ValueError('Ethernet arena must fit the existing aligned DMA window')
        host.dart.invalidate_cache()
        if host.dart.iotranslate(2,iova,size)!=[(None,size)]:
            raise ValueError('Ethernet arena overlaps an active stream-2 mapping')
        physical=host.pages.memalign(DMA_ALIGNMENT,size)
        host.dart.iomap_at(2,iova,physical,size)
        host._flush_domain(host.dart,2)
        self.control.dart.invalidate_cache()
        if self.control.dart.iotranslate(2,iova,size)!=[(physical,size)]:
            raise RuntimeError('Ethernet arena does not map to its owned allocation')
        self.arena=dict(iova=iova,physical=physical,bytes=size,regions=regions)
        self.tx_completion=iova+regions['tx_completion'][0]
        self.tx_ring=iova+regions['tx_submission'][0]
        self.tx_buffer_iova=iova+regions['tx_buffers'][0]
        self.tx_buffer_physical=physical+regions['tx_buffers'][0]
        rx_ring=iova+regions['rx_submission'][0]
        rx_buffers=iova+regions['rx_buffers'][0]
        self.rx_cqs={RX_COMPLETION_QUEUE:dict(ring=iova+regions['rx_completion'][0],
            entries=RX_COMPLETION_ENTRIES,cursor_index=RX_COMPLETION_CURSOR,source=RX_QUEUE,ethernet=True)}
        self.rx_rings={RX_QUEUE:rx_ring}
        records={cookie:RX_DESCRIPTOR.pack(PACKET_CAPACITY << 8 | 3,
            rx_buffers+cookie*PACKET_CAPACITY,cookie) for cookie in range(SUBMISSION_ENTRIES)}
        self.rx_descriptors={RX_QUEUE:records}
        self.rx_seen,self.tx_seen=0,[]
        self.recycle_enabled=True
        self.rx_recycled={12:0,13:0}
        self.rx_reposted_inflight={12:{},13:{}}
        self.rx_repost_completions=[]
        self.write(rx_ring,b''.join(records.values()))
        frames=[
            extended_completion_config(TX_COMPLETION_QUEUE,TX_COMPLETION_CURSOR,self.tx_completion,
                entries=TX_COMPLETION_ENTRIES,source=0xffff,profile=0x12,service=3),
            extended_submission_config(TX_QUEUE,self.tx_ring,receive=False,completion_queue=TX_COMPLETION_QUEUE,service=3),
            extended_completion_config(RX_COMPLETION_QUEUE,RX_COMPLETION_CURSOR,self.rx_cqs[RX_COMPLETION_QUEUE]['ring'],
                entries=RX_COMPLETION_ENTRIES,source=RX_QUEUE,profile=0xb,service=8),
            extended_submission_config(RX_QUEUE,rx_ring,receive=True,completion_queue=RX_QUEUE,service=8),
        ]
        self.configurations=[]
        for frame in frames:
            result=self.control.submit(frame)
            self.configurations.append(result)
            if any(result['completion_words'][2:]):
                raise RuntimeError('Extended queue registration returned nonzero metadata')
        self.set_index(HOST_SUBMISSION_INDICES,RX_QUEUE,SUBMISSION_ENTRIES-1)
        self.u.write(self.control.bank+RX_NOTIFY,1,32)

    def send(self,frame,timeout=5):
        """Transmit from an owned pool distinct from the basic control rings."""
        self.control._require_ready()
        self.poll_tx()
        slot=self.index(HOST_SUBMISSION_INDICES,self.tx_queue)
        consumed=self.index(DEVICE_SUBMISSION_INDICES,self.tx_queue)
        if slot!=consumed or slot>=SUBMISSION_ENTRIES:
            raise RuntimeError('Previous owned data transmit is still outstanding')
        offset=slot%TX_BUFFER_SLOTS*TX_BUFFER_STRIDE
        descriptor=tx_descriptor(len(frame),self.tx_buffer_iova+offset,slot)
        address=self.tx_buffer_physical+offset
        self.u.iface.writemem(address,frame)
        self.p.dc_cvac(address,len(frame))
        self.write(self.tx_ring+slot*TX_DESCRIPTOR.size,descriptor)
        self.set_index(HOST_SUBMISSION_INDICES,self.tx_queue,(slot+1)%SUBMISSION_ENTRIES)
        self.u.write(self.control.bank+TX_NOTIFY,1,32)
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            for completion in self.poll_tx():
                if completion['source']==self.tx_queue and completion['cookie']==slot:
                    return completion
            time.sleep(.005)
        raise TimeoutError('Owned transmit completion did not arrive')
