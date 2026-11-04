# SPDX-License-Identifier: GPL-2.0-only
"""Extract opaque calibration blocks from the device's BWC3 factory record.

Only nonexecutable IMG4/FDR container metadata is parsed. This extracts data
from an explicit local factory file; it does not verify its IMG4 signature or
obtain another device's calibration. Calibration payloads remain opaque.
"""
from dataclasses import dataclass
import struct

MAX_FACTORY_BYTES = 32 * 1024 * 1024
MAX_DER_CHILDREN = 64
# Names are the little-endian FourCC identifiers in the FDR record. Their
# internal calibration fields are not interpreted by this container extractor.
CALIBRATION_ORDER = (b'W2CS', b'WFCS', b'BTCS')
SPAY = int.from_bytes(b'SPAY', 'big')
CALIBRATION_ALIGNMENT = 64


@dataclass(frozen=True)
class Element:
    tag_class: int
    constructed: bool
    number: int
    value: memoryview

    def matches(self, tag_class, constructed, number):
        return (self.tag_class, self.constructed, self.number) == (
            tag_class, constructed, number)


def elements(data):
    """Read one bounded DER child list, without following opaque OCTET values."""
    data = memoryview(data)
    cursor = 0
    result = []
    while cursor < len(data):
        if len(result) >= MAX_DER_CHILDREN:
            raise ValueError('Too many DER children')
        first = data[cursor]
        cursor += 1
        number = first & 31
        if number == 31:
            number = 0
            for index in range(5):
                if cursor >= len(data):
                    raise ValueError('Truncated DER tag')
                part = data[cursor]
                cursor += 1
                if index == 0 and part == 0x80:
                    raise ValueError('Nonminimal DER tag')
                number = (number << 7) | (part & 127)
                if not part & 128:
                    break
            else:
                raise ValueError('DER tag exceeds supported bound')
            if number < 31:
                raise ValueError('Nonminimal DER high tag')
        if cursor >= len(data):
            raise ValueError('Missing DER length')
        size = data[cursor]
        cursor += 1
        if size & 128:
            count = size & 127
            if not 1 <= count <= 4 or cursor + count > len(data):
                raise ValueError('Unsupported or truncated DER length')
            if data[cursor] == 0:
                raise ValueError('Nonminimal DER length')
            size = int.from_bytes(data[cursor:cursor + count], 'big')
            cursor += count
            if size < 128:
                raise ValueError('Nonminimal DER long length')
        if cursor + size > len(data):
            raise ValueError('DER element exceeds its parent')
        result.append(Element(first >> 6, bool(first & 32), number,
                              data[cursor:cursor + size]))
        cursor += size
    return result


def _sequence(data):
    outer = elements(data)
    if len(outer) != 1 or not outer[0].matches(0, True, 16):
        raise ValueError('Expected exactly one DER sequence')
    return elements(outer[0].value)


def _ia5(element, expected=None):
    if not element.matches(0, False, 22):
        raise ValueError('Expected an IA5 string')
    value = bytes(element.value)
    if any(byte > 127 for byte in value) or (expected is not None and value != expected):
        raise ValueError('Unexpected factory record string')
    return value


def factory_components(data):
    """Return named opaque BWC3 SPAY blocks, without reading their contents."""
    if not 0 < len(data) <= MAX_FACTORY_BYTES:
        raise ValueError('Factory record exceeds supported bounds')
    outer = _sequence(data)
    if len(outer) < 2:
        raise ValueError('Truncated IMG4 envelope')
    _ia5(outer[0], b'IMG4')
    if not outer[1].matches(0, True, 16):
        raise ValueError('Missing IMG4 payload sequence')
    payload = elements(outer[1].value)
    if len(payload) != 4:
        raise ValueError('Unsupported IM4P envelope')
    _ia5(payload[0], b'IM4P')
    _ia5(payload[1], b'BWC3')
    _ia5(payload[2])
    if not payload[3].matches(0, False, 4):
        raise ValueError('Missing BWC3 payload OCTET string')
    body = _sequence(payload[3].value)
    if len(body) < 3 or any(not item.matches(0, False, 2) for item in body[:2]):
        raise ValueError('Missing BWC3 integer header fields')
    sections = [item for item in body[2:] if item.matches(3, True, SPAY)]
    if len(sections) != 1:
        raise ValueError('Missing or duplicate BWC3 SPAY section')
    contents = elements(sections[0].value)
    if len(contents) != 1 or not contents[0].matches(0, True, 17):
        raise ValueError('Expected the BWC3 SPAY SET')
    result = {}
    for item in elements(contents[0].value):
        if not item.matches(0, True, 16):
            raise ValueError('Expected a factory component sequence')
        fields = elements(item.value)
        if (len(fields) != 3 or not fields[0].matches(0, False, 2)
                or len(fields[0].value) != 4 or fields[0].value[0] & 128
                or not fields[2].matches(0, False, 4)):
            raise ValueError('Unsupported factory component fields')
        # The DER integer contains the numeric little-endian FourCC value.
        name = int.from_bytes(fields[0].value, 'big').to_bytes(4, 'little')
        _ia5(fields[1], b'.')
        if name in result or not fields[2].value:
            raise ValueError('Duplicate or empty factory component')
        result[name] = bytes(fields[2].value)
    if set(result) != set(CALIBRATION_ORDER):
        raise ValueError('Unsupported calibration component set')
    return result


def extract_calibration(data):
    """Build the host calibration directory around unchanged factory blocks."""
    components = factory_components(data)
    header_bytes = CALIBRATION_ALIGNMENT * (1 + len(CALIBRATION_ORDER))
    size = header_bytes + sum(len(components[name]) for name in CALIBRATION_ORDER)
    if size > MAX_FACTORY_BYTES:
        raise ValueError('Calibration exceeds supported bounds')
    output = bytearray(size)
    struct.pack_into('<I', output, 0, len(CALIBRATION_ORDER))
    cursor = header_bytes
    for index, name in enumerate(CALIBRATION_ORDER):
        value = components[name]
        descriptor = CALIBRATION_ALIGNMENT * (index + 1)
        struct.pack_into('<II', output, descriptor + 8, cursor, len(value))
        output[cursor:cursor + len(value)] = value
        cursor += len(value)
    return bytes(output)
