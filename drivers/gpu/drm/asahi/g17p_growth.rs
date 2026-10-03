// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-only Neo TVB request/reply and limit-report identities.
//! These decoders never dereference addresses supplied by firmware.

pub(crate) const PAGE: u64 = 0x4000;
pub(crate) const BLOCK: u64 = 0x20000;
pub(crate) const UNIT: u64 = 0x8000;
pub(crate) const CONTEXT_BASE: u64 = 0x1000000000;
pub(crate) const GROWTH_BASE: u64 = 0x1002000000;
pub(crate) const INCREMENT: usize = 10;
pub(crate) const REQUEST_LIMIT: u32 = 32;
pub(crate) const GROWTH_END: u64 = GROWTH_BASE + REQUEST_LIMIT as u64 * INCREMENT as u64 * 0x28000;

/// Source G17PFirstRender's global and independently overridable pool bounds.
pub(crate) fn source_pool_limits(global: u32, overrides: [u32; 2]) -> Option<[u32; 2]> {
    fn valid(value: u32) -> bool { value == 0 || (8..=2048).contains(&value) }
    if !valid(global) { return None; }
    let values = overrides.map(|value| if value == u32::MAX { global } else { value });
    if !values.into_iter().all(valid) { return None; }
    Some(values.map(|value| if value == 0 { 2048 } else { value }))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub(crate) vm: u32,
    pub(crate) pool: u32,
}
impl Owner {
    pub(crate) fn valid(self) -> bool {
        (self.vm == 1 && self.pool < super::g17p_render_lifecycle::POOL_SLOTS)
            || (self.vm == 2 && self.pool == 1)
    }
    pub(crate) fn request(self, body: &[u8], counter: u32) -> bool {
        if !self.valid() || body.len() != 0x48 {
            return false;
        }
        let scheduling = (u32_at(body, 16), u64_at(body, 64));
        u32_at(body, 0) == 6
            && u32_at(body, 4) == self.vm
            && u32_at(body, 8) == self.pool
            && u32_at(body, 12) == counter
            && (scheduling == (0, 1) || super::g17p_render_lifecycle::pool_grids(self.pool)
                .is_ok_and(|grids| scheduling == (grids[0], 1u64 << grids[0])))
            && body[20..56].iter().all(|b| *b == 0)
            && u64_at(body, 56) == 1
    }
    pub(crate) fn reply(self, body: &[u8], counter: u32, success: bool) -> Option<[u8; 64]> {
        if !self.request(body, counter) {
            return None;
        }
        let mut out = [0; 64];
        for (i, value) in [8, u32::from(success), self.pool, self.vm, counter]
            .into_iter()
            .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        Some(out)
    }
    pub(crate) fn limit(self, body: &[u8], work: &[u64], fragment: u64, event: u32) -> Option<u64> {
        if !self.valid()
            || body.len() != 0x48
            || !(1..=2).contains(&work.len())
            || work.contains(&0)
            || fragment == 0
        {
            return None;
        }
        (u32_at(body, 0) == 7
            && u32_at(body, 4) == 0
            && u32_at(body, 8) == 1
            && u32_at(body, 12) == event
            && work.contains(&u64_at(body, 16))
            && u64_at(body, 24) != 0
            && u64_at(body, 32) == 0
            && u64_at(body, 40) == fragment
            && u64_at(body, 48) == event as u64
            && u64_at(body, 56) == self.vm as u64
            && u64_at(body, 64) == self.pool as u64)
            .then(|| u64_at(body, 24))
    }
}

pub(crate) fn fatal(body: &[u8]) -> bool {
    body.len() == 0x48 && u32_at(body, 0) == 4 && body[4..].iter().all(|v| *v == 0)
}

pub(crate) fn block_id(va: u64) -> Option<u64> {
    if va < CONTEXT_BASE || va % UNIT != 0 || va.checked_add(BLOCK)? > 1 << 42 {
        return None;
    }
    let id = (va - CONTEXT_BASE) / UNIT;
    (id <= u32::MAX as u64).then_some(id)
}

/// Validate the complete retained+new list before allocating or publishing.
pub(crate) fn block_list(old: &[u64], new: &[u64]) -> bool {
    if new.is_empty()
        || old
            .len()
            .checked_add(new.len())
            .is_none_or(|n| n > u32::MAX as usize)
    {
        return false;
    }
    let blocks = old.iter().chain(new.iter());
    for (i, &a) in blocks.clone().enumerate() {
        if block_id(a).is_none() {
            return false;
        }
        for &b in blocks.clone().take(i) {
            if a < b + BLOCK && b < a + BLOCK {
                return false;
            }
        }
    }
    true
}

pub(crate) fn block_addresses(counter: u32) -> Option<[u64; INCREMENT]> {
    if counter >= REQUEST_LIMIT {
        return None;
    }
    Some(core::array::from_fn(|i| {
        GROWTH_BASE + (counter as u64 * INCREMENT as u64 + i as u64) * 0x28000
    }))
}
