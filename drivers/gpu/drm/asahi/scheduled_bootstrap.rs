// SPDX-License-Identifier: GPL-2.0-only

//! G16C source startup DATA transactions, before accepting caller work.
//! Wire objects remain owned by the common Memory/Vm. RTKit transport and
//! deadlines belong to Session; these functions never infer execution from ACKs.

use super::{g17p_memory::Memory, g17p_user_vm::UserVm, g17p_vm::Vm, scheduled_initgraph as graph};
use kernel::prelude::*;

pub(crate) const ROOT_MESSAGE: u64 = 0x00810c2000100000;
pub(crate) const CONTROL_MESSAGE: u64 = 0x0084000000000011;
const STATUS_A: u64 = graph::BASE + 0x3f180;
const STATUS_B: u64 = graph::BASE + 0x30000;
const CONTROL_STATE: u64 = graph::BASE + 0x20300;
const CONTROL_RING: u64 = graph::BASE + graph::BUNDLE_OFFSET as u64 + 0x1e5c0;

fn read32(memory: &Memory, vm: &Vm, va: u64) -> Result<u32> {
    memory.read_firmware32(vm.physical(memory, 2, va)?)
}
fn write64(memory: &mut Memory, vm: &Vm, va: u64, value: u64) -> Result {
    vm.write(memory, 2, va, &value.to_le_bytes())
}

pub(crate) fn read_words(memory: &Memory, vm: &Vm, va: u64, out: &mut [u8]) -> Result {
    let mut offset=0;
    while offset<out.len() {
        let address=va.checked_add(offset as u64).ok_or(EIO)?;
        let length=(0x4000-(address as usize&0x3fff)).min(out.len()-offset);
        memory.read_firmware_words(vm.physical(memory,2,address)?,&mut out[offset..offset+length])?;
        offset+=length;
    }
    Ok(())
}

pub(crate) fn publish_context(memory: &mut Memory, vm: &Vm, ttbs: u64, slot: usize) -> Result {
    let roots = vm.boot_context_roots(slot)?;
    let at = ttbs.checked_add(slot as u64 * 16).ok_or(EINVAL)?;
    memory.invalidate(at, 16)?;
    if memory.read64(at)? != 0 || memory.read64(at + 8)? != 0 {
        return Err(EBUSY);
    }
    // Match the source's invalid->TLBI->new roots transaction. The contexts
    // are unpublished, and every backing/table remains session-owned.
    memory.write64(at, 0)?;
    memory.write64(at + 8, 0)?;
    memory.clean(at, 16)?;
    UserVm::invalidate(slot as u16);
    for (index, root) in roots.into_iter().enumerate() {
        memory.write64(at + index as u64 * 8, root | ((slot as u64) << 48) | 1)?;
    }
    memory.clean(at, 16)?;
    vm.flush_tables(memory)
}

pub(crate) fn prepare_publication(memory: &mut Memory, vm: &Vm, ttbs: u64) -> Result {
    // Context validity is checked before exposing the first producer.
    memory.invalidate(ttbs, 16)?;
    if memory.read64(ttbs)? != 0 || memory.read64(ttbs + 8)? != 0 {
        return Err(EBUSY);
    }
    vm.write(memory, 2, CONTROL_STATE + 0x20, &1u32.to_le_bytes())?;
    publish_context(memory, vm, ttbs, 0)
}

pub(crate) fn accepted(memory: &Memory, vm: &Vm) -> Result<bool> {
    Ok(read32(memory, vm, STATUS_A + 0x14)? == 1)
}

pub(crate) fn activate_render_status(memory:&mut Memory,vm:&Vm)->Result {
    if !accepted(memory,vm)? {return Err(EIO);}
    // Both independently isolated Max words are required. Preserve all other
    // counters; derive each address from its own status allocation.
    for(address,value)in[(STATUS_B+0xeac0,4u32),(STATUS_A+0x1c,1)] {
        vm.write(memory,2,address,&value.to_le_bytes())?;
    }
    for(address,value)in[(STATUS_B+0xeac0,4u32),(STATUS_A+0x1c,1)] {
        if read32(memory,vm,address)?!=value{return Err(EIO);}
    }
    Ok(())
}

pub(crate) fn prepare_control(memory: &mut Memory, vm: &Vm) -> Result {
    for (offset, value) in [
        (0x4018, (4u64 << 32) | (10 << 40) | (40 << 48)),
        (0x4020, 0x03ff03ff03ff03ff),
        (0x40b0, 5),
        (0x48e0, graph::BASE + 0x150000),
        (0x48e8, graph::BASE + 0x150040),
    ] {
        write64(memory, vm, STATUS_B + offset, value)?;
    }
    Ok(())
}

pub(crate) fn control_retired(memory: &Memory, vm: &Vm, target: u32) -> Result<bool> {
    Ok([
        read32(memory, vm, CONTROL_STATE)?,
        read32(memory, vm, CONTROL_STATE + 0x10)?,
        read32(memory, vm, CONTROL_STATE + 0x20)?,
    ] == [target; 3])
}

