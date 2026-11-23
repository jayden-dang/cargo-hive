//! Reads the loose object file names a Mach-O linked binary's debug map (N_OSO) names.
//!
//! `read` returns `Ok(None)` when the file is not a linked Mach-O binary (no magic,
//! `MH_OBJECT`, or `MH_DSYM`). Fat images contribute the union of every thin slice.
//! Every byte comes from `Fs::read_at`; hitting a size cap is `MapError::Bound`.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::Path;

use cargo_hive_core::caps::{FAT_ARCH_CAP, MACHO_CMDS_CAP, MACHO_STRTAB_CAP, MACHO_SYMTAB_CAP};
use cargo_hive_platform::Fs;

const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_MAGIC: u32 = 0xfeed_face;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const MH_CIGAM: u32 = 0xcefa_edfe;
const FAT_MAGIC_BE: u32 = 0xcafe_babe;
const FAT_MAGIC_64_BE: u32 = 0xcafe_babf;
const MH_OBJECT: u32 = 0x1;
const MH_DSYM: u32 = 0xa;
const LC_SYMTAB: u32 = 0x2;
const N_OSO: u8 = 0x66;
const HEADER64: u64 = 32;
const NLIST64: u64 = 16;
const SYMTAB_CMD: u64 = 24;
const FAT_ARCH: u64 = 20;
const FAT_ARCH_64: u64 = 32;

/// Loose object file names and archive-member count from one linked binary's debug map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugMap {
    pub loose: BTreeSet<String>,
    pub archive_entries: u64,
}

/// Why a Mach-O debug-map read could not finish.
#[derive(Debug)]
pub enum MapError {
    Read(io::Error),
    Truncated(&'static str),
    Bound(&'static str, u64),
    UnknownSlice(u32),
    Unsupported(&'static str),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Read(e) => write!(f, "could not read Mach-O bytes: {e}"),
            MapError::Truncated(what) => {
                write!(
                    f,
                    "Mach-O {what} is truncated; re-link or exclude the binary"
                )
            }
            MapError::Bound(what, got) => {
                write!(
                    f,
                    "Mach-O {what} is {got} bytes, over its read bound; re-link or exclude the binary"
                )
            }
            MapError::UnknownSlice(cputype) => {
                write!(
                    f,
                    "fat Mach-O slice cputype {cputype:#x} is not thin 64-bit little-endian; exclude the binary"
                )
            }
            MapError::Unsupported(what) => {
                write!(f, "Mach-O {what} is unsupported; exclude the binary")
            }
        }
    }
}

impl std::error::Error for MapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MapError::Read(e) => Some(e),
            _ => None,
        }
    }
}

/// Read the debug map of the Mach-O (or fat Mach-O) at `path`.
pub fn read(fs: &dyn Fs, path: &Path) -> Result<Option<DebugMap>, MapError> {
    let magic_bytes = read_at(fs, path, 0, 4)?;
    if magic_bytes.len() < 4 {
        return Ok(None);
    }
    let raw: [u8; 4] = magic_bytes.try_into().unwrap();
    let magic_le = u32::from_le_bytes(raw);
    let magic_be = u32::from_be_bytes(raw);

    match magic_le {
        MH_MAGIC_64 => parse_thin(fs, path, 0, None),
        MH_MAGIC | MH_CIGAM | MH_CIGAM_64 => {
            Err(MapError::Unsupported("32-bit or big-endian thin header"))
        }
        _ if magic_be == FAT_MAGIC_BE => parse_fat(fs, path, false),
        _ if magic_be == FAT_MAGIC_64_BE => parse_fat(fs, path, true),
        _ => Ok(None),
    }
}

