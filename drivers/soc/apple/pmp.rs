// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![recursion_limit = "2048"]

//! Apple PMP driver
//!
//! Copyright (C) The Asahi Linux Contributors

use core::mem;

use kernel::{
    device::{
        self,
        Core, //
    },
    dma::{
        Coherent,
        CoherentBox,
        Device,
        DmaMask, //
    },
    io::{
        mem::DevresIoMem,
        Io,
        IoBase,
        IoSysMap,
        Region,
        SysMem, //
    },
    kvec,
    module_platform_driver,
    new_mutex,
    of,
    platform,
    prelude::*,
    soc::apple::rtkit,
    str::CString,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    },
    transmute::{
        AsBytes,
        FromBytes, //
    },
    types::ForeignOwnable, //
};

const ASC_MMIO_SIZE: usize = 0x4000;
const BOOTARGS_OFFSET: usize = 0x22c;
const BOOTARGS_SIZE: usize = 0x230;
const CPU_CONTROL: usize = 0x44;
const CPU_RUN: u32 = 0x1 << 4;
const PMP_ENDPOINT: u8 = 0x20;
const OPC_GET_IOVA_TABLE: u64 = 0x10;
const OPC_MALLOC: u64 = 0x12;
const OPC_FREE: u64 = 0x14;
const OPC_SET_BUF: u64 = 0x30;
const OPC_REGISTER_IOREG: u64 = 0x32;
const OPC_SET_IOREG: u64 = 0x34;
const OPC_ADVERTISE_DONE: u64 = 0x36;
const OPC_ACK_MASK: u64 = 0x1;
const OPC_SHIFT: u32 = 48;
const MALLOC_SIZE_MASK: u64 = 0xFFFFFF;
const MSG_IOVA_MASK: u64 = 0xFFFFFFFFFFFF;
const SET_IOREG_INDEX_MASK: u64 = 0xFFFF;
const PIO_VM_BASE: u64 = 0xc0000000;
const PIO_GRANULARITY: u64 = 0x1000000;

const fn from_fourcc(b: &[u8; 4]) -> u32 {
    b[3] as u32 | (b[2] as u32) << 8 | (b[1] as u32) << 16 | (b[0] as u32) << 24
}

struct PmpAllocation {
    addr: u64,
    alloc: Coherent<[u8]>,
}

struct IovaTableEntry {
    _host_addr: u64,
    _pio_base: u64,
    _size: u64,
}

// SAFETY: TODO:
unsafe impl AsBytes for IovaTableEntry {}
// SAFETY: TODO:
unsafe impl FromBytes for IovaTableEntry {}

struct PmpState {
    iova_table: Option<Coherent<[IovaTableEntry]>>,
    allocs: KVec<PmpAllocation>,
    value_buf: Option<u64>,
    ioreg_entries: KVec<u32>,
    ioreg_advertisements: usize,
}

struct PmpHwConfig {
    advertise_reply_index: bool,
}

const PMP_HW_T6000: PmpHwConfig = PmpHwConfig {
    advertise_reply_index: true,
};

const PMP_HW_T8132: PmpHwConfig = PmpHwConfig {
    advertise_reply_index: false,
};

impl PmpState {
    fn new() -> Result<PmpState> {
        Ok(PmpState {
            iova_table: None,
            allocs: KVec::with_capacity(10, GFP_KERNEL)?,
            value_buf: None,
            ioreg_entries: KVec::with_capacity(340, GFP_KERNEL)?,
            ioreg_advertisements: 0,
        })
    }
    fn find_alloc(&self, addr: u64) -> Option<usize> {
        // Due to how pmp manages memory, iterating in reverse will
        // usually result in us getting the right one on the first try
        for (i, e) in self.allocs.iter().enumerate().rev() {
            if e.addr == addr {
                return Some(i);
            }
        }
        None
    }
    fn get_buf(&mut self, addr: u64) -> Option<&mut Coherent<[u8]>> {
        let idx = self.find_alloc(addr)?;
        Some(&mut self.allocs[idx].alloc)
    }
}

