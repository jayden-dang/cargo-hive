//! A gc edit rebuilds only gc and the binary; seed stays Fresh.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::repo_root;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const REBUILD: &str = "/private/tmp/hive-split/rebuild";
const REBUILD_TARGET: &str = "/private/tmp/hive-split/rebuild-target";

#[test]
#[ignore]
fn a_gc_edit_leaves_seed_fresh() {
    let root = repo_root();
    let rebuild = PathBuf::from(REBUILD);
    let target = PathBuf::from(REBUILD_TARGET);

    if rebuild.exists() {
        fs::remove_dir_all(&rebuild).expect("remove prior rebuild copy");
    }
    fs::create_dir_all(&rebuild).expect("mkdir rebuild");

    let copy = Command::new("sh")
        .arg("-c")
        .arg("git ls-files -z | tar --null -T - -cf - | tar -xf - -C /private/tmp/hive-split/rebuild")
        .current_dir(&root)
        .output()
        .expect("copy tracked tree");
    assert!(
        copy.status.success(),
        "git ls-files | tar failed: {}",
        String::from_utf8_lossy(&copy.stderr)
    );

    let warm = cargo_offline(&rebuild, &target, &["build", "-v", "--offline"]);
    assert!(
        warm.status.success(),
        "warm cargo build --offline failed: {}",
        text(&warm)
    );

    let liveness = rebuild.join("crates/gc/src/liveness.rs");
    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&liveness)
            .expect("open liveness.rs in rebuild copy");
        writeln!(file, "// rebuild-scope probe").expect("append comment");
    }

    let after_edit = cargo_offline(&rebuild, &target, &["build", "-v", "--offline"]);
    let after_edit_text = text(&after_edit);
    assert!(
        after_edit.status.success(),
        "post-edit cargo build --offline failed: {after_edit_text}"
    );
    assert!(
        after_edit_text.contains("Compiling cargo-hive-gc"),
        "expected Compiling cargo-hive-gc in:\n{after_edit_text}"
    );
    assert!(
        after_edit_text.contains("Compiling cargo-hive v"),
        "expected Compiling cargo-hive v in:\n{after_edit_text}"
    );
    assert!(
        after_edit_text.contains("Fresh cargo-hive-seed"),
        "expected Fresh cargo-hive-seed in:\n{after_edit_text}"
    );

    let no_run = cargo_offline(
        &rebuild,
        &target,
        &["test", "-p", "cargo-hive-gc", "--no-run", "-v"],
    );
    let no_run_text = text(&no_run);
    assert!(
        no_run.status.success(),
        "cargo test -p cargo-hive-gc --no-run failed: {no_run_text}"
    );
    assert!(
        !no_run_text.contains("Compiling cargo-hive-seed"),
        "unexpected Compiling cargo-hive-seed in:\n{no_run_text}"
    );
    assert!(
        !no_run_text.contains("Compiling cargo-hive v"),
        "unexpected Compiling cargo-hive v in:\n{no_run_text}"
    );
}

fn cargo_offline(cwd: &Path, target: &Path, args: &[&str]) -> std::process::Output {
    Command::new("cargo")
        .args(args)
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap_or_else(|e| panic!("spawn cargo {:?}: {e}", args))
}

fn text(out: &std::process::Output) -> String {
    let mut s = String::new();
    s.push_str(&String::from_utf8_lossy(&out.stdout));
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}
