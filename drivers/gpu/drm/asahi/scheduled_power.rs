// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! G16C power-controller data for the scheduled firmware family.
//!
//! Arithmetic follows the source-owned M4 controller builder, with the Max
//! policy qualified on J616c. Names containing an offset identify actual Max
//! bundle fields; they do not assign meanings to unknown selectors.
//! Constant-heavy wire storage uses a source Default template in static memory.

use super::scheduled_board::BoardG16C;
use crate::{f32, float::F32};

pub(crate) const OFFSET: usize = 0x5240;
pub(crate) const SIZE: usize = 0x63d4;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidPower;
type Result<T = ()> = core::result::Result<T, InvalidPower>;

#[repr(C)]
pub(crate) struct PowerControllerG16C {
    pub(crate) pad_0000: [u8; 0x4],
    pub(crate) field_5244: [u8; 0x4],
    pub(crate) field_5248: [u8; 0x4],
    pub(crate) pad_000c: [u8; 0x4],
    pub(crate) field_5250: [u8; 0x4],
    pub(crate) field_5254: [u8; 0x4],
    pub(crate) pad_0018: [u8; 0x14],
    pub(crate) field_526c: [u8; 0x4],
    pub(crate) field_5270: [u8; 0x4],
    pub(crate) pad_0034: [u8; 0x8],
    pub(crate) field_527c: [u8; 0x4],
    pub(crate) field_5280: [u8; 0x4],
    pub(crate) field_5284: [u8; 0x4],
    pub(crate) field_5288: [u8; 0x4],
    pub(crate) pad_004c: [u8; 0x4],
    pub(crate) field_5290: [u8; 0x4],
    pub(crate) pad_0054: [u8; 0xc],
    pub(crate) field_52a0: [u8; 0x4],
    pub(crate) field_52a4: [u8; 0x4],
    pub(crate) pad_0068: [u8; 0x4],
    pub(crate) field_52ac: [u8; 0x4],
    pub(crate) field_52b0: [u8; 0x4],
    pub(crate) pad_0074: [u8; 0x8],
    pub(crate) field_52bc: [u8; 0x4],
    pub(crate) field_52c0: [u8; 0x4],
    pub(crate) field_52c4: [u8; 0x4],
    pub(crate) field_52c8: [u8; 0x4],
    pub(crate) pad_008c: [u8; 0x4],
    pub(crate) field_52d0: [u8; 0x4],
    pub(crate) pad_0094: [u8; 0x2c],
    pub(crate) scale: [u8; 0x2c],
    pub(crate) pad_00ec: [u8; 0x8cc],
    pub(crate) field_5bf8: [u8; 0x4],
    pub(crate) pad_09bc: [u8; 0x8],
    pub(crate) field_5c04: [u8; 0x4],
    pub(crate) pad_09c8: [u8; 0x4],
    pub(crate) field_5c0c: [u8; 0x4],
    pub(crate) pad_09d0: [u8; 0x4],
    pub(crate) field_5c14: [u8; 0x4],
    pub(crate) pad_09d8: [u8; 0x4],
    pub(crate) field_5c1c: [u8; 0x4],
    pub(crate) field_5c20: [u8; 0x4],
    pub(crate) field_5c24: [u8; 0x4],
    pub(crate) pad_09e8: [u8; 0x4],
    pub(crate) field_5c2c: [u8; 0x4],
    pub(crate) field_5c30: [u8; 0x4],
    pub(crate) field_5c34: [u8; 0x4],
    pub(crate) field_5c38: [u8; 0x4],
    pub(crate) pad_09fc: [u8; 0x8],
    pub(crate) field_5c44: [u8; 0x4],
    pub(crate) pad_0a08: [u8; 0x30],
    pub(crate) field_5c78: [u8; 0x4],
    pub(crate) pad_0a3c: [u8; 0x34],
    pub(crate) field_5cb0: [u8; 0x4],
    pub(crate) pad_0a74: [u8; 0x8],
    pub(crate) field_5cbc: [u8; 0x4],
    pub(crate) pad_0a80: [u8; 0x4],
    pub(crate) field_5cc4: [u8; 0x4],
    pub(crate) pad_0a88: [u8; 0x4],
    pub(crate) field_5ccc: [u8; 0x4],
    pub(crate) pad_0a90: [u8; 0x4],
    pub(crate) field_5cd4: [u8; 0x4],
    pub(crate) field_5cd8: [u8; 0x4],
    pub(crate) field_5cdc: [u8; 0x4],
    pub(crate) pad_0aa0: [u8; 0x8],
    pub(crate) field_5ce8: [u8; 0x4],
    pub(crate) field_5cec: [u8; 0x4],
    pub(crate) pad_0ab0: [u8; 0xc],
    pub(crate) field_5cfc: [u8; 0x4],
    pub(crate) pad_0ac0: [u8; 0x8],
    pub(crate) field_5d08: [u8; 0x4],
    pub(crate) pad_0acc: [u8; 0x4],
    pub(crate) field_5d10: [u8; 0x8],
    pub(crate) pad_0ad8: [u8; 0x50],
    pub(crate) field_5d68: [u8; 0x4],
    pub(crate) pad_0b2c: [u8; 0x4],
    pub(crate) field_5d70: [u8; 0x4],
    pub(crate) field_5d74: [u8; 0x4],
    pub(crate) field_5d78: [u8; 0x4],
    pub(crate) pad_0b3c: [u8; 0x4],
    pub(crate) field_5d80: [u8; 0x4],
    pub(crate) field_5d84: [u8; 0x4],
    pub(crate) field_5d88: [u8; 0x4],
    pub(crate) field_5d8c: [u8; 0x4],
    pub(crate) field_5d90: [u8; 0x4],
    pub(crate) field_5d94: [u8; 0x4],
    pub(crate) field_5d98: [u8; 0x4],
    pub(crate) field_5d9c: [u8; 0x4],
    pub(crate) field_5da0: [u8; 0x4],
    pub(crate) field_5da4: [u8; 0x4],
    pub(crate) field_5da8: [u8; 0x4],
    pub(crate) field_5dac: [u8; 0x4],
    pub(crate) field_5db0: [u8; 0x4],
    pub(crate) field_5db4: [u8; 0x4],
    pub(crate) field_5db8: [u8; 0x4],
    pub(crate) field_5dbc: [u8; 0x4],
    pub(crate) field_5dc0: [u8; 0x4],
    pub(crate) pad_0b84: [u8; 0x8],
    pub(crate) field_5dcc: [u8; 0x4],
    pub(crate) pad_0b90: [u8; 0x4],
    pub(crate) field_5dd4: [u8; 0x4],
    pub(crate) pad_0b98: [u8; 0x28],
    pub(crate) field_5e00: [u8; 0x4],
    pub(crate) pad_0bc4: [u8; 0x102b],
    pub(crate) initial_power: [u8; 0x400],
    pub(crate) pad_1fef: [u8; 0x201],
    pub(crate) field_7430: [u8; 0x4],
    pub(crate) field_7434: [u8; 0x4],
    pub(crate) field_7438: [u8; 0x4],
    pub(crate) field_743c: [u8; 0x4],
    pub(crate) pad_2200: [u8; 0x10],
    pub(crate) field_7450: [u8; 0x4],
    pub(crate) pad_2214: [u8; 0x4],
    pub(crate) field_7458: [u8; 0x4],
    pub(crate) field_745c: [u8; 0x4],
    pub(crate) pad_2220: [u8; 0x4],
    pub(crate) field_7464: [u8; 0x4],
    pub(crate) field_7468: [u8; 0x4],
    pub(crate) pad_222c: [u8; 0x3c],
    pub(crate) field_74a8: [u8; 0x8],
    pub(crate) field_74b0: [u8; 0x4],
    pub(crate) pad_2274: [u8; 0x4],
    pub(crate) field_74b8: [u8; 0x4],
    pub(crate) pad_227c: [u8; 0x8],
    pub(crate) field_74c4: [u8; 0x4],
    pub(crate) pad_2288: [u8; 0x4],
    pub(crate) field_74cc: [u8; 0x4],
    pub(crate) pad_2290: [u8; 0x4],
    pub(crate) field_74d4: [u8; 0x4],
    pub(crate) pad_2298: [u8; 0x4],
    pub(crate) field_74dc: [u8; 0x4],
    pub(crate) field_74e0: [u8; 0x4],
    pub(crate) field_74e4: [u8; 0x4],
    pub(crate) pad_22a8: [u8; 0x4],
    pub(crate) field_74ec: [u8; 0x4],
    pub(crate) field_74f0: [u8; 0x4],
    pub(crate) field_74f4: [u8; 0x4],
    pub(crate) field_74f8: [u8; 0x4],
    pub(crate) field_74fc: [u8; 0x4],
    pub(crate) pad_22c0: [u8; 0x4],
    pub(crate) field_7504: [u8; 0x4],
    pub(crate) pad_22c8: [u8; 0x30],
    pub(crate) field_7538: [u8; 0x4],
    pub(crate) pad_22fc: [u8; 0x21c],
    pub(crate) field_7758: [u8; 0x1],
    pub(crate) field_7759: [u8; 0x1],
    pub(crate) pad_251a: [u8; 0x210],
    pub(crate) field_796a: [u8; 0x4],
    pub(crate) pad_272e: [u8; 0x4],
    pub(crate) field_7972: [u8; 0x4],
    pub(crate) pad_2736: [u8; 0x4],
    pub(crate) field_797a: [u8; 0x4],
    pub(crate) pad_273e: [u8; 0x4],
    pub(crate) field_7982: [u8; 0x4],
    pub(crate) field_7986: [u8; 0x4],
    pub(crate) field_798a: [u8; 0x4],
    pub(crate) pad_274e: [u8; 0x4],
    pub(crate) field_7992: [u8; 0x4],
    pub(crate) field_7996: [u8; 0x4],
    pub(crate) field_799a: [u8; 0x4],
    pub(crate) field_799e: [u8; 0x4],
    pub(crate) pad_2762: [u8; 0x4],
    pub(crate) field_79a6: [u8; 0x4],
    pub(crate) field_79aa: [u8; 0x4],
    pub(crate) pad_276e: [u8; 0x4],
    pub(crate) field_79b2: [u8; 0x4],
    pub(crate) pad_2776: [u8; 0x28],
    pub(crate) field_79de: [u8; 0x4],
    pub(crate) pad_27a2: [u8; 0x18],
    pub(crate) field_79fa: [u8; 0x4],
    pub(crate) pad_27be: [u8; 0x14],
    pub(crate) field_7a12: [u8; 0x4],
    pub(crate) field_7a16: [u8; 0x4],
    pub(crate) field_7a1a: [u8; 0x4],
    pub(crate) pad_27de: [u8; 0x92],
    pub(crate) field_7ab0: [u8; 0x4],
    pub(crate) pad_2874: [u8; 0x4c],
    pub(crate) field_7b00: [u8; 0x4],
    pub(crate) pad_28c4: [u8; 0x4],
    pub(crate) field_7b08: [u8; 0x4],
    pub(crate) field_7b0c: [u8; 0x4],
    pub(crate) field_7b10: [u8; 0x4],
    pub(crate) pad_28d4: [u8; 0x8],
    pub(crate) field_7b1c: [u8; 0x4],
    pub(crate) field_7b20: [u8; 0x4],
    pub(crate) field_7b24: [u8; 0x4],
    pub(crate) field_7b28: [u8; 0x4],
    pub(crate) field_7b2c: [u8; 0x8],
    pub(crate) pad_28f4: [u8; 0xc8],
    pub(crate) field_7bfc: [u8; 0x4],
    pub(crate) pad_29c0: [u8; 0x8],
    pub(crate) field_7c08: [u8; 0x4],
    pub(crate) pad_29cc: [u8; 0x4],
    pub(crate) field_7c10: [u8; 0x4],
    pub(crate) pad_29d4: [u8; 0x4],
    pub(crate) field_7c18: [u8; 0x4],
    pub(crate) pad_29dc: [u8; 0x4],
    pub(crate) field_7c20: [u8; 0x4],
    pub(crate) field_7c24: [u8; 0x4],
    pub(crate) field_7c28: [u8; 0x4],
    pub(crate) pad_29ec: [u8; 0x4],
    pub(crate) field_7c30: [u8; 0x4],
    pub(crate) field_7c34: [u8; 0x4],
    pub(crate) field_7c38: [u8; 0x4],
    pub(crate) field_7c3c: [u8; 0x4],
    pub(crate) pad_2a00: [u8; 0x4],
    pub(crate) field_7c44: [u8; 0x4],
    pub(crate) field_7c48: [u8; 0x4],
    pub(crate) pad_2a0c: [u8; 0x8],
    pub(crate) field_7c54: [u8; 0x4],
    pub(crate) pad_2a18: [u8; 0x4],
    pub(crate) field_7c5c: [u8; 0x8],
    pub(crate) pad_2a24: [u8; 0x18],
    pub(crate) field_7c7c: [u8; 0x4],
    pub(crate) pad_2a40: [u8; 0x10c],
    pub(crate) field_7d8c: [u8; 0x4],
    pub(crate) field_7d90: [u8; 0x4],
    pub(crate) pad_2b54: [u8; 0x4],
    pub(crate) field_7d98: [u8; 0x4],
    pub(crate) field_7d9c: [u8; 0x4],
    pub(crate) field_7da0: [u8; 0x4],
    pub(crate) field_7da4: [u8; 0x4],
    pub(crate) field_7da8: [u8; 0x4],
    pub(crate) field_7dac: [u8; 0x4],
    pub(crate) field_7db0: [u8; 0x4],
    pub(crate) field_7db4: [u8; 0x4],
    pub(crate) field_7db8: [u8; 0x4],
    pub(crate) field_7dbc: [u8; 0x4],
    pub(crate) pad_2b80: [u8; 0x4],
    pub(crate) field_7dc4: [u8; 0x4],
    pub(crate) field_7dc8: [u8; 0x4],
    pub(crate) field_7dcc: [u8; 0x4],
    pub(crate) pad_2b90: [u8; 0x4],
    pub(crate) field_7dd4: [u8; 0x4],
    pub(crate) field_7dd8: [u8; 0x4],
    pub(crate) pad_2b9c: [u8; 0x8],
    pub(crate) field_7de4: [u8; 0x4],
    pub(crate) field_7de8: [u8; 0x4],
    pub(crate) field_7dec: [u8; 0x8],
    pub(crate) field_7df4: [u8; 0x8],
    pub(crate) pad_2bbc: [u8; 0x4],
    pub(crate) field_7e00: [u8; 0x4],
    pub(crate) pad_2bc4: [u8; 0x24],
    pub(crate) field_7e28: [u8; 0x4],
    pub(crate) field_7e2c: [u8; 0x4],
    pub(crate) field_7e30: [u8; 0x4],
    pub(crate) field_7e34: [u8; 0x4],
    pub(crate) field_7e38: [u8; 0x4],
    pub(crate) field_7e3c: [u8; 0x4],
    pub(crate) field_7e40: [u8; 0x4],
    pub(crate) pad_2c04: [u8; 0x2c],
    pub(crate) field_7e70: [u8; 0x4],
    pub(crate) field_7e74: [u8; 0x4],
    pub(crate) pad_2c38: [u8; 0x8],
    pub(crate) field_7e80: [u8; 0x4],
    pub(crate) field_7e84: [u8; 0x4],
    pub(crate) pad_2c48: [u8; 0x1e4],
    pub(crate) field_806c: [u8; 0x4],
    pub(crate) pad_2e30: [u8; 0x8],
    pub(crate) field_8078: [u8; 0x4],
    pub(crate) pad_2e3c: [u8; 0x4],
    pub(crate) field_8080: [u8; 0x4],
    pub(crate) pad_2e44: [u8; 0x4],
    pub(crate) field_8088: [u8; 0x4],
    pub(crate) pad_2e4c: [u8; 0x4],
    pub(crate) field_8090: [u8; 0x4],
    pub(crate) field_8094: [u8; 0x4],
    pub(crate) field_8098: [u8; 0x4],
    pub(crate) pad_2e5c: [u8; 0x8],
    pub(crate) field_80a4: [u8; 0x4],
    pub(crate) field_80a8: [u8; 0x4],
    pub(crate) field_80ac: [u8; 0x4],
    pub(crate) pad_2e70: [u8; 0x8],
    pub(crate) field_80b8: [u8; 0x4],
    pub(crate) pad_2e7c: [u8; 0x30],
    pub(crate) field_80ec: [u8; 0x4],
    pub(crate) pad_2eb0: [u8; 0x3c],
    pub(crate) field_812c: [u8; 0x4],
    pub(crate) pad_2ef0: [u8; 0x18],
    pub(crate) field_8148: [u8; 0x8],
    pub(crate) field_8150: [u8; 0x4],
    pub(crate) field_8154: [u8; 0x4],
    pub(crate) pad_2f18: [u8; 0xc38],
    pub(crate) thermal_context: [u8; 0x8],
    pub(crate) field_8d98: [u8; 0x8],
    pub(crate) field_8da0: [u8; 0x4],
    pub(crate) pad_3b64: [u8; 0x2000],
    pub(crate) field_ada4: [u8; 0x4],
    pub(crate) pad_5b68: [u8; 0x7b0],
    pub(crate) auxiliary: [u8; 0x8],
    pub(crate) field_b560: [u8; 0x4],
    pub(crate) field_b564: [u8; 0x4],
    pub(crate) field_b568: [u8; 0x4],
    pub(crate) field_b56c: [u8; 0x4],
    pub(crate) pad_6330: [u8; 0x30],
    pub(crate) field_b5a0: [u8; 0x4],
    pub(crate) pad_6364: [u8; 0x4],
    pub(crate) field_b5a8: [u8; 0x4],
    pub(crate) pad_636c: [u8; 0x4],
    pub(crate) field_b5b0: [u8; 0x4],
    pub(crate) pad_6374: [u8; 0x60],
}

