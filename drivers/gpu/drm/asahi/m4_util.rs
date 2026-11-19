// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from the Asahi utility functions.

//! Miscellaneous utility functions

use core::ops::{Add, BitAnd, Div, Not, Sub};
use kernel::prelude::*;

/// Aligns an integer type to a power of two.
pub(crate) fn align<T>(a: T, b: T) -> T
where
    T: Copy
        + Default
        + BitAnd<Output = T>
        + Not<Output = T>
        + Add<Output = T>
        + Sub<Output = T>
        + Div<Output = T>
        + core::cmp::PartialEq,
{
    let def: T = Default::default();
    #[allow(clippy::eq_op)]
    let one: T = !def / !def;

    assert!((b & (b - one)) == def);

    (a + b - one) & !(b - one)
}

/// Types for which every bit pattern is a valid value, so `Reader` may build
/// them from untrusted bytes.
///
/// # Safety
///
/// Implementors must be plain data: integers, integer arrays and structs of
/// them, with no padding-sensitive invariants, references, enums or bools.
pub(crate) unsafe trait AnyBitPattern: Default + Sized + Copy + 'static {}

pub(crate) struct Reader<'a> {
    buffer: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buffer: &'a [u8]) -> Self {
        Reader { buffer, offset: 0 }
    }

    pub(crate) fn read_up_to<T: AnyBitPattern>(&mut self, max_size: usize) -> Result<T> {
        let end = self.offset.checked_add(max_size).ok_or(EINVAL)?;
        let source = self.buffer.get(self.offset..end).ok_or(EINVAL)?;
        let size = core::mem::size_of::<T>().min(max_size);
        // Forward-compatible records may extend the known structure only
        // with zero bytes; shorter records retain zero defaults.
        if source[size..].iter().any(|byte| *byte != 0) {
            return Err(EINVAL);
        }
        let mut obj: T = Default::default();
        // SAFETY: T admits every bit pattern and the slice fits the object.
        let dst = unsafe { core::slice::from_raw_parts_mut(&mut obj as *mut _ as *mut u8, size) };
        dst.copy_from_slice(&source[..size]);
        self.offset = end;
        Ok(obj)
    }

    pub(crate) fn read<T: AnyBitPattern>(&mut self) -> Result<T> {
        self.read_up_to(core::mem::size_of::<T>())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.offset >= self.buffer.len()
    }
}

/// Write a firmware ABI word in little-endian byte order.
pub(crate) fn put32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Snapshot a task name for diagnostics; callers choose thread or leader.
pub(crate) fn task_comm(task: &kernel::task::Task) -> [u8; 16] {
    // SAFETY: comm is a fixed array in the live task. A concurrent rename
    // can at worst tear this diagnostic string.
    let comm = unsafe { (*task.as_ptr()).comm };
    core::array::from_fn(|i| comm[i] as u8)
}

/// Diagnostics: a process name from a NUL-padded comm.
pub(crate) fn comm_str(comm: &[u8; 16]) -> &str {
    let len = comm.iter().position(|&c| c == 0).unwrap_or(16);
    core::str::from_utf8(&comm[..len]).unwrap_or("?")
}
