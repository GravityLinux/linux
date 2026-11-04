# SPDX-License-Identifier: GPL-2.0-only
"""Centauri FTAB directory metadata; executable payloads remain opaque.

The native host's 592-byte runtime-allocation initializer matches the vendor
container's complete `2ftb` directory entry. Its nested directory describes
region offsets and capacities, including the total host allocation extent.
No firmware instructions, embedded strings or executable formats are decoded.
"""
from dataclasses import dataclass
import struct

MAGIC = b'rkosftab'
HEADER = struct.Struct('<8I8sII')
ENTRY = struct.Struct('<4sIII')
BOOT_LAYOUT_TAG = b'2ftb'
BOOT_LAYOUT_KIND = 0x42
MAX_BOOT_ALLOCATION = 32 * 1024 * 1024
DEVICE_INPUT_TAGS = (b'nvrm', b'_cal', b'_apb', b'_bpb')
REQUIRED_DEVICE_INPUT_TAGS = frozenset(DEVICE_INPUT_TAGS[:3])
IMAGE_ALIGNMENT = 4096


@dataclass(frozen=True)
class Entry:
    tag: bytes
    offset: int
    size: int
    flags: int


class Directory:
    def __init__(self, data, *, layout=False):
        if len(data) < HEADER.size:
            raise ValueError('Truncated FTAB header')
        *words, magic, count, reserved = HEADER.unpack_from(data)
        if magic != MAGIC or words[1] != 1:
            raise ValueError('Unsupported FTAB directory format')
        self.table_end = HEADER.size + count * ENTRY.size
        if not 0 < count <= 4096 or self.table_end > len(data):
            raise ValueError('FTAB directory exceeds its input')
        if layout and words[2] != BOOT_LAYOUT_KIND:
            raise ValueError('Entry is not the qualified runtime allocation layout')
        self.header_words = tuple(words)
        self.reserved = reserved
        self.entries = tuple(Entry(*ENTRY.unpack_from(data, HEADER.size + n * ENTRY.size))
                             for n in range(count))
        if len({e.tag for e in self.entries}) != count:
            raise ValueError('Ambiguous duplicate FTAB directory tag')
        self.data = data
        for entry in self.entries:
            if entry.size and entry.offset < self.table_end:
                raise ValueError('FTAB region overlaps its directory')
            if not layout and entry.offset + entry.size > len(data):
                raise ValueError('FTAB payload exceeds its declared input')

    def entry(self, tag):
        for entry in self.entries:
            if entry.tag == tag:
                return entry
        raise ValueError(f'Missing required FTAB tag {tag!r}')

    def opaque_entry(self, tag):
        entry = self.entry(tag)
        if entry.offset + entry.size > len(self.data):
            raise ValueError('A layout region has no input payload')
        return self.data[entry.offset:entry.offset + entry.size]


def make_boot_allocation(firmware):
    """Generate the runtime host allocation from the firmware's `2ftb` entry.

    Preserve the vendor directory exactly and zero all described regions.
    Tags, capacities, placement and total size come from the normal firmware
    resource, not a recorded host allocation or device-specific constant.
    """
    template = Directory(firmware).opaque_entry(BOOT_LAYOUT_TAG)
    layout = Directory(template, layout=True)
    size = max(len(template), max(e.offset + e.size for e in layout.entries))
    if size > MAX_BOOT_ALLOCATION:
        raise ValueError('FTAB runtime allocation exceeds the bounded prototype limit')
    allocation = bytearray(size)
    allocation[:len(template)] = template
    return bytes(allocation)


def prepare_image(vendor, manifest, device_inputs):
    """Package unchanged vendor payloads with explicit external device inputs.

    The manifest occupies the space immediately after the enlarged directory;
    its offset/length are header words 4/5. Vendor component offsets move by
    that insertion's size. Named host inputs follow the unchanged vendor body,
    and the transfer image ends at a 4 KiB boundary. These relationships come
    from container metadata, not executable inspection.

    This low-level container builder does not obtain or generate signatures or
    device records. Callers must provide them explicitly; their provenance remains a
    separate requirement. No recorded host allocation or firmware state is
    consumed by this packaging operation.
    """
    directory = Directory(vendor)
    if directory.header_words[2] or directory.header_words[4:6] != (0, 0):
        raise ValueError('Expected an unprepared vendor image')
    supplied = set(device_inputs)
    if (not manifest or not REQUIRED_DEVICE_INPUT_TAGS <= supplied or
            supplied - set(DEVICE_INPUT_TAGS)):
        raise ValueError('A manifest and the required named device inputs are needed')
    # Explicit-reset170 qualifies control queues and a Bluetooth connection
    # without the optional _bpb record. Full Wi-Fi data acceptance is pending.
    selected_tags = tuple(tag for tag in DEVICE_INPUT_TAGS if tag in supplied)
    if len(directory.entries) + len(selected_tags) > 4096:
        raise ValueError('Prepared directory exceeds the supported entry count')
    vendor_tags = {e.tag for e in directory.entries}
    if any(tag in vendor_tags for tag in DEVICE_INPUT_TAGS):
        raise ValueError('Vendor directory already contains device inputs')
    if any(not device_inputs[tag] for tag in selected_tags):
        raise ValueError('Device inputs must not be empty')
    new_table_end = directory.table_end + len(selected_tags) * ENTRY.size
    delta = new_table_end - directory.table_end + len(manifest)
    unpadded_size = len(vendor) + delta + sum(len(device_inputs[t]) for t in selected_tags)
    size = (unpadded_size + IMAGE_ALIGNMENT - 1) & -IMAGE_ALIGNMENT
    if size > MAX_BOOT_ALLOCATION:
        raise ValueError('Prepared image exceeds the bounded transfer limit')
    words = list(directory.header_words)
    words[4:6] = new_table_end, len(manifest)
    image = bytearray(size)
    HEADER.pack_into(image, 0, *words, MAGIC,
                     len(directory.entries) + len(selected_tags), directory.reserved)
    entries = [Entry(e.tag, e.offset + delta, e.size, e.flags) for e in directory.entries]
    image[new_table_end:new_table_end + len(manifest)] = manifest
    body_start = directory.table_end + delta
    image[body_start:len(vendor) + delta] = vendor[directory.table_end:]
    cursor = len(vendor) + delta
    for tag in selected_tags:
        data = device_inputs[tag]
        entries.append(Entry(tag, cursor, len(data), 0))
        image[cursor:cursor + len(data)] = data
        cursor += len(data)
    for index, entry in enumerate(entries):
        ENTRY.pack_into(image, HEADER.size + index * ENTRY.size,
                        entry.tag, entry.offset, entry.size, entry.flags)
    return bytes(image)
