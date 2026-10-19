// SPDX-License-Identifier: GPL-2.0-only
// Included by test-native-mesh.py, together with the actual production methods.
use g16_mesh::{DecodeError, Profile};

fn stream() -> Vec<u32> {
    // T16 mesh state with the research carrier's scalar object dispatch.
    vec![
        0x40000d2c, 0x504, 0x10, 0, 0, 0x01000000, 0x22224, 0x01000000, 0x22220, 1, 0, 0x400810, 0,
        5, 0x500, 0x4001000, 0x20000013, 0x70000600, 1, 1, 1, 1, 1, 1, 0xc0000000,
    ]
}
fn decode(words: &[u32], base: u64) -> std::result::Result<Option<Profile>, DecodeError> {
    g16_mesh::decode(base, |va| words.get(((va - base) / 4) as usize).copied())
}

#[test]
fn native_packets_are_relocatable_and_bounded() {
    for base in [0x1004000000, 0x10d1233ffc] {
        // includes a page boundary
        let mut words = stream();
        assert_eq!(decode(&words, base), Ok(Some(Profile { payload: 0 })));
        words[6] = 0x12340;
        words[8] = 0x80004;
        words[15] = 0xc010000;
        assert_eq!(decode(&words, base), Ok(Some(Profile { payload: 0 })));
        for len in 0..words.len() {
            assert!(decode(&words[..len], base).is_err(), "truncated at {len}");
        }
    }
    assert_eq!(decode(&stream(), 3), Err(DecodeError::Unsupported));
    let mut words = stream()[..14].to_vec();
    words.extend(std::iter::repeat_n(0x20000013, 1100));
    assert_eq!(decode(&words, 0x1004000000), Err(DecodeError::Unsupported));
}

#[test]
fn object_dispatch_is_one_scalar_or_physical_t16_workgroup() {
    for base in [0x1004000000, 0x10d1233ffc] {
        for width in [1, 16] {
            let mut words = stream();
            words[18] = width;
            words[21] = width;
            assert_eq!(decode(&words, base), Ok(Some(Profile { payload: 0 })));
            for len in 0..words.len() {
                assert!(decode(&words[..len], base).is_err(), "truncated at {len}");
            }
            for index in 18..24 {
                for value in [0, 1, 2, 15, 16, 17, 32, u32::MAX] {
                    if value == words[index] {
                        continue;
                    }
                    let mut bad = words.clone();
                    bad[index] = value;
                    assert_eq!(decode(&bad, base), Err(DecodeError::Unsupported));
                }
            }
            // Matching total/local dimensions alone must not widen the class.
            for dimensions in [
                [2, 1, 1], [32, 1, 1], [1, 16, 1], [1, 1, 16], [16, 2, 1], [16, 1, 2],
            ] {
                let mut bad = words.clone();
                bad[18..21].copy_from_slice(&dimensions);
                bad[21..24].copy_from_slice(&dimensions);
                assert_eq!(decode(&bad, base), Err(DecodeError::Unsupported));
            }
            let mut twice = words.clone();
            twice.pop();
            twice.extend_from_slice(&words[17..]);
            assert_eq!(decode(&twice, base), Err(DecodeError::Unsupported));
        }
    }
}

#[test]
fn ordinary_data_is_not_searched_for_mesh_magic() {
    let words = [0x4000002e, 0x40000d2c, 0x504, 0x10];
    let mut reads = 0;
    assert_eq!(
        g16_mesh::decode(0x1004000000, |va| {
            reads += 1;
            Some(words[((va - 0x1004000000) / 4) as usize])
        }),
        Ok(None)
    );
    assert_eq!(reads, 1);
    assert_eq!(
        decode(&[0x40000d2d], 0x1004000000),
        Err(DecodeError::Unsupported)
    );
}

#[test]
fn vertex_attributes_accept_the_bounded_scalar_and_row_relationship() {
    for components in 4..=128 {
        let mut words = stream();
        words[1] = components | ((components + 1) << 8);
        words[13] = components + 1;
        assert_eq!(
            decode(&words, 0x1004003ffc),
            Ok(Some(Profile { payload: 0 }))
        );
        // Cover all supported counts, including those between device fixtures.
        for wrong_rows in [0, components, components + 2, u32::MAX] {
            words[13] = wrong_rows;
            assert_eq!(decode(&words, 0x1004003ffc), Err(DecodeError::Unsupported));
        }
        words[13] = components + 1;
        for bit in 8..32 {
            words[1] ^= 1 << bit;
            assert_eq!(decode(&words, 0x1004003ffc), Err(DecodeError::Unsupported));
            words[1] ^= 1 << bit;
        }
    }
}