const fn template() -> PowerControllerG16C {
    // SAFETY: all wire fields are initialized byte arrays; every bit pattern
    // is valid. Compile-time gates below prove alignment one and no padding.
    let mut raw: PowerControllerG16C = unsafe { core::mem::zeroed() };
    raw.field_5bf8 = (625u32).to_le_bytes();
    raw.field_7e70 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_7e74 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7430 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_7458 = (f32!(100.0)).to_bits().to_le_bytes();
    raw.field_5cd8 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_5250 = (4u32).to_le_bytes();
    raw.field_5254 = (f32!(1.0)).to_bits().to_le_bytes();
    raw.field_52a0 = (4u32).to_le_bytes();
    raw.field_52a4 = (f32!(1.0)).to_bits().to_le_bytes();
    raw.field_526c = (1u32).to_le_bytes();
    raw.field_5270 = (1u32).to_le_bytes();
    raw.field_52ac = (1u32).to_le_bytes();
    raw.field_52b0 = (1u32).to_le_bytes();
    raw.field_5290 = (100u32).to_le_bytes();
    raw.field_52d0 = (100u32).to_le_bytes();
    raw.field_527c = (0u32).to_le_bytes();
    raw.field_5284 = (1u32).to_le_bytes();
    raw.field_52bc = (0u32).to_le_bytes();
    raw.field_52c4 = (1u32).to_le_bytes();
    raw.field_5da8 = (f32!(95.0)).to_bits().to_le_bytes();
    raw.field_5d80 = (6u32).to_le_bytes();
    raw.field_5d84 = (1u32).to_le_bytes();
    raw.field_7db0 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7db4 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_7e00 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_7dc4 = (100u32).to_le_bytes();
    raw.field_7d8c = (50u32).to_le_bytes();
    raw.field_7d90 = (1u32).to_le_bytes();
    raw.field_7dd8 = (1400u32).to_le_bytes();
    raw.field_7c20 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7c24 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_7c30 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_74b8 = (4u32).to_le_bytes();
    raw.field_7504 = (11000u32).to_le_bytes();
    raw.field_74c4 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_74cc = (f32!(1.0)).to_bits().to_le_bytes();
    raw.field_74dc = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_74e0 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_74ec = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_797a = (f32!(400.0)).to_bits().to_le_bytes();
    raw.field_7982 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7986 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_798a = (f32!(100.0)).to_bits().to_le_bytes();
    raw.field_7992 = (f32!(-100.0 / 65536.0)).to_bits().to_le_bytes();
    raw.field_79a6 = (f32!(3000.0)).to_bits().to_le_bytes();
    raw.field_7a16 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7a1a = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_7996 = (900u32).to_le_bytes();
    raw.field_799a = (1000u32).to_le_bytes();
    raw.field_799e = (1000u32).to_le_bytes();
    raw.field_79aa = (9900u32).to_le_bytes();
    raw.field_79b2 = (0u32).to_le_bytes();
    raw.field_79de = (1000u32).to_le_bytes();
    raw.field_79fa = (200u32).to_le_bytes();
    raw.field_7a12 = (0u32).to_le_bytes();
    raw.field_8088 = (f32!(5.0)).to_bits().to_le_bytes();
    raw.field_8090 = (f32!(0.0)).to_bits().to_le_bytes();
    raw.field_8094 = (f32!(65536.0)).to_bits().to_le_bytes();
    raw.field_8098 = (f32!(15.0)).to_bits().to_le_bytes();
    raw.field_806c = (4u32).to_le_bytes();
    raw.field_7b00 = (1u32).to_le_bytes();
    raw.field_812c = (100u32).to_le_bytes();
    raw.field_8150 = (125u32).to_le_bytes();
    raw.field_8154 = (1u32).to_le_bytes();
    raw.field_8da0 = (1u32).to_le_bytes();
    raw.field_b5a0 = (0xfu32).to_le_bytes();
    raw.field_b5b0 = (1u32).to_le_bytes();
    raw.field_ada4 = (f32!(5.0)).to_bits().to_le_bytes();
    raw.field_b560 = (u32::from_be_bytes(*b"g0TR")).to_le_bytes();
    raw.field_b564 = (u32::from_be_bytes(*b"g1TR")).to_le_bytes();
    raw.field_b568 = (u32::from_be_bytes(*b"g2TR")).to_le_bytes();
    raw.field_b56c = (u32::from_be_bytes(*b"g3TR")).to_le_bytes();
    raw.field_b5a8 = (u32::from_be_bytes(*b"r0TR")).to_le_bytes();
    raw.field_74a8 = (0x2a222aa22a222aa2u64).to_le_bytes();
    raw.field_8148 = (0x2a222aa22a222aa2u64).to_le_bytes();
    raw.field_8d98 = (0x2a222aa22a222aa2u64).to_le_bytes();
    let mut i = 0;
    while i < 11 {
        let bytes = f32!(1.02).to_bits().to_le_bytes();
        let mut j = 0;
        while j < 4 {
            raw.scale[i * 4 + j] = bytes[j];
            j += 1;
        }
        i += 1;
    }
    i = 0;
    while i < 16 * 32 {
        raw.initial_power[i * 2] = 0xff;
        raw.initial_power[i * 2 + 1] = 0x7f;
        i += 1;
    }
    raw
}

