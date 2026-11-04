# SPDX-License-Identifier: GPL-2.0-only
"""Small runtime management replies observed through owned Wi-Fi queues.

Command names reflect country/channel observations. Unknown flags, tags,
values and trailing areas retain their wire representation.
"""
import struct

INITIAL_PARAMETER_QUERY = 8
COUNTRY_SET = 9
COUNTRY_QUERY = 10
CHANNEL_LIST_QUERY = 47
OBSERVED_CHANNEL_SELECTOR = b'\x01'
ADDRESS_TEMPLATE_SET = 1
INTERFACE_ACTIVATE = 2
INTERFACE_DEACTIVATE = 3
INTERFACE_CHANNEL_LIST_QUERY = 19
INTERFACE_INFO_QUERY = 20
INTERFACE_INFO_HEADER = struct.Struct('<BB6s')
INTERFACE_INACTIVE = 0
INTERFACE_ACTIVE = 2


def address_template(address):
    """Encode the address applied to the next activated firmware interface.

    Experiment 174 verifies an alternate locally administered unicast address
    through deactivate/set/activate/info, then restores the ADT factory address.
    Setting the template alone does not change the current info descriptor.
    """
    address = bytes(address)
    if len(address) != 6 or address == bytes(6) or address[0] & 1:
        raise ValueError('Expected a nonzero six-byte unicast address')
    return address


def interface_operation_body(selector_byte, options_word):
    """Encode the five-byte activation/deactivation form without guessing its bits.

    Selector 0/word 0 and 0/word 11 both activate the observed interface; 0/word 0
    also deactivates it with command 3. An empty command 2 body faults Wi-Fi.
    The individual option bits and selector range semantics are unqualified,
    so both fields are explicit inputs rather than hidden initialization data.
    """
    if not 0 <= selector_byte <= 0xff or not 0 <= options_word <= 0xffffffff:
        raise ValueError('Interface operation fields exceed their wire widths')
    return struct.pack('<BI', selector_byte, options_word)


def decode_interface_info(body):
    """Decode the fixed selector/state/address prefix and bounded TLVs.

    Experiment 174 correlates state 0 with deactivation and state 2 with activation;
    the activated interface's address matches the supplied template. The selector
    byte and the following TLV values retain their wire representations.
    This is interface activity, not evidence of association or link readiness.
    """
    if len(body) < INTERFACE_INFO_HEADER.size:
        raise ValueError('Truncated interface information prefix')
    selector, state, address = INTERFACE_INFO_HEADER.unpack_from(body)
    return dict(selector_byte=selector, state=state, address=address,
                tlvs=decode_tlvs(body[INTERFACE_INFO_HEADER.size:]))


def country_setting(country_code, extra_byte):
    """Encode two country bytes and the observed extra byte.

    Target-70 validates US/1 and an immediate query readback, with channel
    count changing from 56 to 167. The extra byte's meaning is unassigned.
    """
    if len(country_code) != 2 or any(byte < 0x41 or byte > 0x5a for byte in country_code):
        raise ValueError('Country code must contain two uppercase ASCII bytes')
    if not 0 <= extra_byte <= 0xff:
        raise ValueError('Country setting extra byte exceeds its wire width')
    return country_code + bytes((extra_byte,))


def decode_tlvs(body):
    """Decode packed u16-tag/u16-length runtime fields without alignment.

    The complete 570-byte command-8 reply consists of eighteen such fields.
    Values are runtime data and remain uninterpreted; no pointer is followed.
    """
    records = []
    offset = 0
    while offset < len(body):
        if offset + 4 > len(body):
            raise ValueError('Truncated runtime TLV header')
        tag, size = struct.unpack_from('<HH', body, offset)
        offset += 4
        if offset + size > len(body):
            raise ValueError('Runtime TLV exceeds its completed message')
        records.append((tag, body[offset:offset + size]))
        offset += size
    return records


def decode_country_reply(body):
    """Return the observed two-byte country code and unassigned third byte."""
    if len(body) != 3:
        raise ValueError('Unknown country reply length')
    return dict(country_code=body[:2].decode('ascii'), extra_byte=body[2])


def decode_channel_list_reply(body):
    """Read the count and packed channel words, preserving the unused area.

    Bits above the low channel-number byte are not yet decoded. Captures
    contain 56 or 167 active entries and a zero-filled remaining area.
    """
    if not body:
        raise ValueError('Channel reply has no count')
    count = body[0]
    end = 1 + count * 4
    if end > len(body):
        raise ValueError('Channel count exceeds its completed message')
    return dict(count=count, channel_words=struct.unpack_from('<' + 'I' * count, body, 1),
                trailing=body[end:])
