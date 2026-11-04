# SPDX-License-Identifier: GPL-2.0-only
"""Generate the single antenna-SKU NVRAM store from platform metadata.

This is a nonexecutable host configuration format. The observed entry checksum
matches CRC32 over bytes after the24-byte header. Header words16/20 and entry
word12 retain their qualified single-variable profiles; individual meanings
are not assigned without probes. No captured record is an encoder input.
"""
import struct
import zlib

HEADER = struct.Struct('<6I')
ENTRY = struct.Struct('<4sIII')
MAGIC = 0x4e767653
HEADER_WORD16_PROFILE = 1
HEADER_WORD20_PROFILE = 1
ENTRY_WORD4_PROFILE = 0
ENTRY_WORD12_PROFILE = 2 << 16
ANTENNA_SKU_TAG = b'wsku'
MAX_ANTENNA_SKU_BYTES = 8


def make_antenna_nvram(sku):
    """Encode the platform's explicit ASCII antenna SKU, without a terminator."""
    if isinstance(sku, str):
        sku = sku.encode('ascii')
    if not isinstance(sku, bytes) or not 1 <= len(sku) <= MAX_ANTENNA_SKU_BYTES:
        raise ValueError('A nonempty platform antenna SKU of at most8 bytes is required')
    if any(value == 0 or value > 127 for value in sku):
        raise ValueError('Antenna SKU must be ASCII without embedded NUL')
    entry = ENTRY.pack(ANTENNA_SKU_TAG, ENTRY_WORD4_PROFILE, len(sku),
                       ENTRY_WORD12_PROFILE) + sku
    size = HEADER.size + len(entry)
    return HEADER.pack(MAGIC, zlib.crc32(entry), size, size,
                       HEADER_WORD16_PROFILE, HEADER_WORD20_PROFILE) + entry


def sku_from_dt_property(data):
    """Read the existing apple,antenna-sku string property's single terminator."""
    if not isinstance(data, bytes) or not data.endswith(b'\0'):
        raise ValueError('Expected a terminated DT antenna-SKU string')
    sku = data[:-1]
    make_antenna_nvram(sku)
    return sku