impl Default for PowerControllerG16C {
    fn default() -> Self {
        template()
    }
}

impl PowerControllerG16C {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: byte-array fields, alignment one and no implicit padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), SIZE) }
    }
}
static TEMPLATE: PowerControllerG16C = template();

fn checked_float(bits: u32) -> Result<F32> {
    if bits & 0x7f80_0000 == 0x7f80_0000 {
        return Err(InvalidPower);
    }
    Ok(F32::from_bits(bits))
}
fn checked_bits(value: F32) -> Result<u32> {
    checked_float(value.to_bits())?;
    Ok(value.to_bits())
}

fn coefficients(count: u32) -> Result<(F32, F32)> {
    if count == 0 {
        return Err(InvalidPower);
    }
    let count = F32::from(count);
    Ok(((count - f32!(1.0)) / count, f32!(1.0) / count))
}

struct Filter {
    alpha: F32,
    inverse: F32,
    count_x4: u32,
    duration: u32,
    clocks: u64,
}

fn filter(count: u32, period: u32, request: Option<u32>) -> Result<Filter> {
    let (alpha, inverse) = coefficients(count)?;
    let count_x4 = count.checked_mul(4).ok_or(InvalidPower)?;
    let duration = count.checked_mul(period).ok_or(InvalidPower)?;
    let clocks = u64::from(duration.checked_mul(24000).ok_or(InvalidPower)?);
    if let Some(ms) = request {
        if period == 0 || ms / period != count {
            return Err(InvalidPower);
        }
    }
    Ok(Filter {
        alpha,
        inverse,
        count_x4,
        duration: request.unwrap_or(duration),
        clocks,
    })
}