pub(crate) fn control_cursors(memory:&Memory,vm:&Vm)->Result<[u32;3]> {
    let values=[read32(memory,vm,CONTROL_STATE)?,read32(memory,vm,CONTROL_STATE+0x10)?,read32(memory,vm,CONTROL_STATE+0x20)?];
    if values.iter().any(|&v|v>=256) {return Err(EIO);}
    Ok(values)
}
pub(crate) fn publish_control(memory:&mut Memory,vm:&Vm,record:&[u8;64])->Result<u32> {
    let [read,done,producer]=control_cursors(memory,vm)?;
    if read!=producer || done!=producer {return Err(EBUSY);}
    vm.write(memory,2,CONTROL_RING+producer as u64*64,record)?;
    super::g17p_memory::sync();
    let next=(producer+1)%256;
    vm.write(memory,2,CONTROL_STATE+0x20,&next.to_le_bytes())?;
    Ok(next)
}

pub(crate) fn prepare_maintenance(memory: &mut Memory, vm: &Vm) -> Result {
    for engine in 0..3u32 {
        for phase in 1..=2u32 {
            let slot = 1 + engine * 2 + phase - 1;
            let mut record = [0u8; 0x40];
            record[..4].copy_from_slice(&0x1bu32.to_le_bytes());
            record[4..8].copy_from_slice(&engine.to_le_bytes());
            record[8..12].copy_from_slice(&phase.to_le_bytes());
            vm.write(memory, 2, CONTROL_RING + slot as u64 * 0x40, &record)?;
        }
    }
    vm.write(memory, 2, CONTROL_STATE + 0x20, &7u32.to_le_bytes())
}

// Boot has no FList allocation or caller job. Completion hints can be consumed;
// timeout/growth/memory-limit/unowned receipts must remain failures. Runtime
// work will use the shared owner-aware dispatcher before publishing producers.
pub(crate) fn drain(memory: &mut Memory, vm: &Vm) -> Result {
    drain_with_flist(memory, vm, None)
}

pub(crate) fn drain_with_flist(
    memory: &mut Memory,
    vm: &Vm,
    receipt: Option<&mut super::g17p_render::admission::FListReceiptG16C>,
) -> Result {
    drain_events(memory, vm, receipt, None)
}

/// A true callback result accepts ONE owned work event before acknowledging it.
/// A false result leaves that event pending during an active control wait.
pub(crate) fn drain_with_work_event(memory: &mut Memory, vm: &Vm,
    work: &mut dyn FnMut(&Memory, &Vm, &[u8;0x48]) -> Result<bool>) -> Result {
    drain_events(memory, vm, None, Some(work))
}

fn drain_events(memory: &mut Memory, vm: &Vm,
    mut receipt: Option<&mut super::g17p_render::admission::FListReceiptG16C>,
    mut work: Option<&mut dyn FnMut(&Memory, &Vm, &[u8;0x48]) -> Result<bool>>) -> Result {
    for (state, ring, capacity, is_event) in [
        (graph::BASE + 0x3f1c0, graph::BASE + 0x3f440, 256, true),
        (graph::BASE + 0x3f3c0, graph::BASE + 0xe5c40, 512, false),
    ] {
        let mut consumer = read32(memory, vm, state)?;
        let mut producer = read32(memory, vm, state + 0x20)?;
        if consumer >= capacity || producer >= capacity {
            return Err(EIO);
        }
        for _ in 0..256 {
            if consumer == producer {
                break;
            }
            let mut work_accepted = false;
            if is_event {
                match read32(memory, vm, ring + consumer as u64 * 0x48)? {
                    1 => (),
                    13 => {
                        let expected = receipt.as_deref_mut().ok_or_else(||{kernel::pr_err!("G16C: unexpected FList receipt consumer={} producer={}\n",consumer,producer);EIO})?;
                        let mut data = [0u8; 0x48];
                        read_words(memory, vm, ring + consumer as u64 * 0x48, &mut data)?;
                        if !expected.consume(&data) { return Err(EIO); }
                    },
                    6 | 7 => {
                        let handler = work.as_deref_mut().ok_or(EIO)?;
                        let mut data = [0u8;0x48];
                        read_words(memory, vm, ring + consumer as u64 * 0x48, &mut data)?;
                        if !handler(memory, vm, &data)? { break; }
                        work_accepted = true;
                    },
                    4 => {
                        let mut data = [0u8; 0x48];
                        read_words(memory, vm, ring + consumer as u64 * 0x48, &mut data)?;
                        kernel::pr_err!("G16C: firmware timeout event RX={:02x?}\n", data);
                        return Err(ETIMEDOUT);
                    },
                    kind => {kernel::pr_err!("G16C: unhandled event kind={} consumer={} producer={}\n",kind,consumer,producer);return Err(EIO);},
                }
            }
            consumer = (consumer + 1) % capacity;
            vm.write(memory, 2, state, &consumer.to_le_bytes())?;
            if work_accepted { break; }
            producer = read32(memory, vm, state + 0x20)?;
            if producer >= capacity {
                return Err(EIO);
            }
        }
    }
    for offset in [
        0x3f400, 0x3f200, 0x3f230, 0x3f260, 0x3f290, 0x3f2c0, 0x3f2f0,
    ] {
        let state = graph::BASE + offset;
        let consumer = read32(memory, vm, state)?;
        let producer = read32(memory, vm, state + 0x20)?;
        if consumer >= 256 || producer >= 256 {
            return Err(EIO);
        }
        if consumer != producer {
            vm.write(memory, 2, state, &producer.to_le_bytes())?;
        }
    }
    Ok(())
}
