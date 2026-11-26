//! `vendor_boot` images of header version 4: the vendor ramdisk section is
//! a run of ramdisk fragments, each an LZ4 (legacy framing) compressed newc
//! archive, and a table after the device tree blob describes where each one
//! is in that section, its size and what it holds. The bootloader gives the
//! kernel the platform fragment on every start and the recovery fragment
//! beside it when it starts the recovery; the kernel unpacks them in that
//! order, and a file of the second takes the place of the first's of the
//! same name.
//!
//! The recovery fragment is where OPPO keeps the phone's own recovery on a
//! phone without a recovery partition, so that is the one a recovery of our
//! own is written into: the platform fragment, which holds the kernel's
//! modules and the first stage's table, is left as it was.

use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::Path;

/// A fragment the platform's first stage and every start take.
pub const PLATFORM: u32 = 1;
/// A fragment the bootloader adds when it starts the recovery.
pub const RECOVERY: u32 = 2;
/// A fragment of modules the platform loads only where its own table asks.
pub const DLKM: u32 = 3;

const MAGIC: &[u8; 8] = b"VNDRBOOT";
const FIRST_LEN: usize = 8;
const HEADER_SIZE_AT: usize = 2096;
const DTB_SIZE_AT: usize = 2100;
const TABLE_SIZE_AT: usize = 2112;
const TABLE_ENTRIES_AT: usize = 2116;
const TABLE_ENTRY_SIZE_AT: usize = 2120;
const BOOTCONFIG_SIZE_AT: usize = 2124;
/// The header of version 4, which ends in the table's own description.
const HEADER_V4: usize = 2128;
/// A fragment's size, offset and type, then its name and the boards it is
/// for: the layout of a version 4 table entry.
const ENTRY_LEN: usize = 108;
const ENTRY_NAME: usize = 32;
const ENTRY_BOARDS: usize = 16;
const RAMDISK_SIZE_AT: usize = 24;
/// The legacy LZ4 framing's magic number: a fragment starts with it.
const LZ4_LEGACY: u32 = 0x184C_2102;
/// The most a block of the legacy framing unpacks to.
const LZ4_BLOCK: usize = 8 << 20;

/// One fragment of the vendor ramdisk section, as the image holds it.
#[derive(Clone)]
pub struct Fragment {
    pub name: String,
    pub kind: u32,
    pub board: [u32; ENTRY_BOARDS],
    /// The fragment as the image holds it: LZ4 legacy framed bytes.
    pub compressed: Vec<u8>,
}

impl Fragment {
    /// The fragment's cpio archive, unpacked.
    pub fn archive(&self) -> Result<Vec<u8>> {
        unpack_legacy(&self.compressed)
    }
}

/// A `vendor_boot` image, its fragments apart from the rest so that one of
/// them can be written again while every other byte stays as it was.
pub struct VendorBoot {
    header: Vec<u8>,
    page: usize,
    fragments: Vec<Fragment>,
    dtb: Vec<u8>,
    bootconfig: Vec<u8>,
}

fn at(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    bytes.get(offset..offset + len).with_context(|| format!("{len} bytes at {offset} run past the end"))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(at(bytes, offset, 4)?.try_into().expect("four bytes")))
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let field = bytes.get_mut(offset..offset + 4).with_context(|| format!("a field at {offset}"))?;
    field.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// `offset` raised to the next multiple of `page`: where each section of the
/// image starts.
fn page_aligned(offset: usize, page: usize) -> usize {
    offset.div_ceil(page) * page
}

