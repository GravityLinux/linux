// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]

#[path = "../g17p_abi.rs"]
mod g17p_abi;
#[path = "../g17p_initgraph.rs"]
mod g17p_initgraph;
#[path = "../g17p_layout.rs"]
mod g17p_layout;

use g17p_initgraph::{Storage, OBJECT_LAYOUT};
use std::{env, fs, path::Path};

struct Image(Vec<Vec<u8>>);

impl Storage for Image {
    fn object(&mut self, index: usize) -> Result<&mut [u8], g17p_abi::InvalidSize> {
        self.0
            .get_mut(index)
            .map(Vec::as_mut_slice)
            .ok_or(g17p_abi::InvalidSize)
    }
}

fn main() {
    let directory = env::args().nth(1).expect("output directory");
    let directory = Path::new(&directory);
    fs::create_dir_all(directory).unwrap();
    for case in 0..4u64 {
        let base = 0xffff_fc20_0000_0000 + case * 0x2_0000_0000;
        let mut image = Image(
            OBJECT_LAYOUT
                .iter()
                .map(|object| vec![0xa5; object.size])
                .collect(),
        );
        let mut performance = g17p_layout::PERFORMANCE;
        performance.freq_a[10] += case as u32;
        performance.core_voltage[3] += case as u32;
        let graph = g17p_initgraph::build(&mut image, base, &performance).unwrap();
        let mut manifest = String::new();
        for (index, object) in OBJECT_LAYOUT.iter().enumerate() {
            let address = graph.addresses[index];
            manifest += &format!("{address:x} {:x} {:x}\n", object.size, object.pte_flags);
            fs::write(
                directory.join(format!("{case}-{address:x}.bin")),
                &image.0[index],
            )
            .unwrap();
        }
        fs::write(directory.join(format!("{case}-objects.txt")), manifest).unwrap();
        let channels = graph
            .channels
            .iter()
            .flatten()
            .flat_map(|channel| {
                channel
                    .states
                    .iter()
                    .copied()
                    .chain(std::iter::once(channel.ring))
                    .flat_map(u64::to_le_bytes)
            })
            .collect::<Vec<_>>();
        fs::write(directory.join(format!("{case}-channels.bin")), channels).unwrap();
        let aliases = graph
            .primary_aliases()
            .iter()
            .flat_map(|(low, high)| [*low, *high])
            .flat_map(u64::to_le_bytes)
            .collect::<Vec<_>>();
        fs::write(directory.join(format!("{case}-aliases.bin")), aliases).unwrap();
    }
    // An incomplete allocation or wrapping GPU range must fail before writes.
    for broken in 0..3 {
        let mut image = Image(
            OBJECT_LAYOUT
                .iter()
                .map(|object| vec![0xa5; object.size])
                .collect(),
        );
        let base = match broken {
            0 => {
                image.0[11].pop();
                0xffff_fc20_0000_0000
            }
            1 => 0xffff_ffff_ffff_c000,
            _ => 0xffff_fc20_0000_0001,
        };
        assert!(g17p_initgraph::build(&mut image, base, &g17p_layout::PERFORMANCE).is_err());
        assert!(image.0.iter().flatten().all(|byte| *byte == 0xa5));
    }
}
