//! `git` child processes with every `GIT_*` variable scrubbed (an inherited GIT_DIR
//! would override `-C`). Paths come back absolute.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::metadata::METADATA_DEADLINE;
use cargo_hive_platform::proc::{self, Capture, ChildOutputError, Ran, Spec};
use cargo_hive_platform::signals::cancelled;

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

#[derive(Debug)]
pub enum SlotGitError {
    Cancelled,
    Incomplete,
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

    fn command(&self) -> Spec {
        self.git_cmd(&[])
    }

    fn read_only(&self) -> Spec {
        // Plain `git status` refreshes mtimes into .git/index; --no-optional-locks does not.
        self.git_cmd(&["--no-optional-locks"])
    }

    fn git_cmd(&self, leading: &[&str]) -> Spec {
        let mut spec = Spec::new("git");
        // A user's `core.fsmonitor=true` would start a daemon that writes a socket into the
        // checkout's .git; hive only reads.
        spec.args.extend(leading.iter().map(|s| (*s).into()));
        spec.args
            .extend(["-c".into(), "core.fsmonitor=false".into()]);
        spec.args.push("-C".into());
        spec.args.push(self.dir.as_os_str().into());
        let parent: Vec<_> = std::env::vars_os().collect();
        crate::env::git(&mut spec, &parent);
        spec
    }

