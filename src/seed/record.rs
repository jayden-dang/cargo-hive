//! The seed record in `seeds.json`. One target keeps one record. A write drops
//! records whose target is gone, then the oldest, until the file fits its cap.
//! A file that cannot be read is replaced, and the note says so.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::{State, StateError};

pub const SEEDS_FILE: &str = "seeds.json";
pub const MAX_SEEDS_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeededUnit {
    pub rel_profile: PathBuf,
    pub dir_name: String,
    pub stamp: Option<(PathBuf, u64)>,
    pub has_build: bool,
    pub deps_files: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedRecord {
    pub identity: (u64, u64),
    pub written_at: u64,
    pub units: Vec<SeededUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SeedsFile {
    pub targets: BTreeMap<PathBuf, SeedRecord>,
}

pub fn write(
    state: &State,
    target: &Path,
    record: SeedRecord,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<String>, StateError> {
    write_capped(state, target, record, cancel, MAX_SEEDS_BYTES)
}

pub fn write_capped(
    state: &State,
    target: &Path,
    record: SeedRecord,
    cancel: &(dyn Fn() -> bool + Sync),
    cap: u64,
) -> Result<Option<String>, StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let (mut file, note) = match tx.read_bounded(SEEDS_FILE, cap) {
            Ok(file) => (file, None),
            Err(err) => (SeedsFile::default(), Some(replaced(&err))),
        };
        file.targets.insert(target.to_path_buf(), record);
        file.targets.retain(|path, _| target_present(path));
        drop_oldest_until(&mut file, cap, target);
        tx.write_bounded(SEEDS_FILE, &file, cap)?;
        Ok(note)
    })
}

pub fn read(state: &State) -> Result<SeedsFile, StateError> {
    state.read_bounded(SEEDS_FILE, MAX_SEEDS_BYTES)
}

fn replaced(err: &StateError) -> String {
    format!("{err}; this seed replaced the record file")
}

