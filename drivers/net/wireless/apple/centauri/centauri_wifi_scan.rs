// SPDX-License-Identifier: GPL-2.0-only

//! Generated scan commands and bounded standard radiotap/802.11 decoding.
//! Wire fields were qualified by the independent Python scan prototype.

use kernel::prelude::*;

pub(crate) const START: u32 = 0x1000f;
pub(crate) const START_REPLY: u32 = 0x4001000f;
pub(crate) const COMPLETE: u32 = 0x80000005;
pub(crate) const MAX_CHANNELS: usize = 11;
pub(crate) const MAX_REQUEST: usize = 19 + MAX_CHANNELS * 4;
const REPORT_FRAMES: u32 = 1 << 31;
const CHANNEL_VECTOR: u16 = 0x104;

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}
fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

pub(crate) fn request(
    cookie: u8,
    channels: &[u32],
    output: &mut [u8; MAX_REQUEST],
) -> Result<usize> {
    // Only plain 2.4GHz channel words are qualified here. Country-table
    // words contain additional unassigned metadata and are not a scan list.
    if channels.is_empty()
        || channels.len() > MAX_CHANNELS
        || channels.iter().any(|channel| !(1..=11).contains(channel))
    {
        return Err(EINVAL);
    }
    output.fill(0);
    output[..4].copy_from_slice(&(REPORT_FRAMES | u32::from(cookie)).to_le_bytes());
    // Two option words remain zero. The following timing profile was tested
    // at50/50/110; changing the first pair changes observed scan duration.
    for (offset, value) in [(8, 50u16), (10, 50), (12, 110)] {
        output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    output[14..16].copy_from_slice(&CHANNEL_VECTOR.to_le_bytes());
    output[16..18].copy_from_slice(&(1 + channels.len() as u16 * 4).to_le_bytes());
    output[18] = channels.len() as u8;
    for (index, channel) in channels.iter().enumerate() {
        output[19 + index * 4..23 + index * 4].copy_from_slice(&channel.to_le_bytes());
    }
    Ok(19 + channels.len() * 4)
}

#[derive(Clone, Copy)]
pub(crate) struct Completion {
    pub(crate) status: u16,
    pub(crate) frames: u16,
    pub(crate) elapsed_ms: u32,
    pub(crate) cookie: u8,
}

pub(crate) fn completion(body: &[u8]) -> Result<Completion> {
    if body.len() != 9 {
        return Err(EIO);
    }
    Ok(Completion {
        status: u16_at(body, 0),
        frames: u16_at(body, 2),
        elapsed_ms: u32_at(body, 4),
        cookie: body[8],
    })
}

pub(crate) struct Frame<'a> {
    pub(crate) frequency: u16,
    pub(crate) signal_dbm: i8,
    pub(crate) bytes: &'a [u8],
}

/// Reject malformed RF reports locally; a bad beacon must not stop DMA service.
struct RadioFrame<'a> {
    frequency: Option<u16>,
    signal_dbm: Option<i8>,
    bytes: &'a [u8],
}

fn radio_frame(body: &[u8]) -> Result<RadioFrame<'_>> {
    if body.len() < 8 || body[0] != 0 {
        return Err(EINVAL);
    }
    let header = usize::from(u16_at(body, 2));
    if header < 8 || header > body.len() {
        return Err(EINVAL);
    }
    let present = u32_at(body, 4);
    let mut extension = present;
    let mut offset = 8;
    while extension & (1 << 31) != 0 {
        if offset + 4 > header {
            return Err(EINVAL);
        }
        extension = u32_at(body, offset);
        offset += 4;
    }
    let mut flags = 0;
    let mut frequency = None;
    let mut signal = None;
    // Standard radiotap fields TSFT through antenna noise, with alignment
    // relative to the radiotap start. Later vendor fields stay opaque.
    for (bit, alignment, size) in [
        (0, 8, 8),
        (1, 1, 1),
        (2, 1, 1),
        (3, 2, 4),
        (4, 2, 2),
        (5, 1, 1),
        (6, 1, 1),
    ] {
        if present & (1 << bit) == 0 {
            continue;
        }
        offset = (offset + alignment - 1) & !(alignment - 1);
        if offset + size > header {
            return Err(EINVAL);
        }
        match bit {
            1 => flags = body[offset],
            3 => frequency = Some(u16_at(body, offset)),
            5 => signal = Some(body[offset] as i8),
            _ => (),
        }
        offset += size;
    }
    if flags & 0x40 != 0 {
        return Err(EINVAL); // Standard radiotap BADFCS.
    }
    let mut bytes = &body[header..];
    if flags & 0x10 != 0 {
        bytes = bytes
            .get(..bytes.len().checked_sub(4).ok_or(EINVAL)?)
            .ok_or(EINVAL)?;
    }
    Ok(RadioFrame {
        frequency,
        signal_dbm: signal,
        bytes,
    })
}

pub(crate) fn frame(body: &[u8]) -> Result<Frame<'_>> {
    let decoded = radio_frame(body)?;
    let bytes = decoded.bytes;
    if bytes.len() < 36 || !matches!(u16_at(bytes, 0) & 0xff, 0x50 | 0x80) {
        return Err(EINVAL);
    }
    let mut offset = 36;
    let mut ssid_seen = false;
    while offset < bytes.len() {
        if bytes.len() - offset < 2 {
            return Err(EINVAL);
        }
        let tag = bytes[offset];
        let length = usize::from(bytes[offset + 1]);
        if tag == 0 {
            if ssid_seen || length > 32 {
                return Err(EINVAL);
            }
            ssid_seen = true;
        }
        offset += 2 + length;
        if offset > bytes.len() {
            return Err(EINVAL);
        }
    }
    Ok(Frame {
        frequency: decoded.frequency.ok_or(EINVAL)?,
        signal_dbm: decoded.signal_dbm.ok_or(EINVAL)?,
        bytes,
    })
}

pub(crate) struct SaeAuthentication {
    pub(crate) transaction: u16,
    pub(crate) status: u16,
    pub(crate) group: Option<u16>,
}

/// Decode only public SAE metadata for the owned station and selected peer.
pub(crate) fn sae_authentication(
    body: &[u8],
    station: &[u8; 6],
    bssid: &[u8; 6],
) -> Result<SaeAuthentication> {
    let decoded = radio_frame(body)?;
    let frame = decoded.bytes;
    if frame.len() < 30 || station == bssid {
        return Err(EINVAL);
    }
    let control = u16_at(frame, 0);
    if control & 0xff != 0xb0 || control & 0x4400 != 0 || u16_at(frame, 22) & 15 != 0 {
        return Err(EINVAL);
    }
    let receiver = &frame[4..10];
    let transmitter = &frame[10..16];
    if !((receiver == station && transmitter == bssid)
        || (receiver == bssid && transmitter == station))
        || &frame[16..22] != bssid
    {
        return Err(EINVAL);
    }
    let transaction = u16_at(frame, 26);
    let status = u16_at(frame, 28);
    if u16_at(frame, 24) != 3 || !matches!(transaction, 1 | 2) {
        return Err(EINVAL);
    }
    let group = if transaction == 1 && matches!(status, 0 | 126) {
        if frame.len() < 32 {
            return Err(EINVAL);
        }
        Some(u16_at(frame, 30))
    } else {
        None
    };
    Ok(SaeAuthentication {
        transaction,
        status,
        group,
    })
}
