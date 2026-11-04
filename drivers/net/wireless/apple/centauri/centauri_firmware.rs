// SPDX-License-Identifier: GPL-2.0-only

//! FTAB container metadata, qualified by the independent Python prototype.
//!
//! Only directory headers and ranges are interpreted. Signed manifests and
//! executable payloads remain opaque. No captured allocation is an input.

const HEADER_BYTES: usize = 48;
const ENTRY_BYTES: usize = 16;
const MAX_ENTRIES: usize = 4096;
pub(super) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAGIC: &[u8] = b"rkosftab";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InvalidFirmware {
    Header,
    Directory,
    DuplicateTag,
    Range,
    MissingInput,
    Manifest,
    Layout,
}

type Result<T> = core::result::Result<T, InvalidFirmware>;

fn word(data: &[u8], offset: usize) -> Result<usize> {
    let raw = data.get(offset..offset + 4).ok_or(InvalidFirmware::Range)?;
    Ok(u32::from_le_bytes(raw.try_into().map_err(|_| InvalidFirmware::Range)?) as usize)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    pub(super) tag: [u8; 4],
    pub(super) offset: usize,
    pub(super) size: usize,
}

impl Entry {
    fn end(self) -> Result<usize> {
        self.offset
            .checked_add(self.size)
            .ok_or(InvalidFirmware::Range)
    }
}

struct Directory<'a> {
    data: &'a [u8],
    count: usize,
    table_end: usize,
}

impl<'a> Directory<'a> {
    fn parse(data: &'a [u8], layout: bool) -> Result<Self> {
        if data.len() < HEADER_BYTES
            || data.len() > MAX_IMAGE_BYTES
            || data.get(32..40) != Some(MAGIC)
            || word(data, 4)? != 1
        {
            return Err(InvalidFirmware::Header);
        }
        if word(data, 8)? != if layout { 0x42 } else { 0 } {
            return Err(InvalidFirmware::Header);
        }
        let count = word(data, 40)?;
        if count == 0 || count > MAX_ENTRIES {
            return Err(InvalidFirmware::Directory);
        }
        let table_end = HEADER_BYTES + count * ENTRY_BYTES;
        if table_end > data.len() {
            return Err(InvalidFirmware::Directory);
        }
        let directory = Self {
            data,
            count,
            table_end,
        };
        for index in 0..count {
            let entry = directory.at(index)?;
            if entry.size != 0 && entry.offset < table_end {
                return Err(InvalidFirmware::Range);
            }
            let end = entry.end()?;
            if end > MAX_IMAGE_BYTES || (!layout && end > data.len()) {
                return Err(InvalidFirmware::Range);
            }
            for previous in 0..index {
                if directory.at(previous)?.tag == entry.tag {
                    return Err(InvalidFirmware::DuplicateTag);
                }
            }
        }
        Ok(directory)
    }

    fn at(&self, index: usize) -> Result<Entry> {
        if index >= self.count {
            return Err(InvalidFirmware::Directory);
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        Ok(Entry {
            tag: self.data[offset..offset + 4]
                .try_into()
                .map_err(|_| InvalidFirmware::Directory)?,
            offset: word(self.data, offset + 4)?,
            size: word(self.data, offset + 8)?,
        })
    }

    fn find(&self, tag: &[u8; 4]) -> Result<Entry> {
        for index in 0..self.count {
            let entry = self.at(index)?;
            if entry.tag == *tag {
                return Ok(entry);
            }
        }
        Err(InvalidFirmware::MissingInput)
    }

    fn opaque(&self, entry: Entry) -> Result<&'a [u8]> {
        self.data
            .get(entry.offset..entry.end()?)
            .ok_or(InvalidFirmware::Range)
    }
}

pub(super) struct Image<'a> {
    pub(super) layout: &'a [u8],
    pub(super) allocation_bytes: usize,
}

impl<'a> Image<'a> {
    /// Locate a current firmware-produced radio resource by PCI function.
    /// Source offsets need not be page aligned: the real msww starts at640.
    /// Only each destination allocation/size is page aligned for DMA.
    pub(super) fn radio_region(&self, function: u8) -> Result<Entry> {
        let tag = match function {
            1 => b"msww",
            2 => b"mswb",
            _ => return Err(InvalidFirmware::Layout),
        };
        let directory = Directory::parse(self.layout, true)?;
        let entry = directory.find(tag)?;
        if entry.size == 0
            || entry.size & 0x3fff != 0
            || entry.offset < self.layout.len()
            || entry.end()? > self.allocation_bytes
        {
            return Err(InvalidFirmware::Layout);
        }
        Ok(entry)
    }

    /// Validate a prepared image, without interpreting its signed contents.
    /// Device inputs must eventually come from proper firmware/nvmem sources;
    /// container validity does not establish their provenance.
    pub(super) fn parse(data: &'a [u8]) -> Result<Self> {
        let directory = Directory::parse(data, false)?;
        if data.len() & 4095 != 0 {
            return Err(InvalidFirmware::Range);
        }
        let manifest_start = word(data, 16)?;
        let manifest_bytes = word(data, 20)?;
        let manifest_end = manifest_start
            .checked_add(manifest_bytes)
            .ok_or(InvalidFirmware::Manifest)?;
        if manifest_start != directory.table_end || manifest_bytes == 0 || manifest_end > data.len()
        {
            return Err(InvalidFirmware::Manifest);
        }
        for index in 0..directory.count {
            let entry = directory.at(index)?;
            if entry.size != 0 && entry.offset < manifest_end {
                return Err(InvalidFirmware::Manifest);
            }
        }
        for tag in [b"nvrm", b"_cal", b"_apb"] {
            if directory.find(tag)?.size == 0 {
                return Err(InvalidFirmware::MissingInput);
            }
        }
        let layout = directory.opaque(directory.find(b"2ftb")?)?;
        let allocation = Directory::parse(layout, true)?;
        let mut allocation_bytes = layout.len();
        for index in 0..allocation.count {
            allocation_bytes = allocation_bytes.max(allocation.at(index)?.end()?);
        }
        // These per-radio regions are copied only after the current firmware
        // has populated its owned allocation; they are never old host records.
        for tag in [b"msww", b"mswb"] {
            let entry = allocation.find(tag)?;
            if entry.size == 0 || entry.size & 0x3fff != 0 {
                return Err(InvalidFirmware::Layout);
            }
        }
        Ok(Self {
            layout,
            allocation_bytes,
        })
    }

    /// Initialize a new coherent allocation. No firmware-produced bytes are
    /// read here; the directory is copied and every remaining byte is zeroed.
    pub(super) fn initialize_allocation(&self, output: &mut [u8]) -> Result<()> {
        if output.len() != self.allocation_bytes {
            return Err(InvalidFirmware::Range);
        }
        output.fill(0);
        output[..self.layout.len()].copy_from_slice(self.layout);
        Ok(())
    }
}
