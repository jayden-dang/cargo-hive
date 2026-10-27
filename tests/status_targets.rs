mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const TAG: &str = "# This file is a cache directory tag created by cargo.";

fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn status(hive: &Hive, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["status"];
    all.extend_from_slice(args);
    let (out, receipt) = hive.json(&all);
    assert!(
        out.status.success(),
        "{receipt}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    receipt
}

/// The ownership finding (`target.owned`, `target.orphan_candidate`, `target.unresolved`) for `path`.
fn target_kind(receipt: &serde_json::Value, path: &Path) -> Option<(String, String)> {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let kind = f["kind"].as_str().unwrap();
            f["path"] == serde_json::json!(path)
                && kind.starts_with("target.")
                && kind != "target.unreadable"
        })
        .map(|f| {
            (
                f["kind"].as_str().unwrap().to_string(),
                f["detail"].as_str().unwrap().to_string(),
            )
        })
}

fn finding(receipt: &serde_json::Value, kind: &str, path: &Path) -> Option<String> {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == kind && f["path"] == serde_json::json!(path))
        .map(|f| f["detail"].as_str().unwrap().to_string())
}

fn count(receipt: &serde_json::Value, key: &str) -> Option<u64> {
    receipt["counts"][key].as_u64()
}

fn add_bin(repo: &Path) {
    fs::write(
        repo.join("src/main.rs"),
        "fn main() {\n    println!(\"{}\", fixture::answer());\n}\n",
    )
    .unwrap();
}

/// A dir Cargo would recognize: its tag line plus a profile with an (empty) `.fingerprint/`.
fn fake_target(dir: &Path) {
    fs::create_dir_all(dir.join("debug/.fingerprint")).unwrap();
    fs::write(
        dir.join("CACHEDIR.TAG"),
        format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG}\n"),
    )
    .unwrap();
}

#[test]
fn a_built_checkout_reports_its_target_as_owned_with_units_and_resolved_edges() {
    let hive = Hive::ready();
    let s = Scratch::new("owned");
    let repo = init_repo(&s.path.join("app"), "");
    add_bin(&repo);
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let target = repo.join("target");
    let (kind, detail) = target_kind(&r, &target).expect("the target is listed");
    assert_eq!(kind, "target.owned");
    assert!(detail.contains(repo.to_str().unwrap()), "{detail}");
    let profile = target.join("debug").display().to_string();
    // lib + bin: two units, and the bin's dependency on the lib is the one edge.
    assert_eq!(count(&r, &format!("units:{profile}")), Some(2));
    assert_eq!(count(&r, &format!("edges_resolved:{profile}")), Some(1));
    assert_eq!(count(&r, &format!("edges_unresolved:{profile}")), Some(0));
    assert_eq!(count(&r, &format!("edges_ambiguous:{profile}")), Some(0));
}

#[test]
fn a_cache_root_target_no_checkout_resolves_to_is_an_orphan_candidate_only_when_every_repo_is_in_scope(
) {
    let hive = Hive::ready();
    let s = Scratch::new("orphan");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_crate(&a, Some(&cache.join("old-a")));
    register(&hive, &a);
    register(&hive, &b);
    let everything = status(&hive, &[]);
    assert_eq!(
        target_kind(&everything, &cache.join("old-a")).unwrap().0,
        "target.orphan_candidate"
    );
    let scoped = status(&hive, &[a.to_str().unwrap()]);
    let (kind, detail) = target_kind(&scoped, &cache.join("old-a")).unwrap();
    assert_eq!(kind, "target.unresolved");
    assert!(detail.contains("without arguments"), "{detail}");
}

#[test]
fn naming_one_repo_twice_does_not_treat_a_cache_root_target_as_an_orphan() {
    let hive = Hive::ready();
    let s = Scratch::new("dup-arg");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    fake_target(&cache.join("unowned"));
    register(&hive, &a);
    register(&hive, &b);
    let path = a.to_str().unwrap();
    let r = status(&hive, &[path, path]);
    let (kind, detail) = target_kind(&r, &cache.join("unowned")).unwrap();
    assert_eq!(kind, "target.unresolved");
    assert!(detail.contains("without arguments"), "{detail}");
    assert_eq!(count(&r, "orphan_candidates"), Some(0), "{r}");
    assert_eq!(count(&r, "checkouts"), Some(1), "{r}");
}

#[test]
fn a_build_dir_outside_every_search_root_is_owned_and_its_artifact_dir_is_not_a_target() {
    let hive = Hive::ready();
    let s = Scratch::new("builddir");
    let repo = init_repo(&s.path.join("app"), "");
    let bd = s.path.join("elsewhere/bd");
    fs::create_dir_all(repo.join(".cargo")).unwrap();
    fs::write(
        repo.join(".cargo/config.toml"),
        format!("[build]\nbuild-dir = \"{}\"\n", bd.display()),
    )
    .unwrap();
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(target_kind(&r, &bd).unwrap().0, "target.owned");
    assert_eq!(target_kind(&r, &repo.join("target")), None);
}

#[test]
fn a_profile_in_the_newer_layout_is_unsupported_has_no_edges_and_is_recorded() {
    let hive = Hive::ready();
    let s = Scratch::new("newlayout");
    let repo = init_repo(&s.path.join("app"), "");
    let t = repo.join("weird");
    fs::create_dir_all(t.join("debug/build/fixture/0123456789abcdef/fingerprint")).unwrap();
    fs::write(t.join("CACHEDIR.TAG"), format!("{TAG}\n")).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let profile = t.join("debug");
    assert!(finding(&r, "profile.unsupported", &profile)
        .unwrap()
        .contains(".fingerprint"));
    assert_eq!(
        count(&r, &format!("edges_resolved:{}", profile.display())),
        None
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("snapshot.json")).unwrap()).unwrap();
    assert!(
        snapshot["unsupported_profiles"][profile.to_str().unwrap()].is_string(),
        "{snapshot}"
    );
}

