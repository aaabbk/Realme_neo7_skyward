//! A recovery of our own on a phone whose recovery is a fragment of its
//! `vendor_boot`: the files taken out of the stock image's ramdisks, the
//! programs the recovery starts, and the Skyward that is the recovery.
//!
//! The platform fragment is left as it was, so a start of the system is
//! what it was; only the recovery fragment is written again.

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use sha2::Digest;
use skyward_bootimg::{Ramdisk, VendorBoot, RECOVERY};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::avb;

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

/// What the recovery is made of, from the manifest the build names: the
/// files taken out of the stock image, the programs whose libraries are
/// taken with them, and what the recovery's own files say.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// Files taken from the stock image's ramdisks, by their path there.
    #[serde(default)]
    pub take: Vec<String>,
    /// Programs whose libraries are taken with them: each is taken, and so
    /// is every library it names, until every one is there.
    #[serde(default)]
    pub closure: Vec<String>,
    /// Lines added to the stock recovery's `prop.default`.
    #[serde(default)]
    pub prop_append: Vec<String>,
    /// The USB controller the phone's gadget binds to.
    #[serde(default)]
    pub udc: Option<String>,
    /// Files from the computer, put in as they are: the phone's own table
    /// of partitions, which a phone with no system would not have.
    #[serde(default)]
    pub host: Vec<HostFile>,
}

/// A file the computer has, and where it goes in the recovery.
#[derive(Debug, Deserialize)]
pub struct HostFile {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub mode: Option<u32>,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }
}

/// The files of both fragments of a stock image's ramdisks, one table: a
/// file of the recovery fragment takes the place of the platform's of the
/// same name, as it does when the kernel unpacks them one after another.
struct Pool {
    files: BTreeMap<String, skyward_bootimg::Entry>,
}

impl Pool {
    fn of(image: &VendorBoot) -> Result<Self> {
        let mut files = BTreeMap::new();
        for fragment in image.fragments() {
            let ramdisk = Ramdisk::parse(&fragment.archive()?).with_context(|| format!("read the {:?} fragment's archive", fragment.name))?;
            for entry in ramdisk.entries {
                if entry.is_dir() {
                    continue;
                }
                files.insert(entry.name.clone(), entry);
            }
        }
        ensure!(!files.is_empty(), "the stock image's ramdisks hold no files");
        Ok(Self { files })
    }

    fn get(&self, name: &str) -> Option<&skyward_bootimg::Entry> {
        self.files.get(name)
    }

    /// Where the library `name` is in the pool, by the folders the phone's
    /// programs look in.
    fn library(&self, name: &str) -> Option<String> {
        ["system/lib64/", "system/lib64/hw/", "system/lib64/bootstrap/", "system/bin/"]
            .into_iter()
            .map(|folder| format!("{folder}{name}"))
            .find(|path| self.files.contains_key(path))
    }
}

/// Take `name` into `rd` as the stock image holds it, with its mode, and
/// give back its bytes.
fn put(pool: &Pool, rd: &mut Ramdisk, name: &str) -> Result<Vec<u8>> {
    if let Some(there) = rd.find(name) {
        return Ok(there.data.clone());
    }
    let entry = pool.get(name).with_context(|| format!("{name} is in none of the stock image's ramdisks"))?;
    let data = entry.data.clone();
    if entry.mode & S_IFMT == S_IFLNK {
        let target = String::from_utf8(data.clone()).with_context(|| format!("{name} is a link to something that is not a path"))?;
        rd.link(name, &target)?;
    } else {
        // The recovery runs as root, and every file it takes is the phone's
        // own, so each is root's here whatever the archive said.
        rd.add(name, entry.mode & 0o7777, data.clone())?;
    }
    Ok(data)
}

/// Take `root` and every library it names, and theirs, until they are all
/// there: a recovery that cannot load a library it needs does not start.
fn closure(pool: &Pool, rd: &mut Ramdisk, root: &str) -> Result<()> {
    let mut pending: Vec<String> = vec![root.to_string()];
    let mut seen = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let data = put(pool, rd, &name)?;
        for library in needed(&data).with_context(|| format!("read the libraries {name} names"))? {
            match pool.library(&library) {
                Some(found) => pending.push(found),
                None => bail!("{name} needs {library}, which the stock image's ramdisks do not hold"),
            }
        }
    }
    Ok(())
}

