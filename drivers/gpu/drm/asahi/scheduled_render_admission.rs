// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Max FList registration DATA and its owned reply. A matching RX receipt is
//! independent of TX consumption; callers must establish both before reuse.

use super::Error;

#[repr(C)]
pub(crate) struct FListRequestG16C {
    opcode: [u8; 4],
    context: [u8; 4],
    mask: [u8; 4],
    tag: [u8; 4],
    pad_10: [u8; 4],
    support: [u8; 8],
    table: [u8; 8],
    previous_end: [u8; 8],
    added_bytes: [u8; 4],
    mode: [u8; 4],
    enabled: [u8; 4],
    pad_38: [u8; 8],
}
impl Default for FListRequestG16C {
    fn default() -> Self {
        Self {
            opcode: 0x20u32.to_le_bytes(),
            context: [0; 4],
            mask: 0x3fu32.to_le_bytes(),
            tag: [0; 4],
            pad_10: [0; 4],
            support: [0; 8],
            table: [0; 8],
            previous_end: [0; 8],
            added_bytes: [0; 4],
            mode: [0; 4],
            enabled: 1u32.to_le_bytes(),
            pad_38: [0; 8],
        }
    }
}

pub(crate) struct RegistrationG16C {
    pub(crate) context: u32,
    pub(crate) support: u64,
    pub(crate) table: u64,
    pub(crate) added: u32,
    pub(crate) previous: u32,
    pub(crate) mode: u32,
    pub(crate) tag: u32,
    // RX +8 is an independent observed value, not the request tag at +0xc.
    pub(crate) reply_value: u32,
}

pub(crate) struct FListReceiptG16C {
    context: u32,
    support: u64,
    table: u64,
    previous_end: u64,
    added_bytes: u32,
    reply_value: u32,
    pub(crate) received: bool,
}
impl RegistrationG16C {
    /// Construct both before the producer store, so an immediate reply cannot
    /// precede ownership. Geometry and opcode follow the qualified serializer.
    pub(crate) fn prepare(&self) -> Result<(FListRequestG16C, FListReceiptG16C), Error> {
        if !(1..64).contains(&self.context)
            || self.added == 0
            || self.added >= 1 << 29
            || self.mode > 2
        {
            return Err(Error::Invalid);
        }
        let previous_end = self
            .table
            .checked_add(u64::from(self.previous) * 0x40)
            .ok_or(Error::Overflow)?;
        let bytes = self.added * 8;
        let request = FListRequestG16C {
            context: self.context.to_le_bytes(),
            support: self.support.to_le_bytes(),
            table: self.table.to_le_bytes(),
            previous_end: previous_end.to_le_bytes(),
            added_bytes: bytes.to_le_bytes(),
            mode: self.mode.to_le_bytes(),
            tag: self.tag.to_le_bytes(),
            ..Default::default()
        };
        let receipt = FListReceiptG16C {
            context: self.context,
            support: self.support,
            table: self.table,
            previous_end,
            added_bytes: bytes,
            reply_value: self.reply_value,
            received: false,
        };
        Ok((request, receipt))
    }
}
impl FListRequestG16C {
    pub(crate) fn bytes(&self) -> &[u8; 64] {
        // SAFETY: All fields initialized byte arrays; size/alignment below.
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}
impl FListReceiptG16C {
    pub(crate) fn consume(&mut self, data: &[u8; 0x48]) -> bool {
        let word = |at| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let pointer = |at| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
        if self.received
            || word(0) != 13
            || word(4) != self.context
            || word(8) != self.reply_value
            || pointer(0xc) != self.support
            || pointer(0x14) != self.table
            || pointer(0x1c) != self.previous_end
            || word(0x24) != self.added_bytes
        {
            return false;
        }
        self.received = true;
        true
    }
}
const _: () = {
    assert!(core::mem::size_of::<FListRequestG16C>() == 0x40);
    assert!(core::mem::align_of::<FListRequestG16C>() == 1);
    macro_rules! at {
        ($f:ident,$n:expr) => {
            assert!(core::mem::offset_of!(FListRequestG16C, $f) == $n);
        };
    }
    at!(opcode, 0);
    at!(context, 4);
    at!(mask, 8);
    at!(tag, 0xc);
    at!(pad_10, 0x10);
    at!(support, 0x14);
    at!(table, 0x1c);
    at!(previous_end, 0x24);
    at!(added_bytes, 0x2c);
    at!(mode, 0x30);
    at!(enabled, 0x34);
    at!(pad_38, 0x38);
};
