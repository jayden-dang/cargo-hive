//! Pure choice of the units one repo may clone for one toolchain text.
//! The text is a toolchain-table key, not a release name. A missing key returns
//! no units and the text's `release:` line, or an empty release when that line
//! is absent. Units come from one source target's record only, at the variants
//! that target holds, so every unit placed comes from one build (SEED-3.9).
//! This reads no disk and takes no lock.

use std::collections::BTreeSet;
use std::path::Path;

use super::store::{Row, SourceRecord, StoreFile};

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Units {
        rows: Vec<Row>,
        dropped: u64,
    },
    NoRow {
        release: String,
    },
    /// No source target of this repo has a record for this toolchain (SEED-3.10).
    NoSource,
}

/// The source targets of `repo`, best first: those `lock_matches` accepts, then the one
/// whose fingerprint files are newest, then the one holding more units, then by path.
pub fn source_order(
    store: &StoreFile,
    repo: &Path,
    lock_matches: &dyn Fn(&SourceRecord) -> bool,
) -> Vec<std::path::PathBuf> {
    let mut ranked: Vec<(bool, u64, usize, &std::path::PathBuf)> = store
        .sources
        .iter()
        .filter(|(_, record)| record.repo == repo)
        .map(|(target, record)| {
            (
                lock_matches(record),
                record.newest_at,
                record.units.len(),
                target,
            )
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(b.3))
    });
    ranked
        .into_iter()
        .map(|(_, _, _, target)| target.clone())
        .collect()
}