pub struct Recovery<'a> {
    pub stock: &'a Path,
    pub binary: &'a Path,
    pub system_init: &'a Path,
    pub config: &'a Path,
    pub font: &'a Path,
    pub mono: &'a Path,
    pub mono_license: &'a Path,
    pub manifest: &'a Path,
    pub init_rc: &'a Path,
    pub usb_rc: &'a Path,
}

/// The whole `vendor_boot` image with a recovery of our own in its recovery
/// fragment.
pub fn build(r: &Recovery) -> Result<Vec<u8>> {
    let bytes = fs::read(r.stock).with_context(|| format!("read {}", r.stock.display()))?;
    let mut image = VendorBoot::parse(&bytes).with_context(|| format!("read {}", r.stock.display()))?;
    let mut pool = Pool::of(&image)?;
    let manifest = Manifest::read(r.manifest)?;
    ensure!(!manifest.take.is_empty() || !manifest.closure.is_empty(), "{} names no file to take", r.manifest.display());

    // The recovery itself is a program like the others: its libraries are
    // taken with it, by the same closure.
    let binary = fs::read(r.binary).with_context(|| format!("read {}", r.binary.display()))?;
    pool.files.insert(
        "system/bin/recovery".to_string(),
        skyward_bootimg::Entry {
            name: "system/bin/recovery".to_string(),
            mode: 0o100_750,
            uid: 0,
            gid: 0,
            mtime: 0,
            nlink: 1,
            dev: (0, 0),
            rdev: (0, 0),
            data: binary,
        },
    );

    let mut rd = Ramdisk { entries: Vec::new() };
    for name in &manifest.take {
        put(&pool, &mut rd, name)?;
    }
    for root in &manifest.closure {
        closure(&pool, &mut rd, root)?;
    }
    for file in &manifest.host {
        let data = fs::read(&file.from).with_context(|| format!("read {}", file.from))?;
        ensure!(!file.to.starts_with('/') && file.to != "prop.default", "{} is not a path to put a file at", file.to);
        rd.add(&file.to, file.mode.unwrap_or(0o644), data)?;
    }
    ensure!(rd.find("system/bin/init").is_some(), "the recovery takes no init: nothing would start");
    ensure!(rd.find("system/bin/recovery").is_some(), "the recovery takes no program to be the recovery");

    // The rules init runs: our own, in place of the phone maker's, which
    // start their own recovery and the phone's own programs.
    rd.add("system/etc/init/hw/init.rc", 0o644, fs::read(r.init_rc).with_context(|| format!("read {}", r.init_rc.display()))?)?;
    let usb = fs::read_to_string(r.usb_rc).with_context(|| format!("read {}", r.usb_rc.display()))?;
    let usb = match &manifest.udc {
        Some(udc) => usb.replace("@UDC@", udc),
        None => bail!("{} names no USB controller for the phone's gadget", r.manifest.display()),
    };
    ensure!(!usb.contains("@UDC@"), "the USB rules name more than one controller");
    rd.add("system/etc/init/skyward-usb.rc", 0o644, usb.into_bytes())?;

    // The properties the stock recovery's own file holds, with what this
    // recovery adds, and without the build it was made from: the secure
    // world is told the installed system's build instead.
    let stock_props = pool.get("prop.default").context("the stock image's ramdisks hold no prop.default")?;
    let mut props = crate::without(&stock_props.data, &crate::BUILD_VERSION);
    if !props.ends_with(b"\n") {
        props.push(b'\n');
    }
    for line in &manifest.prop_append {
        props.extend_from_slice(line.as_bytes());
        props.push(b'\n');
    }
    rd.add("prop.default", 0o644, props)?;

    let read = |p: &Path| fs::read(p).with_context(|| format!("read {}", p.display()));
    rd.add("skyward/system-init", 0o755, read(r.system_init)?)?;
    // The device configuration the recovery reads is the one it was built
    // for, as every image of Skyward carries it.
    rd.add("skyward/device.toml", 0o644, read(r.config)?)?;
    rd.add("skyward/MiSansVF.ttf", 0o644, read(r.font)?)?;
    rd.add("skyward/KirkMono-Regular.ttf", 0o644, read(r.mono)?)?;
    rd.add("skyward/KirkMono-OFL.txt", 0o644, read(r.mono_license)?)?;
    if rd.find(crate::paths::TASK_PROFILES).is_none() {
        rd.add(crate::paths::TASK_PROFILES, 0o644, crate::TASK_PROFILES.to_vec())?;
    }
    rd.add(crate::paths::MTP_RC, 0o644, crate::MTP_RC.to_vec())?;
    // The commands install scripts written for other recoveries run are the
    // recovery itself, started under their names, as in every image of it.
    let (_, recovery) = crate::paths::RECOVERY.rsplit_once('/').expect("the recovery is in a folder");
    for command in crate::paths::COMMANDS {
        rd.link(command, recovery)?;
    }

    let compressed = rd.encode()?;
    image.replace_fragment(RECOVERY, compressed)?;
    let image = image.to_bytes()?;
    let size = crate::partition_size(r.stock)?;
    let footer = avb::footed(&image, "vendor_boot", size, &sha2::Sha256::digest(&image))?;
    Ok(footer)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let field = bytes.get(offset..offset + 2).with_context(|| format!("two bytes at {offset}"))?;
    Ok(u16::from_le_bytes(field.try_into().expect("two bytes")))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let field = bytes.get(offset..offset + 4).with_context(|| format!("four bytes at {offset}"))?;
    Ok(u32::from_le_bytes(field.try_into().expect("four bytes")))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    let field = bytes.get(offset..offset + 8).with_context(|| format!("eight bytes at {offset}"))?;
    Ok(u64::from_le_bytes(field.try_into().expect("eight bytes")))
}

