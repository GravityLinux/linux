# SPDX-License-Identifier: GPL-2.0-only
"""Qualified pieces of the runtime connection format, not a complete driver.

Native181 and owned182 identify the security boundary and the fixed public
connection result. Remaining request fields and connection setup are still
under investigation; an acknowledgment does not establish a working link.
"""
import struct
from dataclasses import dataclass

CONNECTION_START = 0x20000
CONNECTION_START_REPLY = 0x40020000
CONNECTION_COMPLETE_EVENT = 0x80000000
DISCONNECT_START = 0x20001
DISCONNECT_START_REPLY = 0x40020001
DISCONNECT_COMPLETE_EVENT = 0x80000004
CONNECTION_PREFIX_BYTES = 159
SECURITY_PARAMETERS_OFFSET = 70
SECURITY_PARAMETERS = struct.Struct('<BBBBH')
KEY_MATERIAL_OFFSET = SECURITY_PARAMETERS_OFFSET + SECURITY_PARAMETERS.size
QUALIFIED_KEY_BYTES = 32
CONNECTION_RESULT_BYTES = 90
PUBLIC_BSS_PREFIX = struct.Struct('<32sI6sH')
AUTH_OPEN = 0
AUTH_SAE = 3
AKM_SELECTOR_PSK = 4
AKM_SELECTOR_SAE = 8
CIPHER_SELECTOR_CCMP = 4
SSID_LIST_TAG = 0x156
RSN_TAG = 0x30
PASSPHRASE_TAG = 0x139
RSNXE_TAG = 244
RSNXE_SAE_H2E = 1 << 5


@dataclass(frozen=True)
class ConnectionParameters:
    """Explicit remaining request fields, without inferred defaults.

    Their precise roles still need qualification. Owned186 verifies leading
    byte 1, word132=1, SSID flags4 and timing words2000/6000/6000/60000/300.
    No opaque native connection record supplies this encoder's fields.
    """
    leading_byte: int
    word132: int
    ssid_flags: int
    timing_words: tuple


def psk_connection_request(ssid, bssid, channel_word, pmk, *, parameters, rsn_ie, passphrase=None):
    """Build a single-BSS PSK/CCMP request from explicit host inputs.

    The optional passphrase field can be omitted for controlled PMK-only
    qualification. This encoder alone does not prove firmware key handling.
    The result contains key material; do not save or log it.
    """
    ssid, bssid, rsn_ie = bytes(ssid), bytes(bssid), bytes(rsn_ie)
    words = tuple(parameters.timing_words)
    if not 1 <= len(ssid) <= 32 or len(bssid) != 6 or not any(bssid) or bssid[0] & 1:
        raise ValueError('Expected a nonempty SSID and unicast BSSID')
    if not 0 <= channel_word <= 0xffff or len(words) != 5:
        raise ValueError('Expected a channel word and five timing words')
    if (not 0 <= parameters.leading_byte <= 255 or not 0 <= parameters.ssid_flags <= 255 or
            not 0 <= parameters.word132 <= 0xffff or any(not 0 <= word <= 0xffffffff for word in words)):
        raise ValueError('Connection parameter exceeds its wire width')
    if not 0 < len(rsn_ie) <= 255:
        raise ValueError('Expected a bounded negotiated RSN information element')
    prefix = bytearray(CONNECTION_PREFIX_BYTES)
    prefix[0] = parameters.leading_byte
    prefix[1:7] = bssid
    struct.pack_into('<H', prefix, 7, channel_word)
    struct.pack_into('<H', prefix, 132, parameters.word132)
    struct.pack_into('<5I', prefix, 139, *words)
    prefix = set_connection_security(prefix, auth_selector=AUTH_OPEN,
        akm_selector=AKM_SELECTOR_PSK, cipher_selector=CIPHER_SELECTOR_CCMP, flags=0, key=pmk)
    def field(tag, value):
        return struct.pack('<HH', tag, len(value)) + value
    result = prefix + field(SSID_LIST_TAG, bytes((1, parameters.ssid_flags, len(ssid))) + ssid)
    result += field(RSN_TAG, rsn_ie)
    if passphrase is not None:
        passphrase = bytes(passphrase)
        if not 8 <= len(passphrase) <= 63:
            raise ValueError('Expected a WPA2 passphrase of eight to 63 bytes')
        result += field(PASSPHRASE_TAG, struct.pack('<H', len(passphrase)) + passphrase)
    return result