#[pin_data]
struct PmpData {
    dev: ARef<device::Device>,
    advertise_reply_index: bool,
    pmp_mmio: DevresIoMem,
    asc_mmio: DevresIoMem<ASC_MMIO_SIZE>,
    #[pin]
    rtkit: Mutex<Option<rtkit::RtKit<PmpData>>>,
    #[pin]
    state: Mutex<PmpState>,
}

impl PmpData {
    fn new(dev: &platform::Device<Core<'_>>, cfg: &'static PmpHwConfig) -> Result<Arc<PmpData>> {
        let pmp_req = dev.io_request_by_name(c"pmp").ok_or(EINVAL)?;
        // T6041 places boot arguments beyond the older 512 KiB window.
        // Map the firmware resource supplied by the board's device tree.
        let pmp_mmio = pmp_req.iomap()?.into_devres()?;
        let asc_req = dev.io_request_by_name(c"asc").ok_or(EINVAL)?;
        let asc_mmio = asc_req.iomap_sized::<ASC_MMIO_SIZE>()?.into_devres()?;
        Arc::pin_init(
            try_pin_init!(
                PmpData {
                    dev: dev.as_ref().into(),
                    advertise_reply_index: cfg.advertise_reply_index,
                    pmp_mmio,
                    asc_mmio,
                    rtkit <- new_mutex!(None),
                    state <- new_mutex!(PmpState::new()?)
                }
            ),
            GFP_KERNEL,
        )
    }
    fn start_cpu(&self) -> Result<()> {
        let asc = self.asc_mmio.try_access().ok_or(ENODEV)?;
        let io = asc.as_view().relaxed();
        let val = io.read32(CPU_CONTROL);
        io.write32(val | CPU_RUN, CPU_CONTROL);
        Ok(())
    }
    fn start(&self) -> Result<()> {
        let mut guard = self.rtkit.lock();
        let mut rtk = guard.as_mut().as_pin_mut().unwrap();
        rtk.as_mut().wake()?;
        rtk.start_endpoint(PMP_ENDPOINT)
    }
    fn patch_bootargs(&self, patches: &[(u32, u32)]) -> Result<()> {
        let pmp = self.pmp_mmio.try_access().ok_or(ENODEV)?;
        let io = pmp.as_view().relaxed();
        let offset = io.try_read32(BOOTARGS_OFFSET)? as usize;
        let size = io.try_read32(BOOTARGS_SIZE)? as usize;
        if offset.checked_add(size).ok_or(EINVAL)? > io.size() {
            return Err(EINVAL);
        }
        let mut arg_bytes = kvec![0u8; size]?;
        for (i, b) in arg_bytes.iter_mut().enumerate() {
            *b = io.try_read8(offset + i)?;
        }
        let mut idx = 0;
        while idx < size {
            let key = u32::from_le_bytes(arg_bytes[idx..idx + 4].try_into().unwrap());
            let size = u32::from_le_bytes(arg_bytes[idx + 4..idx + 8].try_into().unwrap()) as usize;
            idx += 8;
            for (k, v) in patches.iter() {
                if *k != key {
                    continue;
                }
                arg_bytes[idx..idx + size].copy_from_slice(&(*v as u64).to_le_bytes()[..size]);
                break;
            }
            idx += size;
        }
        for (i, b) in arg_bytes.iter().enumerate() {
            io.try_write8(*b, offset + i)?;
        }
        Ok(())
    }
    fn get_iova_table(&self) -> Result<u64> {
        let mut state = self.state.lock();
        if state.iova_table.is_some() {
            dev_err!(self.dev, "Asked for iova table with existing buffer");
            return Err(EIO);
        }
        let node = self.dev.fwnode().ok_or(EIO)?;
        let mut pio_base = PIO_VM_BASE;
        let prop_name = c"apple,pio-ranges";
        if !node.property_present(prop_name) {
            return Ok((OPC_GET_IOVA_TABLE | OPC_ACK_MASK) << OPC_SHIFT);
        }
        let n_entries = node.property_count_elem::<u64>(prop_name)? / 2;
        let ranges = node
            .property_read_array_vec::<u64>(prop_name, n_entries * 2)?
            .required_by(&self.dev)?;
        // SAFETY: TODO: ensure self.dev is bound
        let bound_dev = unsafe { self.dev.as_bound() };
        let mut table = CoherentBox::<[IovaTableEntry]>::zeroed_slice(bound_dev, 170, GFP_KERNEL)?;

        for i in 0..n_entries {
            let host_addr = ranges[i * 2];
            let size = ranges[i * 2 + 1];
            unsafe {
                kernel::iommu::map_device_mmio(
                    bound_dev,
                    pio_base as usize,
                    host_addr,
                    size as usize,
                    GFP_KERNEL,
                )?;
            }
            table[i] = IovaTableEntry {
                _host_addr: host_addr,
                _pio_base: pio_base,
                _size: size,
            };
            pio_base += PIO_GRANULARITY;
        }
        let table: Coherent<[IovaTableEntry]> = table.into();
        let msg = (OPC_GET_IOVA_TABLE | OPC_ACK_MASK) << OPC_SHIFT | table.dma_address();
        state.iova_table = Some(table);
        Ok(msg)
    }
    fn malloc(&self, size: u64) -> Result<u64> {
        // SAFETY: TODO: ensure self.dev is bound
        let bound_dev = unsafe { self.dev.as_bound() };
        let iomem = Coherent::<u8>::zeroed_slice(bound_dev, size as usize, GFP_KERNEL)?;
        let mut state = self.state.lock();
        let addr = iomem.dma_address();
        let msg = (OPC_MALLOC | OPC_ACK_MASK) << OPC_SHIFT | addr;
        state.allocs.push(
            PmpAllocation {
                addr: addr,
                alloc: iomem,
            },
            GFP_KERNEL,
        )?;
        Ok(msg)
    }
    fn free(&self, addr: u64) -> Result<u64> {
        let mut state = self.state.lock();
        if let Some(idx) = state.find_alloc(addr) {
            state.allocs.swap_remove(idx);
        } else {
            dev_err!(
                self.dev,
                "Attempted to free memory that was not allocated {}",
                addr
            );
            return Err(EIO);
        }
        let msg = (OPC_FREE | OPC_ACK_MASK) << OPC_SHIFT;
        Ok(msg)
    }
    fn set_buf(&self, addr: u64) -> Result<u64> {
        let mut state = self.state.lock();
        if state.value_buf.is_some() {
            dev_err!(self.dev, "Setting a buffer when one exists");
            return Err(EIO);
        }
        let ptr_buf = if let Some(s) = state.get_buf(addr) {
            s
        } else {
            dev_err!(self.dev, "Unable to find buffer");
            return Err(EIO);
        };
        if ptr_buf.size() < mem::size_of::<u64>() {
            dev_err!(self.dev, "Buffer too small");
            return Err(EIO);
        }
        // SAFETY: The buffer holds at least eight bytes, checked above.
        let ptr = unsafe { core::ptr::read_unaligned(ptr_buf.as_ptr().cast::<u64>()) };
        state.value_buf = Some(ptr);
        let msg = (OPC_SET_BUF | OPC_ACK_MASK) << OPC_SHIFT;
        Ok(msg)
    }
    fn register_ioreg(&self, addr: u64) -> Result<u64> {
        let mut state = self.state.lock();
        let advertisement_index = state.ioreg_advertisements;
        let msg_buf = if let Some(s) = state.get_buf(addr) {
            s
        } else {
            dev_err!(self.dev, "Unable to find buffer");
            return Err(EIO);
        };
        if msg_buf.size() < 0x4c {
            dev_err!(self.dev, "Buffer too small");
            return Err(EIO);
        }
        let mut name_vec = KVec::with_capacity(0x31, GFP_KERNEL)?;
        name_vec
            .extend_from_slice(unsafe { &msg_buf.as_ref()[0..0x30] }, GFP_KERNEL)
            .unwrap();
        name_vec.push(0, GFP_KERNEL).unwrap();
        let name_str = CStr::from_bytes_until_nul(&name_vec).unwrap();

        let display_monitor = name_str == c"dcs-monitor"
            || name_str == c"disp-monitor"
            || name_str == c"dispext0-monitor"
            || name_str == c"dispext1-monitor";

        // SAFETY: TODO
        let mut size = u32::from_le_bytes(unsafe {
            <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x40..0x44]).unwrap()
        });
        let wire_index = u32::from_le_bytes(unsafe {
            <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x48..0x4c]).unwrap()
        });
        if verbose() {
            dev_info!(
                self.dev,
                "PMP descriptor {} wire_index={:#x} {:?}: words30={:#010x} words34={:#010x} words38={:#010x} words3c={:#010x} size={:#x}",
                advertisement_index,
                wire_index,
                name_str,
                u32::from_le_bytes(unsafe {
                    <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x30..0x34]).unwrap()
                }),
                u32::from_le_bytes(unsafe {
                    <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x34..0x38]).unwrap()
                }),
                u32::from_le_bytes(unsafe {
                    <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x38..0x3c]).unwrap()
                }),
                u32::from_le_bytes(unsafe {
                    <&[u8] as TryInto<[u8; 4]>>::try_into(&msg_buf.as_ref()[0x3c..0x40]).unwrap()
                }),
                size
            );
        }
        if display_monitor && size == mem::size_of::<u32>() as u32 {
            /* The monitor descriptors carry their initial value through the
             * shared value buffer.  Native T8132 publishes all four display
             * monitor properties as one; a zero-filled host buffer silently
             * disables that PMP policy even though the descriptor is
             * acknowledged with the correct size. */
            let val_buf_addr = state.value_buf.ok_or(EIO)?;
            let val_buf = state.get_buf(val_buf_addr).ok_or(EIO)?;
            if val_buf.size() < mem::size_of::<u32>() {
                return Err(EINVAL);
            }
            unsafe {
                val_buf.as_mut()[..mem::size_of::<u32>()]
                    .copy_from_slice(&1u32.to_le_bytes());
            }
            if verbose() {
                dev_info!(self.dev, "initialized PMP monitor {:?} to 1", name_str);
            }
        } else if size == 0 {
            let name_str = CString::try_from_fmt(fmt!("apple,tunable-{name_str}"))?;
            let node = self.dev.fwnode().ok_or(EIO)?;
            if state.value_buf.is_none() {
                dev_err!(self.dev, "Value buf not set");
                return Err(EIO);
            }
            let val_buf_addr = state.value_buf.unwrap();
            let val_buf = if let Some(s) = state.get_buf(val_buf_addr) {
                s
            } else {
                dev_err!(self.dev, "Unable to find value buffer");
                return Err(EIO);
            };
            if node.property_present(&name_str) {
                let len = node.property_count_elem::<u8>(&name_str)?;
                if len > val_buf.size() {
                    dev_err!(self.dev, "tunable {:?} does not fit the value buffer", name_str);
                    return Err(EINVAL);
                }
                let data = node
                    .property_read_array_vec::<u8>(&name_str, len)?
                    .required_by(&self.dev)?;
                unsafe {
                    val_buf.as_mut()[0..len].copy_from_slice(&data);
                }
                size = len as u32;
            } else if verbose() {
                dev_info!(self.dev, "unknown property {:?}", name_str);
            }
        }
        if self.advertise_reply_index {
            state.ioreg_entries.push(size, GFP_KERNEL)?;
        } else {
            /* T8132 carries the SetIOReg lookup key in the descriptor at
             * offset 0x48.  Aggregate descriptors reuse keys and do not
             * consume one monotonically, so arrival order is not an index.
             * Keep the most recently advertised size for each wire key. */
            let index = wire_index as usize;
            if state.ioreg_entries.len() <= index {
                state.ioreg_entries.resize(index + 1, 0, GFP_KERNEL)?;
            }
            state.ioreg_entries[index] = size;
        }
        state.ioreg_advertisements += 1;
        /* The older firmware ABI returns the number of advertised entries in
         * bits 47:32.  T8132/26.6 instead returns zero in this field for every
         * descriptor; the descriptor itself carries its SetIOReg index. */
        let reply_index = if self.advertise_reply_index {
            state.ioreg_entries.len() as u64
        } else {
            0
        };
        let msg =
            (OPC_REGISTER_IOREG | OPC_ACK_MASK) << OPC_SHIFT | (reply_index << 32) | size as u64;
        Ok(msg)
    }
    fn set_ioreg(&self, index: u64) -> Result<u64> {
        let state = self.state.lock();
        let len = *state
            .ioreg_entries
            .get(index as usize)
            .ok_or(EIO)? as u64;
        if verbose() {
            dev_info!(
                self.dev,
                "PMP SetIOReg index={:#x} size={:#x} entries={}",
                index,
                len,
                state.ioreg_advertisements
            );
        }
        let msg = (OPC_SET_IOREG | OPC_ACK_MASK) << OPC_SHIFT | len;
        Ok(msg)
    }
    fn advertise_done(&self, count: u64) -> u64 {
        let state = self.state.lock();
        if verbose() {
            dev_info!(
                self.dev,
                "PMP AdvertiseDone count={:#x} advertisements={} wire_entries={} ack=8",
                count,
                state.ioreg_advertisements,
                state.ioreg_entries.len()
            );
        }
        ((OPC_ADVERTISE_DONE | OPC_ACK_MASK) << OPC_SHIFT) | 8
    }
    fn recv_message(&self, msg: u64) -> Result<()> {
        let opc = (msg >> OPC_SHIFT) & 0xFF;
        let reply = match opc {
            OPC_GET_IOVA_TABLE => self.get_iova_table()?,
            OPC_MALLOC => self.malloc(msg & MALLOC_SIZE_MASK)?,
            OPC_FREE => self.free(msg & MSG_IOVA_MASK)?,
            OPC_SET_BUF => self.set_buf(msg & MSG_IOVA_MASK)?,
            OPC_REGISTER_IOREG => self.register_ioreg(msg & MSG_IOVA_MASK)?,
            OPC_SET_IOREG => self.set_ioreg(msg & SET_IOREG_INDEX_MASK)?,
            // The low bits contain the number of properties advertised in
            // this batch.  Current firmware expects a fixed value of 8 in
            // the acknowledgement, rather than the count echoed back.
            OPC_ADVERTISE_DONE => self.advertise_done(msg & MSG_IOVA_MASK),
            _ => {
                dev_err!(self.dev, "Got unknown message {}", msg);
                return Err(EIO);
            }
        };
        let mut rtk_guard = self.rtkit.lock();
        let rtk = rtk_guard.as_mut().as_pin_mut().unwrap();
        rtk.send_message(PMP_ENDPOINT, reply)?;
        Ok(())
    }
}

