# SPDX-License-Identifier: GPL-2.0-only
"""PCIe PIO-DMA configuration descriptors measured with generated buffers.

This is host-controller transport, separate from Centauri's firmware IPC.
Owned122/123 requests verify the read/write profiles and address packing.
Control words are enumerated profiles; their individual flag bits are not
assigned unsupported meanings. No Apple executable contents are inspected.
"""
import struct


ADDRESS_BITS = 36
ADDRESS_LIMIT = 1 << ADDRESS_BITS
ADDRESS_MASK = ADDRESS_LIMIT - 1
CONFIG_READ = 0x04c007
CONFIG_WRITE = 0x20c007
DESCRIPTOR = struct.Struct('<QQ')


def _encode(source, destination, size, control):
    if size not in (1, 2, 4):
        raise ValueError('Configuration transfers must be one, two or four bytes')
    if not 0 <= source < ADDRESS_LIMIT or source + size > ADDRESS_LIMIT:
        raise ValueError('Source exceeds the measured 36-bit address field')
    if not 0 <= destination < ADDRESS_LIMIT or destination + size > ADDRESS_LIMIT:
        raise ValueError('Destination exceeds the measured 36-bit address field')
    first = (source << 24) | ((destination & 15) << 60) | control
    second = (destination >> 4) | (size << 32)
    return DESCRIPTOR.pack(first, second)


def config_read(config_address, result_iova, size=4):
    """Encode a read using the controller's selected configuration aperture.

    The caller validates the full address against its ADT ECAM resource.
    The descriptor carries its low36 bits; this profile supplies the aperture
    selection. This function does not validate/guess a host's MMIO resources.
    """
    if size not in (1, 2, 4) or config_address < 0 or config_address % size:
        raise ValueError('Invalid or unaligned configuration address')
    return _encode(config_address & ADDRESS_MASK, result_iova, size, CONFIG_READ)


def config_write(data_iova, config_address, size=4):
    """Encode a scalar configuration write from caller-owned DMA memory."""
    if size not in (1, 2, 4) or config_address < 0 or config_address % size:
        raise ValueError('Invalid or unaligned configuration address')
    return _encode(data_iova, config_address & ADDRESS_MASK, size, CONFIG_WRITE)


def decode_config_descriptor(data):
    """Decode only the two qualified configuration profiles; no pointer reads."""
    if len(data) != DESCRIPTOR.size:
        raise ValueError('Configuration descriptor must be exactly16 bytes')
    first, second = DESCRIPTOR.unpack(data)
    control = first & 0xffffff
    if control not in (CONFIG_READ, CONFIG_WRITE):
        raise ValueError('Unqualified PIO-DMA descriptor profile')
    source = (first >> 24) & ADDRESS_MASK
    destination = ((second & 0xffffffff) << 4) | (first >> 60)
    size = second >> 32
    # Apply the same bounded scalar-transfer contract when consuming a trace.
    _encode(source, destination, size, control)
    return dict(direction='read' if control == CONFIG_READ else 'write',
                source=source, destination=destination, size=size)
