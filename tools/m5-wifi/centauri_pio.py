# SPDX-License-Identifier: GPL-2.0-only
"""Recognize only the bounded configuration-write shape observed on T8142.

This is a runtime DMA-engine command descriptor, not a firmware file format.
Unknown commands return None and must not authorize a source-buffer read.
"""
import struct
from dataclasses import dataclass

PIO_COMMAND_PREFIX = struct.Struct('<4I')
PIO_CONFIG_WRITE_FLAGS = 0x20c007
PIO_CONFIG_WRITE_LENGTH = 4
# These six exact tokens were correlated with PCI configuration readbacks.
# Their wider bus-address encoding and other destinations remain unresolved.
BOOT_BUFFER_DESTINATIONS = {
    0xcb0100000 | (function << 12) | offset: (function, offset)
    for function in (1, 2) for offset in (0xf88, 0xf8c, 0xf90)
}


@dataclass(frozen=True)
class ConfigWrite:
    source_iova: int
    function: int
    offset: int
    length: int = PIO_CONFIG_WRITE_LENGTH


def decode_boot_buffer_config_write(prefix):
    """Identify a four-byte boot-buffer register write; never follow its source.

    A caller still needs the observed stream-17 mapping and DRAM bounds before
    reading the one configuration word. This decoder permits no firmware or
    arbitrary packet source. Unobserved bits and destination tokens are rejected.
    """
    if len(prefix) != PIO_COMMAND_PREFIX.size:
        raise ValueError('PIO command prefix must contain exactly sixteen bytes')
    word0, word1, word2, length = PIO_COMMAND_PREFIX.unpack(prefix)
    if (word0 & 0xffffff != PIO_CONFIG_WRITE_FLAGS or length != PIO_CONFIG_WRITE_LENGTH
            or word1 & 0x0f000000):
        return None
    destination = (word2 << 4) | (word1 >> 28)
    target = BOOT_BUFFER_DESTINATIONS.get(destination)
    if target is None:
        return None
    source = (word0 >> 24) | ((word1 & 0xffffff) << 8)
    if not source or source & 3 or source + length > 1 << 32:
        return None
    return ConfigWrite(source, *target)


def encode_boot_buffer_config_write(function, offset, source_iova):
    """Encode only the configuration-write form exercised on bare target-54.

    The source names a mapped four-byte host configuration value, not a
    firmware payload. This helper neither maps memory nor submits a command.
    """
    targets = {(fn, register): token for token, (fn, register) in BOOT_BUFFER_DESTINATIONS.items()}
    token = targets.get((function, offset))
    if token is None:
        raise ValueError('Destination is not an observed boot-buffer register')
    if not 0 < source_iova <= (1 << 32) - 4 or source_iova & 3:
        raise ValueError('Source must be an aligned four-byte IOVA below 4 GiB')
    return PIO_COMMAND_PREFIX.pack(
        (source_iova & 255) << 24 | PIO_CONFIG_WRITE_FLAGS,
        (token & 15) << 28 | (source_iova >> 8), token >> 4, PIO_CONFIG_WRITE_LENGTH)