/// Compact host-side derived values; never a firmware wire image. All
/// property validation/arithmetic completes before encode mutates storage.
#[derive(Default)]
pub(crate) struct ParametersG16C {
    field_5244: [u8; 4],
    field_5248: [u8; 4],
    field_5c04: [u8; 4],
    field_5c0c: [u8; 4],
    field_5c14: [u8; 4],
    field_5c1c: [u8; 4],
    field_5c20: [u8; 4],
    field_5c24: [u8; 4],
    field_5c2c: [u8; 4],
    field_5c30: [u8; 4],
    field_5c34: [u8; 4],
    field_5c38: [u8; 4],
    field_5c78: [u8; 4],
    field_5c44: [u8; 4],
    field_7b08: [u8; 4],
    field_7b0c: [u8; 4],
    field_7b10: [u8; 4],
    field_7e80: [u8; 4],
    field_7e84: [u8; 4],
    field_7434: [u8; 4],
    field_7438: [u8; 4],
    field_743c: [u8; 4],
    field_7450: [u8; 4],
    field_745c: [u8; 4],
    field_7ab0: [u8; 4],
    field_7464: [u8; 4],
    field_7468: [u8; 4],
    field_5cbc: [u8; 4],
    field_5cc4: [u8; 4],
    field_5ccc: [u8; 4],
    field_5cd4: [u8; 4],
    field_5cdc: [u8; 4],
    field_5cb0: [u8; 4],
    field_5ce8: [u8; 4],
    field_5cec: [u8; 4],
    field_5cfc: [u8; 4],
    field_5d08: [u8; 4],
    field_5280: [u8; 4],
    field_5288: [u8; 4],
    field_52c0: [u8; 4],
    field_52c8: [u8; 4],
    field_5d8c: [u8; 4],
    field_5d90: [u8; 4],
    field_5d94: [u8; 4],
    field_5d98: [u8; 4],
    field_5d9c: [u8; 4],
    field_5da0: [u8; 4],
    field_5da4: [u8; 4],
    field_5dac: [u8; 4],
    field_5db0: [u8; 4],
    field_5db4: [u8; 4],
    field_5d68: [u8; 4],
    field_5d70: [u8; 4],
    field_5d74: [u8; 4],
    field_5d78: [u8; 4],
    field_5d88: [u8; 4],
    field_5db8: [u8; 4],
    field_5dbc: [u8; 4],
    field_5dc0: [u8; 4],
    field_5dcc: [u8; 4],
    field_5dd4: [u8; 4],
    field_5e00: [u8; 4],
    field_7d98: [u8; 4],
    field_7d9c: [u8; 4],
    field_7da0: [u8; 4],
    field_7da4: [u8; 4],
    field_7da8: [u8; 4],
    field_7dac: [u8; 4],
    field_7db8: [u8; 4],
    field_7dbc: [u8; 4],
    field_7dd4: [u8; 4],
    field_7dc8: [u8; 4],
    field_7dcc: [u8; 4],
    field_7de4: [u8; 4],
    field_7de8: [u8; 4],
    field_7e28: [u8; 4],
    field_7e2c: [u8; 4],
    field_7e30: [u8; 4],
    field_7e34: [u8; 4],
    field_7e38: [u8; 4],
    field_7e3c: [u8; 4],
    field_7e40: [u8; 4],
    field_7b1c: [u8; 4],
    field_7b20: [u8; 4],
    field_7b24: [u8; 4],
    field_7b28: [u8; 4],
    field_7c18: [u8; 4],
    field_7c28: [u8; 4],
    field_7c44: [u8; 4],
    field_7c34: [u8; 4],
    field_7c38: [u8; 4],
    field_7c3c: [u8; 4],
    field_7c48: [u8; 4],
    field_7c7c: [u8; 4],
    field_7bfc: [u8; 4],
    field_7c54: [u8; 4],
    field_7c08: [u8; 4],
    field_7c10: [u8; 4],
    field_74b0: [u8; 4],
    field_74f0: [u8; 4],
    field_74f4: [u8; 4],
    field_74f8: [u8; 4],
    field_74fc: [u8; 4],
    field_7538: [u8; 4],
    field_74d4: [u8; 4],
    field_74e4: [u8; 4],
    field_796a: [u8; 4],
    field_7972: [u8; 4],
    field_8078: [u8; 4],
    field_8080: [u8; 4],
    field_80a4: [u8; 4],
    field_80a8: [u8; 4],
    field_80ac: [u8; 4],
    field_80b8: [u8; 4],
    field_80ec: [u8; 4],
    field_5d10: [u8; 8],
    field_7dec: [u8; 8],
    field_7df4: [u8; 8],
    field_7b2c: [u8; 8],
    field_7c5c: [u8; 8],
    field_7758: [u8; 1],
    field_7759: [u8; 1],
}

/// Relocatable pointers owned by the initialization graph.
#[derive(Clone, Copy)]
pub(crate) struct PowerLinks {
    pub(crate) thermal_context: u64,
    pub(crate) auxiliary: u64,
}

/// Typed FDT power settings, using the same apple,* property names as
/// Asahi. IEEE-754 configuration values are ordinary u32 FDT bit patterns.
#[derive(Default)]
pub(crate) struct SettingsG16C {
    pub(crate) avg_power_filter_tc_ms: Option<u32>,
    pub(crate) avg_power_input_filter_tc: Option<u32>,
    pub(crate) avg_power_ki: Option<u32>,
    pub(crate) avg_power_ki_only: Option<u32>,
    pub(crate) avg_power_kp: Option<u32>,
    pub(crate) avg_power_min_duty_cycle: Option<u32>,
    pub(crate) avg_power_target_filter_tc: Option<u32>,
    pub(crate) fast_die0_integral_gain: Option<u32>,
    pub(crate) fast_die0_prop_tgt_delta: Option<u32>,
    pub(crate) fast_die0_proportional_gain: Option<u32>,
    pub(crate) fast_die0_release_temp: Option<u32>,
    pub(crate) perf_base_pstate: Option<u32>,
    pub(crate) perf_boost_ce_step: Option<u32>,
    pub(crate) perf_boost_min_util: Option<u32>,
    pub(crate) perf_dz: Option<u32>,
    pub(crate) perf_filter_drop_threshold: Option<u32>,
    pub(crate) perf_filter_time_constant: Option<u32>,
    pub(crate) perf_filter_time_constant2: Option<u32>,
    pub(crate) perf_integral_gain: Option<u32>,
    pub(crate) perf_integral_gain2: Option<u32>,
    pub(crate) perf_integral_min_clamp: Option<u32>,
    pub(crate) perf_proportional_gain: Option<u32>,
    pub(crate) perf_proportional_gain2: Option<u32>,
    pub(crate) perf_reset_iters: Option<u32>,
    pub(crate) perf_tgt_utilization: Option<u32>,
    pub(crate) ppm_filter_time_constant_ms: Option<u32>,
    pub(crate) ppm_ki: Option<u32>,
    pub(crate) ppm_kp: Option<u32>,
    pub(crate) pwr_filter_time_constant: Option<u32>,
    pub(crate) pwr_integral_gain: Option<u32>,
    pub(crate) pwr_integral_min_clamp: Option<u32>,
    pub(crate) pwr_min_duty_cycle: Option<u32>,
    pub(crate) pwr_proportional_gain: Option<u32>,
    pub(crate) se_engagement_criteria: Option<u32>,
    pub(crate) se_engagement_tolerance: Option<u32>,
    pub(crate) se_filter_time_constant: Option<u32>,
    pub(crate) se_filter_time_constant_1: Option<u32>,
    pub(crate) se_inactive_threshold: Option<u32>,
    pub(crate) se_ki: Option<u32>,
    pub(crate) se_ki_1: Option<u32>,
    pub(crate) se_kp: Option<u32>,
    pub(crate) se_kp_1: Option<u32>,
    pub(crate) se_reset_criteria: Option<u32>,
    pub(crate) se_reset_tolerance: Option<u32>,
    pub(crate) turbo_controller_input: Option<u32>,
}