unsafe impl Send for PmpData {}
unsafe impl Sync for PmpData {}

struct PmpRtkitBuffer(Coherent<[u8]>);

impl rtkit::Buffer for PmpRtkitBuffer {
    fn iova(&self) -> Result<usize> {
        Ok(self.0.dma_address() as usize)
    }
    fn buf(&mut self) -> Result<IoSysMap<'_, Region>> {
        let region =
            Region::ptr_try_from_raw_parts_mut(self.0.as_mut_ptr().cast::<u8>(), self.0.size())?;
        // SAFETY: The coherent allocation owns `size()` writable bytes at
        // `as_mut_ptr()` and outlives the returned borrowed mapping.
        Ok(IoSysMap::Sys(unsafe { SysMem::new(region) }))
    }
}

#[vtable]
impl rtkit::Operations for PmpData {
    type Data = Arc<PmpData>;
    type Buffer = PmpRtkitBuffer;

    fn shmem_alloc(
        data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        size: usize,
    ) -> Result<Self::Buffer> {
        // SAFETY: The PMP platform device remains bound for the lifetime of
        // `data` and therefore for the coherent allocation returned here.
        let bound_dev = unsafe { data.dev.as_bound() };
        Ok(PmpRtkitBuffer(Coherent::<u8>::zeroed_slice(
            bound_dev, size, GFP_KERNEL,
        )?))
    }

