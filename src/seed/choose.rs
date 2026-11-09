//! Pure choice of the units one repo may clone for one toolchain text.
//! The text is a toolchain-table key, not a release name. A missing key returns
//! no units and the text's `release:` line, or an empty release when that line
//! is absent. This reads no disk and takes no lock.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::store::{Row, StoreFile};

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Units { rows: Vec<Row>, dropped: u64 },
    NoRow { release: String },
}

pub fn choose(store: &StoreFile, repo: &Path, version_text: &str) -> Choice {
    let Some(&value) = store.toolchains.get(version_text) else {
        return Choice::NoRow {
            release: release_line(version_text),
        };
    };

    let mut groups: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for row in store.units.iter().filter(|row| listed(row, value, repo)) {
        groups.entry(row.name_meta.as_str()).or_default().push(row);
    }
    let mut best: Vec<&Row> = groups
        .into_values()
        .filter_map(|group| {
            group
                .into_iter()
                .max_by_key(|row| (row.last_live_at, row.fingerprint_hash.as_str()))
        })
        .collect();

    let mut dropped = 0u64;
    loop {
        let removed = drop_outside(&mut best);
        if removed == 0 {
            break;
        }
        dropped += removed as u64;
    }

    Choice::Units {
        rows: best.into_iter().cloned().collect(),
        dropped,
    }
}

fn listed(row: &Row, value: u64, repo: &Path) -> bool {
    row.toolchain_value == value && row.repos.iter().any(|have| have == repo)
}

fn release_line(version_text: &str) -> String {
    version_text
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .unwrap_or("")
        .to_string()
}

fn drop_outside(best: &mut Vec<&Row>) -> usize {
    let mut next = Vec::with_capacity(best.len());
    let chosen: BTreeSet<&str> = best
        .iter()
        .map(|row| row.fingerprint_hash.as_str())
        .collect();
    for row in &*best {
        if row
            .registry_deps
            .iter()
            .all(|dep| chosen.contains(dep.as_str()))
        {
            next.push(*row);
        }
    }
    let removed = best.len() - next.len();
    *best = next;
    removed
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{choose, Choice};
    use crate::seed::store::{Row, StoreFile};

    fn row(name_meta: &str, fp: &str, value: u64, repo: &str, live: u64, deps: &[&str]) -> Row {
        Row {
            toolchain_value: value,
            name_meta: name_meta.into(),
            fingerprint_hash: fp.into(),
            kind: "lib".into(),
            rel_profile: "debug".into(),
            size: 1,
            filled_at: live,
            last_live_at: live,
            repos: vec![repo.into()],
            registry_deps: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn store(units: Vec<Row>) -> StoreFile {
        StoreFile {
            units,
            toolchains: [("V1".to_string(), 1)].into_iter().collect(),
        }
    }

    fn names(choice: Choice) -> Vec<String> {
        match choice {
            Choice::Units { rows, .. } => {
                let mut v: Vec<_> = rows.into_iter().map(|r| r.fingerprint_hash).collect();
                v.sort();
                v
            }
            Choice::NoRow { .. } => panic!("no row"),
        }
    }

    #[test]
    fn only_this_toolchain_and_this_repo() {
        let s = store(vec![
            row("a-1", "fa", 1, "/r", 5, &[]),
            row("b-1", "fb", 2, "/r", 5, &[]),
            row("c-1", "fc", 1, "/other", 5, &[]),
        ]);
        assert_eq!(names(choose(&s, Path::new("/r"), "V1")), ["fa"]);
    }

    #[test]
    fn no_row_for_the_text_clones_nothing_and_substitutes_nothing() {
        let s = store(vec![row("a-1", "fa", 1, "/r", 5, &[])]);
        let text = "rustc 9\nrelease: 9.9.9\n";
        assert!(
            matches!(choose(&s, Path::new("/r"), text), Choice::NoRow { release } if release == "9.9.9")
        );
    }

    #[test]
    fn newest_per_name_meta_and_ties_take_the_larger_fingerprint() {
        let s = store(vec![
            row("a-1", "f1", 1, "/r", 5, &[]),
            row("a-1", "f2", 1, "/r", 9, &[]),
            row("b-1", "f3", 1, "/r", 7, &[]),
            row("b-1", "f4", 1, "/r", 7, &[]),
        ]);
        assert_eq!(names(choose(&s, Path::new("/r"), "V1")), ["f2", "f4"]);
    }

    #[test]
    fn a_dependent_of_a_fingerprint_not_chosen_is_dropped_and_the_drop_repeats() {
        let s = store(vec![
            row("a-1", "f1", 1, "/r", 5, &[]),
            row("a-1", "f2", 1, "/r", 9, &[]),
            row("b-1", "fb", 1, "/r", 5, &["f1"]),
            row("c-1", "fc", 1, "/r", 5, &["fb"]),
            row("d-1", "fd", 1, "/r", 5, &["f2"]),
        ]);
        match choose(&s, Path::new("/r"), "V1") {
            Choice::Units { rows, dropped } => {
                let mut got: Vec<_> = rows.iter().map(|r| r.fingerprint_hash.as_str()).collect();
                got.sort();
                assert_eq!(got, ["f2", "fd"]);
                assert_eq!(dropped, 2);
            }
            Choice::NoRow { .. } => panic!("no row"),
        }
    }
}