impl SettingsG16C {
    pub(crate) fn from_fdt(node: &kernel::of::Node) -> kernel::prelude::Result<Self> {
        use kernel::c_str;
        Ok(Self {
            avg_power_filter_tc_ms: node
                .get_opt_property(c_str!("apple,avg-power-filter-tc-ms"))?,
            avg_power_input_filter_tc: node
                .get_opt_property(c_str!("apple,avg-power-input-filter-tc"))?,
            avg_power_ki: node.get_opt_property(c_str!("apple,avg-power-ki"))?,
            avg_power_ki_only: node.get_opt_property(c_str!("apple,avg-power-ki-only"))?,
            avg_power_kp: node.get_opt_property(c_str!("apple,avg-power-kp"))?,
            avg_power_min_duty_cycle: node
                .get_opt_property(c_str!("apple,avg-power-min-duty-cycle"))?,
            avg_power_target_filter_tc: node
                .get_opt_property(c_str!("apple,avg-power-target-filter-tc"))?,
            fast_die0_integral_gain: node
                .get_opt_property(c_str!("apple,fast-die0-integral-gain"))?,
            fast_die0_prop_tgt_delta: node
                .get_opt_property(c_str!("apple,fast-die0-prop-tgt-delta"))?,
            fast_die0_proportional_gain: node
                .get_opt_property(c_str!("apple,fast-die0-proportional-gain"))?,
            fast_die0_release_temp: node
                .get_opt_property(c_str!("apple,fast-die0-release-temp"))?,
            perf_base_pstate: node.get_opt_property(c_str!("apple,perf-base-pstate"))?,
            perf_boost_ce_step: node.get_opt_property(c_str!("apple,perf-boost-ce-step"))?,
            perf_boost_min_util: node.get_opt_property(c_str!("apple,perf-boost-min-util"))?,
            perf_dz: node.get_opt_property(c_str!("apple,perf-dz"))?,
            perf_filter_drop_threshold: node
                .get_opt_property(c_str!("apple,perf-filter-drop-threshold"))?,
            perf_filter_time_constant: node
                .get_opt_property(c_str!("apple,perf-filter-time-constant"))?,
            perf_filter_time_constant2: node
                .get_opt_property(c_str!("apple,perf-filter-time-constant2"))?,
            perf_integral_gain: node.get_opt_property(c_str!("apple,perf-integral-gain"))?,
            perf_integral_gain2: node.get_opt_property(c_str!("apple,perf-integral-gain2"))?,
            perf_integral_min_clamp: node
                .get_opt_property(c_str!("apple,perf-integral-min-clamp"))?,
            perf_proportional_gain: node
                .get_opt_property(c_str!("apple,perf-proportional-gain"))?,
            perf_proportional_gain2: node
                .get_opt_property(c_str!("apple,perf-proportional-gain2"))?,
            perf_reset_iters: node.get_opt_property(c_str!("apple,perf-reset-iters"))?,
            perf_tgt_utilization: node.get_opt_property(c_str!("apple,perf-tgt-utilization"))?,
            ppm_filter_time_constant_ms: node
                .get_opt_property(c_str!("apple,ppm-filter-time-constant-ms"))?,
            ppm_ki: node.get_opt_property(c_str!("apple,ppm-ki"))?,
            ppm_kp: node.get_opt_property(c_str!("apple,ppm-kp"))?,
            pwr_filter_time_constant: node
                .get_opt_property(c_str!("apple,pwr-filter-time-constant"))?,
            pwr_integral_gain: node.get_opt_property(c_str!("apple,pwr-integral-gain"))?,
            pwr_integral_min_clamp: node
                .get_opt_property(c_str!("apple,pwr-integral-min-clamp"))?,
            pwr_min_duty_cycle: node.get_opt_property(c_str!("apple,pwr-min-duty-cycle"))?,
            pwr_proportional_gain: node.get_opt_property(c_str!("apple,pwr-proportional-gain"))?,
            se_engagement_criteria: node
                .get_opt_property(c_str!("apple,se-engagement-criteria"))?,
            se_engagement_tolerance: node
                .get_opt_property(c_str!("apple,se-engagement-tolerance"))?,
            se_filter_time_constant: node
                .get_opt_property(c_str!("apple,se-filter-time-constant"))?,
            se_filter_time_constant_1: node
                .get_opt_property(c_str!("apple,se-filter-time-constant-1"))?,
            se_inactive_threshold: node.get_opt_property(c_str!("apple,se-inactive-threshold"))?,
            se_ki: node.get_opt_property(c_str!("apple,se-ki"))?,
            se_ki_1: node.get_opt_property(c_str!("apple,se-ki-1"))?,
            se_kp: node.get_opt_property(c_str!("apple,se-kp"))?,
            se_kp_1: node.get_opt_property(c_str!("apple,se-kp-1"))?,
            se_reset_criteria: node.get_opt_property(c_str!("apple,se-reset-criteria"))?,
            se_reset_tolerance: node.get_opt_property(c_str!("apple,se-reset-tolerance"))?,
            turbo_controller_input: node
                .get_opt_property(c_str!("apple,turbo-controller-input"))?,
        })
    }
}

