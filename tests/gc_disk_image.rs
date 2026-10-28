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
