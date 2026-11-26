//! What a partition holds now: the filesystem on it, which need not be the
//! one the phone's own table names for it. blkid names the filesystems it
//! knows; erofs, which the ramdisk's blkid does not know, is read by the
//! erofs tools the ramdisk carries.

use crate::tools;
use anyhow::Result;
use std::path::Path;

/// blkid is one of the commands in the ramdisk's box of them, which takes
/// the name of the one to run.
const BOX: &str = "toybox";
const BLKID: &str = "blkid";

/// What reads an erofs superblock, ending well only on one.
const DUMP_EROFS: &str = "dump.erofs";
const EROFS: &str = "erofs";

/// The filesystem on `device`; none where it holds nothing known, which is
/// how a partition reads after it has been discarded.
pub fn filesystem(device: &Path) -> Result<Option<String>> {
    let path = tools::path(device)?;
    let ran = tools::find(BOX)?.run(&[BLKID.to_string(), path.clone()])?;
    if let Some(named) = named(&ran.said) {
        return Ok(Some(named));
    }
    let erofs = tools::find(DUMP_EROFS)?.run(&[path])?;
    Ok((erofs.status == 0).then(|| EROFS.to_string()))
}

/// The filesystem blkid names in a line such as
/// `/dev/block/dm-8: LABEL="data" UUID="…" TYPE="f2fs"`.
fn named(said: &str) -> Option<String> {
    said.split_whitespace().find_map(|field| {
        let value = field.strip_prefix("TYPE=")?.trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Where each filesystem a phone's data partition is usually made of keeps
/// the mark that names it, and the mark.
const F2FS_MARK: (u64, u32) = (1024, 0xf2f5_2010);
const EXT4_MARK: (u64, u16) = (1080, 0xef53);

/// What a partition reads as now, apart from the name blkid gives it: where
/// the path the phone's table names leads, how large what it leads to is,
/// and what lies where the two filesystems a phone's data partition is
/// usually made of keep their mark. A partition a phone encrypts whole, or
/// one never made a filesystem, holds none of the marks, and says so here.
pub fn describe(device: &Path, holds: Option<&str>) -> String {
    let leads_to = std::fs::canonicalize(device).unwrap_or_else(|_| device.to_path_buf());
    let holds = holds.unwrap_or("nothing blkid knows");
    let size = match crate::size_bytes(device) {
        Ok(size) => format!("{size} bytes"),
        Err(_) => "a size unread".to_string(),
    };
    let marks = format!(
        "the f2fs mark at {} reads {:#010x} and the ext4 mark at {} reads {:#06x}",
        F2FS_MARK.0,
        mark(device, F2FS_MARK.0, 4).unwrap_or(0),
        EXT4_MARK.0,
        mark(device, EXT4_MARK.0, 2).unwrap_or(0),
    );
    format!("{} leads to {}, {size}, which blkid says holds {holds}; {marks}", device.display(), leads_to.display())
}

/// The first `bytes` bytes of `device` from `at`, read as a number, the
/// first byte the least of them; none where the device cannot be read.
fn mark(device: &Path, at: u64, bytes: u32) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(device).ok()?;
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut read = vec![0u8; bytes as usize];
    file.read_exact(&mut read).ok()?;
    Some(read.iter().rev().fold(0, |all, byte| all << 8 | u32::from(*byte)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filesystem_is_read_off_the_line_blkid_prints() {
        assert_eq!(named("/dev/block/dm-8: TYPE=\"f2fs\"\n").as_deref(), Some("f2fs"));
        assert_eq!(named("/dev/block/dm-9: LABEL=\"metadata\" UUID=\"2d0f\" TYPE=\"ext4\"").as_deref(), Some("ext4"));
    }

    #[test]
    fn a_partition_blkid_makes_nothing_of_holds_no_filesystem_it_knows() {
        assert_eq!(named(""), None, "a partition it says nothing about");
        assert_eq!(named("/dev/block/dm-8: PTTYPE=\"dos\""), None, "a partition table is not a filesystem");
    }

    #[test]
    fn what_a_partition_reads_as_says_the_marks_its_filesystem_would_have_left() {
        let at = std::env::temp_dir().join(format!("skyward-probe-{}", std::process::id()));
        std::fs::create_dir_all(&at).unwrap();
        let made = at.join("f2fs.img");
        let mut image = vec![0u8; 2048];
        image[1024..1028].copy_from_slice(&0xf2f5_2010u32.to_le_bytes());
        image[1080..1082].copy_from_slice(&0xef53u16.to_le_bytes());
        std::fs::write(&made, &image).unwrap();
        let said = describe(&made, Some("f2fs"));
        assert!(said.contains("which blkid says holds f2fs"), "{said}");
        assert!(said.contains("the f2fs mark at 1024 reads 0xf2f52010"), "{said}");
        assert!(said.contains("the ext4 mark at 1080 reads 0xef53"), "{said}");
        std::fs::remove_file(&made).unwrap();
        let unwritten = at.join("nothing.img");
        std::fs::write(&unwritten, vec![0x5au8; 2048]).unwrap();
        let said = describe(&unwritten, None);
        assert!(said.contains("holds nothing blkid knows"), "{said}");
        assert!(said.contains("reads 0x5a5a5a5a"), "the mark is read as the bytes lie: {said}");
        std::fs::remove_dir_all(&at).unwrap();
    }
}