#[test]
fn attributes_do_not_admit_other_output_or_execution_classes() {
    for components in (0..4).chain(129..=255) {
        let mut words = stream();
        words[1] = components | ((components + 1) << 8);
        words[13] = components + 1;
        assert_eq!(decode(&words, 0x1004000000), Err(DecodeError::Unsupported));
    }
    for components in [7, 12, 128] {
        for (index, value) in [
            (2, 0xd0), // primitive output mode
            (3, 1),
            (4, 1), // unqualified state/payload
            (9, 0), // missing object stage
            (10, 1),
            (10, 1 << 30),  // more primitives, culling
            (11, 0x400820), // wider mesh workgroup
            (12, 1),        // primitive output components
            (18, 2),        // more than one direct object invocation
        ] {
            let mut words = stream();
            words[1] = components | ((components + 1) << 8);
            words[13] = components + 1;
            words[index] = value;
            assert_eq!(decode(&words, 0x1004000000), Err(DecodeError::Unsupported));
        }
    }
}

#[test]
fn reject_unqualified_resource_classes_and_packets() {
    for (index, value) in [
        (1, 0x605),
        (2, 0xd0),
        (9, 0),
        (10, 1),
        (11, 0x400820),
        (12, 3),
        (13, 9),
        (17, 0x74000600),
        (18, 2),
    ] {
        let mut words = stream();
        words[index] = value;
        assert_eq!(
            decode(&words, 0x1004000000),
            Err(DecodeError::Unsupported),
            "word {index}"
        );
    }
    for tail in [
        vec![0xc0000000],
        vec![0x80000000, 0x4000000],
        vec![0x60000600],
        vec![0x40000d2c],
    ] {
        let mut words = stream()[..14].to_vec();
        words.extend(tail);
        assert_eq!(decode(&words, 0x1004000000), Err(DecodeError::Unsupported));
    }
    let mut twice = stream();
    twice.pop();
    twice.extend_from_slice(&stream()[17..]);
    assert_eq!(decode(&twice, 0x1004000000), Err(DecodeError::Unsupported));
}

fn read32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}
fn read64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

#[test]
fn initialized_pool_covers_all_advertised_native_pages() {
    for start in [0x1080000000, 0x10e0000000] {
        let pool = g16_render::OperandPool::new(start, start + 0x20000000).unwrap();
        let header = pool.support(7, 0xfffffc2002810000);
        assert_eq!(read32(&header, 0x20), 0);
        assert_eq!(read32(&header, 0x24), 24 * 512);
        assert_eq!(read32(&header, 0x2c), 24 * 512);
        assert_eq!(read32(&header, 0x48), 24 * 8);
        let aliases = g16_render::private_aliases(&pool);
        assert_eq!(read64(&header, 0x14), aliases[3].0);
        assert_eq!(read64(&header, 0x30), aliases[4].0);
        let mut descriptors = [0; 0x4000];
        g16_render::private_page(&pool, 0x440000, &mut descriptors).unwrap();
        for buffer in 0..24 {
            let (va, backing, size) = aliases[16 + buffer];
            assert_eq!(read64(&descriptors, buffer * 0x40), va | (1 << 61));
            assert_eq!(size, 0x200000);
            assert!(backing + size as u64 <= g16_render::PRIVATE_SIZE as u64);
            if buffer != 0 {
                let previous = aliases[15 + buffer];
                assert!(va >= previous.0 + previous.2 as u64 + 0x8000);
            }
        }
        for offset in (0x40000..0x58000).step_by(0x4000) {
            let mut page = [0; 0x4000];
            g16_render::private_page(&pool, offset, &mut page).unwrap();
            for index in 0..0x800 {
                let entry = (offset - 0x40000) / 8 + index;
                let (va, _, size) = aliases[16 + entry / 512];
                let address = read64(&page, index * 8);
                assert_eq!(address, va + (entry % 512) as u64 * 0x1000);
                assert!(address + 0x1000 <= va + size as u64);
            }
        }
    }
}
macro_rules! parameters {
    ($module:ident, $scene:expr, $layers:expr, $samples:expr) => {{
        let mut p = $module::Parameters::default();
        p.set_private(0x10e0000000, 0x1100000000).unwrap();
        p.width = 640 + $scene;
        p.height = 480;
        p.layers = $layers;
        p.samples = $samples;
        p.utile_width = 32;
        p.utile_height = 16;
        p.tib_blocks = 64;
        p.encoder = p.context_base + 0x10000;
        p.scene_slot = $scene;
        p.cycle = 0x240000 + $scene * 0x30;
        p.tiling_gate = 0xa900400020;
        p.fragment_gate = 0xd500400020;
        p.tiling_work_stamp = 0x12345678;
        p.fragment_work_stamp = 0x76543210;
        p.request_tvb_growth = true;
        p.completion_control = 1;
        p
    }};
}

