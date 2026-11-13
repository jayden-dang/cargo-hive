//! Allocated bytes of a tree: Σ st_blocks × 512 over regular files, each (dev, inode)
//! counted once, never following symlinks, with an explicit depth limit.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Deeper than any Cargo target layout (`OUT_DIR` trees included); hitting it is reported, not skipped.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug)]
pub enum SizeError {
    DepthLimit(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for SizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SizeError::DepthLimit(p) => {
                write!(f, "{} is deeper than the size walk's limit", p.display())
            }
            SizeError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

pub fn allocated_bytes(root: &Path, max_depth: usize) -> Result<u64, SizeError> {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| SizeError::Io(dir.clone(), e))?;
        for entry in entries {
            let path = entry.map_err(|e| SizeError::Io(dir.clone(), e))?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| SizeError::Io(path.clone(), e))?;
            if meta.is_dir() {
                if depth + 1 > max_depth {
                    return Err(SizeError::DepthLimit(path));
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
                total += meta.blocks() * 512;
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("hive-size-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn each_inode_is_counted_once_and_symlinks_are_not_followed() {
        let root = scratch("inodes");
        let tree = root.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("f"), vec![1u8; 100_000]).unwrap();
        fs::hard_link(tree.join("f"), tree.join("sub/g")).unwrap();
        fs::write(tree.join("sub/h"), vec![2u8; 10_000]).unwrap();
        let outside = root.join("big");
        fs::write(&outside, vec![3u8; 1 << 20]).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();
        let blocks = |rel: &str| fs::metadata(tree.join(rel)).unwrap().blocks() * 512;
        assert_eq!(
            allocated_bytes(&tree, MAX_DEPTH).unwrap(),
            blocks("f") + blocks("sub/h")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_depth_limit_is_explicit() {
        let root = scratch("depth");
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        assert!(
            matches!(allocated_bytes(&root, 2), Err(SizeError::DepthLimit(p)) if p.ends_with("a/b/c"))
        );
        assert!(allocated_bytes(&root, 3).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
