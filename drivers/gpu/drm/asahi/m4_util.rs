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
