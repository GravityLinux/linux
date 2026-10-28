// SPDX-License-Identifier: GPL-2.0-only

//! Scheduled-family board configuration from typed FDT properties and OPPs.
//!
//! m1n1 selects/calibrates the board's performance states. Linux consumes
//! ordinary FDT cells, as Asahi does; it never parses or groups raw ADT data.

use super::scheduled_hwdata::{CsState, ParametersG16C};
use super::scheduled_initdata::{HardwareRegion, MmioRegion};
use kernel::{c_str, of, prelude::*};

pub(crate) struct BoardG16C {
    pub(crate) freq_a: [u32; 11],
    pub(crate) freq_b: [u32; 11],
    pub(crate) core_voltage: [u32; 11],
    pub(crate) memory_voltage: [u32; 11],
    pub(crate) cluster_core: [[u32; 4]; 11],
    pub(crate) cluster_memory: [[u32; 4]; 11],
    pub(crate) index_a: [u32; 11],
    pub(crate) index_b: [u32; 11],
    pub(crate) relative_a: [u32; 11],
    pub(crate) relative_b: [u32; 11],
    pub(crate) cs: [CsState; 16],
    pub(crate) cs_count: usize,
    pub(crate) revision: u32,
    pub(crate) period_clocks: u32,
    pub(crate) period_ms: u32,
    pub(crate) maximum_power: u32,
    pub(crate) gpc: bool,
}

impl BoardG16C {
    pub(crate) fn from_fdt(node: &of::Node) -> Result<Self> {
        let mut out = Self {
            freq_a: [0; 11],
            freq_b: [0; 11],
            core_voltage: [0; 11],
            memory_voltage: [0; 11],
            cluster_core: [[0; 4]; 11],
            cluster_memory: [[0; 4]; 11],
            index_a: [0; 11],
            index_b: [0; 11],
            relative_a: [0; 11],
            relative_b: [0; 11],
            cs: [CsState {
                frequency: 0,
                core_voltage: 0,
                memory_voltage: 0,
            }; 16],
            cs_count: 0,
            revision: node.get_property(c_str!("apple,chip-revision"))?,
            period_clocks: node.get_property(c_str!("apple,pwr-sample-period-aic-clks"))?,
            period_ms: node.get_property(c_str!("apple,power-sample-period"))?,
            maximum_power: node.get_property(c_str!("apple,max-power-milliwatt"))?,
            gpc: node.get_property::<u32>(c_str!("apple,gpc-enabled"))? != 0,
        };
        if out.period_ms == 0
            || out.period_clocks / 24000 != out.period_ms
            || out.maximum_power == 0
        {
            return Err(EINVAL);
        }
        let opps = node
            .parse_phandle(c_str!("operating-points-v2"), 0)
            .ok_or(ENODEV)?;
        let mut count = 0;
        for opp in opps.children() {
            if count == 11 {
                return Err(EINVAL);
            }
            let high: u64 = opp.get_property(c_str!("opp-hz"))?;
            let low: u64 = opp.get_property(c_str!("apple,alternate-opp-hz"))?;
            let core: KVec<u32> = opp.get_property(c_str!("opp-microvolt"))?;
            let memory: KVec<u32> = opp.get_property(c_str!("apple,sram-microvolt"))?;
            let index: KVec<u32> = opp.get_property(c_str!("apple,perf-index"))?;
            let relative: KVec<u32> = opp.get_property(c_str!("apple,relative-power"))?;
            if core.len() != 4
                || memory.len() != 4
                || index.len() != 2
                || relative.len() != 2
                || index.iter().any(|v| *v >= 16)
                || relative.iter().any(|v| *v > 100)
            {
                return Err(EINVAL);
            }
            out.freq_a[count] = (high / 1_000_000).try_into()?;
            out.freq_b[count] = (low / 1_000_000).try_into()?;
            if (count == 0 && (out.freq_a[count] != 0 || out.freq_b[count] != 0))
                || (count != 0
                    && (out.freq_a[count] <= out.freq_a[count - 1]
                        || out.freq_b[count] <= out.freq_b[count - 1]))
            {
                return Err(EINVAL);
            }
            out.index_a[count] = index[0];
            out.index_b[count] = index[1];
            out.relative_a[count] = relative[0];
            out.relative_b[count] = relative[1];
            for cluster in 0..4 {
                out.cluster_core[count][cluster] = core[cluster] / 1000;
                out.cluster_memory[count][cluster] = memory[cluster] / 1000;
            }
            out.core_voltage[count] = out.cluster_core[count][0];
            out.memory_voltage[count] = out.cluster_memory[count][0];
            count += 1;
        }
        if count != 11 {
            return Err(EINVAL);
        }
        let minimum: u32 = node.get_property(c_str!("apple,csafr-min-sram-microvolt"))?;
        let opps = node
            .parse_phandle(c_str!("apple,cs-opp"), 0)
            .ok_or(ENODEV)?;
        for opp in opps.children() {
            if out.cs_count == 16 {
                return Err(EINVAL);
            }
            let hz: u64 = opp.get_property(c_str!("opp-hz"))?;
            let voltage: u32 = opp.get_property(c_str!("opp-microvolt"))?;
            out.cs[out.cs_count] = CsState {
                frequency: (hz / 1_000_000).try_into()?,
                core_voltage: voltage / 1000,
                memory_voltage: voltage.max(minimum) / 1000,
            };
            out.cs_count += 1;
        }
        if out.cs_count == 0 {
            return Err(EINVAL);
        }
        Ok(out)
    }
}

/// All relocatable graph links are explicit inputs to construction.
#[derive(Clone, Copy)]
pub(crate) struct HardwarePointers {
    pub(crate) status: u64,
    pub(crate) computed: u64,
    pub(crate) timestamp: u64,
}

impl BoardG16C {
    pub(crate) fn hardware<'a>(
        &'a self,
        registers: &'a [(usize, MmioRegion)],
        pointers: HardwarePointers,
        regions: Option<&'a [HardwareRegion; 2]>,
    ) -> ParametersG16C<'a> {
        ParametersG16C {
            registers,
            freq_a: &self.freq_a,
            freq_b: &self.freq_b,
            core_voltage: &self.core_voltage,
            memory_voltage: &self.memory_voltage,
            cluster_core: &self.cluster_core,
            cluster_memory: &self.cluster_memory,
            relative_a: &self.relative_a,
            relative_b: &self.relative_b,
            index_a: &self.index_a,
            index_b: &self.index_b,
            cs: &self.cs[..self.cs_count],
            revision: self.revision,
            gpc: self.gpc,
            period_ms: self.period_ms,
            hardware_status: pointers.status,
            computed: pointers.computed,
            timestamp: pointers.timestamp,
            regions,
        }
    }
}
