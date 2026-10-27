#![allow(dead_code)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory under /private/tmp, canonicalized, removed on drop.
pub struct Scratch {
    pub path: PathBuf,
}

impl Scratch {
    pub fn new(label: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = Path::new("/private/tmp").join(format!(
            "hive-it-{label}-{}-{n}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Scratch {
            path: path.canonicalize().unwrap(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = Command::new("chmod")
            .arg("-R")
            .arg("u+rwx")
            .arg(&self.path)
            .status();
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// One hive installation: its own HIVE_HOME, plus a scratch CARGO_HOME for child cargo.
pub struct Hive {
    pub home: Scratch,
    pub cargo_home: Scratch,
}

impl Hive {
    pub fn new() -> Hive {
        Hive {
            home: Scratch::new("home"),
            cargo_home: Scratch::new("cargo-home"),
        }
    }

    pub fn config(&self, json: &str) {
        fs::write(self.home.path.join("config.json"), json).unwrap();
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargo-hive"));
        cmd.args(args)
            .env("HIVE_HOME", &self.home.path)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Runs with `--json` and parses stdout as exactly one JSON value.
    pub fn json(&self, args: &[&str]) -> (Output, serde_json::Value) {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.run(&all);
        let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout was not one JSON receipt ({e}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out, value)
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=hive",
            "-c",
            "user.email=hive@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.fsmonitor=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        // Fixtures never see the developer's git config (an fsmonitor there leaves a
        // socket in .git and a daemon running).
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A git repo at `dir` with a Cargo package at `crate_rel` (relative, "" for the root) and one commit.
pub fn init_repo(dir: &Path, crate_rel: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    let krate = dir.join(crate_rel);
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(krate.join("Cargo.toml"), "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();
    fs::write(krate.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    git(dir, &["add", "-A"]);
    // The path in the message makes every fixture's root commit unique, even for
    // tests that run in the same second.
    git(
        dir,
        &["commit", "-q", "-m", &format!("init {}", dir.display())],
    );
    dir.canonicalize().unwrap()
}
