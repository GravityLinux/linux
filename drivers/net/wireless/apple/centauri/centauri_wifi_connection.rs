// SPDX-License-Identifier: GPL-2.0-only

//! WPA2-PMK and SAE/default-PWE connection wire formats qualified in Python.
//! Remaining profile fields are explicit observed values, not inferred flags.

use core::ptr;
use kernel::prelude::*;

pub(crate) const START: u32 = 0x20000;
pub(crate) const START_REPLY: u32 = 0x40020000;
pub(crate) const COMPLETE: u32 = 0x80000000;
pub(crate) const DISCONNECT: u32 = 0x20001;
pub(crate) const DISCONNECT_REPLY: u32 = 0x40020001;
pub(crate) const DISCONNECT_COMPLETE: u32 = 0x80000004;
//252: eight zero bytes work without the native optional148 TLV or scalar8.
// Four bytes are rejected; the two words' individual roles remain unassigned.
pub(crate) const DISCONNECT_BODY: [u8; 8] = [0; 8];
pub(crate) const RESULT_BYTES: usize = 90;
const PREFIX_BYTES: usize = 159;
const SSID_LIST: u16 = 0x156;
const RSN: u16 = 0x30;
const KEY_OFFSET: usize = 76;
const KEY_BYTES: usize = 32;
const PASSWORD_BYTES: usize = 63;
const PASSWORD: u16 = 0x139;
const RSNXE: u16 = 244;
const RSNXE_SAE_H2E: u8 = 1 << 5;
const MAX_REQUEST: usize = PREFIX_BYTES + 4 + 3 + 32 + 4 + 255 + 6 + PASSWORD_BYTES + 5;
pub(crate) const SECURITY_PSK: u32 = 0;
pub(crate) const SECURITY_SAE: u32 = 1;

/// Host-owned inputs copied by the cfg80211 adapter; never persist or log.
#[repr(C)]
pub(crate) struct Parameters {
    pub(crate) ssid: [u8; 32],
    pub(crate) ssid_bytes: u32,
    pub(crate) bssid: [u8; 6],
    pub(crate) channel: u16,
    pub(crate) pmk: [u8; KEY_BYTES],
    pub(crate) rsn: [u8; 255],
    pub(crate) rsn_bytes: u32,
    pub(crate) security: u32,
    pub(crate) password: [u8; PASSWORD_BYTES],
    pub(crate) password_bytes: u32,
    pub(crate) h2e_only: u32,
}

pub(crate) fn rsnxe_h2e(capabilities: &[u8]) -> bool {
    capabilities.first().is_some_and(|byte| {
        capabilities.len() == usize::from(byte & 15) + 1 && byte & RSNXE_SAE_H2E != 0
    })
}

/// # Safety
/// C retains the bounded RSNXE data under its BSS RCU read lock for this call.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_rsnxe_h2e(
    capabilities: *const u8,
    bytes: usize,
) -> bool {
    if capabilities.is_null() || bytes == 0 || bytes > 16 {
        return false;
    }
    // SAFETY: The caller pins this immutable, length-checked IE body.
    rsnxe_h2e(unsafe { core::slice::from_raw_parts(capabilities, bytes) })
}

pub(crate) struct Request {
    bytes: [u8; MAX_REQUEST],
    length: usize,
}

