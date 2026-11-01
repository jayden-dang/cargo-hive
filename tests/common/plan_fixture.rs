//! A registered main checkout and linked worktrees under `/private/tmp`, each with a target
//! whose earlier builds are older than the window. `Planned` runs `gc --plan` and reads the
//! receipt. Directory listings stop at a fixed bound.

use super::cargo_fixture::{backdate_all, set_mtime};
use super::{git, init_repo, register, Hive, Scratch};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A registered main checkout and linked worktrees under `/private/tmp` (a default
/// secondary root, so hive classes the worktrees Secondary and the main Protected; never
/// set `secondary_roots` in tests that rely on this). Each checkout has a built target whose
/// earlier builds are older than the 7-day window and were superseded by rebuilds under new
/// package versions, so every target holds dead units.
pub struct Planned {
    pub hive: Hive,
    pub scratch: Scratch,
    pub main: PathBuf,
    pub worktree: PathBuf,
    pub more_worktrees: Vec<PathBuf>,
}

impl Planned {
    pub fn new(label: &str) -> Planned {
        Planned::build(label, 1, 1)
    }

    /// The first worktree is superseded `n` times, so it holds `n` dead units.
    pub fn with_dead_units(label: &str, n: usize) -> Planned {
        Planned::build(label, n, 1)
    }

    /// `n` linked worktrees, all Secondary.
    pub fn with_worktrees(label: &str, n: usize) -> Planned {
        Planned::build(label, 1, n)
    }

    fn build(label: &str, supersessions: usize, worktrees: usize) -> Planned {
        let scratch = Scratch::new(label);
        let hive = Hive::ready();
        let main = init_repo(&scratch.path.join("main"), "");
        register(&hive, &main);
        let mut made = Vec::new();
        for i in 0..worktrees {
            let wt = scratch.path.join(format!("wt{i}"));
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("wt{i}"),
                    wt.to_str().unwrap(),
                ],
            );
            made.push(wt);
        }
        let p = Planned {
            hive,
            scratch,
            main,
            worktree: made[0].clone(),
            more_worktrees: made[1..].to_vec(),
        };
        p.supersede(&p.main, 1);
        p.supersede(&p.worktree, supersessions);
        for wt in &p.more_worktrees {
            p.supersede(wt, 1);
        }
        p
    }

    /// Builds `dir`, then `times` more versions, backdating every earlier build past the window.
    fn supersede(&self, dir: &Path, times: usize) {
        self.hive.build_crate(dir, None);
        let manifest = dir.join("Cargo.toml");
        for n in 1..=times {
            backdate_all(&dir.join("target"), Duration::from_secs(10 * 86_400));
            let text = std::fs::read_to_string(&manifest).unwrap();
            let next = text.replace(&format!("0.1.{}", n - 1), &format!("0.1.{n}"));
            std::fs::write(&manifest, next).unwrap();
            self.hive.build_crate(dir, None);
        }
    }

    pub fn main_target(&self) -> PathBuf {
        self.main.join("target")
    }
    pub fn worktree_target(&self) -> PathBuf {
        self.worktree.join("target")
    }

    /// `gc --plan` over every registered repo; returns the receipt.
    pub fn plan(&self) -> serde_json::Value {
        self.plan_for(&[])
    }

    pub fn plan_for(&self, repos: &[&str]) -> serde_json::Value {
        let mut args = vec!["gc", "--plan"];
        args.extend_from_slice(repos);
        let (out, r) = self.hive.json(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        r
    }

    /// The id of the plan of `class` in a `gc --plan` or `apply` receipt.
    pub fn id_of(receipt: &serde_json::Value, class: &str) -> String {
        receipt["plans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["class"] == class)
            .unwrap_or_else(|| panic!("no {class} plan in {receipt}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The hash tail of the `.fingerprint/fixture-*` unit whose `invoked.timestamp` is oldest
    /// in `target/debug`: the dead unit.
    pub fn dead_unit_hash(&self, target: &Path) -> String {
        let fp = target.join("debug/.fingerprint");
        let mut best: Option<(SystemTime, String)> = None;
        let mut seen = 0usize;
        for e in std::fs::read_dir(&fp).unwrap() {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let dir = e.unwrap().path();
            let name = dir.file_name().unwrap().to_str().unwrap().to_string();
            if !name.starts_with("fixture-") {
                continue;
            }
            let t = std::fs::metadata(dir.join("invoked.timestamp"))
                .unwrap()
                .modified()
                .unwrap();
            if best.as_ref().is_none_or(|(b, _)| t < *b) {
                best = Some((t, name.rsplit('-').next().unwrap().to_string()));
            }
        }
        best.expect("a fixture unit").1
    }

    /// The stored plans file under `HIVE_HOME`.
    pub fn plans_json(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.hive.home.path.join("plans.json")).unwrap())
            .unwrap()
    }

    /// Makes the dead unit `hash` of `target` live again by touching its stamp.
    pub fn revive(&self, target: &Path, hash: &str) {
        let stamp = target.join(format!(
            "debug/.fingerprint/fixture-{hash}/invoked.timestamp"
        ));
        set_mtime(&stamp, SystemTime::now());
    }
}

/// Every path under `root` with its length and mtime, not following symlinks.
pub fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, SystemTime)>, depth: usize) {
        assert!(depth < 64, "walk bound hit");
        let mut seen = 0usize;
        for entry in std::fs::read_dir(dir).unwrap() {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let entry = entry.unwrap();
            let md = std::fs::symlink_metadata(entry.path()).unwrap();
            out.insert(entry.path(), (md.len(), md.modified().unwrap()));
            if md.is_dir() {
                walk(&entry.path(), out, depth + 1);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out, 0);
    out
}