impl VendorBoot {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    /// The image `bytes` hold, as a partition holds it: the image, then
    /// whatever fills the partition after it.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= HEADER_V4, "the image is shorter than a version 4 header");
        ensure!(at(bytes, 0, FIRST_LEN)? == MAGIC, "the image does not start with VNDRBOOT");
        let version = u32_at(bytes, 8)?;
        ensure!(version == 4, "vendor_boot header version {version}: only version 4 keeps its ramdisk in fragments");
        let page = u32_at(bytes, 12)? as usize;
        ensure!(page >= 512 && page.is_power_of_two(), "the header's page size {page} is not a power of two");
        let ramdisk_size = u32_at(bytes, RAMDISK_SIZE_AT)? as usize;
        let header_size = u32_at(bytes, HEADER_SIZE_AT)? as usize;
        let dtb_size = u32_at(bytes, DTB_SIZE_AT)? as usize;
        let table_size = u32_at(bytes, TABLE_SIZE_AT)? as usize;
        let entries = u32_at(bytes, TABLE_ENTRIES_AT)? as usize;
        let entry_size = u32_at(bytes, TABLE_ENTRY_SIZE_AT)? as usize;
        let bootconfig_size = u32_at(bytes, BOOTCONFIG_SIZE_AT)? as usize;
        ensure!(header_size >= HEADER_V4, "the header says it is {header_size} bytes, shorter than a version 4 header");
        ensure!(entry_size == ENTRY_LEN, "a table entry is {entry_size} bytes, where this reads {ENTRY_LEN}");
        ensure!(
            table_size == entries * entry_size,
            "the table is {table_size} bytes for {entries} entries of {entry_size}"
        );
        let ramdisk_at = page_aligned(header_size, page);
        let dtb_at = page_aligned(ramdisk_at + ramdisk_size, page);
        let table_at = page_aligned(dtb_at + dtb_size, page);
        let bootconfig_at = page_aligned(table_at + table_size, page);
        let end = bootconfig_at + bootconfig_size;
        ensure!(end <= bytes.len(), "the image's sections end at {end}, past its {} bytes", bytes.len());

        let mut fragments = Vec::with_capacity(entries);
        for n in 0..entries {
            let entry = at(bytes, table_at + n * ENTRY_LEN, ENTRY_LEN)?;
            let size = u32::from_le_bytes(entry[0..4].try_into().expect("four bytes")) as usize;
            let offset = u32::from_le_bytes(entry[4..8].try_into().expect("four bytes")) as usize;
            let kind = u32::from_le_bytes(entry[8..12].try_into().expect("four bytes"));
            let name = entry[12..12 + ENTRY_NAME].split(|b| *b == 0).next().unwrap_or_default();
            let name = String::from_utf8_lossy(name).into_owned();
            let mut board = [0u32; ENTRY_BOARDS];
            for (i, slot) in board.iter_mut().enumerate() {
                let at = 12 + ENTRY_NAME + i * 4;
                *slot = u32::from_le_bytes(entry[at..at + 4].try_into().expect("four bytes"));
            }
            ensure!(
                offset.checked_add(size).is_some_and(|end| end <= ramdisk_size),
                "fragment {n} ({name:?}) runs past the {ramdisk_size}-byte vendor ramdisk"
            );
            let compressed = at(bytes, ramdisk_at + offset, size)?.to_vec();
            ensure!(
                compressed.len() >= 4 && u32_at(&compressed, 0)? == LZ4_LEGACY,
                "fragment {n} ({name:?}) is not LZ4 legacy framed"
            );
            fragments.push(Fragment { name, kind, board, compressed });
        }