fn read_at(fs: &dyn Fs, path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, MapError> {
    fs.read_at(path, offset, len).map_err(MapError::Read)
}

fn parse_fat(fs: &dyn Fs, path: &Path, is_64: bool) -> Result<Option<DebugMap>, MapError> {
    let hdr = read_at(fs, path, 0, 8)?;
    if hdr.len() < 8 {
        return Err(MapError::Truncated("fat header"));
    }
    let nfat_arch = u32::from_be_bytes(hdr[4..8].try_into().unwrap());
    if nfat_arch == 0 || nfat_arch > FAT_ARCH_CAP {
        return Err(MapError::Unsupported("nfat_arch out of range"));
    }
    let arch_size = if is_64 { FAT_ARCH_64 } else { FAT_ARCH };
    let table_len = (u64::from(nfat_arch))
        .checked_mul(arch_size)
        .ok_or(MapError::Truncated("fat arch table"))?;
    let table = read_at(fs, path, 8, table_len as usize)?;
    if table.len() as u64 != table_len {
        return Err(MapError::Truncated("fat arch table"));
    }

    let mut merged = DebugMap {
        loose: BTreeSet::new(),
        archive_entries: 0,
    };
    let mut any_linked = false;
    for i in 0..nfat_arch as usize {
        let off = i * arch_size as usize;
        let cputype = u32::from_be_bytes(table[off..off + 4].try_into().unwrap());
        let (slice_off, slice_size) = if is_64 {
            let o = u64::from_be_bytes(table[off + 8..off + 16].try_into().unwrap());
            let s = u64::from_be_bytes(table[off + 16..off + 24].try_into().unwrap());
            (o, s)
        } else {
            let o = u32::from_be_bytes(table[off + 8..off + 12].try_into().unwrap()) as u64;
            let s = u32::from_be_bytes(table[off + 12..off + 16].try_into().unwrap()) as u64;
            (o, s)
        };
        if slice_size < 4 {
            return Err(MapError::UnknownSlice(cputype));
        }
        let slice_end = slice_off
            .checked_add(slice_size)
            .ok_or(MapError::Truncated("fat slice range"))?;
        let slice_magic = read_range(fs, path, slice_off, 0, 4, Some(slice_end))?;
        if slice_magic.len() < 4 {
            return Err(MapError::UnknownSlice(cputype));
        }
        let slice_le = u32::from_le_bytes(slice_magic.try_into().unwrap());
        if slice_le != MH_MAGIC_64 {
            return Err(MapError::UnknownSlice(cputype));
        }
        match parse_thin(fs, path, slice_off, Some(slice_end))? {
            None => {}
            Some(map) => {
                any_linked = true;
                merged.archive_entries += map.archive_entries;
                merged.loose.extend(map.loose);
            }
        }
    }
    if any_linked {
        Ok(Some(merged))
    } else {
        Ok(None)
    }
}

fn read_range(
    fs: &dyn Fs,
    path: &Path,
    base: u64,
    relative: u64,
    len: usize,
    end: Option<u64>,
) -> Result<Vec<u8>, MapError> {
    let offset = base
        .checked_add(relative)
        .ok_or(MapError::Truncated("offset"))?;
    let finish = offset
        .checked_add(len as u64)
        .ok_or(MapError::Truncated("range"))?;
    if end.is_some_and(|end| finish > end) {
        return Err(MapError::Truncated("fat slice range"));
    }
    read_at(fs, path, offset, len)
}

fn parse_thin(
    fs: &dyn Fs,
    path: &Path,
    base: u64,
    end: Option<u64>,
) -> Result<Option<DebugMap>, MapError> {
    let header = read_range(fs, path, base, 0, HEADER64 as usize, end)?;
    if header.len() < HEADER64 as usize {
        return Err(MapError::Truncated("header"));
    }
    let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
    if magic != MH_MAGIC_64 {
        return Err(MapError::Unsupported("32-bit or big-endian thin header"));
    }
    let filetype = u32::from_le_bytes(header[12..16].try_into().unwrap());
    if filetype == MH_OBJECT || filetype == MH_DSYM {
        return Ok(None);
    }
    let ncmds = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let sizeofcmds = u32::from_le_bytes(header[20..24].try_into().unwrap()) as u64;
    if sizeofcmds > MACHO_CMDS_CAP {
        return Err(MapError::Bound("sizeofcmds", sizeofcmds));
    }

    let cmds = if sizeofcmds == 0 {
        Vec::new()
    } else {
        let cmds = read_range(fs, path, base, HEADER64, sizeofcmds as usize, end)?;
        if cmds.len() as u64 != sizeofcmds {
            return Err(MapError::Truncated("load commands"));
        }
        cmds
    };

    let mut symtab: Option<(u32, u32, u32, u32)> = None;
    let mut cursor = 0usize;
    for _ in 0..ncmds {
        if cursor + 8 > cmds.len() {
            return Err(MapError::Truncated("load command header"));
        }
        let cmd = u32::from_le_bytes(cmds[cursor..cursor + 4].try_into().unwrap());
        let cmdsize = u32::from_le_bytes(cmds[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        if cmdsize < 8 || !cmdsize.is_multiple_of(8) || cursor + cmdsize > cmds.len() {
            return Err(MapError::Truncated("load command size"));
        }
        if cmd == LC_SYMTAB && symtab.is_none() {
            if cmdsize < SYMTAB_CMD as usize {
                return Err(MapError::Truncated("LC_SYMTAB"));
            }
            let symoff = u32::from_le_bytes(cmds[cursor + 8..cursor + 12].try_into().unwrap());
            let nsyms = u32::from_le_bytes(cmds[cursor + 12..cursor + 16].try_into().unwrap());
            let stroff = u32::from_le_bytes(cmds[cursor + 16..cursor + 20].try_into().unwrap());
            let strsize = u32::from_le_bytes(cmds[cursor + 20..cursor + 24].try_into().unwrap());
            symtab = Some((symoff, nsyms, stroff, strsize));
        }
        cursor += cmdsize;
    }

    let Some((symoff, nsyms, stroff, strsize)) = symtab else {
        return Ok(Some(DebugMap {
            loose: BTreeSet::new(),
            archive_entries: 0,
        }));
    };

    let sym_bytes = u64::from(nsyms)
        .checked_mul(NLIST64)
        .ok_or(MapError::Bound("symtab", u64::MAX))?;
    if sym_bytes > MACHO_SYMTAB_CAP {
        return Err(MapError::Bound("symtab", sym_bytes));
    }
    let str_bytes = u64::from(strsize);
    if str_bytes > MACHO_STRTAB_CAP {
        return Err(MapError::Bound("strtab", str_bytes));
    }

    let symtab_data = if sym_bytes == 0 {
        Vec::new()
    } else {
        let data = read_range(fs, path, base, u64::from(symoff), sym_bytes as usize, end)?;
        if data.len() as u64 != sym_bytes {
            return Err(MapError::Truncated("symbol table"));
        }
        data
    };
    let strtab = if str_bytes == 0 {
        Vec::new()
    } else {
        let data = read_range(fs, path, base, u64::from(stroff), str_bytes as usize, end)?;
        if data.len() as u64 != str_bytes {
            return Err(MapError::Truncated("string table"));
        }
        data
    };

    let mut loose = BTreeSet::new();
    let mut archive_entries = 0u64;
    for i in 0..nsyms as usize {
        let off = i * NLIST64 as usize;
        let n_strx = u32::from_le_bytes(symtab_data[off..off + 4].try_into().unwrap());
        let n_type = symtab_data[off + 4];
        if n_type != N_OSO {
            continue;
        }
        let start = n_strx as usize;
        if start >= strtab.len() {
            return Err(MapError::Truncated("n_strx"));
        }
        let end = strtab[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .ok_or(MapError::Truncated("string"))?;
        let raw = &strtab[start..end];
        if raw.last() == Some(&b')') && raw.contains(&b'(') {
            archive_entries += 1;
            continue;
        }
        let name = match raw.rsplit(|b| *b == b'/').next() {
            Some(file) => String::from_utf8_lossy(file).into_owned(),
            None => String::from_utf8_lossy(raw).into_owned(),
        };
        loose.insert(name);
    }

    Ok(Some(DebugMap {
        loose,
        archive_entries,
    }))
}

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;
    use crate::reader_testing::ReaderFs;

    fn thin(name: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 73];
        for (at, n) in [
            (0, MH_MAGIC_64),
            (12, 2),
            (16, 1),
            (20, 24),
            (32, LC_SYMTAB),
            (36, 24),
            (40, 56),
            (44, 1),
            (48, 72),
            (52, (name.len() + 2) as u32),
            (56, 1),
        ] {
            bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        bytes[60] = N_OSO;
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes
    }

    pub fn debug_map(data: &[u8]) {
        let path = Path::new("binary");
        let _ = read(&ReaderFs::new(data.to_vec()), path);
        let name = format!("object-{}.rcgu.o", data.first().copied().unwrap_or(0));
        let bytes = thin(&name);
        let map = read(&ReaderFs::new(bytes.clone()), path).unwrap().unwrap();
        assert_eq!(map.loose, BTreeSet::from([name]));
        assert_eq!(map.archive_entries, 0);
        let archive = thin("libprobe.rlib(member.o)");
        let map = read(&ReaderFs::new(archive), path).unwrap().unwrap();
        assert!(map.loose.is_empty());
        assert_eq!(map.archive_entries, 1);
        let second = thin("libprobe.rlib(member.o)");
        let mut fat = vec![0u8; 48];
        fat[..4].copy_from_slice(&FAT_MAGIC_BE.to_be_bytes());
        fat[4..8].copy_from_slice(&2u32.to_be_bytes());
        for (at, offset, size) in [
            (8usize, 48u32, bytes.len() as u32),
            (28, 48 + bytes.len() as u32, second.len() as u32),
        ] {
            fat[at..at + 4].copy_from_slice(&0x0100_000cu32.to_be_bytes());
            fat[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
            fat[at + 12..at + 16].copy_from_slice(&size.to_be_bytes());
        }
        fat.extend_from_slice(&bytes);
        fat.extend_from_slice(&second);
        let merged = read(&ReaderFs::new(fat), path).unwrap().unwrap();
        assert_eq!(
            merged.loose,
            BTreeSet::from([format!(
                "object-{}.rcgu.o",
                data.first().copied().unwrap_or(0)
            )])
        );
        assert_eq!(merged.archive_entries, 1);
        let mut fs = ReaderFs::new(bytes);
        fs.chunk = 3;
        assert!(read(&fs, path).unwrap().is_none());
        fs.reads.set(0);
        fs.chunk = 8;
        assert!(read(&fs, path).is_err());
        fs.reads.set(0);
        fs.read_error = Some(io::ErrorKind::PermissionDenied);
        assert!(read(&fs, path).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugMap, MapError, read};
    use cargo_hive_core::caps::{FAT_ARCH_CAP, MACHO_CMDS_CAP, MACHO_STRTAB_CAP, MACHO_SYMTAB_CAP};
    use cargo_hive_platform::{
        CompressOutcome, EntryKind, EntryStat, Fs, LockHolder, LockKind, LockState, Sharing,
        TempDir, TryLock, VolumeInfo,
    };
    use std::collections::{BTreeSet, HashMap};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const MH_MAGIC: u32 = 0xfeed_face;
    const MH_CIGAM_64: u32 = 0xcffa_edfe;
    const FAT_MAGIC: u32 = 0xcafe_babe;
    const FAT_MAGIC_64: u32 = 0xcafe_babf;
    const MH_OBJECT: u32 = 0x1;
    const MH_EXECUTE: u32 = 0x2;
    const MH_DSYM: u32 = 0xa;
    const LC_SYMTAB: u32 = 0x2;
    const N_OSO: u8 = 0x66;
    const NLIST64: usize = 16;
    const HEADER64: usize = 32;
    const SYMTAB_CMD: usize = 24;

    struct MemFs {
        files: HashMap<PathBuf, Vec<u8>>,
    }

    impl MemFs {
        fn with(path: &Path, bytes: Vec<u8>) -> Self {
            let mut files = HashMap::new();
            files.insert(path.to_path_buf(), bytes);
            Self { files }
        }
    }

    fn unused() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "not used by debug_map tests")
    }

    impl Fs for MemFs {
        fn temp_dir(&self, _: &Path, _: &str) -> io::Result<TempDir> {
            Err(unused())
        }
        fn write_file(&self, _: &Path, _: &[u8]) -> io::Result<()> {
            Err(unused())
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))
        }
        fn read_at(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let bytes = self.read(path)?;
            let Ok(start) = usize::try_from(offset) else {
                return Ok(Vec::new());
            };
            if start >= bytes.len() {
                return Ok(Vec::new());
            }
            let end = start.saturating_add(len).min(bytes.len());
            Ok(bytes[start..end].to_vec())
        }
        fn hardlink(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(unused())
        }
        fn set_mtime(&self, _: &Path, _: SystemTime) -> io::Result<()> {
            Err(unused())
        }
        fn mtime(&self, _: &Path) -> io::Result<SystemTime> {
            Err(unused())
        }
        fn inode(&self, _: &Path) -> io::Result<u64> {
            Err(unused())
        }
        fn clone_file(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(unused())
        }
        fn create_dir(&self, _: &Path) -> io::Result<()> {
            Err(unused())
        }
        fn sharing(&self, _: &Path) -> io::Result<Sharing> {
            Err(unused())
        }
        fn compress(&self, _: &Path) -> CompressOutcome {
            CompressOutcome::Failed("not used".into())
        }
        fn hold_lock(&self, _: LockKind, _: &Path) -> io::Result<LockHolder> {
            Err(unused())
        }
        fn try_lock_exclusive(&self, _: &Path) -> TryLock {
            TryLock::Unopenable(unused())
        }
        fn try_lock_elsewhere(&self, _: LockKind, _: &Path) -> io::Result<LockState> {
            Err(unused())
        }
        fn entry_stat(&self, _: &Path) -> io::Result<EntryStat> {
            Err(unused())
        }
        fn read_dir_bounded(&self, _: &Path, _: usize) -> io::Result<Vec<(String, EntryKind)>> {
            Err(unused())
        }
        fn remove_tree(&self, _: &Path) -> io::Result<()> {
            Err(unused())
        }
        fn remove_file(&self, _: &Path) -> io::Result<()> {
            Err(unused())
        }
        fn volume(&self, _: &Path) -> io::Result<VolumeInfo> {
            Err(unused())
        }
    }

    fn put_u32_le(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u32_be(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_be_bytes());
    }

    fn put_u64_le(buf: &mut Vec<u8>, v: u64) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64_be(buf: &mut Vec<u8>, v: u64) {
        buf.extend_from_slice(&v.to_be_bytes());
    }

    fn set_u32_le(buf: &mut [u8], at: usize, v: u32) {
        buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Thin MH_EXECUTE with the given N_OSO strings (and optional non-OSO filler).
    fn thin_with_oso(strings: &[&str]) -> Vec<u8> {
        thin_with_oso_filetype(MH_EXECUTE, strings)
    }

    fn thin_with_oso_filetype(filetype: u32, strings: &[&str]) -> Vec<u8> {
        let mut strtab = vec![0u8]; // index 0 is empty
        let mut str_offs = Vec::new();
        for s in strings {
            str_offs.push(strtab.len() as u32);
            strtab.extend_from_slice(s.as_bytes());
            strtab.push(0);
        }

        let nsyms = strings.len() as u32;
        let symoff = (HEADER64 + SYMTAB_CMD) as u32;
        let stroff = symoff + nsyms * NLIST64 as u32;
        let strsize = strtab.len() as u32;

        let mut buf = Vec::new();
        // mach_header_64
        put_u32_le(&mut buf, MH_MAGIC_64);
        put_u32_le(&mut buf, 0x0100_000c); // CPU_TYPE_ARM64
        put_u32_le(&mut buf, 0);
        put_u32_le(&mut buf, filetype);
        put_u32_le(&mut buf, 1); // ncmds
        put_u32_le(&mut buf, SYMTAB_CMD as u32); // sizeofcmds
        put_u32_le(&mut buf, 0); // flags
        put_u32_le(&mut buf, 0); // reserved
        // LC_SYMTAB
        put_u32_le(&mut buf, LC_SYMTAB);
        put_u32_le(&mut buf, SYMTAB_CMD as u32);
        put_u32_le(&mut buf, symoff);
        put_u32_le(&mut buf, nsyms);
        put_u32_le(&mut buf, stroff);
        put_u32_le(&mut buf, strsize);
        // nlist_64 entries
        for off in str_offs {
            put_u32_le(&mut buf, off);
            buf.push(N_OSO);
            buf.push(0); // n_sect
            buf.extend_from_slice(&0u16.to_le_bytes()); // n_desc
            put_u64_le(&mut buf, 0); // n_value
        }
        buf.extend_from_slice(&strtab);
        buf
    }

    fn thin_no_symtab(filetype: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        put_u32_le(&mut buf, MH_MAGIC_64);
        put_u32_le(&mut buf, 0);
        put_u32_le(&mut buf, 0);
        put_u32_le(&mut buf, filetype);
        put_u32_le(&mut buf, 0); // ncmds
        put_u32_le(&mut buf, 0); // sizeofcmds
        put_u32_le(&mut buf, 0);
        put_u32_le(&mut buf, 0);
        buf
    }

    fn wrap_fat(slices: &[(u32, Vec<u8>)], magic64: bool) -> Vec<u8> {
        let n = slices.len() as u32;
        let arch_size = if magic64 { 32u32 } else { 20u32 };
        let header_len = 8 + arch_size * n;
        let mut offsets = Vec::new();
        let mut cursor = u64::from(header_len);
        // Align each slice to 16 bytes.
        for (_, data) in slices {
            let align = (cursor + 15) & !15;
            offsets.push(align);
            cursor = align + data.len() as u64;
        }
        let mut buf = Vec::new();
        if magic64 {
            put_u32_be(&mut buf, FAT_MAGIC_64);
        } else {
            put_u32_be(&mut buf, FAT_MAGIC);
        }
        put_u32_be(&mut buf, n);
        for (i, (cputype, data)) in slices.iter().enumerate() {
            put_u32_be(&mut buf, *cputype);
            put_u32_be(&mut buf, 0); // cpusubtype
            if magic64 {
                put_u64_be(&mut buf, offsets[i]);
                put_u64_be(&mut buf, data.len() as u64);
                put_u32_be(&mut buf, 4); // align = 2^4
                put_u32_be(&mut buf, 0); // reserved
            } else {
                put_u32_be(&mut buf, offsets[i] as u32);
                put_u32_be(&mut buf, data.len() as u32);
                put_u32_be(&mut buf, 4);
            }
        }
        for (i, (_, data)) in slices.iter().enumerate() {
            while (buf.len() as u64) < offsets[i] {
                buf.push(0);
            }
            buf.extend_from_slice(data);
        }
        buf
    }

    #[test]
    fn generated_debug_maps_and_truncations_keep_exact_objects() {
        for data in [&b""[..], &b"\xff"[..], &b"probe"[..]] {
            super::fuzzing::debug_map(data);
        }
        let bytes = thin_with_oso(&["kept.rcgu.o"]);
        let path = Path::new("binary");
        for end in 4..bytes.len() {
            assert!(
                read(&MemFs::with(path, bytes[..end].to_vec()), path).is_err(),
                "cut {end}"
            );
        }
        let fs = crate::reader_testing::ReaderFs::new(vec![]);
        assert!(super::read_range(&fs, path, u64::MAX, 1, 1, None).is_err());
        assert_eq!(fs.reads.get(), 0);
    }

    #[test]
    fn a_fat_slice_cannot_borrow_tables_from_bytes_after_its_declared_end() {
        let thin = thin_with_oso(&["/synthetic/kept.rcgu.o"]);
        for fat64 in [false, true] {
            for size in [32usize, 56, 72, thin.len() - 1] {
                let mut bytes = wrap_fat(&[(0x0100_000c, thin.clone())], fat64);
                if fat64 {
                    bytes[24..32].copy_from_slice(&(size as u64).to_be_bytes());
                } else {
                    bytes[20..24].copy_from_slice(&(size as u32).to_be_bytes());
                }
                let path = Path::new("binary");
                assert!(
                    read(&MemFs::with(path, bytes), path).is_err(),
                    "tables must belong to their declared slice, fat64={fat64}, size={size}"
                );
            }
        }
    }

    #[test]
    fn a_fat_slice_with_an_overflowing_end_is_a_range_refusal() {
        let mut bytes = wrap_fat(&[(0x0100_000c, thin_with_oso(&["kept.rcgu.o"]))], true);
        bytes[16..24].copy_from_slice(&(u64::MAX - 31).to_be_bytes());
        bytes[24..32].copy_from_slice(&64u64.to_be_bytes());
        let path = Path::new("binary");
        assert!(matches!(
            read(&MemFs::with(path, bytes), path),
            Err(MapError::Truncated("fat slice range"))
        ));
    }

    #[test]
    fn a_thin_executable_yields_its_loose_object_names() {
        let path = Path::new("/bin/app");
        let bytes = thin_with_oso(&[
            "/target/debug/deps/foo-hash.0.rcgu.o",
            "/target/debug/deps/bar-hash.1.rcgu.o",
        ]);
        let fs = MemFs::with(path, bytes);
        let map = read(&fs, path).unwrap().expect("linked binary");
        assert_eq!(
            map.loose,
            BTreeSet::from(["foo-hash.0.rcgu.o".into(), "bar-hash.1.rcgu.o".into(),])
        );
        assert_eq!(map.archive_entries, 0);
    }

    #[test]
    fn archive_entries_name_no_loose_object() {
        let path = Path::new("/bin/app");
        let bytes = thin_with_oso(&[
            "/target/debug/deps/libfoo-hash.rlib(foo-hash.0.rcgu.o)",
            "/target/debug/deps/loose-hash.0.rcgu.o",
        ]);
        let fs = MemFs::with(path, bytes);
        let map = read(&fs, path).unwrap().expect("linked binary");
        assert_eq!(map.loose, BTreeSet::from(["loose-hash.0.rcgu.o".into()]));
        assert_eq!(map.archive_entries, 1);
    }

    #[test]
    fn a_path_with_spaces_keeps_its_whole_file_name() {
        let path = Path::new("/bin/app");
        let bytes = thin_with_oso(&["/build/my crate/deps/foo bar-hash.0.rcgu.o"]);
        let fs = MemFs::with(path, bytes);
        let map = read(&fs, path).unwrap().expect("linked binary");
        assert_eq!(map.loose, BTreeSet::from(["foo bar-hash.0.rcgu.o".into()]));
    }

    #[test]
    fn object_and_dsym_files_are_not_linked_binaries() {
        let path = Path::new("/obj");
        for filetype in [MH_OBJECT, MH_DSYM] {
            let fs = MemFs::with(path, thin_with_oso_filetype(filetype, &["/x.o"]));
            assert!(read(&fs, path).unwrap().is_none());
        }
    }

    #[test]
    fn a_file_without_mach_o_magic_is_not_a_binary() {
        let path = Path::new("/not-macho");
        let fs = MemFs::with(path, b"not a mach-o file".to_vec());
        assert!(read(&fs, path).unwrap().is_none());
        let fs = MemFs::with(path, b"abc".to_vec());
        assert!(read(&fs, path).unwrap().is_none());
        let fs = MemFs::with(path, Vec::new());
        assert!(read(&fs, path).unwrap().is_none());
    }

    #[test]
    fn a_binary_with_no_symtab_has_an_empty_map() {
        let path = Path::new("/bin/app");
        let fs = MemFs::with(path, thin_no_symtab(MH_EXECUTE));
        let map = read(&fs, path).unwrap().expect("linked binary");
        assert_eq!(
            map,
            DebugMap {
                loose: BTreeSet::new(),
                archive_entries: 0
            }
        );
    }

    #[test]
    fn fat_and_fat64_slices_are_read_relative_to_the_slice() {
        let path = Path::new("/bin/fat");
        let s0 = thin_with_oso(&["/a/one.rcgu.o"]);
        let s1 = thin_with_oso(&["/b/two.rcgu.o"]);
        for magic64 in [false, true] {
            let bytes = wrap_fat(
                &[(0x0100_000c, s0.clone()), (0x0100_0007, s1.clone())],
                magic64,
            );
            let fs = MemFs::with(path, bytes);
            let map = read(&fs, path).unwrap().expect("fat linked binary");
            assert_eq!(
                map.loose,
                BTreeSet::from(["one.rcgu.o".into(), "two.rcgu.o".into()]),
                "magic64={magic64}"
            );
        }
    }

    #[test]
    fn a_32_bit_or_big_endian_thin_header_is_unsupported() {
        let path = Path::new("/bin/odd");
        for magic in [MH_MAGIC, MH_CIGAM_64] {
            let mut bytes = vec![0u8; HEADER64];
            bytes[..4].copy_from_slice(&magic.to_le_bytes());
            let fs = MemFs::with(path, bytes);
            let err = read(&fs, path).unwrap_err();
            assert!(matches!(err, MapError::Unsupported(_)), "{err:?}");
        }
    }

    #[test]
    fn an_unknown_fat_slice_is_an_error() {
        let path = Path::new("/bin/fat");
        let good = thin_with_oso(&["/a/one.rcgu.o"]);
        let mut bad = thin_with_oso(&["/b/two.rcgu.o"]);
        // Make the second slice 32-bit Mach-O.
        bad[..4].copy_from_slice(&MH_MAGIC.to_le_bytes());
        let bytes = wrap_fat(&[(0x0100_000c, good), (0x0000_0007, bad)], false);
        let fs = MemFs::with(path, bytes);
        let err = read(&fs, path).unwrap_err();
        assert!(matches!(err, MapError::UnknownSlice(_)), "{err:?}");
    }

    #[test]
    fn nfat_arch_of_zero_or_past_the_cap_is_unsupported() {
        let path = Path::new("/bin/fat");
        for nfat in [0u32, FAT_ARCH_CAP + 1] {
            let mut bytes = Vec::new();
            put_u32_be(&mut bytes, FAT_MAGIC);
            put_u32_be(&mut bytes, nfat);
            let fs = MemFs::with(path, bytes);
            let err = read(&fs, path).unwrap_err();
            assert!(
                matches!(err, MapError::Unsupported(_)),
                "nfat={nfat} {err:?}"
            );
        }
    }

    #[test]
    fn a_magic_with_a_short_header_is_truncated() {
        let path = Path::new("/bin/short");
        let mut bytes = MH_MAGIC_64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 10]); // 14 bytes total, under 32
        let fs = MemFs::with(path, bytes);
        let err = read(&fs, path).unwrap_err();
        assert!(matches!(err, MapError::Truncated(_)), "{err:?}");
    }

    #[test]
    fn cmdsize_not_a_multiple_of_8_or_past_sizeofcmds_is_truncated() {
        let path = Path::new("/bin/badcmd");
        // cmdsize 12 is not a multiple of 8.
        let mut bytes = thin_no_symtab(MH_EXECUTE);
        set_u32_le(&mut bytes, 16, 1); // ncmds
        set_u32_le(&mut bytes, 20, 12); // sizeofcmds
        bytes.extend_from_slice(&LC_SYMTAB.to_le_bytes());
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        let fs = MemFs::with(path, bytes);
        let err = read(&fs, path).unwrap_err();
        assert!(matches!(err, MapError::Truncated(_)), "{err:?}");

        // cmdsize 24 past sizeofcmds 16.
        let mut bytes = thin_no_symtab(MH_EXECUTE);
        set_u32_le(&mut bytes, 16, 1);
        set_u32_le(&mut bytes, 20, 16);
        bytes.extend_from_slice(&LC_SYMTAB.to_le_bytes());
        bytes.extend_from_slice(&24u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        let fs = MemFs::with(path, bytes);
        let err = read(&fs, path).unwrap_err();
        assert!(matches!(err, MapError::Truncated(_)), "{err:?}");
    }

    #[test]
    fn an_n_strx_past_the_string_table_is_truncated() {
        let path = Path::new("/bin/app");
        let mut bytes = thin_with_oso(&["/a/x.o"]);
        // n_strx of the sole symbol → past strsize.
        let symoff = HEADER64 + SYMTAB_CMD;
        set_u32_le(&mut bytes, symoff, 10_000);
        let fs = MemFs::with(path, bytes);
        let err = read(&fs, path).unwrap_err();
        assert!(matches!(err, MapError::Truncated(_)), "{err:?}");
    }

    #[test]
    fn each_cap_accepts_its_limit_and_rejects_one_past() {
        let path = Path::new("/bin/cap");

        // MACHO_CMDS_CAP: exactly at → empty map; one past → Bound.
        {
            let sizeofcmds = MACHO_CMDS_CAP as u32;
            let mut bytes = Vec::new();
            put_u32_le(&mut bytes, MH_MAGIC_64);
            put_u32_le(&mut bytes, 0);
            put_u32_le(&mut bytes, 0);
            put_u32_le(&mut bytes, MH_EXECUTE);
            put_u32_le(&mut bytes, 1); // ncmds
            put_u32_le(&mut bytes, sizeofcmds);
            put_u32_le(&mut bytes, 0);
            put_u32_le(&mut bytes, 0);
            // One load command spanning the whole sizeofcmds (multiple of 8).
            put_u32_le(&mut bytes, 0x19); // LC_SEGMENT_64, ignored
            put_u32_le(&mut bytes, sizeofcmds);
            bytes.resize(HEADER64 + sizeofcmds as usize, 0);
            let fs = MemFs::with(path, bytes.clone());
            let map = read(&fs, path).unwrap().expect("at cmds cap");
            assert_eq!(map.archive_entries, 0);
            assert!(map.loose.is_empty());

            let mut over = bytes;
            set_u32_le(&mut over, 20, (MACHO_CMDS_CAP + 1) as u32);
            let fs = MemFs::with(path, over);
            assert!(matches!(
                read(&fs, path).unwrap_err(),
                MapError::Bound(_, v) if v == MACHO_CMDS_CAP + 1
            ));
        }

        // MACHO_SYMTAB_CAP: exactly at passes the bound (short body → Truncated); one past → Bound.
        {
            let nsyms = (MACHO_SYMTAB_CAP / NLIST64 as u64) as u32;
            let mut bytes = thin_no_symtab(MH_EXECUTE);
            set_u32_le(&mut bytes, 16, 1);
            set_u32_le(&mut bytes, 20, SYMTAB_CMD as u32);
            put_u32_le(&mut bytes, LC_SYMTAB);
            put_u32_le(&mut bytes, SYMTAB_CMD as u32);
            put_u32_le(&mut bytes, (HEADER64 + SYMTAB_CMD) as u32); // symoff
            put_u32_le(&mut bytes, nsyms);
            put_u32_le(&mut bytes, (HEADER64 + SYMTAB_CMD) as u32); // stroff
            put_u32_le(&mut bytes, 1); // strsize
            let fs = MemFs::with(path, bytes.clone());
            let err = read(&fs, path).unwrap_err();
            assert!(
                matches!(err, MapError::Truncated(_)),
                "exact symtab cap should read, got {err:?}"
            );

            let mut over = bytes;
            // nsyms one past: (nsyms+1)*16 = cap + 16
            set_u32_le(&mut over, HEADER64 + 12, nsyms + 1);
            let fs = MemFs::with(path, over);
            assert!(matches!(
                read(&fs, path).unwrap_err(),
                MapError::Bound(_, v) if v == MACHO_SYMTAB_CAP + NLIST64 as u64
            ));
        }

        // MACHO_STRTAB_CAP: exactly at passes the bound; one past → Bound.
        {
            let mut bytes = thin_no_symtab(MH_EXECUTE);
            set_u32_le(&mut bytes, 16, 1);
            set_u32_le(&mut bytes, 20, SYMTAB_CMD as u32);
            put_u32_le(&mut bytes, LC_SYMTAB);
            put_u32_le(&mut bytes, SYMTAB_CMD as u32);
            put_u32_le(&mut bytes, (HEADER64 + SYMTAB_CMD) as u32);
            put_u32_le(&mut bytes, 0); // nsyms
            put_u32_le(&mut bytes, (HEADER64 + SYMTAB_CMD) as u32);
            put_u32_le(&mut bytes, MACHO_STRTAB_CAP as u32);
            let fs = MemFs::with(path, bytes.clone());
            let err = read(&fs, path).unwrap_err();
            assert!(
                matches!(err, MapError::Truncated(_)),
                "exact strtab cap should read, got {err:?}"
            );

            let mut over = bytes;
            set_u32_le(&mut over, HEADER64 + 20, (MACHO_STRTAB_CAP + 1) as u32);
            let fs = MemFs::with(path, over);
            assert!(matches!(
                read(&fs, path).unwrap_err(),
                MapError::Bound(_, v) if v == MACHO_STRTAB_CAP + 1
            ));
        }

        // FAT_ARCH_CAP: exactly at reads; one past is Unsupported (not Bound).
        {
            let slice = thin_no_symtab(MH_EXECUTE);
            let slices: Vec<(u32, Vec<u8>)> = (0..FAT_ARCH_CAP)
                .map(|i| (0x0100_000c + i, slice.clone()))
                .collect();
            let bytes = wrap_fat(&slices, false);
            let fs = MemFs::with(path, bytes);
            let map = read(&fs, path).unwrap().expect("at fat arch cap");
            assert!(map.loose.is_empty());

            let mut bytes = Vec::new();
            put_u32_be(&mut bytes, FAT_MAGIC);
            put_u32_be(&mut bytes, FAT_ARCH_CAP + 1);
            let fs = MemFs::with(path, bytes);
            assert!(matches!(
                read(&fs, path).unwrap_err(),
                MapError::Unsupported(_)
            ));
        }
    }
}
