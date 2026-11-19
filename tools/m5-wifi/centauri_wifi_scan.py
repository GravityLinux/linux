# SPDX-License-Identifier: GPL-2.0-only
"""Owned scan requests and standard radiotap/802.11 receive decoding.

Request fields come from controlled runtime experiments. Standard radio and
management frame definitions agree with include/net/ieee80211_radiotap.h and
include/linux/ieee80211.h in this Linux tree. No firmware code is inspected.
"""
import struct

SCAN_START = 0x1000f
SCAN_START_REPLY = 0x4001000f
SCAN_COMPLETE_EVENT = 0x80000005
SCAN_CHANNEL_VECTOR = 0x104
SCAN_REPORT_FRAMES = 1 << 31
SCAN_HEADER = struct.Struct('<I5H')
SCAN_COMPLETION = struct.Struct('<HHIB')


def scan_request(cookie, channels, *, report_frames, option_words, timing_words):
    """Generate a scan without captured SSID/options records.

    The low cookie byte is echoed in completion and receive reports. Bit 31
    enables observed queue-4 frame delivery: isolated tests of the other
    observed option words did not deliver frames. Remaining words are explicit
    because their individual semantics are not fully qualified. Changing both
    first timing words from 20 to 50 changes elapsed time from 26 to 57.
    Channel words are explicit numeric inputs. The country query also carries
    unqualified metadata bits; its raw table is not directly a valid scan list.
    """
    channels = tuple(channels)
    option_words, timing_words = tuple(option_words), tuple(timing_words)
    words = option_words + timing_words
    if not 0 <= cookie <= 0xff or len(option_words) != 2 or len(timing_words) != 3:
        raise ValueError('Expected an eight-bit cookie, two option and three timing words')
    if any(not 0 <= word <= 0xffff for word in words):
        raise ValueError('Scan header words exceed their wire widths')
    if not 1 <= len(channels) <= 255 or any(not 0 <= word <= 0xffffffff for word in channels):
        raise ValueError('Expected one to 255 numeric channel words')
    header = SCAN_HEADER.pack(cookie | (SCAN_REPORT_FRAMES if report_frames else 0), *words)
    vector = bytes((len(channels),)) + struct.pack('<' + 'I' * len(channels), *channels)
    return header + struct.pack('<HH', SCAN_CHANNEL_VECTOR, len(vector)) + vector


def decode_scan_complete(body):
    """Decode status, delivered frame count, observed milliseconds and cookie."""
    if len(body) != SCAN_COMPLETION.size:
        raise ValueError('Unknown scan completion extent')
    status, frames, elapsed, cookie = SCAN_COMPLETION.unpack(body)
    return dict(status=status, frames=frames, elapsed_ms=elapsed, cookie=cookie)


def _decode_radio_frame(body):
    """Locate a bounded 802.11 frame and validate standard radiotap flags."""
    if len(body) < 8:
        raise ValueError('Truncated radiotap header')
    version, pad, header_bytes, present = struct.unpack_from('<BBHI', body)
    if version != 0 or not 8 <= header_bytes <= len(body):
        raise ValueError('Unsupported radiotap version or extent')
    offset, extension = 8, present
    while extension & (1 << 31):
        if offset + 4 > header_bytes:
            raise ValueError('Truncated radiotap presence extension')
        extension = struct.unpack_from('<I', body, offset)[0]
        offset += 4
    result = dict(radiotap_bytes=header_bytes, radiotap_present=present)
    flags = 0
    # Alignment and extent for standard presence bits TSFT through antenna noise.
    for bit, alignment, size in ((0, 8, 8), (1, 1, 1), (2, 1, 1), (3, 2, 4),
                                 (4, 2, 2), (5, 1, 1), (6, 1, 1)):
        if not present & (1 << bit):
            continue
        offset = (offset + alignment - 1) & -alignment
        if offset + size > header_bytes:
            raise ValueError('Radiotap field exceeds its header')
        if bit == 1:
            flags = body[offset]
        elif bit == 2:
            result['rate_500kbps'] = body[offset]
        elif bit == 3:
            frequency, channel_flags = struct.unpack_from('<HH', body, offset)
            result.update(frequency_mhz=frequency, channel_flags=channel_flags)
        elif bit in (5, 6):
            result['signal_dbm' if bit == 5 else 'noise_dbm'] = struct.unpack_from('<b', body, offset)[0]
        offset += size
    frame = body[header_bytes:]
    if flags & 0x40:
        raise ValueError('Radio marked the frame with a bad FCS')
    if flags & 0x10:
        if len(frame) < 4:
            raise ValueError('Truncated frame FCS')
        frame = frame[:-4]
    return result, frame