        Ok(Self {
            header: at(bytes, 0, header_size)?.to_vec(),
            page,
            fragments,
            dtb: at(bytes, dtb_at, dtb_size)?.to_vec(),
            bootconfig: at(bytes, bootconfig_at, bootconfig_size)?.to_vec(),
        })
    }

    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }

    pub fn page_size(&self) -> usize {
        self.page
    }

    pub fn dtb(&self) -> &[u8] {
        &self.dtb
    }

    /// Write `compressed`, an LZ4 legacy framed cpio archive, in place of
    /// the single fragment of `kind`, every other fragment staying as it was.
    pub fn replace_fragment(&mut self, kind: u32, compressed: Vec<u8>) -> Result<()> {
        ensure!(compressed.len() >= 4 && u32_at(&compressed, 0)? == LZ4_LEGACY, "the fragment to write is not LZ4 legacy framed");
        let matching: Vec<usize> = self.fragments.iter().enumerate().filter(|(_, f)| f.kind == kind).map(|(i, _)| i).collect();
        ensure!(matching.len() == 1, "the image holds {} fragments of type {kind}, where one is written", matching.len());
        self.fragments[matching[0]].compressed = compressed;
        Ok(())
    }

    /// The image as a partition holds it up to the end of its bootconfig:
    /// the header, each fragment one after another, the device tree blob,
    /// the table and the bootconfig, every section from the page after the
    /// one before it.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut header = self.header.clone();
        put_u32(&mut header, RAMDISK_SIZE_AT, 0)?;
        let mut out = header;
        out.resize(page_aligned(out.len(), self.page), 0);

        let ramdisk_at = out.len();
        let mut entries = Vec::with_capacity(self.fragments.len());
        for fragment in &self.fragments {
            let offset = out.len() - ramdisk_at;
            out.extend_from_slice(&fragment.compressed);
            entries.push((offset as u32, fragment.compressed.len() as u32));
        }
        let ramdisk_size = out.len() - ramdisk_at;

        out.resize(page_aligned(out.len(), self.page), 0);
        out.extend_from_slice(&self.dtb);

        out.resize(page_aligned(out.len(), self.page), 0);
        let table_at = out.len();
        for (fragment, (offset, size)) in self.fragments.iter().zip(&entries) {
            let mut entry = [0u8; ENTRY_LEN];
            entry[0..4].copy_from_slice(&size.to_le_bytes());
            entry[4..8].copy_from_slice(&offset.to_le_bytes());
            entry[8..12].copy_from_slice(&fragment.kind.to_le_bytes());
            let name = fragment.name.as_bytes();
            let name = &name[..name.len().min(ENTRY_NAME)];
            entry[12..12 + name.len()].copy_from_slice(name);
            for (i, board) in fragment.board.iter().enumerate() {
                let at = 12 + ENTRY_NAME + i * 4;
                entry[at..at + 4].copy_from_slice(&board.to_le_bytes());
            }
            out.extend_from_slice(&entry);
        }
        let table_size = out.len() - table_at;

        out.resize(page_aligned(out.len(), self.page), 0);
        out.extend_from_slice(&self.bootconfig);

        let header = out.get_mut(..self.header.len()).expect("the header is at the image's start");
        put_u32(header, RAMDISK_SIZE_AT, u32::try_from(ramdisk_size)?)?;
        put_u32(header, DTB_SIZE_AT, u32::try_from(self.dtb.len())?)?;
        put_u32(header, TABLE_SIZE_AT, u32::try_from(table_size)?)?;
        put_u32(header, TABLE_ENTRIES_AT, u32::try_from(self.fragments.len())?)?;
        put_u32(header, TABLE_ENTRY_SIZE_AT, u32::try_from(ENTRY_LEN)?)?;
        put_u32(header, BOOTCONFIG_SIZE_AT, u32::try_from(self.bootconfig.len())?)?;
        Ok(out)
    }
}