/// The libraries an ELF 64-bit program names as needed, in the order it
/// names them: what the loader would load before it runs.
fn needed(elf: &[u8]) -> Result<Vec<String>> {
    if elf.len() < 64 || elf.get(..4) != Some(b"\x7fELF".as_slice()) {
        return Ok(Vec::new());
    }
    ensure!(elf[4] == 2 && elf[5] == 1, "not a little-endian 64-bit ELF file");
    let phoff = u64_at(elf, 0x20)? as usize;
    let phentsize = u16_at(elf, 0x36)? as usize;
    let phnum = u16_at(elf, 0x38)? as usize;
    let mut loads = Vec::new();
    let mut dynamic = None;
    for n in 0..phnum {
        let at = phoff + n * phentsize;
        let kind = u32_at(elf, at)?;
        if kind == 1 {
            loads.push((u64_at(elf, at + 8)?, u64_at(elf, at + 16)?, u64_at(elf, at + 32)?));
        } else if kind == 2 {
            dynamic = Some((u64_at(elf, at + 8)?, u64_at(elf, at + 32)?));
        }
    }
    let Some((dyn_at, dyn_len)) = dynamic else {
        return Ok(Vec::new());
    };
    let mut offsets = Vec::new();
    let mut strtab = None;
    let mut at = dyn_at as usize;
    let end = at + dyn_len as usize;
    while at + 16 <= end {
        let tag = u64_at(elf, at)?;
        let value = u64_at(elf, at + 8)?;
        match tag {
            0 => break,
            1 => offsets.push(value),
            5 => strtab = Some(value),
            _ => {}
        }
        at += 16;
    }
    let Some(strtab) = strtab else {
        return Ok(Vec::new());
    };
    let base = loads
        .iter()
        .find(|(_offset, vaddr, filesz)| strtab >= *vaddr && strtab < vaddr + filesz)
        .map(|(offset, vaddr, _)| offset + (strtab - vaddr))
        .with_context(|| format!("the strings of an ELF file are at {strtab:#x}, in none of its segments"))?;
    let mut out = Vec::new();
    for offset in offsets {
        let start = (base + offset) as usize;
        let rest = elf.get(start..).with_context(|| format!("a library's name at {start} runs past the file"))?;
        let len = rest.iter().position(|b| *b == 0).context("a library's name never ends")?;
        out.push(String::from_utf8(rest[..len].to_vec()).context("a library's name is not text")?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_is_no_elf_names_no_library() {
        assert!(needed(b"#! /system/bin/sh\n").unwrap().is_empty());
        assert!(needed(b"").unwrap().is_empty());
    }

    #[test]
    fn the_library_of_a_program_is_found_by_the_folders_the_phone_looks_in() {
        let entry = skyward_bootimg::Entry {
            name: "system/lib64/libc.so".into(),
            mode: 0o100644,
            uid: 0,
            gid: 0,
            mtime: 0,
            nlink: 1,
            dev: (0, 0),
            rdev: (0, 0),
            data: Vec::new(),
        };
        let pool = Pool { files: [("system/lib64/libc.so".to_string(), entry)].into_iter().collect() };
        assert_eq!(pool.library("libc.so").as_deref(), Some("system/lib64/libc.so"));
        assert_eq!(pool.library("libz.so"), None);
    }
}
