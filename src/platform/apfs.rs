//! macOS `Fs`: APFS clones and sharing attributes through `libc`.

use super::{Fs, Sharing, TempDir};
use std::ffi::{CString, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// `<sys/attr.h>` extended flags that `libc` does not define.
const EF_MAY_SHARE_BLOCKS: u64 = 0x1;
const EF_SHARES_ALL_BLOCKS: u64 = 0x40;

pub struct Apfs {
    holder: PathBuf,
}

impl Apfs {
    pub fn new(holder: PathBuf) -> Apfs {
        Apfs { holder }
    }

    /// The executable that implements the hidden `__hold-lock` command.
    pub fn holder(&self) -> &Path {
        &self.holder
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

impl Fs for Apfs {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
        let mut template = c_path(&parent.join(format!("{prefix}XXXXXXXX")))?.into_bytes_with_nul();
        // SAFETY: `template` is a writable NUL-terminated buffer that mkdtemp fills in place.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        template.pop();
        Ok(TempDir::adopt(PathBuf::from(OsString::from_vec(template))))
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        fs::hard_link(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        fs::File::options().write(true).open(path)?.set_modified(t)
    }

    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        fs::metadata(path)?.modified()
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        Ok(fs::metadata(path)?.ino())
    }

    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        let (s, d) = (c_path(src)?, c_path(dst)?);
        // SAFETY: both are valid NUL-terminated paths for the duration of the call.
        if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn sharing(&self, path: &Path) -> io::Result<Sharing> {
        let p = c_path(path)?;
        let mut list = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: 0,
            dirattr: 0,
            fileattr: 0,
            // With FSOPT_ATTR_CMN_EXTENDED this group carries the common-extended attributes.
            forkattr: libc::ATTR_CMNEXT_PRIVATESIZE
                | libc::ATTR_CMNEXT_CLONEID
                | libc::ATTR_CMNEXT_EXT_FLAGS,
        };
        let mut buf = [0u8; 64];
        // SAFETY: `list` and `buf` outlive the call; the kernel writes at most `buf.len()` bytes.
        let rc = unsafe {
            libc::getattrlist(
                p.as_ptr(),
                (&mut list as *mut libc::attrlist).cast(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::FSOPT_ATTR_CMN_EXTENDED,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // A u32 total length, then the attributes in bit order: PRIVATESIZE (off_t),
        // CLONEID (u64), EXT_FLAGS (u64).
        let word =
            |range: std::ops::Range<usize>| -> [u8; 8] { buf[range].try_into().expect("8 bytes") };
        let len = u32::from_ne_bytes(buf[0..4].try_into().expect("4 bytes"));
        if len < 28 {
            return Err(io::Error::other(format!(
                "getattrlist returned {len} bytes; expected 28"
            )));
        }
        let private = i64::from_ne_bytes(word(4..12));
        let flags = u64::from_ne_bytes(word(20..28));
        Ok(Sharing {
            private_bytes: u64::try_from(private).unwrap_or(0),
            clone_id: u64::from_ne_bytes(word(12..20)),
            may_share: flags & EF_MAY_SHARE_BLOCKS != 0,
            shares_all: flags & EF_SHARES_ALL_BLOCKS != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apfs() -> Apfs {
        Apfs::new(PathBuf::from("/nonexistent/cargo-hive"))
    }

    fn tmp(label: &str) -> TempDir {
        apfs()
            .temp_dir(Path::new("/private/tmp"), &format!("hive-apfs-{label}-"))
            .unwrap()
    }

    #[test]
    fn a_temp_dir_is_created_with_its_prefix_and_removed_on_drop() {
        let t = tmp("drop");
        let path = t.path().to_path_buf();
        assert!(path.is_dir());
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("hive-apfs-drop-"));
        apfs().write_file(&path.join("f"), b"x").unwrap();
        drop(t);
        assert!(!path.exists());
    }

    #[test]
    fn a_clone_shares_every_block_until_it_is_rewritten() {
        let t = tmp("clone");
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        apfs().write_file(&src, &vec![7u8; 1 << 20]).unwrap();
        let alone = apfs().sharing(&src).unwrap();
        assert_eq!(alone.private_bytes, 1 << 20);
        assert!(!alone.shares_all && !alone.may_share);
        apfs().clone_file(&src, &dst).unwrap();
        let (s, d) = (apfs().sharing(&src).unwrap(), apfs().sharing(&dst).unwrap());
        assert_eq!((s.private_bytes, d.private_bytes), (0, 0));
        assert!(d.shares_all && d.may_share);
        assert_eq!(s.clone_id, d.clone_id);
        apfs().write_file(&dst, &vec![9u8; 1 << 20]).unwrap();
        let rewritten = apfs().sharing(&dst).unwrap();
        assert_ne!(rewritten.clone_id, s.clone_id);
        assert!(!rewritten.shares_all);
        assert_eq!(apfs().read(&dst).unwrap()[0], 9);
    }

    #[test]
    fn hardlinks_share_an_inode_and_mtimes_can_be_set() {
        let t = tmp("links");
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        apfs().write_file(&a, b"same").unwrap();
        apfs().hardlink(&a, &b).unwrap();
        assert_eq!(apfs().inode(&a).unwrap(), apfs().inode(&b).unwrap());
        let fixed = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        apfs().set_mtime(&a, fixed).unwrap();
        assert_eq!(apfs().mtime(&b).unwrap(), fixed);
    }
}
