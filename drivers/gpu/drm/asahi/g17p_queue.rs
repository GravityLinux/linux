// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Queue serializers and synchronous publication ordering from g17p.py and
//! G17PSubmitter.stage. Caller owns storage, cache maintenance and notification.
//! Deferred outer producers stay invisible until the startup/runtime boundary.

// CL_2 is entry 8 in the TA/3D/CL channel table. Its mailbox selector is
// separately encoded as 0x0a; it is not an index into that table.
pub(crate) const COMPUTE_CHANNEL: usize = 8;
pub(crate) const COMPUTE_DOORBELL: u64 = 0x008300000000000a;
pub(crate) const POINTER_DONE: u64 = 0;
pub(crate) const POINTER_READ: u64 = 0x30;
pub(crate) const POINTER_WRITE: u64 = 0x40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Invalid,
    Full,
    Overflow,
}
type Result<T> = core::result::Result<T, Error>;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Tiling = 0,
    Fragment = 1,
    Compute = 2,
}

fn put32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) struct Record {
    pub(crate) pointers: u64,
    pub(crate) ring: u64,
    pub(crate) job_list: u64,
    pub(crate) context: u64,
    pub(crate) uuid: u32,
    pub(crate) priority: u32,
    pub(crate) prio5: u32,
    pub(crate) unk_2c: u32,
    pub(crate) unk_38: u32,
    pub(crate) unk_30: Option<u64>,
    pub(crate) unk_94: u32,
    pub(crate) sentinel_size: usize,
}
impl Record {
    pub(crate) fn build(&self) -> Result<[u8; 0xc0]> {
        if self.sentinel_size > 6 {
            return Err(Error::Invalid);
        }
        let mut out = [0; 0xc0];
        for (offset, value) in [
            (0, self.pointers),
            (8, self.ring),
            (0x10, self.job_list),
            (0x9c, self.context),
        ] {
            put64(&mut out, offset, value);
        }
        if let Some(value) = self.unk_30 {
            put64(&mut out, 0x30, value);
        } else {
            out[0x38 - self.sentinel_size..0x38].fill(0xff);
        }
        for (offset, value) in [
            (0x24, u32::MAX),
            (0x28, self.priority),
            (0x2c, self.unk_2c),
            (0x38, self.unk_38),
            (0x40, self.prio5),
            (0x44, u32::MAX),
            (0x48, self.uuid),
            (0x94, self.unk_94),
        ] {
            put32(&mut out, offset, value);
        }
        Ok(out)
    }
}

pub(crate) fn pointers(ring_size: u32) -> [u8; 0x60] {
    let mut out = [0; 0x60];
    put32(&mut out, 0x50, ring_size);
    out
}
pub(crate) fn job_list(address: u64) -> [u8; 0x18] {
    let mut out = [0; 0x18];
    put64(&mut out, 8, address);
    out
}
pub(crate) fn ring_slot(queue: u64, head: u32, grid: u32, first: bool, kind: Kind) -> [u8; 0x18] {
    let mut out = [0; 0x18];
    put64(&mut out, 8, queue);
    put32(&mut out, 0x10, kind as u32);
    put32(
        &mut out,
        0x14,
        (head & 0xffff) | ((grid & 0xff) << 16) | ((first as u32) << 24),
    );
    out
}
pub(crate) fn event(
    group: u32,
    grid: u32,
    kind: Kind,
    subtype: Option<u32>,
    counter: Option<u32>,
    counter_low: u32,
) -> Result<[u8; 0x40]> {
    // Python packs group<<8 before applying an optional counter override.
    if group > 0xffffff {
        return Err(Error::Overflow);
    }
    let mut out = [0; 0x40];
    put32(&mut out, 0, 0x0e);
    put32(&mut out, 4, subtype.unwrap_or(0x10000 | (grid & 0xffff)));
    put32(&mut out, 8, counter.unwrap_or((group << 8) | counter_low));
    put32(&mut out, 0x10, (kind as u32) << 8);
    Ok(out)
}

/// Both independently advancing consumers constrain channel reuse.
#[derive(Clone, Copy)]
pub(crate) struct Counters(pub(crate) [u8; 3]);
impl Counters {
    pub(crate) fn new(values: [u32; 3]) -> Result<Self> {
        if values.iter().any(|v| *v > 255) {
            return Err(Error::Invalid);
        }
        Ok(Self(values.map(|v| v as u8)))
    }
    pub(crate) fn available(self) -> u8 {
        self.0[0]
            .wrapping_sub(self.0[2])
            .wrapping_sub(1)
            .min(self.0[1].wrapping_sub(self.0[2]).wrapping_sub(1))
    }
    pub(crate) fn slot(self) -> Result<u8> {
        if self.available() == 0 {
            Err(Error::Full)
        } else {
            Ok(self.0[2])
        }
    }
}
pub(crate) fn reached(start: u8, current: u8, target: u8) -> bool {
    current.wrapping_sub(start) >= target.wrapping_sub(start)
}

pub(crate) trait Writer {
    type Error;
    /// Must make the bounded driver-owned bytes visible in this exact order.
    fn write(&mut self, address: u64, bytes: &[u8]) -> core::result::Result<(), Self::Error>;
}