impl ParametersG16C {
    pub(crate) fn derive(board: &BoardG16C, settings: &SettingsG16C) -> Result<Self> {
        if board.period_ms == 0 || board.maximum_power == 0 {
            return Err(InvalidPower);
        }
        let mut raw = Self::default();
        let max_scaled = 100 * (board.freq_a.len() as u32 - 1);
        let maximum = F32::from(board.maximum_power);
        let duty = settings.pwr_min_duty_cycle.unwrap_or(40);
        let minimum = settings.pwr_integral_min_clamp.unwrap_or(0);
        if duty > max_scaled || minimum > board.maximum_power {
            return Err(InvalidPower);
        }
        let minimum = F32::from(minimum);
        let dt = F32::from(board.period_ms) / f32!(1000.0);
        let micros = board.period_ms.checked_mul(1000).ok_or(InvalidPower)?;
        raw.field_5244 = (board.period_clocks).to_le_bytes();
        raw.field_5248 = (board.period_clocks).to_le_bytes();
        let (alpha, inverse) = coefficients(settings.pwr_filter_time_constant.unwrap_or(313))?;
        raw.field_5c04 = checked_bits(alpha)?.to_le_bytes();
        raw.field_5c0c = checked_bits(inverse)?.to_le_bytes();
        raw.field_5c14 = checked_bits(checked_float(
            settings
                .pwr_integral_gain
                .unwrap_or(f32!(0.0202129).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5c1c = checked_bits(minimum)?.to_le_bytes();
        raw.field_5c20 = checked_bits(maximum)?.to_le_bytes();
        raw.field_5c24 = checked_bits(checked_float(
            settings
                .pwr_proportional_gain
                .unwrap_or(f32!(5.2831855).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5c2c = checked_bits(-F32::from(max_scaled) / maximum)?.to_le_bytes();
        raw.field_5c30 = (duty).to_le_bytes();
        raw.field_5c34 = (max_scaled).to_le_bytes();
        raw.field_5c38 = (max_scaled).to_le_bytes();
        raw.field_5c78 = (max_scaled).to_le_bytes();
        raw.field_5c44 = (board.maximum_power).to_le_bytes();
        raw.field_7b08 = (board.maximum_power).to_le_bytes();
        raw.field_7b0c = (board.maximum_power).to_le_bytes();
        raw.field_7b10 = (board.maximum_power).to_le_bytes();

        raw.field_7e80 = (duty).to_le_bytes();
        raw.field_7e84 = (max_scaled).to_le_bytes();
        raw.field_7434 = checked_bits(F32::from(duty))?.to_le_bytes();
        raw.field_7438 = checked_bits(F32::from(max_scaled))?.to_le_bytes();
        raw.field_743c = (board.freq_a[10]).to_le_bytes();
        raw.field_7450 = (duty).to_le_bytes();
        raw.field_745c = (max_scaled).to_le_bytes();
        raw.field_7ab0 = (max_scaled).to_le_bytes();
        let (alpha, inverse) = coefficients(5)?;
        raw.field_7464 = checked_bits(alpha)?.to_le_bytes();
        raw.field_7468 = checked_bits(inverse)?.to_le_bytes();

        let ppm_ms = settings.ppm_filter_time_constant_ms.unwrap_or(16);
        let ppm = filter(ppm_ms / board.period_ms, board.period_ms, Some(ppm_ms))?;
        raw.field_5cbc = checked_bits(ppm.alpha)?.to_le_bytes();
        raw.field_5cc4 = checked_bits(ppm.inverse)?.to_le_bytes();
        raw.field_5ccc =
            checked_bits(dt * checked_float(settings.ppm_ki.unwrap_or(f32!(17.0).to_bits()))?)?
                .to_le_bytes();
        raw.field_5cd4 = checked_bits(minimum)?.to_le_bytes();
        raw.field_5cdc = checked_bits(checked_float(
            settings.ppm_kp.unwrap_or(f32!(0.3).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5cb0 = (ppm.count_x4).to_le_bytes();
        raw.field_5ce8 = (duty).to_le_bytes();
        raw.field_5cec = (max_scaled).to_le_bytes();
        raw.field_5cfc = (board.maximum_power).to_le_bytes();
        raw.field_5d08 = (ppm.duration).to_le_bytes();
        raw.field_5d10 = (ppm.clocks).to_le_bytes();

        let base = settings.perf_base_pstate.unwrap_or(1);
        let utilization = settings.perf_tgt_utilization.unwrap_or(80);
        let dead_zone = settings.perf_dz.unwrap_or(0);
        let boost = settings.perf_boost_min_util.unwrap_or(85);
        let integral_minimum = settings.perf_integral_min_clamp.unwrap_or(0);
        if base == 0
            || base >= 11
            || utilization > 100
            || dead_zone > utilization
            || boost > 100
            || integral_minimum > 95
        {
            return Err(InvalidPower);
        }
        let base_scaled = 100 * base;
        let target = utilization - dead_zone;

        raw.field_5280 = (base_scaled).to_le_bytes();

        raw.field_5288 = (max_scaled).to_le_bytes();

        raw.field_52c0 = (base_scaled).to_le_bytes();

        raw.field_52c8 = (max_scaled).to_le_bytes();
        let first = coefficients(settings.perf_filter_time_constant.unwrap_or(5))?;
        let second = coefficients(settings.perf_filter_time_constant2.unwrap_or(200))?;
        raw.field_5d8c = checked_bits(first.0)?.to_le_bytes();
        raw.field_5d90 = checked_bits(second.0)?.to_le_bytes();
        raw.field_5d94 = checked_bits(first.1)?.to_le_bytes();
        raw.field_5d98 = checked_bits(second.1)?.to_le_bytes();
        raw.field_5d9c = checked_bits(checked_float(
            settings
                .perf_integral_gain
                .unwrap_or(f32!(0.8937).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5da0 = checked_bits(checked_float(
            settings
                .perf_integral_gain2
                .unwrap_or(f32!(0.8937).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5da4 = checked_bits(F32::from(integral_minimum))?.to_le_bytes();
        raw.field_5dac = checked_bits(checked_float(
            settings
                .perf_proportional_gain
                .unwrap_or(f32!(2.979).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5db0 = checked_bits(checked_float(
            settings
                .perf_proportional_gain2
                .unwrap_or(f32!(2.979).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_5db4 =
            checked_bits(F32::from(max_scaled - base_scaled) / f32!(95.0))?.to_le_bytes();
        raw.field_5d68 = (target).to_le_bytes();
        raw.field_5d70 = (boost).to_le_bytes();
        raw.field_5d74 = (settings.perf_boost_ce_step.unwrap_or(50)).to_le_bytes();
        raw.field_5d78 = (settings.perf_reset_iters.unwrap_or(6)).to_le_bytes();
        raw.field_5d88 = (settings.perf_filter_drop_threshold.unwrap_or(0)).to_le_bytes();
        raw.field_5db8 = (base_scaled).to_le_bytes();
        raw.field_5dbc = (max_scaled).to_le_bytes();
        raw.field_5dc0 = (base_scaled).to_le_bytes();
        raw.field_5dcc = (target).to_le_bytes();
        raw.field_5dd4 = (dead_zone).to_le_bytes();
        raw.field_5e00 = (base_scaled).to_le_bytes();

        let first = filter(
            settings.se_filter_time_constant.unwrap_or(9),
            board.period_ms,
            None,
        )?;
        let second = filter(
            settings.se_filter_time_constant_1.unwrap_or(3),
            board.period_ms,
            None,
        )?;
        raw.field_7d98 = checked_bits(first.alpha)?.to_le_bytes();
        raw.field_7d9c = checked_bits(second.alpha)?.to_le_bytes();
        raw.field_7da0 = checked_bits(first.inverse)?.to_le_bytes();
        raw.field_7da4 = checked_bits(second.inverse)?.to_le_bytes();
        raw.field_7da8 =
            checked_bits(checked_float(settings.se_ki.unwrap_or(f32!(-50.0).to_bits()))? * dt)?
                .to_le_bytes();
        raw.field_7dac =
            checked_bits(checked_float(settings.se_ki_1.unwrap_or(f32!(-50.0).to_bits()))? * dt)?
                .to_le_bytes();
        raw.field_7db8 = checked_bits(checked_float(
            settings.se_kp.unwrap_or(f32!(-5.0).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_7dbc = checked_bits(checked_float(
            settings.se_kp_1.unwrap_or(f32!(-5.0).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_7dd4 = checked_bits(F32::from(micros))?.to_le_bytes();
        raw.field_7dc8 = (max_scaled).to_le_bytes();
        raw.field_7dcc = (base_scaled).to_le_bytes();
        raw.field_7de4 = (first.duration).to_le_bytes();
        raw.field_7de8 = (second.duration).to_le_bytes();
        raw.field_7e28 = (micros).to_le_bytes();
        raw.field_7e2c = ((33 + board.period_ms / 2) / board.period_ms).to_le_bytes();
        raw.field_7e30 = (settings.se_inactive_threshold.unwrap_or(2500)).to_le_bytes();
        raw.field_7e34 = (settings.se_engagement_criteria.unwrap_or(600)).to_le_bytes();
        raw.field_7e38 = (settings.se_engagement_tolerance.unwrap_or(2)).to_le_bytes();
        raw.field_7e3c = (settings.se_reset_tolerance.unwrap_or(4)).to_le_bytes();
        raw.field_7e40 = (settings.se_reset_criteria.unwrap_or(50)).to_le_bytes();
        raw.field_7dec = (first.clocks).to_le_bytes();
        raw.field_7df4 = (second.clocks).to_le_bytes();

        let average_target = filter(
            settings.avg_power_target_filter_tc.unwrap_or(1),
            board.period_ms,
            None,
        )?;
        raw.field_7b1c = checked_bits(average_target.alpha)?.to_le_bytes();
        raw.field_7b20 = checked_bits(average_target.inverse)?.to_le_bytes();
        raw.field_7b24 = (average_target.count_x4).to_le_bytes();
        raw.field_7b28 = (average_target.duration).to_le_bytes();
        raw.field_7b2c = (average_target.clocks).to_le_bytes();
        let ki_only = settings.avg_power_ki_only;
        let direct_ki = settings.avg_power_ki;
        let mut avg_ki = checked_float(ki_only.or(direct_ki).unwrap_or(f32!(3.75).to_bits()))?;
        if ki_only.is_some() || direct_ki.is_none() {
            avg_ki = avg_ki * dt;
        }
        let avg_duty = settings.avg_power_min_duty_cycle.unwrap_or(40);
        if avg_duty > max_scaled {
            return Err(InvalidPower);
        }
        raw.field_7c18 = checked_bits(avg_ki)?.to_le_bytes();
        raw.field_7c28 = checked_bits(checked_float(
            settings.avg_power_kp.unwrap_or(f32!(0.6).to_bits()),
        )?)?
        .to_le_bytes();
        raw.field_7c44 = checked_bits(maximum)?.to_le_bytes();
        raw.field_7c34 = (avg_duty).to_le_bytes();
        raw.field_7c38 = (max_scaled).to_le_bytes();
        raw.field_7c3c = (max_scaled).to_le_bytes();
        raw.field_7c48 = (board.maximum_power).to_le_bytes();
        raw.field_7c7c = (max_scaled).to_le_bytes();
        let request = settings.avg_power_filter_tc_ms;
        let count = match request {
            Some(ms) => ms / board.period_ms,
            None => settings
                .avg_power_input_filter_tc
                .unwrap_or(248 / board.period_ms),
        };
        let average = filter(count, board.period_ms, request)?;
        raw.field_7bfc = (average.count_x4).to_le_bytes();
        raw.field_7c54 = (average.duration).to_le_bytes();
        raw.field_7c08 = checked_bits(average.alpha)?.to_le_bytes();
        raw.field_7c10 = checked_bits(average.inverse)?.to_le_bytes();
        raw.field_7c5c = (average.clocks).to_le_bytes();

        // Fast-die uses the untruncated AIC interval, unlike the other loops.
        let fast_dt = F32::from(board.period_clocks) / f32!(24000.0) / f32!(1000.0);
        let release = settings.fast_die0_release_temp.unwrap_or(80);
        let delta = settings.fast_die0_prop_tgt_delta.unwrap_or(0);
        if release > u32::MAX / 100 || delta > u32::MAX / 100 {
            return Err(InvalidPower);
        }

        raw.field_74b0 = ((F32::from(release) * f32!(100.0))
            .to_u32_checked()
            .ok_or(InvalidPower)?)
        .to_le_bytes();
        raw.field_74f0 = (duty).to_le_bytes();
        raw.field_74f4 = (max_scaled).to_le_bytes();
        raw.field_74f8 = (max_scaled).to_le_bytes();
        raw.field_74fc = ((F32::from(delta) * f32!(100.0))
            .to_u32_checked()
            .ok_or(InvalidPower)?)
        .to_le_bytes();
        raw.field_7538 = (max_scaled).to_le_bytes();
        raw.field_74d4 = checked_bits(
            checked_float(
                settings
                    .fast_die0_integral_gain
                    .unwrap_or(f32!(700.0).to_bits()),
            )? * fast_dt,
        )?
        .to_le_bytes();
        raw.field_74e4 = checked_bits(checked_float(
            settings
                .fast_die0_proportional_gain
                .unwrap_or(f32!(34.0).to_bits()),
        )?)?
        .to_le_bytes();

        let turbo_input = settings.turbo_controller_input;
        let mode = turbo_input.unwrap_or(2);
        let (alpha, inverse) = coefficients(200)?;
        raw.field_7758 = (u8::from(turbo_input.is_some())).to_le_bytes();
        raw.field_7759 = (u8::from(mode == 1)).to_le_bytes();
        raw.field_796a = checked_bits(alpha)?.to_le_bytes();
        raw.field_7972 = checked_bits(inverse)?.to_le_bytes();

        let (alpha, inverse) = coefficients(32)?;
        raw.field_8078 = checked_bits(alpha)?.to_le_bytes();
        raw.field_8080 = checked_bits(inverse)?.to_le_bytes();
        raw.field_80a4 = (max_scaled - 100).to_le_bytes();
        raw.field_80a8 = (max_scaled).to_le_bytes();
        raw.field_80ac = (max_scaled).to_le_bytes();
        raw.field_80b8 = (max_scaled).to_le_bytes();
        raw.field_80ec = (max_scaled).to_le_bytes();

        Ok(raw)
    }

    pub(crate) fn encode(&self, out: &mut [u8], links: PowerLinks) -> Result {
        if out.len() < SIZE {
            return Err(InvalidPower);
        }
        out[..SIZE].copy_from_slice(TEMPLATE.as_bytes());
        // SAFETY: exclusive initialized host staging, not live DMA. The
        // validated extent is sufficient; this wire type has alignment one,
        // no padding, and all byte-array fields. The reference does not escape.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<PowerControllerG16C>() };
        raw.field_5244 = self.field_5244;
        raw.field_5248 = self.field_5248;
        raw.field_5c04 = self.field_5c04;
        raw.field_5c0c = self.field_5c0c;
        raw.field_5c14 = self.field_5c14;
        raw.field_5c1c = self.field_5c1c;
        raw.field_5c20 = self.field_5c20;
        raw.field_5c24 = self.field_5c24;
        raw.field_5c2c = self.field_5c2c;
        raw.field_5c30 = self.field_5c30;
        raw.field_5c34 = self.field_5c34;
        raw.field_5c38 = self.field_5c38;
        raw.field_5c78 = self.field_5c78;
        raw.field_5c44 = self.field_5c44;
        raw.field_7b08 = self.field_7b08;
        raw.field_7b0c = self.field_7b0c;
        raw.field_7b10 = self.field_7b10;
        raw.field_7e80 = self.field_7e80;
        raw.field_7e84 = self.field_7e84;
        raw.field_7434 = self.field_7434;
        raw.field_7438 = self.field_7438;
        raw.field_743c = self.field_743c;
        raw.field_7450 = self.field_7450;
        raw.field_745c = self.field_745c;
        raw.field_7ab0 = self.field_7ab0;
        raw.field_7464 = self.field_7464;
        raw.field_7468 = self.field_7468;
        raw.field_5cbc = self.field_5cbc;
        raw.field_5cc4 = self.field_5cc4;
        raw.field_5ccc = self.field_5ccc;
        raw.field_5cd4 = self.field_5cd4;
        raw.field_5cdc = self.field_5cdc;
        raw.field_5cb0 = self.field_5cb0;
        raw.field_5ce8 = self.field_5ce8;
        raw.field_5cec = self.field_5cec;
        raw.field_5cfc = self.field_5cfc;
        raw.field_5d08 = self.field_5d08;
        raw.field_5280 = self.field_5280;
        raw.field_5288 = self.field_5288;
        raw.field_52c0 = self.field_52c0;
        raw.field_52c8 = self.field_52c8;
        raw.field_5d8c = self.field_5d8c;
        raw.field_5d90 = self.field_5d90;
        raw.field_5d94 = self.field_5d94;
        raw.field_5d98 = self.field_5d98;
        raw.field_5d9c = self.field_5d9c;
        raw.field_5da0 = self.field_5da0;
        raw.field_5da4 = self.field_5da4;
        raw.field_5dac = self.field_5dac;
        raw.field_5db0 = self.field_5db0;
        raw.field_5db4 = self.field_5db4;
        raw.field_5d68 = self.field_5d68;
        raw.field_5d70 = self.field_5d70;
        raw.field_5d74 = self.field_5d74;
        raw.field_5d78 = self.field_5d78;
        raw.field_5d88 = self.field_5d88;
        raw.field_5db8 = self.field_5db8;
        raw.field_5dbc = self.field_5dbc;
        raw.field_5dc0 = self.field_5dc0;
        raw.field_5dcc = self.field_5dcc;
        raw.field_5dd4 = self.field_5dd4;
        raw.field_5e00 = self.field_5e00;
        raw.field_7d98 = self.field_7d98;
        raw.field_7d9c = self.field_7d9c;
        raw.field_7da0 = self.field_7da0;
        raw.field_7da4 = self.field_7da4;
        raw.field_7da8 = self.field_7da8;
        raw.field_7dac = self.field_7dac;
        raw.field_7db8 = self.field_7db8;
        raw.field_7dbc = self.field_7dbc;
        raw.field_7dd4 = self.field_7dd4;
        raw.field_7dc8 = self.field_7dc8;
        raw.field_7dcc = self.field_7dcc;
        raw.field_7de4 = self.field_7de4;
        raw.field_7de8 = self.field_7de8;
        raw.field_7e28 = self.field_7e28;
        raw.field_7e2c = self.field_7e2c;
        raw.field_7e30 = self.field_7e30;
        raw.field_7e34 = self.field_7e34;
        raw.field_7e38 = self.field_7e38;
        raw.field_7e3c = self.field_7e3c;
        raw.field_7e40 = self.field_7e40;
        raw.field_7b1c = self.field_7b1c;
        raw.field_7b20 = self.field_7b20;
        raw.field_7b24 = self.field_7b24;
        raw.field_7b28 = self.field_7b28;
        raw.field_7c18 = self.field_7c18;
        raw.field_7c28 = self.field_7c28;
        raw.field_7c44 = self.field_7c44;
        raw.field_7c34 = self.field_7c34;
        raw.field_7c38 = self.field_7c38;
        raw.field_7c3c = self.field_7c3c;
        raw.field_7c48 = self.field_7c48;
        raw.field_7c7c = self.field_7c7c;
        raw.field_7bfc = self.field_7bfc;
        raw.field_7c54 = self.field_7c54;
        raw.field_7c08 = self.field_7c08;
        raw.field_7c10 = self.field_7c10;
        raw.field_74b0 = self.field_74b0;
        raw.field_74f0 = self.field_74f0;
        raw.field_74f4 = self.field_74f4;
        raw.field_74f8 = self.field_74f8;
        raw.field_74fc = self.field_74fc;
        raw.field_7538 = self.field_7538;
        raw.field_74d4 = self.field_74d4;
        raw.field_74e4 = self.field_74e4;
        raw.field_796a = self.field_796a;
        raw.field_7972 = self.field_7972;
        raw.field_8078 = self.field_8078;
        raw.field_8080 = self.field_8080;
        raw.field_80a4 = self.field_80a4;
        raw.field_80a8 = self.field_80a8;
        raw.field_80ac = self.field_80ac;
        raw.field_80b8 = self.field_80b8;
        raw.field_80ec = self.field_80ec;
        raw.field_5d10 = self.field_5d10;
        raw.field_7dec = self.field_7dec;
        raw.field_7df4 = self.field_7df4;
        raw.field_7b2c = self.field_7b2c;
        raw.field_7c5c = self.field_7c5c;
        raw.field_7758 = self.field_7758;
        raw.field_7759 = self.field_7759;
        raw.thermal_context = links.thermal_context.to_le_bytes();
        raw.auxiliary = links.auxiliary.to_le_bytes();
        Ok(())
    }
}

const _: () = {
    assert!(core::mem::size_of::<ParametersG16C>() <= 0x400);
    assert!(core::mem::size_of::<PowerControllerG16C>() == SIZE);
    assert!(core::mem::align_of::<PowerControllerG16C>() == 1);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5244) == 0x4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5248) == 0x8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5250) == 0x10);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5254) == 0x14);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_526c) == 0x2c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5270) == 0x30);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_527c) == 0x3c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5280) == 0x40);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5284) == 0x44);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5288) == 0x48);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5290) == 0x50);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52a0) == 0x60);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52a4) == 0x64);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52ac) == 0x6c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52b0) == 0x70);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52bc) == 0x7c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52c0) == 0x80);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52c4) == 0x84);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52c8) == 0x88);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_52d0) == 0x90);
    assert!(core::mem::offset_of!(PowerControllerG16C, scale) == 0xc0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5bf8) == 0x9b8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c04) == 0x9c4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c0c) == 0x9cc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c14) == 0x9d4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c1c) == 0x9dc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c20) == 0x9e0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c24) == 0x9e4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c2c) == 0x9ec);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c30) == 0x9f0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c34) == 0x9f4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c38) == 0x9f8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c44) == 0xa04);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5c78) == 0xa38);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cb0) == 0xa70);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cbc) == 0xa7c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cc4) == 0xa84);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5ccc) == 0xa8c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cd4) == 0xa94);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cd8) == 0xa98);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cdc) == 0xa9c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5ce8) == 0xaa8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cec) == 0xaac);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5cfc) == 0xabc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d08) == 0xac8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d10) == 0xad0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d68) == 0xb28);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d70) == 0xb30);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d74) == 0xb34);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d78) == 0xb38);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d80) == 0xb40);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d84) == 0xb44);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d88) == 0xb48);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d8c) == 0xb4c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d90) == 0xb50);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d94) == 0xb54);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d98) == 0xb58);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5d9c) == 0xb5c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5da0) == 0xb60);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5da4) == 0xb64);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5da8) == 0xb68);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5dac) == 0xb6c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5db0) == 0xb70);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5db4) == 0xb74);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5db8) == 0xb78);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5dbc) == 0xb7c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5dc0) == 0xb80);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5dcc) == 0xb8c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5dd4) == 0xb94);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_5e00) == 0xbc0);
    assert!(core::mem::offset_of!(PowerControllerG16C, initial_power) == 0x1bef);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7430) == 0x21f0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7434) == 0x21f4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7438) == 0x21f8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_743c) == 0x21fc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7450) == 0x2210);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7458) == 0x2218);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_745c) == 0x221c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7464) == 0x2224);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7468) == 0x2228);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74a8) == 0x2268);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74b0) == 0x2270);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74b8) == 0x2278);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74c4) == 0x2284);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74cc) == 0x228c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74d4) == 0x2294);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74dc) == 0x229c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74e0) == 0x22a0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74e4) == 0x22a4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74ec) == 0x22ac);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74f0) == 0x22b0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74f4) == 0x22b4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74f8) == 0x22b8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_74fc) == 0x22bc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7504) == 0x22c4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7538) == 0x22f8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7758) == 0x2518);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7759) == 0x2519);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_796a) == 0x272a);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7972) == 0x2732);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_797a) == 0x273a);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7982) == 0x2742);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7986) == 0x2746);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_798a) == 0x274a);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7992) == 0x2752);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7996) == 0x2756);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_799a) == 0x275a);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_799e) == 0x275e);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_79a6) == 0x2766);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_79aa) == 0x276a);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_79b2) == 0x2772);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_79de) == 0x279e);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_79fa) == 0x27ba);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7a12) == 0x27d2);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7a16) == 0x27d6);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7a1a) == 0x27da);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7ab0) == 0x2870);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b00) == 0x28c0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b08) == 0x28c8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b0c) == 0x28cc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b10) == 0x28d0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b1c) == 0x28dc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b20) == 0x28e0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b24) == 0x28e4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b28) == 0x28e8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7b2c) == 0x28ec);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7bfc) == 0x29bc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c08) == 0x29c8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c10) == 0x29d0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c18) == 0x29d8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c20) == 0x29e0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c24) == 0x29e4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c28) == 0x29e8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c30) == 0x29f0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c34) == 0x29f4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c38) == 0x29f8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c3c) == 0x29fc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c44) == 0x2a04);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c48) == 0x2a08);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c54) == 0x2a14);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c5c) == 0x2a1c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7c7c) == 0x2a3c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7d8c) == 0x2b4c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7d90) == 0x2b50);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7d98) == 0x2b58);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7d9c) == 0x2b5c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7da0) == 0x2b60);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7da4) == 0x2b64);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7da8) == 0x2b68);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dac) == 0x2b6c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7db0) == 0x2b70);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7db4) == 0x2b74);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7db8) == 0x2b78);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dbc) == 0x2b7c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dc4) == 0x2b84);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dc8) == 0x2b88);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dcc) == 0x2b8c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dd4) == 0x2b94);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dd8) == 0x2b98);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7de4) == 0x2ba4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7de8) == 0x2ba8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7dec) == 0x2bac);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7df4) == 0x2bb4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e00) == 0x2bc0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e28) == 0x2be8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e2c) == 0x2bec);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e30) == 0x2bf0);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e34) == 0x2bf4);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e38) == 0x2bf8);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e3c) == 0x2bfc);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e40) == 0x2c00);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e70) == 0x2c30);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e74) == 0x2c34);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e80) == 0x2c40);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_7e84) == 0x2c44);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_806c) == 0x2e2c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8078) == 0x2e38);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8080) == 0x2e40);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8088) == 0x2e48);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8090) == 0x2e50);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8094) == 0x2e54);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8098) == 0x2e58);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_80a4) == 0x2e64);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_80a8) == 0x2e68);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_80ac) == 0x2e6c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_80b8) == 0x2e78);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_80ec) == 0x2eac);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_812c) == 0x2eec);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8148) == 0x2f08);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8150) == 0x2f10);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8154) == 0x2f14);
    assert!(core::mem::offset_of!(PowerControllerG16C, thermal_context) == 0x3b50);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8d98) == 0x3b58);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_8da0) == 0x3b60);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_ada4) == 0x5b64);
    assert!(core::mem::offset_of!(PowerControllerG16C, auxiliary) == 0x6318);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b560) == 0x6320);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b564) == 0x6324);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b568) == 0x6328);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b56c) == 0x632c);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b5a0) == 0x6360);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b5a8) == 0x6368);
    assert!(core::mem::offset_of!(PowerControllerG16C, field_b5b0) == 0x6370);
};
