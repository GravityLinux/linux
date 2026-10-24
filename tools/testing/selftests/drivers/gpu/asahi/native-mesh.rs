// SPDX-License-Identifier: GPL-2.0-only
// Included by test-native-mesh.py, together with the actual production methods.
use g16_mesh::{DecodeError, Mode, Profile};

fn stream() -> Vec<u32> {
    // T16 mesh state with the research carrier's scalar object dispatch.
    vec![
        0x40000d2c, 0x504, 0x10, 0, 0, 0x01000000, 0x22224, 0x01000000, 0x22220, 1, 0, 0x400810, 0,
        5, 0x500, 0x4001000, 0x20000013, 0x70000600, 1, 1, 1, 1, 1, 1, 0xc0000000,
    ]
}
fn decode(words: &[u32], base: u64) -> std::result::Result<Option<Profile>, DecodeError> {
    g16_mesh::decode(base, Mode::MultipleParents, |va| {
        words.get(((va - base) / 4) as usize).copied()
    })
}

#[test]
fn mode_values_keep_disabled_and_single_parent_behavior_explicit() {
    for (value, expected) in [
        (0, Mode::Disabled),
        (1, Mode::SingleParent),
        (2, Mode::MultipleParents),
        (3, Mode::Research),
        (4, Mode::Qualified),
    ] {
        let mode = Mode::from_parameter(value).unwrap();
        assert_eq!(mode, expected);
        assert_eq!(mode.enabled(), value != 0);
    }
    for value in [5, 6, 255, u32::MAX] {
        assert_eq!(Mode::from_parameter(value), None);
    }
    assert_eq!(
        g16_mesh::decode(3, Mode::Disabled, |_| {
            panic!("disabled native mode must not read command memory")
        }),
        Ok(None)
    );
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
fn object_dispatch_accepts_complete_scalar_or_physical_t16_t32_grids() {
    for base in [0x1004000000, 0x10d1233ffc] {
        for (mesh, width) in [(16, 1), (16, 16), (16, 32), (32, 1), (32, 16), (32, 32)] {
            let mut words = stream();
            words[11] = 0x400800 | mesh;
            words[21] = width;
            for groups in [
                [1, 1, 1],
                [2, 1, 1],
                [32, 1, 1],
                [1, 2, 1],
                [1, 1, 32],
                [2, 3, 4],
                [32, 32, 32],
                [65535, 1, 1],
            ] {
                words[18..21].copy_from_slice(&[groups[0] * width, groups[1], groups[2]]);
                assert_eq!(decode(&words, base), Ok(Some(Profile { payload: 0 })));
                let single = g16_mesh::decode(base, Mode::SingleParent, |va| {
                    words.get(((va - base) / 4) as usize).copied()
                });
                assert_eq!(
                    single,
                    if groups == [1, 1, 1] {
                        Ok(Some(Profile { payload: 0 }))
                    } else {
                        Err(DecodeError::Unsupported)
                    }
                );
                for len in 0..words.len() {
                    assert!(decode(&words[..len], base).is_err(), "truncated at {len}");
                }
                for index in 18..21 {
                    let mut bad = words.clone();
                    bad[index] = 0;
                    assert_eq!(decode(&bad, base), Err(DecodeError::Unsupported));
                }
            }
            words[18..21].copy_from_slice(&[width, 1, 1]);
            // Partial workgroups and wider/nonlinear local shapes stay out.
            if width > 1 {
                for total in [1, width - 1, width + 1, 32 * width - 1, u32::MAX] {
                    let mut bad = words.clone();
                    bad[18] = total;
                    assert_eq!(decode(&bad, base), Err(DecodeError::Unsupported));
                }
            }
            for dimensions in [
                [0, 1, 1],
                [16, 0, 1],
                [32, 1, 0],
                [2, 1, 1],
                [31, 1, 1],
                [33, 1, 1],
                [64, 1, 1],
                [128, 1, 1],
                [1, 16, 1],
                [1, 1, 16],
                [16, 2, 1],
                [16, 1, 2],
                [1, 32, 1],
                [1, 1, 32],
                [32, 2, 1],
                [32, 1, 2],
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
fn mesh_physical_width_accepts_only_exact_t16_t32_state() {
    for width in 0..=255 {
        let mut words = stream();
        words[11] = 0x400800 | width;
        let result = decode(&words, 0x1004000000);
        if width == 16 || width == 32 {
            assert_eq!(result, Ok(Some(Profile { payload: 0 })));
        } else {
            assert_eq!(result, Err(DecodeError::Unsupported));
        }
    }
    for state in [0x400810, 0x400820] {
        for bit in 0..32 {
            let mut words = stream();
            words[11] = state ^ (1 << bit);
            assert_eq!(decode(&words, 0x1004000000), Err(DecodeError::Unsupported));
        }
    }
}

#[test]
fn child_grid_and_launch_backing_are_not_cpu_admission_inputs() {
    // Direct API XYZ and indirect buffer XYZ both live behind the object
    // launch's argument pointer. The GPU reads them after submission. Neither
    // that pointer, the grid, nor the USC frame/allocation bytes are packet
    // fields, and the parser must not chase them or snapshot a GPU-produced grid.
    let base = 0x1004003ffc;
    for mesh in [16, 32] {
        let mut words = stream();
        words[11] = 0x400800 | mesh;
        words[18] = 16;
        words[21] = 16;
        let mut reads = Vec::new();
        let result = g16_mesh::decode(base, Mode::MultipleParents, |va| {
            reads.push(va);
            // All launch, argument, indirect and count-buffer backing is
            // intentionally absent from this CPU command-memory view.
            let offset: usize = va.checked_sub(base)?.try_into().ok()?;
            words.get(offset / 4).copied()
        });
        assert_eq!(result, Ok(Some(Profile { payload: 0 })));
        assert_eq!(
            reads,
            (0..words.len())
                .map(|i| base + 4 * i as u64)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn ordinary_data_is_not_searched_for_mesh_magic() {
    let words = [0x4000002e, 0x40000d2c, 0x504, 0x10];
    let mut reads = 0;
    assert_eq!(
        g16_mesh::decode(0x1004000000, Mode::MultipleParents, |va| {
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
            (11, 0x400840), // physical T64 remains excluded
            (12, 1),        // primitive output components
            (18, 0),        // empty parent dispatch
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
        (11, 0x400840),
        (12, 3),
        (13, 9),
        (17, 0x74000600),
        (18, 0),
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

// Research admission (native_mesh=3). `research` builds a stream forward from
// the study's model; the decoder must accept exactly those streams.
fn research(planes: u32, primitives: u32, quads: u32, components: u32, draw: u32) -> Vec<u32> {
    let mut words = stream();
    words[1] = planes | ((planes + 1) << 8);
    words[2] = if components > 0 { 0xd0 } else { 0x10 };
    words[10] = (primitives - 1) | ((primitives.div_ceil(16) - 1) << 16);
    words[12] = primitives * components;
    words[13] = 1 + quads * planes;
    words[17] = draw;
    words
}
fn decode_mode(words: &[u32], mode: Mode) -> std::result::Result<Option<Profile>, DecodeError> {
    let base = 0x1004003ffc;
    g16_mesh::decode(base, mode, |va| words.get(((va - base) / 4) as usize).copied())
}
fn model_output(planes: u32, primitives: u32, quads: u32, components: u32) -> u32 {
    quads * 16 * (planes - u32::from(components > 0)) + 4 * primitives * components
}
const DRAWS: [u32; 3] = [0x70000000, 0x70000100, 0x70000600];
const ACCEPT: std::result::Result<Option<Profile>, DecodeError> = Ok(Some(Profile { payload: 0 }));
const REJECT: std::result::Result<Option<Profile>, DecodeError> = Err(DecodeError::Unsupported);

#[test]
fn research_mode_admits_the_capability_study_fixtures() {
    // check_admission.py's nine fixtures: (V, P, n, C, draw).
    for (v, p, n, c, draw) in [
        (3, 1, 4, 0, 0x70000600),
        (4, 2, 4, 0, 0x70000600),
        (16, 16, 4, 0, 0x70000600),
        (64, 128, 4, 0, 0x70000600),
        (128, 256, 4, 0, 0x70000600),
        (256, 512, 4, 0, 0x70000600),
        (4, 4, 5, 0, 0x70000000),
        (16, 16, 4, 0, 0x70000100),
        (4, 2, 5, 4, 0x70000600),
    ] {
        let words = research(n, p, u32::div_ceil(v, 4), c, draw);
        assert_eq!(decode_mode(&words, Mode::Research), ACCEPT, "V{v} P{p}");
        let narrow = (v, p, c, draw) == (3, 1, 0, 0x70000600);
        for mode in [Mode::SingleParent, Mode::MultipleParents] {
            assert_eq!(decode_mode(&words, mode), if narrow { ACCEPT } else { REJECT });
        }
    }
}

#[test]
fn research_mode_accepts_the_model_range_and_nothing_beyond_it() {
    for planes in [3, 4, 5, 6, 32, 127, 128, 129] {
        for primitives in (1..=513).filter(|p| *p <= 17 || p % 31 == 0 || *p >= 511) {
            for quads in [0, 1, 2, 16, 63, 64, 65] {
                for components in [0, 1, 4, 15, 64, 128, 129] {
                    let words = research(planes, primitives, quads, components, 0x70000600);
                    let valid = (4..=128).contains(&planes)
                        && primitives <= 512
                        && (1..=64).contains(&quads)
                        && components <= 128
                        && (components == 0 || planes >= 5)
                        && model_output(planes, primitives, quads, components) <= 0x8000;
                    let result = decode_mode(&words, Mode::Research);
                    assert_eq!(result, if valid { ACCEPT } else { REJECT },
                               "n{planes} P{primitives} q{quads} C{components}");
                }
            }
        }
    }
    // The compiler's largest accepted case: V256 with seven float4 varyings.
    assert_eq!(decode_mode(&research(32, 1, 64, 0, 0x70000600), Mode::Research), ACCEPT);
    assert_eq!(decode_mode(&research(33, 1, 64, 0, 0x70000600), Mode::Research), REJECT);
}

#[test]
fn research_mutations_are_rejected_or_reproduce_the_model() {
    for (n, p, q, c) in [(4, 1, 1, 0), (4, 512, 64, 0), (5, 2, 1, 4), (17, 33, 9, 3), (128, 1, 1, 0)] {
        for draw in DRAWS {
            let words = research(n, p, q, c, draw);
            assert_eq!(decode_mode(&words, Mode::Research), ACCEPT);
            for index in [1, 2, 3, 4, 9, 10, 11, 12, 13, 17] {
                for bit in 0..32 {
                    let mut bad = words.clone();
                    bad[index] ^= 1 << bit;
                    if decode_mode(&bad, Mode::Research) != ACCEPT {
                        continue;
                    }
                    // An accepted mutation must itself be a model stream.
                    let n = bad[1] & 0xff;
                    let p = (bad[10] & 0xffff) + 1;
                    let c = bad[12] / p;
                    let q = (bad[13] - 1) / n;
                    let draw = bad[17];
                    let mut expected = research(n, p, q, c, draw);
                    expected[11] = bad[11];
                    assert_eq!(bad, expected, "word {index} bit {bit}");
                    assert!(DRAWS.contains(&draw));
                    assert!(model_output(n, p, q, c) <= 0x8000);
                }
            }
        }
    }
}

#[test]
fn research_mode_keeps_stream_structure_and_read_bounds() {
    for draw in DRAWS {
        let words = research(4, 512, 64, 0, draw);
        let mut culled = words.clone();
        culled[10] |= 1 << 30;
        assert_eq!(decode_mode(&culled, Mode::Research), REJECT);
        for header in [0x70000200, 0x70000700, 0x74000600, draw | 1] {
            let mut bad = words.clone();
            bad[17] = header;
            assert_eq!(decode_mode(&bad, Mode::Research), REJECT);
        }
        let mut twice = words.clone();
        twice.pop();
        twice.extend_from_slice(&words[17..]);
        assert_eq!(decode_mode(&twice, Mode::Research), REJECT);
        for len in 0..words.len() {
            assert!(decode_mode(&words[..len], Mode::Research).is_err());
        }
        // Multiple parents, as in mode 2.
        let mut parents = words.clone();
        parents[18] = 4;
        assert_eq!(decode_mode(&parents, Mode::Research), ACCEPT);
        assert_eq!(decode_mode(&parents, Mode::SingleParent), REJECT);
        // Exactly one sequential read per packet word, as in modes 1 and 2.
        let base = 0x1004003ffc;
        let mut reads = Vec::new();
        let result = g16_mesh::decode(base, Mode::Research, |va| {
            reads.push(va);
            words.get(((va - base) / 4) as usize).copied()
        });
        assert_eq!(result, ACCEPT);
        assert_eq!(reads, (0..words.len()).map(|i| base + 4 * i as u64).collect::<Vec<_>>());
    }
    let mut words = stream()[..14].to_vec();
    words.extend(std::iter::repeat_n(0x20000013, 1100));
    assert_eq!(decode_mode(&words, Mode::Research), REJECT);
}

#[test]
fn modes_one_and_two_are_unchanged_by_research_admission() {
    // Every wider model stream stays rejected outside mode 3.
    for draw in DRAWS {
        for (n, p, q, c) in [(4, 2, 1, 0), (4, 1, 2, 0), (5, 1, 1, 1), (4, 512, 64, 0)] {
            let words = research(n, p, q, c, draw);
            assert_eq!(decode_mode(&words, Mode::Research), ACCEPT);
            for mode in [Mode::SingleParent, Mode::MultipleParents] {
                assert_eq!(decode_mode(&words, mode), REJECT);
            }
        }
    }
    for draw in [0x70000000, 0x70000100] {
        for mode in [Mode::SingleParent, Mode::MultipleParents] {
            assert_eq!(decode_mode(&research(4, 1, 1, 0, draw), mode), REJECT);
        }
    }
}

#[test]
fn research_mode_admits_physical_w64_only_for_the_probe_shape() {
    const W64: u32 = 0x400840;
    for draw in DRAWS {
        // The W64 study's shape: 4 planes, one primitive, one quad, no data.
        let mut words = research(4, 1, 1, 0, draw);
        words[11] = W64;
        assert_eq!(decode_mode(&words, Mode::Research), ACCEPT);
        for mode in [Mode::SingleParent, Mode::MultipleParents] {
            assert_eq!(decode_mode(&words, mode), REJECT);
        }
        // Any other model shape stays T16/T32-only at W64.
        for (n, p, q, c) in [(5, 1, 1, 0), (4, 2, 1, 0), (4, 1, 2, 0), (5, 1, 1, 1)] {
            let mut wide = research(n, p, q, c, draw);
            assert_eq!(decode_mode(&wide, Mode::Research), ACCEPT);
            wide[11] = W64;
            assert_eq!(decode_mode(&wide, Mode::Research), REJECT, "n{n} P{p} q{q} C{c}");
        }
        // Wider physical widths and other mesh state words stay rejected.
        for state in [0x400880, 0x400900, 0x400841, 0x400040] {
            let mut bad = words.clone();
            bad[11] = state;
            assert_eq!(decode_mode(&bad, Mode::Research), REJECT);
        }
    }
}

// The 31 qualified classes, (physical width, maximum V, maximum P), in
// Mesa's hk_apple9_native_index_shapes.def order.
const QUALIFIED_CLASSES: [(u32, u32, u32); 31] = [
    (16, 3, 1), (16, 3, 2), (16, 3, 3), (16, 3, 4), (16, 3, 5), (16, 3, 6), (16, 3, 7), (16, 3, 8),
    (16, 3, 11), (16, 3, 17), (16, 3, 20), (16, 3, 21), (16, 4, 1), (16, 4, 2), (16, 6, 2), (16, 8, 3),
    (16, 8, 5), (16, 8, 8), (16, 16, 4), (16, 16, 8), (16, 16, 17), (16, 16, 21), (32, 3, 1), (32, 24, 3),
    (32, 24, 17), (32, 24, 31), (32, 24, 32), (32, 32, 16), (32, 32, 32), (32, 32, 41), (32, 32, 42),
];
fn width_word(width: u32) -> u32 {
    if width == 16 { 0x400810 } else { 0x400820 }
}
fn qualified_stream(width: u32, vertices: u32, primitives: u32) -> Vec<u32> {
    let mut words = research(4, primitives, vertices.div_ceil(4), 0, 0x70000600);
    words[11] = width_word(width);
    words
}

#[test]
fn qualified_mode_admits_the_31_classes() {
    for (t, v, p) in QUALIFIED_CLASSES {
        // The scalar fixture carrier and Honeykrisp's (16, 1, 1) single parent.
        for local in [1, 16] {
            let mut words = qualified_stream(t, v, p);
            words[18..24].copy_from_slice(&[local, 1, 1, local, 1, 1]);
            assert_eq!(decode_mode(&words, Mode::Qualified), ACCEPT, "T{t}V{v}P{p} local {local}");
            // The research mode they were qualified under admits them too.
            assert_eq!(decode_mode(&words, Mode::Research), ACCEPT, "T{t}V{v}P{p} local {local}");
        }
    }
}

#[test]
fn qualified_mode_admits_nothing_else_from_the_research_model() {
    let layout = |t: u32, q: u32, p: u32| {
        QUALIFIED_CLASSES.iter().any(|&(ct, cv, cp)| ct == t && cv.div_ceil(4) == q && cp == p)
    };
    let mut admitted = 0;
    for t in [16, 32] {
        for planes in [4, 5, 8] {
            for components in [0, 1] {
                for quads in 1..=10 {
                    for p in 1..=64 {
                        for draw in DRAWS {
                            let mut words = research(planes, p, quads, components, draw);
                            words[11] = width_word(t);
                            if components > 0 && planes < 5 {
                                continue;
                            }
                            let expected = planes == 4 && components == 0 && draw == 0x70000600 && layout(t, quads, p);
                            let result = decode_mode(&words, Mode::Qualified);
                            assert_eq!(result, if expected { ACCEPT } else { REJECT },
                                       "T{t} n{planes} C{components} q{quads} P{p} draw {draw:#x}");
                            admitted += usize::from(expected);
                        }
                    }
                }
            }
        }
    }
    // 29 layouts: V3 and V4 share one at P1 and P2.
    assert_eq!(admitted, 29);
    // Excluded on purpose: T16V16P16, W64 and multi-parent dispatch.
    assert_eq!(decode_mode(&qualified_stream(16, 16, 16), Mode::Qualified), REJECT);
    let mut w64 = research(4, 1, 1, 0, 0x70000600);
    w64[11] = 0x400840;
    assert_eq!(decode_mode(&w64, Mode::Research), ACCEPT);
    assert_eq!(decode_mode(&w64, Mode::Qualified), REJECT);
    // Multiple parents (scalar or T16), and the T32 local size, stay rejected.
    for dispatch in [[2, 1, 1, 1, 1, 1], [32, 1, 1, 16, 1, 1], [16, 2, 1, 16, 1, 1], [32, 1, 1, 32, 1, 1], [64, 1, 1, 32, 1, 1]] {
        let mut words = qualified_stream(16, 3, 1);
        words[18..24].copy_from_slice(&dispatch);
        assert_eq!(decode_mode(&words, Mode::Qualified), REJECT, "dispatch {dispatch:?}");
    }
}

#[test]
fn qualified_mutations_are_rejected() {
    for (t, v, p) in QUALIFIED_CLASSES {
        let words = qualified_stream(t, v, p);
        for index in 1..words.len() {
            for flip in [1u32, 0x10, 0x100, 0x10000, 0x80000000] {
                let mut bad = words.clone();
                bad[index] ^= flip;
                let result = decode_mode(&bad, Mode::Qualified);
                if result != ACCEPT {
                    continue;
                }
                // Only a flip that lands on another qualified layout may pass,
                // or one outside the state, draw and dispatch: launch handles
                // (words 5..8, consumed as submitted) and the PPP update and
                // coherency barrier (words 14..16), which every mode accepts
                // alike, as the research mode confirms.
                let other = QUALIFIED_CLASSES.iter().any(|&(ct, cv, cp)| qualified_stream(ct, cv, cp) == bad);
                let structural = (5..=8).contains(&index)
                    || ((14..=16).contains(&index) && decode_mode(&bad, Mode::Research) == ACCEPT);
                assert!(other || structural, "T{t}V{v}P{p} word {index} ^ {flip:#x}");
            }
        }
    }
}

#[test]
fn the_unwaited_peek_agrees_with_decode_on_every_first_word() {
    // decode returns Ok(None) after reading only the first word exactly when
    // may_be_native is false; otherwise it reads on (and decides only then).
    let mut first_words: Vec<u32> = vec![0x40000d2c, 0x40000d2d, 0x40000c2c, 0x4000082c, 0x40000000, 0x5fffffff,
                                        0, 1, 0x20000013, 0x70000600, 0xc0000000, 0x60000000, u32::MAX];
    let mut x = 0x9e3779b9u32;
    for _ in 0..20000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        first_words.push(x);
        first_words.push((x & !0xe000_0000) | 0x4000_0000);
    }
    for first in first_words {
        for mode in [Mode::SingleParent, Mode::MultipleParents, Mode::Research, Mode::Qualified] {
            let mut reads = 0;
            let result = g16_mesh::decode(0x1000, mode, |va| {
                reads += 1;
                (va == 0x1000).then_some(first)
            });
            if g16_mesh::may_be_native(first) {
                assert!(result.is_err() || reads > 1, "{first:#x} {mode:?}");
            } else {
                assert_eq!((result, reads), (Ok(None), 1), "{first:#x} {mode:?}");
            }
        }
    }
    assert!(g16_mesh::may_be_native(stream()[0]));
}

#[test]
fn every_mode_write_starts_a_new_generation() {
    // The only test touching the runtime mode, so it owns the static.
    g16_mesh::init_mode(4);
    assert_eq!(g16_mesh::snapshot(), (0, 4));
    assert_eq!(g16_mesh::set_mode(0), 1);
    assert_eq!(g16_mesh::snapshot(), (1, 0));
    // Rewriting the current value still bumps the generation.
    assert_eq!(g16_mesh::set_mode(0), 2);
    assert_eq!(g16_mesh::set_mode(4), 3);
    assert_eq!(g16_mesh::snapshot(), (3, 4));
    assert_eq!(g16_mesh::mode(), 4);
    // Out-of-range values are clamped (debugfs rejects them before this).
    g16_mesh::set_mode(200);
    assert_eq!(g16_mesh::snapshot(), (4, g16_mesh::MAX_MODE));
    g16_mesh::init_mode(0);
}

#[test]
fn the_advertisement_line_is_exact_per_mode() {
    let line = |generation, mode| {
        let mut out = String::new();
        g16_mesh::advertise(generation, mode, &mut out).unwrap();
        out
    };
    let hash = g16_mesh::QUALIFIED_CLASSES_SHA256;
    assert_eq!(line(7, 4), format!("asahi-native-mesh v1 generation=7 profile=qualified classes={hash}\n"));
    assert_eq!(line(0, 3), "asahi-native-mesh v1 generation=0 profile=research classes=-\n");
    for mode in [0, 1, 2] {
        assert_eq!(line(3, mode), "asahi-native-mesh v1 generation=3 profile=none classes=-\n");
    }
    // The longest line fits the sysfs buffer bound.
    assert!(line(u64::MAX >> 8, 4).len() <= g16_mesh::ADVERTISEMENT_MAX);
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
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
