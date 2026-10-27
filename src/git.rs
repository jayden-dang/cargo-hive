//! `git` child processes with every `GIT_*` variable scrubbed (an inherited GIT_DIR
//! would override `-C`). Paths come back absolute.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub enum GitError {
    NotGit,
    Failed { args: String, stderr: String },
    Spawn(std::io::Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotGit => write!(f, "not inside a git checkout"),
            GitError::Failed { args, stderr } => {
                write!(f, "git {args} failed: {}", stderr.trim())
            }
            GitError::Spawn(e) => write!(f, "could not run git: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn at(dir: &Path) -> Git {
        Git {
            dir: dir.to_path_buf(),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        // A user's `core.fsmonitor=true` would start a daemon that writes a socket into the
        // checkout's .git; hive only reads.
        cmd.args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(&self.dir);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(&key);
            }
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Result<std::process::Output, GitError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(GitError::Spawn)?;
        if out.status.success() {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if stderr.contains("not a git repository") || !self.dir.exists() {
            return Err(GitError::NotGit);
        }
        Err(GitError::Failed {
            args: args.join(" "),
            stderr,
        })
    }

    fn line(&self, args: &[&str]) -> Result<String, GitError> {
        Ok(String::from_utf8_lossy(&self.run(args)?.stdout)
            .trim()
            .to_string())
    }

    pub fn toplevel(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .map(PathBuf::from)
    }

    pub fn git_dir(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--git-dir"])
            .map(PathBuf::from)
    }

    pub fn common_dir(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .map(PathBuf::from)
    }

    pub fn superproject(&self) -> Result<Option<PathBuf>, GitError> {
        let s = self.line(&["rev-parse", "--show-superproject-working-tree"])?;
        Ok((!s.is_empty()).then(|| PathBuf::from(s)))
    }

    pub fn has_commits(&self) -> Result<bool, GitError> {
        let out = self
            .command()
            .args(["rev-parse", "--verify", "--quiet", "HEAD"])
            .output()
            .map_err(GitError::Spawn)?;
        Ok(out.status.success())
    }

    pub fn root_commits(&self) -> Result<BTreeSet<String>, GitError> {
        let text =
            String::from_utf8_lossy(&self.run(&["rev-list", "--max-parents=0", "HEAD"])?.stdout)
                .to_string();
        Ok(text
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect())
    }

    pub fn is_shallow(&self) -> Result<bool, GitError> {
        Ok(self.line(&["rev-parse", "--is-shallow-repository"])? == "true")
    }

    /// `git worktree list --porcelain -z`: attributes are NUL-terminated, and a
    /// record ends with an empty attribute.
    pub fn worktrees(&self) -> Result<Vec<WorktreeRecord>, GitError> {
        let out = self.run(&["worktree", "list", "--porcelain", "-z"])?;
        let mut records = Vec::new();
        let mut current: Option<WorktreeRecord> = None;
        for field in out.stdout.split(|b| *b == 0) {
            let field = String::from_utf8_lossy(field);
            if field.is_empty() {
                records.extend(current.take());
                continue;
            }
            let (key, value) = field.split_once(' ').unwrap_or((&field, ""));
            match key {
                "worktree" => {
                    records.extend(current.take());
                    current = Some(WorktreeRecord {
                        path: PathBuf::from(value),
                        ..Default::default()
                    });
                }
                _ => {
                    let Some(rec) = current.as_mut() else {
                        continue;
                    };
                    match key {
                        "HEAD" => rec.head = Some(value.to_string()),
                        "branch" => rec.branch = Some(value.to_string()),
                        "bare" => rec.bare = true,
                        "detached" => rec.detached = true,
                        "locked" => rec.locked = true,
                        "prunable" => rec.prunable = true,
                        _ => {}
                    }
                }
            }
        }
        records.extend(current);
        Ok(records)
    }
}
