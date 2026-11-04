# SPDX-License-Identifier: GPL-2.0-only
"""Build nonexecutable AP-policy TLVs from decoded platform metadata.

The29-byte policy block is independently matched to the M5 ADT. Record2's
one-byte profile and record4's boot-varying four-byte value retain neutral names.
Supplying a new record4 value is a controlled experiment, not a claim about
its meaning or an already qualified replacement for external policy inputs.
"""
import struct

POLICY_BYTES = 512
POLICY_TLV = 1
PROFILE_TLV = 2
BOOT_VARYING_TLV = 4
END_TLV = 255
PROFILE_VALUE = 1


def tlv(kind, payload):
    if not isinstance(payload, bytes) or not 0 <= kind <= 255 or len(payload) > 253:
        raise ValueError('Invalid bounded policy TLV')
    return bytes((kind, len(payload) + 2)) + payload


def make_ap_policy(properties, *, record4):
    """Encode decoded DT cell values and an explicit experimental record4 value."""
    if not isinstance(record4, bytes) or len(record4) != 4:
        raise ValueError('An explicit four-byte record4 value is required')
    body = bytearray()
    for table, threshold in (
        ('wifi_power_bo_2g_table', 'wifi_power_bo_2g_temp_threshold'),
        ('wifi_power_bo_5g_table', 'wifi_power_bo_5g_temp_threshold'),
        ('wifi_duty_cycle_table', 'wifi_duty_cycle_temp_threshold'),
    ):
        values = tuple(properties[table])
        temperature = properties[threshold]
        if (len(values) != 6 or any(type(v) is not int or not 0 <= v <= 255 for v in values)
                or type(temperature) is not int or not 0 <= temperature <= 255):
            raise ValueError('Policy table/threshold exceeds its qualified byte geometry')
        body += bytes(values) + bytes((temperature,))
    for name in ('wifi_volt_lo_threshold', 'wifi_volt_hi_threshold'):
        value = properties[name]
        if type(value) is not int or not 0 <= value <= 0xffffffff:
            raise ValueError('Voltage threshold must preserve its raw32-bit representation')
        body += struct.pack('<I', value)
    encoded = (tlv(POLICY_TLV, bytes(body)) + tlv(PROFILE_TLV, bytes((PROFILE_VALUE,)))
               + tlv(BOOT_VARYING_TLV, record4) + tlv(END_TLV, b''))
    return encoded + bytes(POLICY_BYTES - len(encoded))