pub(crate) struct Stage<'a> {
    pub(crate) queue: u64,
    pub(crate) pointers: u64,
    pub(crate) item_ring: u64,
    pub(crate) item_capacity: u32,
    pub(crate) write_index: u32,
    pub(crate) channel_ring: u64,
    pub(crate) channel_producer: u64,
    pub(crate) counters: Counters,
    pub(crate) slot: Option<u8>,
    pub(crate) items: &'a [u64],
    pub(crate) group: u32,
    pub(crate) grid: u32,
    pub(crate) kind: Kind,
    pub(crate) first: bool,
    pub(crate) in_place: bool,
    pub(crate) announce: bool,
    pub(crate) defer_inner: bool,
    pub(crate) defer_outer: bool,
    pub(crate) event_subtype: Option<u32>,
    pub(crate) event_counter: Option<u32>,
    pub(crate) event_counter_low: u32,
}
#[derive(Debug)]
pub(crate) enum StageError<E> {
    Protocol(Error),
    Access(E),
}
impl<E> From<Error> for StageError<E> {
    fn from(error: Error) -> Self {
        Self::Protocol(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Publication {
    pub(crate) slot: u8,
    pub(crate) producer: u8,
    pub(crate) consumers_before: [u8; 2],
    pub(crate) write_before: u32,
    pub(crate) write_after: u32,
    pub(crate) deferred_inner: Option<(u64, u32)>,
    pub(crate) deferred_outer: Option<(u64, u32)>,
}
impl Publication {
    pub(crate) fn accepted(&self, read: u32) -> bool {
        read >= self.write_after
    }
    pub(crate) fn completed(&self, done: u32, counters: Counters) -> bool {
        done >= self.write_after
            && (0..2).all(|i| reached(self.consumers_before[i], counters.0[i], self.producer))
    }
}

impl Stage<'_> {
    pub(crate) fn publish<W: Writer>(
        &self,
        writer: &mut W,
    ) -> core::result::Result<Publication, StageError<W::Error>> {
        let count = u32::try_from(self.items.len()).map_err(|_| Error::Overflow)?;
        if count == 0 {
            return Err(Error::Invalid.into());
        }
        let first = if self.in_place {
            self.write_index.checked_sub(count).ok_or(Error::Invalid)?
        } else {
            self.write_index
        };
        let new_write = first.checked_add(count).ok_or(Error::Overflow)?;
        if new_write > self.item_capacity {
            return Err(Error::Full.into());
        }
        let slot = match self.slot {
            Some(slot) => slot,
            None => self.counters.slot()?,
        };
        let producer = slot.wrapping_add(1);
        let event = event(
            self.group,
            self.grid,
            self.kind,
            self.event_subtype,
            self.event_counter,
            self.event_counter_low,
        )?;
        let slot_body = ring_slot(self.queue, new_write, self.grid, self.first, self.kind);
        // Validate every range before the first store. Runtime Writer applies
        // allocation/VA ownership checks and cache maintenance on each write.
        self.item_ring
            .checked_add(new_write as u64 * 8)
            .ok_or(Error::Overflow)?;
        let event_at = *self.items.last().ok_or(Error::Invalid)?;
        event_at
            .checked_add(event.len() as u64)
            .ok_or(Error::Overflow)?;
        let inner_at = self.pointers.checked_add(0x40).ok_or(Error::Overflow)?;
        inner_at.checked_add(4).ok_or(Error::Overflow)?;
        let slot_at = self
            .channel_ring
            .checked_add(slot as u64 * 0x18)
            .ok_or(Error::Overflow)?;
        slot_at.checked_add(0x18).ok_or(Error::Overflow)?;
        self.channel_producer
            .checked_add(4)
            .ok_or(Error::Overflow)?;
        let announce_at = self.queue.checked_add(0x7c).ok_or(Error::Overflow)?;
        announce_at.checked_add(4).ok_or(Error::Overflow)?;
        for (index, address) in self.items.iter().enumerate() {
            writer
                .write(
                    self.item_ring + (first as u64 + index as u64) * 8,
                    &address.to_le_bytes(),
                )
                .map_err(StageError::Access)?;
        }
        writer.write(event_at, &event).map_err(StageError::Access)?;
        if !self.defer_inner {
            writer
                .write(inner_at, &new_write.to_le_bytes())
                .map_err(StageError::Access)?;
        }
        writer
            .write(slot_at, &slot_body)
            .map_err(StageError::Access)?;
        if !self.defer_outer {
            writer
                .write(self.channel_producer, &(producer as u32).to_le_bytes())
                .map_err(StageError::Access)?;
        }
        if self.announce {
            for value in [0u32, 1] {
                writer
                    .write(announce_at, &value.to_le_bytes())
                    .map_err(StageError::Access)?;
            }
        }
        Ok(Publication {
            slot,
            producer,
            consumers_before: [self.counters.0[0], self.counters.0[1]],
            write_before: self.write_index,
            write_after: new_write,
            deferred_inner: self.defer_inner.then_some((inner_at, new_write)),
            deferred_outer: self
                .defer_outer
                .then_some((self.channel_producer, producer as u32)),
        })
    }
}