    fn recv_message(data: <Self::Data as ForeignOwnable>::Borrowed<'_>, _ep: u8, msg: u64) {
        let ret = data.recv_message(msg);
        if let Err(e) = ret {
            dev_err!(data.dev, "Failed to handle rtkit message, error: {:?}", e);
        }
    }

    fn crashed(data: <Self::Data as ForeignOwnable>::Borrowed<'_>, _crashlog: Option<&[u8]>) {
        dev_err!(data.dev, "PMP firmware crashed");
    }
}

#[allow(dead_code)]
struct PmpDriver(Arc<PmpData>);

kernel::of_device_table!(
    OF_TABLE,
    <PmpDriver as platform::Driver>::IdInfo,
    [
        (of::DeviceId::new(c"apple,t8132-pmp-v2"), &PMP_HW_T8132),
        (of::DeviceId::new(c"apple,t6041-pmp-v2"), &PMP_HW_T8132),
        (of::DeviceId::new(c"apple,t6000-pmp-v2"), &PMP_HW_T6000),
    ]
);

/// Whether to log PMP's start-up handshake (apple_pmp.verbose=1).
fn verbose() -> bool {
    crate::module_parameters::verbose.value() != 0
}

impl platform::Driver for PmpDriver {
    type IdInfo = &'static PmpHwConfig;
    type Data<'bound> = PmpDriver;

    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe<'bound>(
        pdev: &'bound platform::Device<Core<'_>>,
        info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        let cfg = info.ok_or(ENODEV)?;
        // apple_pmp.enable=0 leaves PMP stopped: the system runs as without
        // PMP (its report votes are written but nothing acts on them).
        if crate::module_parameters::enable.value() == 0 {
            dev_info!(pdev.as_ref(), "PMP disabled (apple_pmp.enable=0)\n");
            return Err(ENODEV);
        }
        // PMP DART IOVAs live in the 0x100_00000000 aperture on T8132, so
        // the default 32-bit coherent DMA mask cannot satisfy RTKit's shared
        // memory requests.
        unsafe { pdev.dma_set_mask_and_coherent(DmaMask::new::<42>())? };
        let dev: ARef<device::Device> = pdev.as_ref().into();
        let data = PmpData::new(pdev, cfg)?;
        let node = dev.fwnode().ok_or(EIO)?;
        let dvid = node
            .property_read(c"apple,dram-vendor-id")
            .required_by(&dev)?;
        let bdid = node.property_read(c"apple,board-id").required_by(&dev)?;
        match node.property_read(c"apple,dram-capacity").optional() {
            Some(dcap) => data.patch_bootargs(
                &[
                    (from_fourcc(b"BDID"), bdid),
                    (from_fourcc(b"DCAP"), dcap),
                    (from_fourcc(b"DVID"), dvid),
                ],
            )?,
            None => data.patch_bootargs(
                &[(from_fourcc(b"BDID"), bdid), (from_fourcc(b"DVID"), dvid)],
            )?,
        };
        let rtkit = rtkit::RtKit::<PmpData>::new(&dev, None, 0, data.clone())?;
        data.rtkit.lock().as_mut().set(Some(rtkit));
        data.start_cpu()?;
        data.start()?;
        Ok(PmpDriver(data))
    }
}

module_platform_driver! {
    type: PmpDriver,
    name: "apple_pmp",
    description: "Apple Power Management Processor",
    license: "Dual MIT/GPL",
    params: {
        enable: u32 {
            default: 1,
            description: "1 (default) starts PMP; 0 leaves it stopped, as on kernels without PMP",
        },
        verbose: u32 {
            default: 0,
            description: "1 logs every descriptor of PMP's start-up handshake",
        },
    },
}