    fn run_read_only(&self, args: &[&str]) -> Result<Ran, GitError> {
        let mut spec = self.read_only();
        spec.args.extend(args.iter().map(|s| (*s).into()));
        match proc::run_bounded(
            &spec,
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
        let mut spec = self.command();
        spec.args.extend(args.iter().map(|s| (*s).into()));
        let out = proc::output(&spec).map_err(GitError::Spawn)?;
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
        let mut spec = self.command();
        spec.args.extend(
            ["rev-parse", "--verify", "--quiet", "HEAD"]
                .into_iter()
                .map(Into::into),
        );
        let out = proc::output(&spec).map_err(GitError::Spawn)?;
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

    /// Reads Git evidence for a guarded slot with a strict byte limit.
    pub fn slot_read_only(&self, args: &[&str], max: usize) -> Result<Vec<u8>, SlotGitError> {
        let stdout_cap = max.checked_add(1).ok_or(SlotGitError::Incomplete)?;
        let mut spec = self.git_cmd(&["--no-optional-locks", "--no-replace-objects"]);
        spec.args.extend(args.iter().map(|arg| (*arg).into()));
        // Older Git versions without the lazy-fetch switch still honor the transport allowlist.
        let controlled = [("GIT_NO_LAZY_FETCH", "1"), ("GIT_ALLOW_PROTOCOL", "")];
        spec.env_remove
            .retain(|key| !controlled.iter().any(|(name, _)| key == *name));
        spec.env_remove.extend(
            [
                "CARGO_TARGET_DIR",
                "CARGO_BUILD_TARGET_DIR",
                "CARGO_BUILD_BUILD_DIR",
            ]
            .map(Into::into),
        );
        spec.env_set
            .extend(controlled.map(|(key, value)| (key.into(), value.into())));
        match proc::run_bounded_complete(
            &spec,
            Duration::from_secs(10),
            Capture {
                stdout_cap,
                stderr_cap: 1,
            },
            cancelled,
        ) {
            Ok(ran)
                if ran.status.success()
                    && ran.stderr_tail.is_empty()
                    && ran.stdout.len() <= max =>
            {
                Ok(ran.stdout)
            }
            Err(ChildOutputError::Cancelled) => Err(SlotGitError::Cancelled),
            _ => Err(SlotGitError::Incomplete),
        }
    }

    /// Requires complete NUL records from a non-bare repository's worktree inventory.
    pub fn worktrees_complete(&self, max: usize) -> Result<Vec<WorktreeRecord>, SlotGitError> {
        let bytes = self.slot_read_only(&["worktree", "list", "--porcelain", "-z"], max)?;
        complete_worktrees(&bytes).ok_or(SlotGitError::Incomplete)
    }
}

fn complete_worktrees(bytes: &[u8]) -> Option<Vec<WorktreeRecord>> {
    let records = std::str::from_utf8(bytes).ok()?.strip_suffix("\0\0")?;
    let mut paths = BTreeSet::new();
    let mut oid_len = None;
    records
        .split("\0\0")
        .map(|record| {
            let mut fields = record.split('\0');
            let path = fields.next()?.strip_prefix("worktree ")?;
            if path.len() > 4096
                || !path.starts_with('/')
                || path[1..]
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
                || !paths.insert(path)
            {
                return None;
            }
            let head = fields.next()?.strip_prefix("HEAD ")?;
            if !matches!(head.len(), 40 | 64)
                || *oid_len.get_or_insert(head.len()) != head.len()
                || !head
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            {
                return None;
            }
            let mut record = WorktreeRecord {
                path: PathBuf::from(path),
                head: Some(head.into()),
                ..Default::default()
            };
            match fields.next()? {
                "detached" => record.detached = true,
                kind => {
                    let branch = kind
                        .strip_prefix("branch ")
                        .filter(|name| valid_git_ref(name))?;
                    record.branch = Some(branch.into());
                }
            }
            for field in fields {
                let (name, reason) = field.split_once(' ').unwrap_or((field, ""));
                let seen = match name {
                    "locked" => &mut record.locked,
                    "prunable" => &mut record.prunable,
                    _ => return None,
                };
                if (reason.is_empty() && name != field) || std::mem::replace(seen, true) {
                    return None;
                }
            }
            Some(record)
        })
        .collect()
}

fn valid_git_ref(name: &str) -> bool {
    name.contains('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
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

    #[test]
    fn slot_inventory_accepts_the_exact_cap_and_rejects_a_complete_retained_prefix() {
        let repo = scratch_repo("slot-git-cap");
        let linked = repo.path().join("linked");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let cap = 64 * 1024;
        let inventory = Git::at(repo.path());
        let baseline = inventory
            .run(&["worktree", "list", "--porcelain", "-z"])
            .unwrap()
            .stdout;
        let reason = "a".repeat(cap - baseline.len() - b"locked \0".len());
        git(
            repo.path(),
            &[
                "worktree",
                "lock",
                "--reason",
                &reason,
                linked.to_str().unwrap(),
            ],
        );
        let exact = inventory
            .run(&["worktree", "list", "--porcelain", "-z"])
            .unwrap()
            .stdout;
        assert_eq!(exact.len(), cap);
        assert_eq!(inventory.worktrees_complete(cap).unwrap().len(), 2);
        assert!(inventory.worktrees_complete(cap - 1).is_err());
        assert!(complete_worktrees(&exact[..exact.len() - 1]).is_none());
        let overflow = [&exact[..], b"w"].concat();
        assert!(complete_worktrees(&overflow).is_none());
    }

    fn assert_error<E: std::error::Error>(_: &E) {}

    #[test]
    fn git_errors_are_std_errors() {
        let err = GitError::NotGit;
        assert_error(&err);
        assert_eq!(err.to_string(), "not inside a git checkout");
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn worktree_inventory_requires_complete_exclusive_fields() {
        let head = "a".repeat(40);
        let valid_main =
            format!("worktree /registered-main\0HEAD {head}\0branch refs/heads/main\0\0");
        let linked = format!(
            "worktree /linked\0HEAD {head}\0detached\0locked because\nof this\0prunable reason\0\0"
        );
        let records = complete_worktrees(format!("{valid_main}{linked}").as_bytes()).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records[1].locked && records[1].prunable);
        assert!(complete_worktrees(b"worktree /registered-main\0bare\0\0").is_none());

        for fields in [
            "branch refs/heads/main".to_owned(),
            "HEAD short\0detached".to_owned(),
            format!("HEAD {}\0detached", "a".repeat(64)),
            format!("HEAD {}\0detached", "z".repeat(40)),
            format!("HEAD {head}"),
            format!("HEAD {head}\0HEAD {head}\0detached"),
            format!("HEAD {head}\0detached extra"),
            format!("HEAD {head}\0branch refs/heads/m..ain"),
            format!("HEAD {head}\0branch refs/heads/main\0detached"),
            "bare".to_owned(),
            format!("HEAD {head}\0bare"),
            format!("HEAD {head}\0detached\0locked\0locked again"),
            format!("HEAD {head}\0detached\0prunable one\0prunable two"),
        ] {
            let malformed = format!("{valid_main}worktree /linked\0{fields}\0\0");
            assert!(
                complete_worktrees(malformed.as_bytes()).is_none(),
                "accepted {fields:?}"
            );
        }
    }
}
