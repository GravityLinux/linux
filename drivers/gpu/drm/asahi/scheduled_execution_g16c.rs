// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-qualified engine bindings. Event IDs name physical engine slots;
//! logical queue lanes remain TA/fragment/compute independently of this choice.

pub(crate) struct BindingG16C {
    event: u8,
    tag: u64,
    address: u64,
}
impl BindingG16C {
    fn new(event: u8, tag: u64, address: u64) -> Option<Self> {
        if event > 3 || address < 0x4000 || address >= 1 << 42 || address & 0x3fff != 0 {
            return None;
        }
        Some(Self {
            event,
            tag,
            address,
        })
    }
    pub(crate) fn compute(event: u8, address: u64) -> Option<Self> {
        // The held four-queue Max replay qualifies all four CS engines.
        if event > 3 {
            return None;
        }
        Self::new(event, 0x3100000000000000, address)
    }
    /// The source protocol binds fragment before tiling.
    pub(crate) fn render(first: u8, tiling: u64, fragment: u64) -> Option<[Self; 2]> {
        if first > 2 || tiling == fragment {
            return None;
        }
        Some([
            Self::new(first + 1, 0x2100000000000000, fragment)?,
            Self::new(first, 0x1100000000000000, tiling)?,
        ])
    }
    /// Reused native engines retain their scratch binding and engine state.
    /// The four-write resume is the tail of the initial binding transaction.
    pub(crate) fn resume_transactions(&self) -> [(usize, u8, u64); 4] {
        let initial = self.transactions();
        [initial[8], initial[9], initial[10], initial[11]]
    }
    pub(crate) fn transactions(&self) -> [(usize, u8, u64); 12] {
        let selector = [
            0x0020040000021008,
            0x00200c0000021008,
            0x0020140000021008,
            0x00201c0000021008,
        ][self.event as usize];
        [
            (0xd06030, 32, 4),
            (0x4090, 64, selector),
            (0x4098, 64, self.tag | self.address),
            (0xd06030, 32, 0),
            (0xd06030, 32, 4),
            (0x21070, 64, 0),
            (0x21068, 64, 1 << self.event),
            (0xd06030, 32, 0),
            (0xd06030, 32, 4),
            (0x4090, 64, selector + 0x10),
            (0x4098, 64, 1),
            (0xd06030, 32, 0),
        ]
    }
}