fn target_present(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn drop_oldest_until(file: &mut SeedsFile, cap: u64, inserted: &Path) {
    while encoded_len(file).is_some_and(|len| len > cap) {
        let Some(oldest) = oldest_other(file, inserted) else {
            break;
        };
        file.targets.remove(&oldest);
    }
}

fn encoded_len(file: &SeedsFile) -> Option<u64> {
    let mut bytes = serde_json::to_vec_pretty(file).ok()?;
    bytes.push(b'\n');
    Some(bytes.len() as u64)
}

fn oldest_other(file: &SeedsFile, inserted: &Path) -> Option<PathBuf> {
    let max_at = file
        .targets
        .values()
        .map(|record| record.written_at)
        .max()?;
    file.targets
        .iter()
        .filter(|(path, record)| {
            record.written_at < max_at
                || (record.written_at == max_at && path.as_path() != inserted)
        })
        .min_by(|(left_path, left), (right_path, right)| {
            left.written_at
                .cmp(&right.written_at)
                .then_with(|| left_path.cmp(right_path))
        })
        .map(|(path, _)| path.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    use crate::state::State;

    fn scratch(label: &str) -> (State, PathBuf) {
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-seedrec-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = State::open_at(root.join("state")).unwrap();
        (state, root)
    }

    fn target(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn record(identity: (u64, u64), written_at: u64, pad: usize) -> SeedRecord {
        SeedRecord {
            identity,
            written_at,
            units: vec![SeededUnit {
                rel_profile: PathBuf::from("debug"),
                dir_name: "n".repeat(pad),
                stamp: Some((
                    PathBuf::from(format!("debug/build/{}/invoked.timestamp", "n".repeat(pad))),
                    5,
                )),
                has_build: true,
                deps_files: 1,
            }],
        }
    }

    fn file_of(entries: &[(&Path, SeedRecord)]) -> SeedsFile {
        let mut targets = BTreeMap::new();
        for (path, stored) in entries {
            targets.insert((*path).to_path_buf(), stored.clone());
        }
        SeedsFile { targets }
    }

    fn encoded(file: &SeedsFile) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(file).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn fitted_cap(old: &SeedsFile, newest: &SeedsFile, both: &SeedsFile) -> u64 {
        let cap = encoded(newest).len() as u64;
        assert!(encoded(old).len() as u64 <= cap);
        assert!(encoded(both).len() as u64 > cap);
        assert!(encoded(newest).len() as u64 <= cap);
        cap
    }

    #[test]
    fn a_write_drops_records_whose_target_is_gone() {
        let (state, root) = scratch("gone");
        let gone = target(&root, "gone");
        let kept = target(&root, "kept");
        assert!(
            super::write(&state, &gone, record((1, 9), 100, 4), &|| false)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(&gone).unwrap();
        assert!(super::write(&state, &kept, record((2, 8), 1, 4), &|| false)
            .unwrap()
            .is_none());
        let file = read(&state).unwrap();
        assert!(!file.targets.contains_key(&gone), "{file:?}");
        let kept_record = file.targets.get(&kept).expect("the live target stays");
        assert_eq!(kept_record.identity, (2, 8));
        assert_eq!(kept_record.written_at, 1);
        assert_eq!(kept_record.units.len(), 1);
    }

    #[test]
    fn a_write_over_the_cap_drops_the_oldest_records_until_it_fits() {
        let (state, root) = scratch("cap");
        let older = target(&root, "older");
        let middle = target(&root, "middle");
        let newest = target(&root, "newest");
        let first = record((1, 1), 1, 8);
        let second = record((2, 2), 2, 8);
        let mut pad = 8usize;
        let (third, cap) = loop {
            assert!(pad < 20_000, "no pad made the newest the one that fits");
            let third = record((3, 3), 3, pad);
            let old = file_of(&[(&older, first.clone()), (&middle, second.clone())]);
            let alone = file_of(&[(&newest, third.clone())]);
            let with_middle = file_of(&[(&middle, second.clone()), (&newest, third.clone())]);
            if encoded(&old).len() as u64 <= encoded(&alone).len() as u64
                && encoded(&with_middle).len() as u64 > encoded(&alone).len() as u64
            {
                break (third, fitted_cap(&old, &alone, &with_middle));
            }
            pad += 32;
        };
        assert!(write_capped(&state, &older, first, &|| false, cap)
            .unwrap()
            .is_none());
        assert!(write_capped(&state, &middle, second, &|| false, cap)
            .unwrap()
            .is_none());
        let before = read(&state).unwrap();
        assert!(before.targets.contains_key(&older), "{before:?}");
        assert!(before.targets.contains_key(&middle), "{before:?}");
        assert!(write_capped(&state, &newest, third.clone(), &|| false, cap)
            .unwrap()
            .is_none());
        let file = read(&state).unwrap();
        assert!(!file.targets.contains_key(&older), "{file:?}");
        assert!(!file.targets.contains_key(&middle), "{file:?}");
        assert_eq!(
            file.targets.get(&newest).map(|row| &row.units),
            Some(&third.units)
        );
        let on_disk = fs::metadata(state.root().join(SEEDS_FILE)).unwrap().len();
        assert!(on_disk <= cap, "on disk {on_disk} cap {cap}");
    }

    #[test]
    fn the_newest_record_survives_the_cap() {
        let (state, root) = scratch("newest");
        let older = target(&root, "older");
        let middle = target(&root, "middle");
        let newest = target(&root, "newest");
        let first = record((4, 1), 10, 4);
        let second = record((4, 2), 20, 4);
        let mut pad = 4usize;
        let (third, cap) = loop {
            assert!(pad < 20_000, "no pad left the newest under the cap");
            let third = record((4, 3), 30, pad);
            let old = file_of(&[(&older, first.clone()), (&middle, second.clone())]);
            let alone = file_of(&[(&newest, third.clone())]);
            let all = file_of(&[
                (&older, first.clone()),
                (&middle, second.clone()),
                (&newest, third.clone()),
            ]);
            if encoded(&old).len() <= encoded(&alone).len()
                && encoded(&all).len() > encoded(&alone).len()
            {
                break (third, encoded(&alone).len() as u64);
            }
            pad += 32;
        };
        write_capped(&state, &older, first, &|| false, cap).unwrap();
        write_capped(&state, &middle, second, &|| false, cap).unwrap();
        write_capped(&state, &newest, third.clone(), &|| false, cap).unwrap();
        let file = read(&state).unwrap();
        let kept = file
            .targets
            .get(&newest)
            .expect("the newest record is not the one dropped");
        assert_eq!(kept.written_at, 30);
        assert_eq!(kept.units, third.units);
        assert!(file.targets.len() < 3, "{file:?}");
        let on_disk = fs::metadata(state.root().join(SEEDS_FILE)).unwrap().len();
        assert!(on_disk <= cap, "on disk {on_disk} cap {cap}");
    }

    #[test]
    fn an_unreadable_seeds_file_is_replaced_and_the_note_names_it() {
        let (state, root) = scratch("unreadable");
        let kept = target(&root, "kept");
        fs::write(state.root().join(SEEDS_FILE), b"{not json").unwrap();
        let note = super::write(&state, &kept, record((7, 7), 7, 3), &|| false)
            .unwrap()
            .expect("an unreadable file is a note");
        assert!(note.contains("seeds.json"), "{note}");
        let file = read(&state).unwrap();
        assert_eq!(file.targets.len(), 1, "{file:?}");
        assert!(file.targets.contains_key(&kept), "{file:?}");
    }
}