#[test]
fn ordinary_work_is_byte_identical_to_214() {
    for scene in 0..80 {
        for layers in [1, 2, 16] {
            for samples in [1, 2, 4] {
                let old = parameters!(baseline, scene, layers, samples);
                let new = parameters!(g16_render, scene, layers, samples);
                assert_eq!(old.tiling_registers(), new.tiling_registers());
                assert_eq!(old.fragment_registers(), new.fragment_registers());
                let old_a = baseline::Addresses::bootstrap();
                let new_a = g16_render::Addresses::bootstrap();
                let mut old_ta = [0; 0x940];
                let mut new_ta = [0; 0x940];
                let mut old_frag = [0; 0xcc0];
                let mut new_frag = [0; 0xcc0];
                old.tiling_work(&old_a, &mut old_ta).unwrap();
                new.tiling_work(&new_a, &mut new_ta).unwrap();
                old.fragment_work(&old_a, &mut old_frag).unwrap();
                new.fragment_work(&new_a, &mut new_frag).unwrap();
                assert_eq!(old_ta, new_ta);
                assert_eq!(old_frag, new_frag);
            }
        }
    }
}

#[test]
fn mesh_ta_has_ordered_extension_and_preserves_scene_selection() {
    for scene in 0..80 {
        let mut p = parameters!(g16_render, scene, 1, 1);
        let ordinary = p.tiling_registers().unwrap();
        let fragment = p.fragment_registers().unwrap();
        let payload = 0x10e4000000 + scene * 0x4000;
        p.mesh = Some(Profile { payload });
        let registers = p.tiling_registers().unwrap();
        assert_eq!(registers[55].0, 0x10799);
        assert_eq!(registers[56], (0x1ca48, 0x0200000000000000 | payload));
        assert!(!registers.iter().any(|r| r.0 == 0x10209));
        for &(reg, value) in &ordinary {
            if reg != 0x10209 && reg != 0xa5a1 {
                assert!(registers.contains(&(reg, value)));
            }
        }
        assert_eq!(p.fragment_registers().unwrap(), fragment);
        assert_eq!(
            registers.iter().find(|r| r.0 == 0xa5a1).unwrap().1,
            Profile::GATE
        );
        let mut work = [0; 0x940];
        p.tiling_work(&g16_render::Addresses::bootstrap(), &mut work)
            .unwrap();
        assert_eq!(&work[0x748..0x74c], &[0x42, 0, 0x18, 3]);
        assert_eq!(read32(&work, 0x86c), 1);
        assert_eq!(read64(&work, 0x908), 0x13300);
    }
    for payload in [0, 1, 0x10e4000001, 1 << 40] {
        assert_eq!(Profile { payload }.payload_register(), None);
    }
}

// Small I/O double for the production scratch allocation method.
type Result<T = ()> = std::result::Result<T, i32>;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
mod g16_timeline {
    pub struct Timer;
    pub static P_PREPARE_RENDER: () = ();
    impl Timer {
        pub fn start(_: &()) -> Self {
            Self
        }
    }
}
const UAT_PGSZ: usize = 0x4000;
struct Scratch {
    range: std::ops::Range<u64>,
}
struct ScratchVm {
    next: u64,
    leases: Vec<(usize, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
}
impl ScratchVm {
    fn lease_scratch(&mut self, _: bool, size: usize, metadata: usize) -> Result<Scratch> {
        self.leases.push((size, metadata));
        let start = self.next;
        self.next += size as u64;
        Ok(Scratch {
            range: start..self.next,
        })
    }
    fn write_low(&mut self, va: u64, data: &[u8]) -> Result {
        self.writes.push((va, data.to_vec()));
        Ok(())
    }
    fn sync(&self) {}
    /* PREPARE_RENDER */
}

#[test]
fn payload_has_a_distinct_cleared_page_in_each_render_lease() {
    let mut vm = ScratchVm {
        next: 0x10e4000000,
        leases: Vec::new(),
        writes: Vec::new(),
    };
    let mut ordinary = parameters!(g16_render, 0, 1, 1);
    let ordinary_lease = vm.prepare_render(&mut ordinary).unwrap();
    assert_eq!(vm.writes.len(), 1);
    let mut previous = ordinary_lease.range;
    for _ in 0..8 {
        let mut mesh = parameters!(g16_render, 0, 1, 1);
        mesh.mesh = Some(Profile { payload: 0 });
        vm.writes.clear();
        let lease = vm.prepare_render(&mut mesh).unwrap();
        let address = mesh.mesh.unwrap().payload;
        assert_eq!(lease.range.end - address, Profile::PAYLOAD_BYTES as u64);
        assert!(address >= mesh.aux_fb + 0x4000);
        assert!(lease.range.start >= previous.end);
        let clears: Vec<_> = vm.writes.iter().filter(|(va, _)| *va >= address).collect();
        assert_eq!(
            clears.iter().map(|(_, data)| data.len()).sum::<usize>(),
            Profile::PAYLOAD_BYTES
        );
        assert!(clears.iter().all(|(_, data)| data.iter().all(|&b| b == 0)));
        previous = lease.range;
    }
    assert_eq!(vm.leases[1].0 - vm.leases[0].0, Profile::PAYLOAD_BYTES);
}