def decode_scan_receive(body):
    """Decode a radiotap-wrapped beacon/probe response, with bounded IEs."""
    result, frame = _decode_radio_frame(body)
    if len(frame) < 36:
        raise ValueError('Truncated beacon/probe response')
    frame_control = struct.unpack_from('<H', frame)[0]
    if frame_control & 0xff not in (0x50, 0x80):
        raise ValueError('Receive report is not a beacon or probe response')
    timestamp, interval, capabilities = struct.unpack_from('<QHH', frame, 24)
    result.update(kind='probe_response' if frame_control & 0xf0 == 0x50 else 'beacon',
                  frame_control=frame_control, receiver=frame[4:10], transmitter=frame[10:16],
                  bssid=frame[16:22], timestamp=timestamp, beacon_interval=interval,
                  capabilities=capabilities, information_elements=[])
    offset = 36
    while offset < len(frame):
        if offset + 2 > len(frame):
            raise ValueError('Truncated information element header')
        tag, size = frame[offset:offset + 2]
        offset += 2
        if offset + size > len(frame):
            raise ValueError('Information element exceeds the received frame')
        value = frame[offset:offset + size]
        result['information_elements'].append((tag, value))
        if tag == 0:
            if size > 32 or 'ssid' in result:
                raise ValueError('Invalid or repeated SSID information element')
            result['ssid'] = value
        elif tag == 3 and size == 1:
            result['channel'] = value[0]
        offset += size
    return result


SAE_AUTHENTICATION_ALGORITHM = 3
SAE_COMMIT_TRANSACTION = 1
SAE_HASH_TO_ELEMENT_STATUS = 126


def decode_sae_authentication(body, *, station, bssid):
    """Return public SAE authentication metadata for one explicitly selected peer.

    Fresh277/278 expose these frames through queue4 during firmware SAE offload.
    Standard radiotap length locates the frame. Commit status126 identifies H2E;
    status0 identifies hunting-and-pecking. No commit scalar, element, token,
    confirm value or credential is returned. A successful connection outcome
    remains separate from this metadata observation.
    """
    station, bssid = bytes(station), bytes(bssid)
    if len(station) != 6 or len(bssid) != 6 or station == bssid:
        raise ValueError('Expected distinct six-byte station and peer addresses')
    radio, frame = _decode_radio_frame(body)
    if len(frame) < 30:
        raise ValueError('Truncated authentication frame header')
    control, sequence_control = struct.unpack_from('<H', frame)[0], struct.unpack_from('<H', frame, 22)[0]
    if control & 0xff != 0xb0 or control & 0x4400 or sequence_control & 15:
        raise ValueError('Expected an unfragmented, unprotected authentication frame')
    receiver, transmitter, peer = frame[4:10], frame[10:16], frame[16:22]
    if (receiver, transmitter) not in ((station, bssid), (bssid, station)) or peer != bssid:
        raise ValueError('Authentication addresses do not match the selected peer')
    algorithm, transaction, status = struct.unpack_from('<HHH', frame, 24)
    if algorithm != SAE_AUTHENTICATION_ALGORITHM or transaction not in (1, 2):
        raise ValueError('Expected an SAE commit or confirm transaction')
    result = dict(radiotap_bytes=radio['radiotap_bytes'],
                  station_transmits=transmitter == station, transaction=transaction, status=status)
    if transaction == SAE_COMMIT_TRANSACTION and status in (0, SAE_HASH_TO_ELEMENT_STATUS):
        if len(frame) < 32:
            raise ValueError('SAE commit lacks its group identifier')
        result['group'] = struct.unpack_from('<H', frame, 30)[0]
        result['method'] = 'hash-to-element' if status == SAE_HASH_TO_ELEMENT_STATUS else 'hunt-and-peck'
    return result
