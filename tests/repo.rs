mod common;
use common::{git, init_repo, Hive, Scratch};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn refusal(receipt: &serde_json::Value) -> String {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "refusal")
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

#[test]
fn registering_a_main_checkout_records_its_realpath_and_root_commits() {
    let hive = Hive::new();
    let s = Scratch::new("reg");
    let repo = init_repo(&s.path.join("app"), "");
    let root = git(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
    let sub = repo.join("src");
    let (out, receipt) = hive.json(&["repo", "add", sub.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(receipt["counts"]["registered"], 1);
    let (_, list) = hive.json(&["repo", "list"]);
    assert_eq!(list["scope"]["repos"], serde_json::json!([repo]));
    let text = String::from_utf8(hive.run(&["repo", "list"]).stdout).unwrap();
    assert!(
        text.contains(repo.to_str().unwrap()) && text.contains(&root),
        "{text}"
    );
    let reg: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("registry.json")).unwrap()).unwrap();
    assert_eq!(reg["repos"][0]["root_commits"], serde_json::json!([root]));
}

#[test]
fn a_cargo_manifest_two_levels_down_is_accepted_and_three_levels_down_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("depth");
    let ok = init_repo(&s.path.join("ok"), "backend/api");
    assert!(hive
        .run(&["repo", "add", ok.to_str().unwrap()])
        .status
        .success());
    let deep = init_repo(&s.path.join("deep"), "a/b/c");
    let (out, receipt) = hive.json(&["repo", "add", deep.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("Cargo.toml"), "{receipt}");
}

#[test]
fn a_linked_worktree_is_refused_and_names_its_main() {
    let hive = Hive::new();
    let s = Scratch::new("linked");
    let main = init_repo(&s.path.join("main"), "");
    git(
        &main,
        &["worktree", "add", "-q", s.path.join("wt").to_str().unwrap()],
    );
    let (out, receipt) = hive.json(&["repo", "add", s.path.join("wt").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        refusal(&receipt).contains(main.to_str().unwrap()),
        "{receipt}"
    );
}

#[test]
fn a_submodule_is_refused_and_names_its_superproject() {
    let hive = Hive::new();
    let s = Scratch::new("submodule");
    let inner = init_repo(&s.path.join("inner"), "");
    let outer = init_repo(&s.path.join("outer"), "");
    git(
        &outer,
        &["submodule", "add", "-q", inner.to_str().unwrap(), "sub"],
    );
    let (out, receipt) = hive.json(&["repo", "add", outer.join("sub").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        refusal(&receipt).contains(outer.to_str().unwrap()),
        "{receipt}"
    );
}

#[test]
fn a_failed_git_probe_is_refused_with_the_git_error() {
    let modes = [
        ("super", "superproject probe exploded", "registered"),
        ("head", "HEAD probe exploded", "no commits"),
        ("shallow", "shallow probe exploded", "shallow clone"),
    ];
    for (mode, needle, absent) in modes {
        let hive = Hive::new();
        let s = Scratch::new("git-probe");
        let repo = init_repo(&s.path.join("app"), "");
        let bin = s.path.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log = s.path.join("git.log");
        fs::write(
            bin.join("git"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HIVE_GIT_LOG\"\njoined=\"$*\"\ntop=\"$HIVE_GIT_TOP\"\ncase \"$joined\" in\n  *--show-toplevel*) printf '%s\\n' \"$top\" ;;\n  *--show-superproject-working-tree*)\n    if [ \"$HIVE_GIT_FAIL\" = super ]; then echo 'fatal: superproject probe exploded' >&2; exit 128; fi ;;\n  *--git-dir*) printf '%s\\n' \"$top/.git\" ;;\n  *--git-common-dir*) printf '%s\\n' \"$top/.git\" ;;\n  *--verify*)\n    if [ \"$HIVE_GIT_FAIL\" = head ]; then echo 'fatal: HEAD probe exploded' >&2; exit 128; fi\n    echo abc ;;\n  *--is-shallow-repository*)\n    if [ \"$HIVE_GIT_FAIL\" = shallow ]; then echo 'fatal: shallow probe exploded' >&2; exit 128; fi\n    echo false ;;\n  *rev-list*) echo abcdef0123456789abcdef0123456789abcdef01 ;;\n  *) echo \"stub git: $joined\" >&2; exit 1 ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let out = hive
            .command(&["repo", "add", repo.to_str().unwrap(), "--json"])
            .env("PATH", &bin)
            .env("HIVE_GIT_TOP", &repo)
            .env("HIVE_GIT_FAIL", mode)
            .env("HIVE_GIT_LOG", &log)
            .output()
            .unwrap();
        let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{mode}: {e}\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let why = refusal(&receipt);
        assert!(why.contains(needle), "{mode}: {why}");
        assert!(!why.contains(absent), "{mode}: {why}");
        if mode == "super" {
            let text = fs::read_to_string(&log).unwrap();
            let line = text
                .lines()
                .find(|l| l.contains("show-superproject"))
                .unwrap_or("");
            assert!(
                line.contains("--path-format=absolute"),
                "superproject call: {line}"
            );
        }
    }
}

#[test]
fn a_repository_without_commits_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("unborn");
    let dir = s.path.join("empty");
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    let (out, receipt) = hive.json(&["repo", "add", dir.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("no commits"), "{receipt}");
}

#[test]
fn a_shallow_clone_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("shallow");
    let src = init_repo(&s.path.join("src"), "");
    fs::write(src.join("README"), "x").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "second"]);
    git(
        &s.path,
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            &format!("file://{}", src.display()),
            "shallow",
        ],
    );
    let (out, receipt) = hive.json(&["repo", "add", s.path.join("shallow").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("shallow"), "{receipt}");
}

#[test]
fn a_directory_outside_git_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("nogit");
    fs::write(
        s.path.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    let (out, receipt) = hive.json(&["repo", "add", s.path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        refusal(&receipt).contains("not inside a git checkout"),
        "{receipt}"
    );
}

#[test]
fn registering_twice_leaves_the_registry_unchanged() {
    let hive = Hive::new();
    let s = Scratch::new("twice");
    let repo = init_repo(&s.path.join("app"), "");
    assert!(hive
        .run(&["repo", "add", repo.to_str().unwrap()])
        .status
        .success());
    let before = fs::read(hive.home.path.join("registry.json")).unwrap();
    let (out, receipt) = hive.json(&["repo", "add", repo.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(receipt["counts"]["registered"], 0);
    assert!(receipt["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("already registered")));
    assert_eq!(
        fs::read(hive.home.path.join("registry.json")).unwrap(),
        before
    );
}
