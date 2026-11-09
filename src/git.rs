//! `git` child processes with every `GIT_*` variable scrubbed (an inherited GIT_DIR
//! would override `-C`). Paths come back absolute.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cargo_target::{run_bounded, Capture, ChildOutputError, Ran, METADATA_DEADLINE};

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

impl std::error::Error for GitError {}

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
        self.git_cmd(&[])
    }

    fn read_only(&self) -> Command {
        // Plain `git status` refreshes mtimes into .git/index; --no-optional-locks does not.
        self.git_cmd(&["--no-optional-locks"])
    }

    fn git_cmd(&self, leading: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        // A user's `core.fsmonitor=true` would start a daemon that writes a socket into the
        // checkout's .git; hive only reads.
        cmd.args(leading)
            .args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(&self.dir);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(&key);
            }
        }
        cmd
    }

    fn run_read_only(&self, args: &[&str]) -> Result<Ran, GitError> {
        let mut cmd = self.read_only();
        cmd.args(args);
        match run_bounded(
            &mut cmd,
            METADATA_DEADLINE,
            Capture {
                stdout_cap: 64 * 1024,
                stderr_cap: 4 * 1024,
            },
            || false,
        ) {
            Ok(ran) if ran.status.success() => Ok(ran),
            Ok(ran) => Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&ran.stderr_tail).to_string(),
            }),
            Err(ChildOutputError::TimedOut) => Err(GitError::Failed {
                args: args.join(" "),
                stderr: format!("{METADATA_DEADLINE:?}"),
            }),
            Err(ChildOutputError::Cancelled) => Err(GitError::Failed {
                args: args.join(" "),
                stderr: "cancelled".into(),
            }),
            Err(ChildOutputError::Failed(msg)) => Err(GitError::Failed {
                args: args.join(" "),
                stderr: msg,
            }),
        }
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

    pub fn head(&self) -> Result<String, GitError> {
        let ran = self.run_read_only(&["rev-parse", "--verify", "HEAD"])?;
        Ok(String::from_utf8_lossy(&ran.stdout).trim().to_string())
    }

    pub fn has_tracked_changes(&self) -> Result<bool, GitError> {
        let ran = self.run_read_only(&["status", "--porcelain", "--untracked-files=no"])?;
        Ok(!ran.stdout.is_empty())
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
        let s = self.line(&[
            "rev-parse",
            "--path-format=absolute",
            "--show-superproject-working-tree",
        ])?;
        Ok((!s.is_empty()).then(|| PathBuf::from(s)))
    }

    /// `Ok(false)` only when git quietly reports a missing HEAD. Any diagnostic
    /// on stderr is a failed probe, not an empty repository.
    pub fn has_commits(&self) -> Result<bool, GitError> {
        let out = self
            .command()
            .args(["rev-parse", "--verify", "--quiet", "HEAD"])
            .output()
            .map_err(GitError::Spawn)?;
        if out.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if stderr.trim().is_empty() {
            return Ok(false);
        }
        Err(GitError::Failed {
            args: "rev-parse --verify --quiet HEAD".into(),
            stderr,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    struct ScratchRepo(PathBuf);

    impl ScratchRepo {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn scratch_repo(label: &str) -> ScratchRepo {
        let dir = PathBuf::from(format!(
            "/private/tmp/hive-it-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a"), "a").unwrap();
        git(&dir, &["add", "a"]);
        git(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "a",
            ],
        );
        ScratchRepo(dir)
    }

    #[test]
    fn reading_a_touched_but_unchanged_tree_leaves_the_index_bytes_unchanged() {
        let repo = scratch_repo("bench-git");
        let dir = repo.path();
        let index = dir.join(".git/index");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.join("a"), std::fs::read(dir.join("a")).unwrap()).unwrap();
        let before = std::fs::read(&index).unwrap();
        let g = Git::at(dir);
        assert_eq!(g.head().unwrap().len(), 40);
        assert!(!g.has_tracked_changes().unwrap());
        assert_eq!(std::fs::read(&index).unwrap(), before);
    }

    #[test]
    fn a_changed_tracked_file_is_a_change_and_an_untracked_file_is_not() {
        let repo = scratch_repo("bench-git-changes");
        let dir = repo.path();
        std::fs::write(dir.join("new"), "x").unwrap();
        assert!(!Git::at(dir).has_tracked_changes().unwrap());
        std::fs::write(dir.join("a"), "changed").unwrap();
        assert!(Git::at(dir).has_tracked_changes().unwrap());
    }

    fn assert_error<E: std::error::Error>(_: &E) {}

    #[test]
    fn git_errors_are_std_errors() {
        let err = GitError::NotGit;
        assert_error(&err);
        assert_eq!(err.to_string(), "not inside a git checkout");
        assert!(std::error::Error::source(&err).is_none());
    }
}
