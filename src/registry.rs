//! Registered repositories. Task 3 adds `register`.

use crate::git::{Git, GitError};
use crate::state::{StateError, Tx};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

pub const FILE: &str = "registry.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub repos: Vec<Repo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub main: PathBuf,
    pub root_commits: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered(Repo),
    AlreadyRegistered(Repo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterRefusal {
    NotGit,
    Submodule { superproject: PathBuf },
    LinkedWorktree { main: PathBuf },
    NoCommits,
    Shallow,
    NoCargoToml { root: PathBuf },
}

impl fmt::Display for RegisterRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterRefusal::NotGit => write!(f, "the path is not inside a git checkout"),
            RegisterRefusal::Submodule { superproject } => write!(
                f,
                "the path is inside a git submodule; register the superproject {} instead",
                superproject.display()
            ),
            RegisterRefusal::LinkedWorktree { main } => write!(
                f,
                "the path is a linked worktree; register its main checkout {} instead",
                main.display()
            ),
            RegisterRefusal::NoCommits => write!(f, "the repository has no commits yet"),
            RegisterRefusal::Shallow => write!(
                f,
                "the repository is a shallow clone, so its root commits cannot be determined"
            ),
            RegisterRefusal::NoCargoToml { root } => write!(
                f,
                "no Cargo.toml within 2 directory levels of {}",
                root.display()
            ),
        }
    }
}

/// Bounded search: is there a `Cargo.toml` at depth 0, 1 or 2 below `root`?
pub fn has_cargo_toml_within(root: &Path, depth: usize) -> bool {
    if root.join("Cargo.toml").is_file() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let skip = matches!(name.to_str(), Some("target" | "node_modules" | ".git"));
        !skip
            && e.file_type().is_ok_and(|t| t.is_dir())
            && has_cargo_toml_within(&e.path(), depth - 1)
    })
}

pub fn register(
    path: &Path,
    tx: &Tx<'_>,
) -> Result<Result<RegisterOutcome, RegisterRefusal>, StateError> {
    let refusal = |r| Ok(Err(r));
    let Ok(real) = path.canonicalize() else {
        return refusal(RegisterRefusal::NotGit);
    };
    let git = Git::at(&real);
    let top = match git.toplevel() {
        Ok(t) => t,
        Err(_) => return refusal(RegisterRefusal::NotGit),
    };
    let top_git = Git::at(&top);
    if let Ok(Some(superproject)) = top_git.superproject() {
        return refusal(RegisterRefusal::Submodule { superproject });
    }
    let (git_dir, common) = match (top_git.git_dir(), top_git.common_dir()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return refusal(RegisterRefusal::NotGit),
    };
    let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
    if canon(git_dir) != canon(common.clone()) {
        let main = canon(common.clone())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(common);
        return refusal(RegisterRefusal::LinkedWorktree { main });
    }
    if !top_git.has_commits().unwrap_or(false) {
        return refusal(RegisterRefusal::NoCommits);
    }
    if top_git.is_shallow().unwrap_or(true) {
        return refusal(RegisterRefusal::Shallow);
    }
    let main = canon(top);
    if !has_cargo_toml_within(&main, 2) {
        return refusal(RegisterRefusal::NoCargoToml { root: main });
    }
    let root_commits = match top_git.root_commits() {
        Ok(r) => r,
        Err(GitError::NotGit) => return refusal(RegisterRefusal::NotGit),
        Err(_) => return refusal(RegisterRefusal::NoCommits),
    };
    let mut reg: Registry = tx.read(FILE)?;
    if let Some(existing) = reg.repos.iter().find(|r| r.main == main) {
        return Ok(Ok(RegisterOutcome::AlreadyRegistered(existing.clone())));
    }
    let repo = Repo { main, root_commits };
    reg.repos.push(repo.clone());
    reg.repos.sort_by(|a, b| a.main.cmp(&b.main));
    tx.write(FILE, &reg)?;
    Ok(Ok(RegisterOutcome::Registered(repo)))
}