def set_connection_security(prefix, *, auth_selector, akm_selector, cipher_selector, flags, key):
    """Replace the qualified security fields without shifting adjacent data.

    Selectors are explicit: native SAE uses 3/8/4/0 and native WPA2-PSK uses
    0/4/4/0. The firmware PSK selector 4 differs from RSN's AKM suite type 2;
    do not pass RSN type numbers directly. Remaining setup is not implied.
    Only a 32-byte key area is qualified. Do not save or log returned prefixes.
    """
    if len(prefix) != CONNECTION_PREFIX_BYTES or len(key) != QUALIFIED_KEY_BYTES:
        raise ValueError('Expected the qualified connection prefix and 32-byte key')
    if any(not 0 <= value <= 255 for value in (auth_selector, akm_selector, cipher_selector, flags)):
        raise ValueError('Security selector exceeds its wire width')
    result = bytearray(prefix)
    SECURITY_PARAMETERS.pack_into(result, SECURITY_PARAMETERS_OFFSET,
                                  auth_selector, akm_selector, cipher_selector, flags, len(key))
    result[KEY_MATERIAL_OFFSET:KEY_MATERIAL_OFFSET + len(key)] = key
    return bytes(result)


def rsnxe_supports_h2e(capabilities):
    """Validate the standard RSNXE capability extent before checking SAE H2E."""
    return (bool(capabilities) and len(capabilities) == (capabilities[0] & 15) + 1
            and bool(capabilities[0] & RSNXE_SAE_H2E))


def sae_connection_request(ssid, bssid, channel_word, password, *, parameters, rsn_ie,
                           h2e_only=False):
    """Encode the independent SAE/default-PWE path qualified on Pixel_5254.

    The firmware performs SAE and derives the PMK. Experiments269/270 use a
    zero-filled32-byte key slot and the length-prefixed password TLV139, with
    no captured native extension records.270 also obtains a fresh DHCP lease.
    H2E-only callers validate the selected peer with rsnxe_supports_h2e and
    include the standard explicit H2E capability. Queue4 authentication
    metadata independently checks the resulting method; encoding is not
    evidence of authentication success. The request contains a password;
    never save or log it.
    """
    password = bytes(password)
    if not 1 <= len(password) <= 63:
        raise ValueError('Expected1..63 SAE password bytes for the qualified profile')
    body = bytearray(psk_connection_request(ssid, bssid, channel_word, bytes(32),
        parameters=parameters, rsn_ie=rsn_ie))
    body[:CONNECTION_PREFIX_BYTES] = set_connection_security(body[:CONNECTION_PREFIX_BYTES],
        auth_selector=AUTH_SAE, akm_selector=AKM_SELECTOR_SAE,
        cipher_selector=CIPHER_SELECTOR_CCMP, flags=0, key=bytes(32))
    body += struct.pack('<HHH', PASSPHRASE_TAG, len(password) + 2, len(password)) + password
    if h2e_only:
        # The caller must select an H2E-capable peer. Explicitly supply the
        # standard negotiated capability; no unassigned vendor mode bits.
        body += struct.pack('<HHB', RSNXE_TAG, 1, RSNXE_SAE_H2E)
    return body


def decode_connection_complete(body):
    """Decode the observed SSID/BSSID/status prefix of a 90-byte result.

    Native success has status 0; owned failed joins report 1. Other status
    values and the remaining bytes retain their protocol representation.
    No address or embedded pointer from the result is followed.
    """
    if len(body) != CONNECTION_RESULT_BYTES:
        raise ValueError('Unknown connection result extent')
    ssid_area, ssid_length, bssid, status = PUBLIC_BSS_PREFIX.unpack_from(body)
    if ssid_length > len(ssid_area):
        raise ValueError('SSID length exceeds its fixed result area')
    return dict(ssid=ssid_area[:ssid_length], bssid=bssid, status=status,
                remaining=body[PUBLIC_BSS_PREFIX.size:])


def disconnect_request():
    """Generate the qualified eight-byte default disconnect header.

    Trial252 accepts values8/3/0 in the low word with the high word0; four
    bytes are rejected with01530000. Zero works without the optional148 TLV
    seen in native requests. The two words' individual roles remain unassigned;
    do not interpret them as an IEEE reason without independent RF evidence.
    The empty completion event follows a distinct acknowledgment, with the
    same management transaction. Only then can interface deactivation proceed.
    """
    return bytes(8)