impl Request {
    pub(crate) fn new(input: &Parameters) -> Result<Self> {
        let ssid_bytes = input.ssid_bytes as usize;
        let rsn_bytes = input.rsn_bytes as usize;
        let password_bytes = input.password_bytes as usize;
        if !(1..=32).contains(&ssid_bytes)
            || !(1..=255).contains(&rsn_bytes)
            || input.bssid.iter().all(|byte| *byte == 0)
            || input.bssid[0] & 1 != 0
            || !(1..=11).contains(&input.channel)
            || input.h2e_only > 1
        {
            return Err(EINVAL);
        }
        match input.security {
            SECURITY_PSK if password_bytes == 0 && input.h2e_only == 0 => (),
            SECURITY_SAE
                if (1..=PASSWORD_BYTES).contains(&password_bytes)
                    && input.pmk.iter().all(|byte| *byte == 0) =>
            {
                ()
            }
            _ => return Err(EINVAL),
        }
        let mut request = Self {
            bytes: [0; MAX_REQUEST],
            length: 0,
        };
        let data = &mut request.bytes;
        data[0] = 1; // Qualified leading profile byte; role unassigned.
        data[1..7].copy_from_slice(&input.bssid);
        data[7..9].copy_from_slice(&input.channel.to_le_bytes());
        // Open-system/PSK4 or SAE3/AKM8; CCMP4 and zero flags for both.
        // Firmware PSK4 differs from IEEE RSN's PSK suite type2.
        let (auth, akm) = if input.security == SECURITY_SAE {
            (3, 8)
        } else {
            (0, 4)
        };
        data[70..76].copy_from_slice(&[auth, akm, 4, 0, KEY_BYTES as u8, 0]);
        if input.security == SECURITY_PSK {
            data[KEY_OFFSET..KEY_OFFSET + KEY_BYTES].copy_from_slice(&input.pmk);
        }
        data[132..134].copy_from_slice(&1u16.to_le_bytes());
        for (index, value) in [2000u32, 6000, 6000, 60000, 300].iter().enumerate() {
            data[139 + index * 4..143 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut offset = PREFIX_BYTES;
        data[offset..offset + 2].copy_from_slice(&SSID_LIST.to_le_bytes());
        data[offset + 2..offset + 4].copy_from_slice(&(3 + ssid_bytes as u16).to_le_bytes());
        data[offset + 4..offset + 7].copy_from_slice(&[1, 4, ssid_bytes as u8]);
        data[offset + 7..offset + 7 + ssid_bytes].copy_from_slice(&input.ssid[..ssid_bytes]);
        offset += 7 + ssid_bytes;
        data[offset..offset + 2].copy_from_slice(&RSN.to_le_bytes());
        data[offset + 2..offset + 4].copy_from_slice(&(rsn_bytes as u16).to_le_bytes());
        data[offset + 4..offset + 4 + rsn_bytes].copy_from_slice(&input.rsn[..rsn_bytes]);
        offset += 4 + rsn_bytes;
        if input.security == SECURITY_SAE {
            data[offset..offset + 2].copy_from_slice(&PASSWORD.to_le_bytes());
            data[offset + 2..offset + 4]
                .copy_from_slice(&(password_bytes as u16 + 2).to_le_bytes());
            data[offset + 4..offset + 6].copy_from_slice(&(password_bytes as u16).to_le_bytes());
            data[offset + 6..offset + 6 + password_bytes]
                .copy_from_slice(&input.password[..password_bytes]);
            offset += 6 + password_bytes;
            if input.h2e_only != 0 {
                data[offset..offset + 2].copy_from_slice(&RSNXE.to_le_bytes());
                data[offset + 2..offset + 4].copy_from_slice(&1u16.to_le_bytes());
                data[offset + 4] = RSNXE_SAE_H2E;
                offset += 5;
            }
        }
        request.length = offset;
        Ok(request)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        for byte in &mut self.bytes {
            // SAFETY: This private request contains a PMK or password. Volatile stores
            // prevent the compiler from discarding the final local wipe.
            unsafe { ptr::write_volatile(byte, 0) };
        }
    }
}

pub(crate) struct Outcome<'a> {
    pub(crate) ssid: &'a [u8],
    pub(crate) bssid: &'a [u8],
    pub(crate) status: u16,
}

/// Decode only the44-byte public prefix; the remaining46 bytes stay opaque.
pub(crate) fn outcome(prefix: &[u8; 44]) -> Result<Outcome<'_>> {
    let bytes = u32::from_le_bytes(prefix[32..36].try_into().unwrap()) as usize;
    if bytes > 32 {
        return Err(EIO);
    }
    Ok(Outcome {
        ssid: &prefix[..bytes],
        bssid: &prefix[36..42],
        status: u16::from_le_bytes(prefix[42..44].try_into().unwrap()),
    })
}
