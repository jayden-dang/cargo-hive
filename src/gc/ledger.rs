//! Per-inode accounting of the bytes a gc run frees. Bytes a surviving hard link or clone
//! still holds are counted apart, never as freed.

use crate::platform::{EntryStat, Sharing};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// At most this many unmeasured paths are listed in a receipt; the rest are only counted.
pub const UNMEASURED_CAP: usize = 100;

/// Bytes freed and bytes deleted but still held elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bytes {
    pub freed: u64,
    pub still_shared: u64,
}

struct Seen {
    first_nlink: u64,
    links: u64,
    blocks: u64,
    /// Private size, kept only for a sharing-flagged, uncompressed file.
    private: Option<u64>,
    target: PathBuf,
}

pub(crate) struct Ledger {
    inodes: HashMap<(u64, u64), Seen>,
    unmeasured: Vec<(PathBuf, String)>,
    unmeasured_more: u64,
}

pub(crate) struct Tally {
    pub per_target: BTreeMap<PathBuf, Bytes>,
    pub total: Bytes,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
}

impl Ledger {
    pub(crate) fn new() -> Ledger {
        Ledger {
            inodes: HashMap::new(),
            unmeasured: Vec::new(),
            unmeasured_more: 0,
        }
    }

    /// One regular file, inside `target`, that this run deletes (or a dry run would).
    pub(crate) fn file(&mut self, target: &Path, stat: &EntryStat, sharing: Option<&Sharing>) {
        let seen = self
            .inodes
            .entry((stat.dev, stat.ino))
            .or_insert_with(|| Seen {
                first_nlink: stat.nlink,
                links: 0,
                blocks: stat.blocks,
                private: None,
                target: target.to_path_buf(),
            });
        seen.links += 1;
        if seen.target.as_path() != target {
            seen.target = target.to_path_buf();
        }
        if let Some(s) = sharing {
            if (s.may_share || s.shares_all) && !stat.compressed {
                seen.private = Some(s.private_bytes);
            }
        }
    }

    pub(crate) fn unmeasured(&mut self, path: PathBuf, why: String) {
        if self.unmeasured.len() < UNMEASURED_CAP {
            self.unmeasured.push((path, why));
        } else {
            self.unmeasured_more += 1;
        }
    }

    pub(crate) fn finish(self) -> Tally {
        let mut per_target: BTreeMap<PathBuf, Bytes> = BTreeMap::new();
        let mut total = Bytes::default();
        for seen in self.inodes.into_values() {
            let all = seen.blocks.saturating_mul(512);
            let freed = if seen.links >= seen.first_nlink {
                seen.private.map_or(all, |p| p.min(all))
            } else {
                0
            };
            let entry = per_target.entry(seen.target).or_default();
            entry.freed += freed;
            entry.still_shared += all - freed;
            total.freed += freed;
            total.still_shared += all - freed;
        }
        Tally {
            per_target,
            total,
            unmeasured: self.unmeasured,
            unmeasured_more: self.unmeasured_more,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::EntryKind;

    fn stat(ino: u64, nlink: u64, blocks: u64, compressed: bool) -> EntryStat {
        EntryStat {
            dev: 1,
            ino,
            nlink,
            blocks,
            kind: EntryKind::File,
            compressed,
        }
    }

    fn plain() -> Sharing {
        Sharing {
            private_bytes: 0,
            clone_id: 0,
            may_share: false,
            shares_all: false,
        }
    }

    fn clone_flagged(private_bytes: u64) -> Sharing {
        Sharing {
            private_bytes,
            clone_id: 7,
            may_share: true,
            shares_all: false,
        }
    }

    #[test]
    fn freed_bytes_follow_links_and_sharing() {
        let a = Path::new("/t/a");
        type Case = (&'static str, Vec<(EntryStat, Sharing)>, Bytes);
        let cases: Vec<Case> = vec![
            (
                "one link, deleted",
                vec![(stat(1, 1, 8, false), plain())],
                Bytes {
                    freed: 4096,
                    still_shared: 0,
                },
            ),
            (
                "a link survives elsewhere",
                vec![(stat(2, 2, 8, false), plain())],
                Bytes {
                    freed: 0,
                    still_shared: 4096,
                },
            ),
            (
                "both links deleted, real order (nlink drops in between)",
                vec![
                    (stat(3, 2, 8, false), plain()),
                    (stat(3, 1, 8, false), plain()),
                ],
                Bytes {
                    freed: 4096,
                    still_shared: 0,
                },
            ),
            (
                "both links deleted, dry-run order (nlink unchanged)",
                vec![
                    (stat(4, 2, 8, false), plain()),
                    (stat(4, 2, 8, false), plain()),
                ],
                Bytes {
                    freed: 4096,
                    still_shared: 0,
                },
            ),
            (
                "a clone keeps the shared extent",
                vec![(stat(5, 1, 8, false), clone_flagged(1024))],
                Bytes {
                    freed: 1024,
                    still_shared: 3072,
                },
            ),
            (
                "compressed: private size is not trusted",
                vec![(stat(6, 1, 8, true), clone_flagged(0))],
                Bytes {
                    freed: 4096,
                    still_shared: 0,
                },
            ),
        ];
        for (name, files, want) in cases {
            let mut ledger = Ledger::new();
            for (s, sh) in &files {
                ledger.file(a, s, Some(sh));
            }
            assert_eq!(ledger.finish().total, want, "{name}");
        }
    }

    #[test]
    fn a_freed_inode_is_attributed_to_the_target_of_its_last_link() {
        let (a, b) = (Path::new("/t/a"), Path::new("/t/b"));
        let mut ledger = Ledger::new();
        ledger.file(a, &stat(9, 2, 8, false), Some(&plain()));
        ledger.file(b, &stat(9, 1, 8, false), Some(&plain()));
        ledger.file(a, &stat(10, 1, 16, false), Some(&plain()));
        let tally = ledger.finish();
        assert_eq!(
            tally.per_target[b],
            Bytes {
                freed: 4096,
                still_shared: 0
            }
        );
        assert_eq!(
            tally.per_target[a],
            Bytes {
                freed: 8192,
                still_shared: 0
            }
        );
        let sum = tally
            .per_target
            .values()
            .fold(Bytes::default(), |s, x| Bytes {
                freed: s.freed + x.freed,
                still_shared: s.still_shared + x.still_shared,
            });
        assert_eq!(sum, tally.total);
    }

    #[test]
    fn unmeasured_paths_are_capped_and_the_rest_counted() {
        assert_eq!(UNMEASURED_CAP, 100);
        for (given, listed, more) in [
            (UNMEASURED_CAP, UNMEASURED_CAP, 0),
            (UNMEASURED_CAP + 1, UNMEASURED_CAP, 1),
        ] {
            let mut ledger = Ledger::new();
            for i in 0..given {
                ledger.unmeasured(PathBuf::from(format!("/t/{i}")), "unreadable".into());
            }
            let tally = ledger.finish();
            assert_eq!(
                (tally.unmeasured.len(), tally.unmeasured_more),
                (listed, more as u64),
                "{given}"
            );
        }
    }
}
