//! Reading a git object store directly, to rule out a repository without starting git (CORE-3.9).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

const IDX_MAGIC: [u8; 4] = [0xff, b't', b'O', b'c'];
const IDX_VERSION: u32 = 2;
const IDX_FANOUT_END: u64 = 8 + 256 * 4;

/// True only when `git_dir` is a plain git directory whose object store can be read, has no
/// alternates, and holds none of `roots`, neither as a loose object nor in a version-2 pack
/// index. Every doubt answers false, so the caller asks git as before.
pub fn holds_none(git_dir: &Path, roots: &BTreeSet<String>) -> bool {
    let Some(ids) = roots
        .iter()
        .map(|root| parse_sha1(root))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if ids.is_empty() || !is_plain_sha1_repo(git_dir) {
        return false;
    }
    let objects = git_dir.join("objects");
    if !absent(&objects.join("info/alternates")) {
        return false;
    }
    for id in &ids {
        let hex = to_hex(id);
        if !absent(&objects.join(&hex[..2]).join(&hex[2..])) {
            return false;
        }
    }
    let Ok(entries) = std::fs::read_dir(objects.join("pack")) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".pack") {
            if absent(&objects.join("pack").join(format!("{stem}.idx"))) {
                return false;
            }
        } else if name.ends_with(".idx") && idx_holds_any(&entry.path(), &ids) != Some(false) {
            return false;
        }
    }
    true
}

/// A `.git` directory with a HEAD, an objects directory and a config that sets no object format.
fn is_plain_sha1_repo(git_dir: &Path) -> bool {
    let is_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    if !is_dir(git_dir) || !is_dir(&git_dir.join("objects")) || !git_dir.join("HEAD").is_file() {
        return false;
    }
    match std::fs::read_to_string(git_dir.join("config")) {
        Ok(config) => !config.to_ascii_lowercase().contains("objectformat"),
        Err(_) => false,
    }
}

/// True only for a path that provably does not exist.
fn absent(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == ErrorKind::NotFound)
}

/// Whether a version-2 pack index lists any of `ids`; None when it cannot be read as one.
fn idx_holds_any(path: &Path, ids: &[[u8; 20]]) -> Option<bool> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = [0u8; IDX_FANOUT_END as usize];
    file.read_exact(&mut head).ok()?;
    if head[..4] != IDX_MAGIC || u32::from_be_bytes(head[4..8].try_into().ok()?) != IDX_VERSION {
        return None;
    }
    let fanout = |i: usize| -> u64 {
        let at = 8 + i * 4;
        u64::from(u32::from_be_bytes([
            head[at],
            head[at + 1],
            head[at + 2],
            head[at + 3],
        ]))
    };
    let total = fanout(255);
    if IDX_FANOUT_END + total * 20 > len {
        return None;
    }
    for id in ids {
        let first = usize::from(id[0]);
        let (mut lo, mut hi) = (
            if first == 0 { 0 } else { fanout(first - 1) },
            fanout(first),
        );
        if lo > hi || hi > total {
            return None;
        }
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let mut name = [0u8; 20];
            file.seek(SeekFrom::Start(IDX_FANOUT_END + mid * 20)).ok()?;
            file.read_exact(&mut name).ok()?;
            match name.cmp(id) {
                std::cmp::Ordering::Equal => return Some(true),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
    }
    Some(false)
}

fn parse_sha1(hex: &str) -> Option<[u8; 20]> {
    if hex.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn to_hex(id: &[u8; 20]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ROOT: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A temp dir removed when the test ends, so no fake repository is left where hive's
    /// default secondary roots would find it.
    struct Store(PathBuf);

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    impl std::ops::Deref for Store {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    /// A hand-built `.git` with one pack index naming `listed`, under a fresh temp dir.
    fn store(name: &str, config: &str, listed: &[[u8; 20]], version: u32) -> Store {
        let dir = std::env::temp_dir().join(format!("hive-odb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let git = dir.join(".git");
        std::fs::create_dir_all(git.join("objects/pack")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("config"), config).unwrap();
        let mut sorted = listed.to_vec();
        sorted.sort();
        let mut idx = IDX_MAGIC.to_vec();
        idx.extend(version.to_be_bytes());
        for b in 0..256usize {
            let n = sorted.iter().filter(|id| usize::from(id[0]) <= b).count() as u32;
            idx.extend(n.to_be_bytes());
        }
        for id in &sorted {
            idx.extend(id);
        }
        std::fs::write(git.join("objects/pack/pack-a.idx"), idx).unwrap();
        std::fs::write(git.join("objects/pack/pack-a.pack"), b"PACK").unwrap();
        Store(git)
    }

    fn roots() -> BTreeSet<String> {
        [ROOT.to_string()].into()
    }

    fn other() -> [u8; 20] {
        parse_sha1("fedcba9876543210fedcba9876543210fedcba98").unwrap()
    }

    #[test]
    fn a_store_whose_index_lacks_every_root_holds_none() {
        let git = store("lacks", "[core]\n", &[other()], 2);
        assert!(holds_none(&git, &roots()));
    }

    #[test]
    fn a_root_listed_in_a_pack_index_is_not_ruled_out() {
        let git = store(
            "lists",
            "[core]\n",
            &[other(), parse_sha1(ROOT).unwrap()],
            2,
        );
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn a_loose_root_object_is_not_ruled_out() {
        let git = store("loose", "[core]\n", &[other()], 2);
        let dir = git.join("objects").join(&ROOT[..2]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(&ROOT[2..]), b"x").unwrap();
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn a_store_with_alternates_is_left_to_git() {
        let git = store("alt", "[core]\n", &[other()], 2);
        std::fs::create_dir_all(git.join("objects/info")).unwrap();
        std::fs::write(git.join("objects/info/alternates"), "/elsewhere/objects\n").unwrap();
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn a_repository_with_another_object_format_is_left_to_git() {
        let git = store(
            "sha256",
            "[extensions]\n\tobjectFormat = sha256\n",
            &[other()],
            2,
        );
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn a_pack_without_its_index_is_left_to_git() {
        let git = store("noidx", "[core]\n", &[other()], 2);
        std::fs::write(git.join("objects/pack/pack-b.pack"), b"PACK").unwrap();
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn an_index_of_another_version_is_left_to_git() {
        let git = store("v3", "[core]\n", &[other()], 3);
        assert!(!holds_none(&git, &roots()));
    }

    #[test]
    fn a_truncated_index_is_left_to_git() {
        let git = store("short", "[core]\n", &[other()], 2);
        let idx = git.join("objects/pack/pack-a.idx");
        let bytes = std::fs::read(&idx).unwrap();
        std::fs::write(&idx, &bytes[..bytes.len() - 5]).unwrap();
        assert!(!holds_none(&git, &roots()));
    }
}
