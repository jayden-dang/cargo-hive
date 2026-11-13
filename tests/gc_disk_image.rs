#![cfg(target_os = "macos")]
mod common;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::Fs;
use common::{crate_dirs, init_repo, plant_session, sessions_in, Hive, Scratch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(args: &[&str]) {
    let out = Command::new(args[0]).args(&args[1..]).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Detaches and removes disk-image scratch dirs left by test processes that no longer exist.
fn sweep_stale() {
    for e in fs::read_dir("/private/tmp").unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix("hive-it-diskimage-")
            .and_then(|r| r.split('-').next())
        else {
            continue;
        };
        let alive = Command::new("kill")
            .args(["-0", pid])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            let _ = Command::new("hdiutil")
                .args(["detach", "-force", "-quiet"])
                .arg(e.path().join("mnt"))
                .status();
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// An APFS image attached inside its scratch dir, never under /Volumes, unindexed; detached on drop.
struct Image {
    _scratch: Scratch,
    mount: PathBuf,
}

impl Image {
    fn new() -> Image {
        sweep_stale();
        let scratch = Scratch::new("diskimage");
        let (img, mount) = (scratch.path.join("hive.dmg"), scratch.path.join("mnt"));
        fs::create_dir_all(&mount).unwrap();
        sh(&[
            "hdiutil",
            "create",
            "-size",
            "512m",
            "-fs",
            "APFS",
            "-volname",
            "hive-it",
            "-quiet",
            img.to_str().unwrap(),
        ]);
        sh(&[
            "hdiutil",
            "attach",
            "-nobrowse",
            "-quiet",
            "-mountpoint",
            mount.to_str().unwrap(),
            img.to_str().unwrap(),
        ]);
        fs::write(mount.join(".metadata_never_index"), b"").unwrap();
        Image {
            mount: mount.canonicalize().unwrap(),
            _scratch: scratch,
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mount)
            .status();
    }
}

fn big(path: &Path, mib: usize) {
    fs::write(path, vec![7u8; mib << 20]).unwrap();
}

fn mib2_text() -> Vec<u8> {
    vec![b'a'; 2 << 20]
}

fn mib2_random(seed: u64) -> Vec<u8> {
    let mut bytes = vec![0u8; 2 << 20];
    let mut state = seed;
    for b in &mut bytes {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (state >> 33) as u8;
    }
    bytes
}

/// Hive state rooted on the disk image so volume samples see store eviction.
struct ImageHive {
    home: PathBuf,
    cargo_home: Scratch,
}

impl ImageHive {
    fn new(mount: &Path) -> ImageHive {
        let home = mount.join("hive");
        fs::create_dir_all(home.join("no-cache")).unwrap();
        let hive = ImageHive {
            home,
            cargo_home: Scratch::new("cargo-home"),
        };
        let release = cargo_hive::toolchain::resolve_toolchain(&hive.home)
            .expect("the proof machine has a Rust toolchain")
            .release;
        fs::write(
            hive.home.join("capabilities.json"),
            serde_json::json!({
                "toolchain_release": release,
                "disabled": {},
                "lock_check": 1,
                "failed_marker_check": 1
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            hive.home.join("config.json"),
            serde_json::json!({
                "cache_roots": [hive.home.join("no-cache")]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(hive.home.join("registry.json"), r#"{"repos":[]}"#).unwrap();
        hive
    }

    fn json(&self, args: &[&str]) -> (std::process::Output, serde_json::Value) {
        let mut all = args.to_vec();
        all.push("--json");
        let out = Command::new(env!("CARGO_BIN_EXE_cargo-hive"))
            .args(&all)
            .env("HIVE_HOME", &self.home)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .output()
            .unwrap();
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

fn plant_aged_entry(
    home: &Path,
    value: u64,
    name_meta: &str,
    fingerprint_hash: &str,
    file_name: &str,
    bytes: Vec<u8>,
    secs_ago: u64,
) -> PathBuf {
    use cargo_hive::seed::store::{Row, StoreFile, MAX_STORE_BYTES, STORE_FILE};
    use cargo_hive::state::State;
    use std::time::{SystemTime, UNIX_EPOCH};

    let dir = home
        .join("store")
        .join(value.to_string())
        .join(format!("{name_meta}-{fingerprint_hash}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file_name);
    fs::write(&path, &bytes).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last_live_at = now.saturating_sub(secs_ago);
    let row = Row {
        toolchain_value: value,
        name_meta: name_meta.into(),
        fingerprint_hash: fingerprint_hash.into(),
        kind: "lib".into(),
        rel_profile: PathBuf::from("debug"),
        size: bytes.len() as u64,
        filled_at: last_live_at,
        last_live_at,
        repos: Vec::new(),
        registry_deps: Vec::new(),
        variant: 0,
    };
    let state = State::open_at(home.to_path_buf()).unwrap();
    state
        .locked(|tx| {
            let mut file: StoreFile = tx
                .read_bounded(STORE_FILE, MAX_STORE_BYTES)
                .unwrap_or_default();
            file.toolchains
                .entry(format!("synth-{value}"))
                .or_insert(value);
            file.units.push(row);
            tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
        })
        .unwrap();
    path
}

#[test]
fn freed_bytes_never_exceed_the_space_the_volume_gains() {
    let image = Image::new();
    let hive = Hive::ready();
    let repo = init_repo(&image.mount.join("app"), "");
    hive.build_with_sessions(&repo, None);
    let target = repo.join("target");
    let deps = target.join("debug/deps");
    for (i, d) in crate_dirs(&target).iter().enumerate() {
        let kept = sessions_in(d)
            .into_iter()
            .max_by_key(|n| n.split('-').nth(1).unwrap().to_string())
            .unwrap();
        for j in 0..8 {
            let (old, _) = plant_session(d, 7200 + j, "svh0", false);
            big(&d.join(&old).join("big.bin"), 4);
        }
        // Old-session files still held elsewhere must never count as freed.
        let (linked, _) = plant_session(d, 9000, "svh0", false);
        big(&deps.join(format!("hive-linked-{i}.o")), 16);
        fs::hard_link(
            deps.join(format!("hive-linked-{i}.o")),
            d.join(&linked).join("in-deps.o"),
        )
        .unwrap();
        big(&d.join(&kept).join(format!("hive-both-{i}.o")), 16);
        fs::hard_link(
            d.join(&kept).join(format!("hive-both-{i}.o")),
            d.join(&linked).join("in-both.o"),
        )
        .unwrap();
        fs::hard_link(
            d.join(&kept).join(format!("hive-both-{i}.o")),
            deps.join(format!("hive-both-{i}.o")),
        )
        .unwrap();
    }
    assert!(hive
        .run(&["repo", "add", repo.to_str().unwrap()])
        .status
        .success());
    sh(&["sync"]);
    let apfs = Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into());
    let before = apfs.volume(&image.mount).unwrap();
    let (out, r) = hive.json(&["gc", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    sh(&["sync"]);
    let after = apfs.volume(&image.mount).unwrap();
    let freed = r["counts"]["freed_bytes"].as_u64().unwrap();
    let gained = after.available_bytes.saturating_sub(before.available_bytes);
    // Writes by system daemons between the two samples; far below the bytes under test.
    const SLACK: u64 = 1 << 20;
    assert!(
        freed >= 32 << 20,
        "the fixture freed too little to test the bound: {freed}"
    );
    assert!(
        freed <= gained + SLACK,
        "freed {freed} > gained {gained} + slack"
    );
    assert!(
        r["counts"]["deleted_bytes_still_shared"].as_u64().unwrap() >= 32 << 20,
        "{r}"
    );
    let note = format!("container {} ", before.container);
    assert!(
        r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains(&note)),
        "{r}"
    );
}

#[test]
fn store_freed_bytes_never_exceed_what_the_volume_regained() {
    let image = Image::new();
    let hive = ImageHive::new(&image.mount);
    let apfs = Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into());
    const DAY: u64 = 86_400;

    let packed = plant_aged_entry(
        &hive.home,
        91_001,
        "imgpack-aaaaaaaaaaaaaaaa",
        "1111111111111111",
        "packed",
        mib2_text(),
        31 * DAY,
    );
    assert!(matches!(
        apfs.compress(&packed),
        cargo_hive::platform::CompressOutcome::Compressed
    ));

    let noise = plant_aged_entry(
        &hive.home,
        91_002,
        "imgnoise-bbbbbbbbbbbbbbbb",
        "2222222222222222",
        "noise",
        mib2_random(11),
        31 * DAY,
    );
    let keep = image.mount.join("clone-keep");
    fs::create_dir_all(&keep).unwrap();
    apfs.clone_file(&noise, &keep.join("noise")).unwrap();

    sh(&["sync"]);
    let before = apfs.volume(&image.mount).unwrap();
    let (out, r) = hive.json(&["gc"]);
    assert!(out.status.success(), "{r}");
    sh(&["sync"]);
    let after = apfs.volume(&image.mount).unwrap();
    let freed = r["counts"]["store_freed_bytes"].as_u64().unwrap();
    let gained = after.available_bytes.saturating_sub(before.available_bytes);
    const SLACK: u64 = 1 << 20;
    assert!(
        freed > 0,
        "the fixture freed too little to test the bound: {freed}; {r}"
    );
    assert!(
        freed <= gained + SLACK,
        "store_freed_bytes {freed} > gained {gained} + slack; {r}"
    );
    assert!(
        r["counts"]["store_bytes_still_shared"].as_u64().unwrap() >= 2 << 20,
        "{r}"
    );
}
