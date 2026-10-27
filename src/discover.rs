//! Checkouts of a registered repo and (Task 13) its Cargo target dirs. Every walk is
//! depth-bounded and never enters `node_modules`, `target` or `.git`.

use crate::classify::{classify, CheckoutFacts, CheckoutKind, Class};
use crate::git::Git;
use crate::registry::Repo;
use crate::state::Config;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const SKIP_DIRS: [&str; 3] = ["node_modules", "target", ".git"];

#[derive(Debug, Clone, Serialize)]
pub struct Checkout {
    pub path: PathBuf,
    pub kind: CheckoutKind,
    pub class: Class,
    pub reason: String,
    pub missing: bool,
}

fn skipped(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// Directories at most `depth` levels below `root` that are git checkouts (`.git` file
/// or directory). A found checkout is not descended into.
pub fn git_checkouts_within(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth == 0 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !skipped(&e.file_name()) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        if dir.join(".git").exists() {
            found.push(dir);
        } else {
            found.extend(git_checkouts_within(&dir, depth - 1));
        }
    }
    found
}

pub fn checkouts(repo: &Repo, registered_mains: &[PathBuf], cfg: &Config) -> Vec<Checkout> {
    let mut out: Vec<Checkout> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let push =
        |out: &mut Vec<Checkout>, path: PathBuf, kind: CheckoutKind, git: Result<(), String>| {
            let realpath = path.canonicalize().map_err(|e| e.to_string());
            let facts = CheckoutFacts {
                realpath: realpath.clone(),
                kind: kind.clone(),
                git,
            };
            let (class, reason) = classify(&facts, registered_mains, &cfg.secondary_roots);
            out.push(Checkout {
                path: realpath.unwrap_or(path),
                kind,
                class,
                reason,
                missing: false,
            });
        };
    match Git::at(&repo.main).worktrees() {
        Ok(records) => {
            for rec in records.into_iter().filter(|r| !r.bare) {
                let is_main = rec.path.canonicalize().ok().as_ref() == Some(&repo.main)
                    || rec.path == repo.main;
                let kind = if is_main {
                    CheckoutKind::Main
                } else {
                    CheckoutKind::Linked {
                        main: repo.main.clone(),
                    }
                };
                if !rec.path.exists() {
                    out.push(Checkout {
                        path: rec.path,
                        kind,
                        class: Class::Protected,
                        reason: "missing: the worktree directory does not exist".into(),
                        missing: true,
                    });
                    continue;
                }
                seen.insert(rec.path.canonicalize().unwrap_or(rec.path.clone()));
                push(&mut out, rec.path, kind, Ok(()));
            }
        }
        Err(e) => {
            seen.insert(repo.main.clone());
            push(
                &mut out,
                repo.main.clone(),
                CheckoutKind::Main,
                Err(e.to_string()),
            );
        }
    }
    let mut clones: Vec<PathBuf> = Vec::new();
    for root in &cfg.secondary_roots {
        for dir in git_checkouts_within(root, 2) {
            let real = dir.canonicalize().unwrap_or(dir.clone());
            if seen.contains(&real) {
                continue;
            }
            let g = Git::at(&real);
            let primary = matches!((g.git_dir(), g.common_dir()), (Ok(a), Ok(b)) if a == b);
            let not_sub = matches!(g.superproject(), Ok(None));
            let shares_root = g
                .root_commits()
                .is_ok_and(|roots| !roots.is_disjoint(&repo.root_commits));
            if primary && not_sub && shares_root {
                seen.insert(real.clone());
                clones.push(real);
            }
        }
    }
    clones.sort();
    for c in clones {
        push(&mut out, c, CheckoutKind::Clone, Ok(()));
    }
    out
}