/// The cpio archive an LZ4 legacy framed `data` holds: the magic number,
/// then blocks, each with its compressed length before it. A fragment's
/// padding after its last block reads as a block of no bytes.
pub fn unpack_legacy(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 4, "a legacy frame is shorter than its magic number");
    let magic = u32_at(data, 0)?;
    ensure!(magic == LZ4_LEGACY, "the frame starts {magic:#010x}, where a legacy frame starts {LZ4_LEGACY:#010x}");
    let mut out = Vec::new();
    let mut buf = vec![0u8; LZ4_BLOCK];
    let mut at = 4;
    while at + 4 <= data.len() {
        let len = u32_at(data, at)? as usize;
        at += 4;
        if len == 0 || len > data.len() - at {
            break;
        }
        let n = lz4_flex::block::decompress_into(&data[at..at + len], &mut buf).context("unpack a block of the legacy frame")?;
        out.extend_from_slice(&buf[..n]);
        at += len;
    }
    ensure!(!out.is_empty(), "the legacy frame holds nothing");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small vendor_boot: a header, two fragments of compressed bytes,
    /// a blob of device tree and a table describing both.
    fn image(platform: &[u8], recovery: &[u8]) -> Vec<u8> {
        let page = 4096usize;
        let mut header = vec![0u8; HEADER_V4];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&4u32.to_le_bytes());
        header[12..16].copy_from_slice(&(page as u32).to_le_bytes());
        header[HEADER_SIZE_AT..HEADER_SIZE_AT + 4].copy_from_slice(&(HEADER_V4 as u32).to_le_bytes());
        header[TABLE_ENTRY_SIZE_AT..TABLE_ENTRY_SIZE_AT + 4].copy_from_slice(&(ENTRY_LEN as u32).to_le_bytes());
        let mut out = header;
        out.resize(page_aligned(out.len(), page), 0);
        let ramdisk_at = out.len();
        out.extend_from_slice(platform);
        let platform_len = out.len() - ramdisk_at;
        out.extend_from_slice(recovery);
        let ramdisk_size = out.len() - ramdisk_at;
        out.resize(page_aligned(out.len(), page), 0);
        let dtb_at = out.len();
        let dtb = b"\xd0\x0d\xfe\xeddevice tree".to_vec();
        out.extend_from_slice(&dtb);
        out.resize(page_aligned(out.len(), page), 0);
        let table_at = out.len();
        for (name, kind, offset, size) in [("", PLATFORM, 0u32, platform_len as u32), ("recovery", RECOVERY, platform_len as u32, recovery.len() as u32)] {
            let mut entry = [0u8; ENTRY_LEN];
            entry[0..4].copy_from_slice(&size.to_le_bytes());
            entry[4..8].copy_from_slice(&offset.to_le_bytes());
            entry[8..12].copy_from_slice(&kind.to_le_bytes());
            entry[12..12 + name.len()].copy_from_slice(name.as_bytes());
            out.extend_from_slice(&entry);
        }
        let table_size = out.len() - table_at;
        let _ = dtb_at;
        out[RAMDISK_SIZE_AT..RAMDISK_SIZE_AT + 4].copy_from_slice(&(ramdisk_size as u32).to_le_bytes());
        out[DTB_SIZE_AT..DTB_SIZE_AT + 4].copy_from_slice(&(dtb.len() as u32).to_le_bytes());
        out[TABLE_SIZE_AT..TABLE_SIZE_AT + 4].copy_from_slice(&(table_size as u32).to_le_bytes());
        out[TABLE_ENTRIES_AT..TABLE_ENTRIES_AT + 4].copy_from_slice(&2u32.to_le_bytes());
        out.resize(page_aligned(out.len(), page), 0);
        out
    }

    fn frame(content: &[u8]) -> Vec<u8> {
        crate::ramdisk::lz4_legacy(content)
    }

    #[test]
    fn a_fragment_is_unpacked_from_its_own_frame() {
        let packed = frame(b"the recovery's archive");
        assert_eq!(unpack_legacy(&packed).unwrap(), b"the recovery's archive");
    }

    #[test]
    fn the_recovery_fragment_is_replaced_and_the_platform_one_kept() {
        let platform = frame(b"the platform's archive");
        let recovery = frame(b"the recovery's archive");
        let mut img = VendorBoot::parse(&image(&platform, &recovery)).unwrap();
        assert_eq!(img.fragments().len(), 2);
        assert_eq!(img.fragments()[1].name, "recovery");
        assert_eq!(img.fragments()[1].kind, RECOVERY);
        assert_eq!(img.fragments()[1].archive().unwrap(), b"the recovery's archive");
        let wrote = frame(b"our own recovery's archive");
        img.replace_fragment(RECOVERY, wrote.clone()).unwrap();
        let bytes = img.to_bytes().unwrap();
        let back = VendorBoot::parse(&bytes).unwrap();
        assert_eq!(back.fragments()[0].compressed, platform, "the platform fragment stays as it was");
        assert_eq!(back.fragments()[1].compressed, wrote);
        assert_eq!(back.dtb(), b"\xd0\x0d\xfe\xeddevice tree");
        assert_eq!(bytes.len() % 4096, 0);
    }

    #[test]
    fn a_fragment_that_is_not_a_legacy_frame_is_refused() {
        let err = VendorBoot::parse(&image(&frame(b"a"), b"plain bytes"));
        assert!(err.is_err(), "a fragment starts with the legacy magic number");
    }

    #[test]
    fn replacing_a_kind_the_image_holds_none_of_is_refused() {
        let mut img = VendorBoot::parse(&image(&frame(b"a"), &frame(b"b"))).unwrap();
        assert!(img.replace_fragment(DLKM, frame(b"c")).is_err());
    }

    #[test]
    fn the_header_says_where_the_sections_are() {
        let bytes = image(&frame(b"a"), &frame(b"b"));
        assert_eq!(u32_at(&bytes, 8).unwrap(), 4);
        assert_eq!(u32_at(&bytes, 12).unwrap(), 4096);
        assert_eq!(u32_at(&bytes, TABLE_ENTRIES_AT).unwrap(), 2);
        assert_eq!(u32_at(&bytes, TABLE_ENTRY_SIZE_AT).unwrap(), 108);
    }
}