pub fn choose(
    store: &StoreFile,
    repo: &Path,
    version_text: &str,
    order: &[std::path::PathBuf],
) -> Choice {
    let Some(&value) = store.toolchains.get(version_text) else {
        return Choice::NoRow {
            release: release_line(version_text),
        };
    };
    let mut recorded = false;
    for target in order {
        let Some(record) = store.sources.get(target) else {
            continue;
        };
        if record.repo != repo {
            continue;
        }
        recorded = true;
        let wanted: BTreeSet<(&str, &str, u32)> = record
            .units
            .iter()
            .filter(|unit| unit.toolchain_value == value)
            .map(|unit| {
                (
                    unit.name_meta.as_str(),
                    unit.fingerprint_hash.as_str(),
                    unit.variant,
                )
            })
            .collect();
        let mut best: Vec<&Row> = store
            .units
            .iter()
            .filter(|row| {
                listed(row, value, repo)
                    && wanted.contains(&(
                        row.name_meta.as_str(),
                        row.fingerprint_hash.as_str(),
                        row.variant,
                    ))
            })
            .collect();
        if best.is_empty() {
            continue;
        }
        let mut dropped = 0u64;
        loop {
            let removed = drop_outside(&mut best);
            if removed == 0 {
                break;
            }
            dropped += removed as u64;
        }
        return Choice::Units {
            rows: best.into_iter().cloned().collect(),
            dropped,
        };
    }
    if recorded {
        // A source was recorded, but none of its units is in the store any more.
        return Choice::Units {
            rows: Vec::new(),
            dropped: 0,
        };
    }
    Choice::NoSource
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
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{choose, source_order, Choice};
    use crate::seed::store::{Row, SourceRecord, SourceUnitRef, StoreFile};

    fn row(name_meta: &str, fp: &str, value: u64, repo: &str, variant: u32, deps: &[&str]) -> Row {
        Row {
            toolchain_value: value,
            name_meta: name_meta.into(),
            fingerprint_hash: fp.into(),
            kind: "lib".into(),
            rel_profile: "debug".into(),
            size: 1,
            filled_at: 5,
            last_live_at: 5,
            repos: vec![repo.into()],
            registry_deps: deps.iter().map(|d| d.to_string()).collect(),
            variant,
        }
    }

    fn record(repo: &str, newest_at: u64, units: &[(&str, &str, u64, u32)]) -> SourceRecord {
        SourceRecord {
            repo: repo.into(),
            checkout: format!("{repo}/checkout").into(),
            newest_at,
            units: units
                .iter()
                .map(|(name_meta, fp, value, variant)| SourceUnitRef {
                    toolchain_value: *value,
                    name_meta: (*name_meta).into(),
                    fingerprint_hash: (*fp).into(),
                    variant: *variant,
                })
                .collect(),
        }
    }

    fn store(units: Vec<Row>, sources: Vec<(&str, SourceRecord)>) -> StoreFile {
        StoreFile {
            units,
            toolchains: [("V1".to_string(), 1)].into_iter().collect(),
            sources: sources
                .into_iter()
                .map(|(target, record)| (PathBuf::from(target), record))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn chosen(choice: Choice) -> Vec<(String, u32)> {
        match choice {
            Choice::Units { rows, .. } => {
                let mut v: Vec<_> = rows
                    .into_iter()
                    .map(|r| (r.fingerprint_hash, r.variant))
                    .collect();
                v.sort();
                v
            }
            other => panic!("{other:?}"),
        }
    }

    fn order(targets: &[&str]) -> Vec<PathBuf> {
        targets.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn only_this_toolchain_and_this_repo() {
        let s = store(
            vec![
                row("a-1", "fa", 1, "/r", 0, &[]),
                row("b-1", "fb", 2, "/r", 0, &[]),
                row("c-1", "fc", 1, "/other", 0, &[]),
            ],
            vec![(
                "/t",
                record(
                    "/r",
                    5,
                    &[
                        ("a-1", "fa", 1, 0),
                        ("b-1", "fb", 2, 0),
                        ("c-1", "fc", 1, 0),
                    ],
                ),
            )],
        );
        assert_eq!(
            chosen(choose(&s, Path::new("/r"), "V1", &order(&["/t"]))),
            [("fa".to_string(), 0)]
        );
    }

    #[test]
    fn no_row_for_the_text_clones_nothing_and_substitutes_nothing() {
        let s = store(vec![row("a-1", "fa", 1, "/r", 0, &[])], vec![]);
        let text = "rustc 9\nrelease: 9.9.9\n";
        assert!(
            matches!(choose(&s, Path::new("/r"), text, &[]), Choice::NoRow { release } if release == "9.9.9")
        );
    }

    #[test]
    fn units_come_from_the_first_source_only_at_its_variants() {
        // Two builds of one key with different bytes: variant 0 from /t1, 1 from /t2.
        let s = store(
            vec![
                row("dep-1", "fd", 1, "/r", 0, &[]),
                row("dep-1", "fd", 1, "/r", 1, &[]),
                row("top-1", "ft", 1, "/r", 0, &["fd"]),
                row("only-1-1", "fo", 1, "/r", 0, &[]),
            ],
            vec![
                (
                    "/t1",
                    record("/r", 9, &[("dep-1", "fd", 1, 0), ("only-1-1", "fo", 1, 0)]),
                ),
                (
                    "/t2",
                    record("/r", 5, &[("dep-1", "fd", 1, 1), ("top-1", "ft", 1, 0)]),
                ),
            ],
        );
        assert_eq!(
            chosen(choose(&s, Path::new("/r"), "V1", &order(&["/t2", "/t1"]))),
            [("fd".to_string(), 1), ("ft".to_string(), 0)]
        );
        assert_eq!(
            chosen(choose(&s, Path::new("/r"), "V1", &order(&["/t1", "/t2"]))),
            [("fd".to_string(), 0), ("fo".to_string(), 0)]
        );
    }

    #[test]
    fn a_dependent_of_a_fingerprint_not_held_is_dropped_and_the_drop_repeats() {
        let s = store(
            vec![
                row("b-1", "fb", 1, "/r", 0, &["f1"]),
                row("c-1", "fc", 1, "/r", 0, &["fb"]),
                row("d-1", "fd", 1, "/r", 0, &[]),
            ],
            vec![(
                "/t",
                record(
                    "/r",
                    5,
                    &[
                        ("b-1", "fb", 1, 0),
                        ("c-1", "fc", 1, 0),
                        ("d-1", "fd", 1, 0),
                    ],
                ),
            )],
        );
        match choose(&s, Path::new("/r"), "V1", &order(&["/t"])) {
            Choice::Units { rows, dropped } => {
                let got: Vec<_> = rows.iter().map(|r| r.fingerprint_hash.as_str()).collect();
                assert_eq!(got, ["fd"]);
                assert_eq!(dropped, 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_record_for_the_repo_is_no_source() {
        let s = store(
            vec![row("a-1", "fa", 1, "/r", 0, &[])],
            vec![("/t", record("/other", 5, &[("a-1", "fa", 1, 0)]))],
        );
        assert_eq!(
            choose(&s, Path::new("/r"), "V1", &order(&["/t"])),
            Choice::NoSource
        );
    }

    #[test]
    fn a_matching_lock_file_ranks_first_then_the_newest() {
        let s = store(
            vec![],
            vec![
                ("/old-match", record("/r", 1, &[])),
                ("/new", record("/r", 9, &[])),
                ("/mid", record("/r", 5, &[])),
                ("/foreign", record("/other", 99, &[])),
            ],
        );
        let got = source_order(&s, Path::new("/r"), &|rec| rec.newest_at == 1);
        assert_eq!(got, order(&["/old-match", "/new", "/mid"]));
    }
}
