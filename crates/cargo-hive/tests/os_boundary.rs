//! OS-boundary guard: only the platform crate may touch the OS.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const RULE: &str = "only the platform crate may touch the OS";
const FORBID: &str =
    "#![forbid(unsafe_code, reason = \"only the platform crate may touch the OS (ARCH-15)\")]";
const MAX_WALK_DEPTH: usize = 32;
const MAX_WALK_ENTRIES: usize = 100_000;

fn metadata() -> serde_json::Value {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("spawn cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("cargo metadata json")
}

fn members() -> Vec<(String, PathBuf, serde_json::Value)> {
    let meta = metadata();
    let ids: HashSet<&str> = meta["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|v| v.as_str().expect("member id"))
        .collect();
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| ids.contains(p["id"].as_str().expect("package id")))
        .map(|p| {
            let name = p["name"].as_str().expect("name").to_string();
            let manifest = PathBuf::from(p["manifest_path"].as_str().expect("manifest_path"));
            let root = manifest.parent().expect("manifest parent").to_path_buf();
            (name, root, p.clone())
        })
        .collect()
}

fn cfg_predicate(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("#[cfg(")?;
    let end = rest.rfind(')')?;
    Some(&rest[..end])
}

/// Line numbers (1-based) of `cfg(target_os …)` that are not test code.
fn non_test_cfg_target_os(src: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut test_exit_depth: Option<i32> = None;
    let mut pending_test = false;

    for (idx, line) in src.lines().enumerate() {
        let trimmed = line.trim();
        let in_test = test_exit_depth.is_some() || pending_test;

        if let Some(pred) = cfg_predicate(trimmed) {
            let attr_is_test = pred.contains("test");
            if attr_is_test {
                pending_test = true;
            }
            let treat_as_test = in_test || attr_is_test || pending_test;
            if pred.contains("target_os") && !treat_as_test {
                hits.push(idx + 1);
            }
            continue;
        }

        if trimmed.starts_with("#[") || trimmed.starts_with("//") || trimmed.is_empty() {
            continue;
        }

        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;

        if pending_test {
            test_exit_depth = Some(brace_depth);
            pending_test = false;
        }

        brace_depth += opens - closes;

        if let Some(exit_at) = test_exit_depth
            && brace_depth <= exit_at
        {
            test_exit_depth = None;
        }
    }
    hits
}

fn leading_inner_block(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//!") || t.starts_with("#![") {
            out.push_str(line);
            out.push('\n');
        } else {
            break;
        }
    }
    out
}

fn walk_src_rs(root: &Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut total = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        assert!(
            depth <= MAX_WALK_DEPTH,
            "walk depth bound hit at {}",
            dir.display()
        );
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
            panic!("read_dir {}: {e}", dir.display());
        });
        let mut seen = 0usize;
        for entry in entries {
            seen += 1;
            assert!(
                seen <= MAX_WALK_ENTRIES,
                "walk entry bound hit in {}",
                dir.display()
            );
            total += 1;
            assert!(
                total <= MAX_WALK_ENTRIES,
                "walk entry bound hit under {}",
                root.display()
            );
            let entry = entry.unwrap_or_else(|e| {
                panic!("read_dir entry under {}: {e}", dir.display());
            });
            let path = entry.path();
            let ft = entry.file_type().unwrap_or_else(|e| {
                panic!("file_type {}: {e}", path.display());
            });
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
}

#[test]
#[should_panic(expected = "walk depth bound hit")]
fn walk_src_rs_fails_past_depth_bound() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(format!(
        "/private/tmp/hive-it-walk-depth-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let mut cur = root.clone();
    for _ in 0..=MAX_WALK_DEPTH {
        cur.push("d");
    }
    fs::create_dir_all(&cur).unwrap();
    let mut out = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        walk_src_rs(&root, &mut out);
    }));
    let _ = fs::remove_dir_all(&root);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn non_test_cfg_target_os_cases() {
    assert_eq!(
        non_test_cfg_target_os(
            "#[cfg(test)]\nmod t {\n #[cfg(target_os = \"macos\")]\n fn f(){}\n}"
        ),
        Vec::<usize>::new()
    );
    assert_eq!(
        non_test_cfg_target_os("#[cfg(all(test, target_os = \"macos\"))]\nfn f(){}"),
        Vec::<usize>::new()
    );
    assert_eq!(
        non_test_cfg_target_os("#[cfg(test)]\n#[cfg(target_os = \"macos\")]\nmod t {\n}"),
        Vec::<usize>::new()
    );
    assert_eq!(
        non_test_cfg_target_os("#[cfg(target_os = \"macos\")]\nfn f(){}"),
        vec![1]
    );
}

#[test]
fn no_member_but_platform_depends_on_libc() {
    for (name, _, pkg) in members() {
        if name == "cargo-hive-platform" {
            continue;
        }
        for dep in pkg["dependencies"].as_array().expect("dependencies") {
            let dep_name = dep["name"].as_str().expect("dep name");
            if dep_name == "libc" && dep["kind"].is_null() {
                panic!("{RULE}: {name} depends on libc as a normal dependency");
            }
        }
    }
}

#[test]
fn no_member_but_platform_has_non_test_cfg_target_os() {
    for (name, root, _) in members() {
        if name == "cargo-hive-platform" {
            continue;
        }
        let src_root = root.join("src");
        let mut files = Vec::new();
        walk_src_rs(&src_root, &mut files);
        for path in files {
            let text =
                fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let hits = non_test_cfg_target_os(&text);
            assert!(
                hits.is_empty(),
                "{RULE}: {name} has non-test cfg(target_os) in {}: lines {hits:?}",
                path.display()
            );
        }
    }
}

#[test]
fn every_member_but_platform_forbids_unsafe() {
    for (name, _, pkg) in members() {
        if name == "cargo-hive-platform" {
            continue;
        }
        for target in pkg["targets"].as_array().expect("targets") {
            let kinds = target["kind"].as_array().expect("kind");
            let is_lib_or_bin = kinds.iter().any(|k| {
                let s = k.as_str().expect("kind str");
                s == "lib" || s == "bin"
            });
            if !is_lib_or_bin {
                continue;
            }
            let src_path = PathBuf::from(target["src_path"].as_str().expect("src_path"));
            let text = fs::read_to_string(&src_path)
                .unwrap_or_else(|e| panic!("{}: {e}", src_path.display()));
            let block = leading_inner_block(&text);
            assert!(
                block.contains(FORBID),
                "{RULE}: {name} target {} is missing the forbid attribute",
                src_path.display()
            );
        }
    }
}
