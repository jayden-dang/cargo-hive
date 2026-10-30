#![allow(dead_code)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

    /// Writes config.json. `cache_roots` defaults to `<home>/no-cache`, so tests never walk
    /// the real ~/.cache/cargo-target; pass it explicitly to override.
    pub fn config(&self, json: &str) {
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .entry("cache_roots")
            .or_insert_with(|| serde_json::json!([self.home.path.join("no-cache")]));
        fs::write(self.home.path.join("config.json"), value.to_string()).unwrap();
    }

    /// A hive whose doctor results are recorded for the active toolchain with every
    /// capability enabled, and whose cache root is a scratch dir.
    pub fn ready() -> Hive {
        let hive = Hive::new();
        hive.write_ready_capabilities();
        hive.config("{}");
        hive
    }

    pub fn write_ready_capabilities(&self) {
        let release = cargo_hive::doctor::resolve_toolchain(&self.home.path)
            .expect("the proof machine has a Rust toolchain")
            .release;
        let caps = serde_json::json!({
            "toolchain_release": release,
            "disabled": {},
            "lock_check": 1,
            "failed_marker_check": 1
        });
        fs::write(self.home.path.join("capabilities.json"), caps.to_string()).unwrap();
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

    /// `cargo build --offline` in `dir` with the scratch CARGO_HOME, into `dir/target`
    /// unless `target_dir` is given.
    pub fn build_crate(&self, dir: &Path, target_dir: Option<&Path>) {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--offline", "--quiet"])
            .current_dir(dir)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        if let Some(t) = target_dir {
            cmd.env("CARGO_TARGET_DIR", t);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "cargo build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn build_crate_failing(&self, dir: &Path) {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--offline", "--quiet"])
            .current_dir(dir)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        let out = cmd.output().unwrap();
        assert!(
            !out.status.success(),
            "cargo build succeeded: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `cargo check --offline` in `dir` with the same environment as `build_crate`.
    pub fn check_crate(&self, dir: &Path) {
        let mut cmd = Command::new("cargo");
        cmd.args(["check", "--offline", "--quiet"])
            .current_dir(dir)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "cargo check failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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

    /// Builds `repo` three times, editing its lib in between, so each incremental crate dir
    /// holds a superseded session beside the newest one.
    pub fn build_with_sessions(&self, repo: &Path, target_dir: Option<&Path>) {
        for i in 0..3 {
            fs::write(
                repo.join("src/lib.rs"),
                format!("pub fn answer() -> u32 {{ {} }}\n", 42 + i),
            )
            .unwrap();
            self.build_crate(repo, target_dir);
        }
    }

    /// Starts `__hold-lock <kind> <path>` and returns once it reports `held`.
    pub fn hold_lock_child(&self, kind: &str, path: &Path) -> Child {
        let mut child = self
            .command(&["__hold-lock", kind, path.to_str().unwrap()])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert_eq!(line.trim(), "held");
        child
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

pub fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

/// Crate dirs under `<target>/debug/incremental`.
pub fn crate_dirs(target: &Path) -> Vec<PathBuf> {
    let inc = target.join("debug/incremental");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&inc)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Session dir names in one crate dir (lock files excluded).
pub fn sessions_in(crate_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(crate_dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn b36(mut n: u64) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// Plants a session `secs_ago` old (named as rustc names them) holding one 8 KiB file, plus
/// its lock file unless `lockless`. Returns the (dir, lock) names.
pub fn plant_session(
    crate_dir: &Path,
    secs_ago: u64,
    last: &str,
    lockless: bool,
) -> (String, String) {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let micros = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
        - Duration::from_secs(secs_ago))
    .as_micros() as u64;
    let key = format!("s-{}-p{}", b36(micros), NEXT.fetch_add(1, Ordering::SeqCst));
    let dir = format!("{key}-{last}");
    fs::create_dir_all(crate_dir.join(&dir)).unwrap();
    fs::write(
        crate_dir.join(&dir).join("query-cache.bin"),
        vec![5u8; 8192],
    )
    .unwrap();
    let lock = format!("{key}.lock");
    if !lockless {
        fs::write(crate_dir.join(&lock), b"").unwrap();
    }
    (dir, lock)
}

/// Waits for `child` up to `limit`; a hang fails the test instead of blocking the suite.
pub fn wait_within(mut child: Child, limit: Duration) -> Output {
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("the command did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}