#[test]
fn a_corrupted_fingerprint_makes_its_profile_unsupported() {
    let hive = Hive::ready();
    let s = Scratch::new("corrupt");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    let fp = repo.join("target/debug/.fingerprint");
    let unit = fs::read_dir(&fp)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("fixture-")
        })
        .unwrap();
    let json = fs::read_dir(&unit)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .unwrap();
    let text = fs::read_to_string(&json)
        .unwrap()
        .replacen('{', "{\"version\":1,", 1);
    fs::write(&json, text).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    assert!(
        finding(&r, "profile.unsupported", &repo.join("target/debug")).is_some(),
        "{r}"
    );
}

#[test]
fn an_unreadable_target_is_reported_and_the_others_are_still_listed() {
    let hive = Hive::ready();
    let s = Scratch::new("unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    fake_target(&repo.join("extra"));
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o000)).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        finding(&r, "target.unreadable", &repo.join("target"))
            .unwrap()
            .contains("ermission denied"),
        "{r}"
    );
    let field = format!("bytes:{}", repo.join("target").display());
    assert!(
        r["unmeasured"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["field"] == serde_json::json!(field)),
        "{r}"
    );
    assert!(
        r["counts"].get(&field).is_none(),
        "an unmeasured size is omitted, not reported as a number"
    );
    assert!(target_kind(&r, &repo.join("extra")).is_some());
}

#[test]
fn target_search_is_bounded_and_never_enters_node_modules_or_another_target() {
    let hive = Hive::ready();
    let s = Scratch::new("bounds");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let repo = init_repo(&s.path.join("app"), "");
    let present: Vec<PathBuf> = vec![repo.join("a/b/t3"), repo.join("outer"), cache.join("x/t2")];
    let absent: Vec<PathBuf> = vec![
        repo.join("a/b/c/t4"),
        repo.join("node_modules/t"),
        repo.join("outer/inner"),
        cache.join("x/y/t3"),
    ];
    for dir in present.iter().chain(&absent) {
        fake_target(dir);
    }
    register(&hive, &repo);
    let r = status(&hive, &[]);
    for p in &present {
        assert!(
            target_kind(&r, p).is_some(),
            "{} should be listed",
            p.display()
        );
    }
    for p in &absent {
        assert_eq!(
            target_kind(&r, p),
            None,
            "{} must not be listed",
            p.display()
        );
    }
}

#[test]
fn the_callers_target_dir_variable_is_ignored_and_noted() {
    let hive = Hive::ready();
    let s = Scratch::new("callerenv");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let out = hive
        .command(&["status", repo.to_str().unwrap(), "--json"])
        .env("CARGO_TARGET_DIR", s.path.join("elsewhere"))
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        target_kind(&r, &repo.join("target")).unwrap().0,
        "target.owned"
    );
    assert!(
        r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("CARGO_TARGET_DIR")),
        "{r}"
    );
}

#[test]
fn metadata_results_are_cached_until_a_stamped_file_changes() {
    let hive = Hive::ready();
    let s = Scratch::new("cache-stamp");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let first = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(
        (
            count(&first, "metadata_calls"),
            count(&first, "metadata_cache_hits")
        ),
        (Some(1), Some(0))
    );
    let second = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(
        (
            count(&second, "metadata_calls"),
            count(&second, "metadata_cache_hits")
        ),
        (Some(0), Some(1))
    );
    let manifest = repo.join("Cargo.toml");
    let later =
        fs::metadata(&manifest).unwrap().modified().unwrap() + std::time::Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&manifest)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let third = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(count(&third, "metadata_calls"), Some(1));
}

#[test]
fn a_different_cargo_home_is_not_a_metadata_cache_hit() {
    let hive = Hive::ready();
    let s = Scratch::new("cargo-home-cache");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let first = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(
        (
            count(&first, "metadata_calls"),
            count(&first, "metadata_cache_hits")
        ),
        (Some(1), Some(0))
    );
    let other = s.path.join("other-cargo-home");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("config.toml"),
        "[build]\ntarget-dir = \"from-other-home\"\n",
    )
    .unwrap();
    let out = hive
        .command(&["status", repo.to_str().unwrap(), "--json"])
        .env("CARGO_HOME", &other)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let second: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            count(&second, "metadata_calls"),
            count(&second, "metadata_cache_hits")
        ),
        (Some(1), Some(0)),
        "{second}"
    );
}

#[test]
fn each_target_reports_its_allocated_bytes_unless_sizes_are_skipped() {
    let hive = Hive::ready();
    let s = Scratch::new("sizes");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let expected = cargo_hive::size::allocated_bytes(&target, cargo_hive::size::MAX_DEPTH).unwrap();
    assert!(expected > 0);
    assert_eq!(
        count(&r, &format!("bytes:{}", target.display())),
        Some(expected)
    );
    let skipped = status(&hive, &[repo.to_str().unwrap(), "--no-sizes"]);
    assert!(
        skipped["counts"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| !k.starts_with("bytes")),
        "{skipped}"
    );
    assert!(skipped["unmeasured"].as_array().unwrap().iter().any(|u| {
        u["field"] == "sizes" && u["reason"].as_str().unwrap().contains("--no-sizes")
    }));
}
