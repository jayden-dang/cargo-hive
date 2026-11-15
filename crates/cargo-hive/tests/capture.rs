//! Compare this binary's receipts, exit codes and state with the 0.1.0 binary.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::registry_fixture::RegistryRepo;
use common::{Hive, wait_within};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CAPTURE: &str = "/private/tmp/hive-capture";
const GOLDEN: &str = "/private/tmp/hive-capture/golden";
const WORK: &str = "/private/tmp/hive-capture/work";
const HOME: &str = "/private/tmp/hive-capture/home";
const CARGO_HOME: &str = "/private/tmp/hive-capture/cargo-home";
const REG: &str = "/private/tmp/hive-capture/reg";

const COMMAND_DEADLINE: Duration = Duration::from_secs(300);
const COPY_DEADLINE: Duration = Duration::from_secs(300);
const REPAIR_DEADLINE: Duration = Duration::from_secs(60);
const MAX_WALK_DEPTH: usize = 32;
const MAX_WALK_ENTRIES: usize = 100_000;
const FILE_READ_CAP: u64 = 512 * 1024 * 1024;

const NORMALIZE_KEYS: &[&str] = &[
    "id",
    "at",
    "ts",
    "made_at",
    "attempted_at",
    "written_at",
    "filled_at",
    "last_live_at",
    "newest_at",
    "stamp",
    "identity",
    "root_dev",
    "root_ino",
];

fn capture_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn v010_bin() -> PathBuf {
    PathBuf::from(
        std::env::var("HIVE_V010_BIN")
            .expect("HIVE_V010_BIN must point at the 0.1.0 cargo-hive binary"),
    )
}

fn new_bin() -> PathBuf {
    if let Ok(path) = std::env::var("HIVE_NEW_BIN") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_BIN_EXE_cargo-hive"))
    }
}

fn normalized(v: &Value) -> Value {
    normalize_value(v, false)
}

fn normalized_when_versions_differ(v: &Value) -> Value {
    normalize_value(v, true)
}

fn normalize_value(v: &Value, also_hive_version: bool) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                if NORMALIZE_KEYS.contains(&k.as_str())
                    || (also_hive_version && k == "hive_version")
                {
                    out.insert(k.clone(), Value::String("<n>".into()));
                } else {
                    out.insert(k.clone(), normalize_value(val, also_hive_version));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(item, also_hive_version))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn norm_for_compare(v: &Value) -> Value {
    if std::env::var_os("HIVE_NEW_BIN").is_some() {
        normalized_when_versions_differ(v)
    } else {
        normalized(v)
    }
}

fn cp_cr(src: &Path, dst: &Path) {
    let child = Command::new("cp")
        .arg("-cR")
        .arg(src)
        .arg(dst)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, COPY_DEADLINE);
    assert!(
        out.status.success(),
        "cp -cR {} {} failed with {}: {}",
        src.display(),
        dst.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repair_worktree(main: &Path, wt: &Path) {
    let child = Command::new("git")
        .args(["worktree", "repair"])
        .arg(wt)
        .current_dir(main)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, REPAIR_DEADLINE);
    assert!(
        out.status.success(),
        "git worktree repair {} failed with {}: {}",
        wt.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_cargo_home_config() {
    fs::create_dir_all(CARGO_HOME).unwrap();
    fs::write(
        Path::new(CARGO_HOME).join("config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"fixture\"\n\n[source.fixture]\nlocal-registry = \"{}\"\n",
            Path::new(REG).display()
        ),
    )
    .unwrap();
}

fn preserve_path(scratch: &mut common::Scratch) {
    let placeholder = Path::new("/private/tmp").join(format!(
        "hive-capture-drop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&placeholder).unwrap();
    scratch.path = placeholder;
}

fn capture_hive() -> Hive {
    let mut hive = Hive::new();
    let _ = fs::remove_dir_all(&hive.home.path);
    let _ = fs::remove_dir_all(&hive.cargo_home.path);
    fs::create_dir_all(HOME).unwrap();
    fs::create_dir_all(CARGO_HOME).unwrap();
    hive.home.path = PathBuf::from(HOME);
    hive.cargo_home.path = PathBuf::from(CARGO_HOME);
    hive
}

fn ensure_golden() {
    let golden_main = Path::new(GOLDEN).join("main");
    let golden_wt = Path::new(GOLDEN).join("wt");
    if golden_main.join("target").is_dir() && golden_wt.join("target").is_dir() {
        write_cargo_home_config();
        return;
    }

    let _ = fs::remove_dir_all(CAPTURE);
    fs::create_dir_all(CAPTURE).unwrap();

    let mut hive = Hive::new();
    let _ = fs::remove_dir_all(&hive.cargo_home.path);
    fs::create_dir_all(CARGO_HOME).unwrap();
    hive.cargo_home.path = PathBuf::from(CARGO_HOME);

    let repo = RegistryRepo::new(&hive);
    let wt = repo.worktree("wt");
    hive.build_crate(&repo.main, None);
    hive.build_crate(&wt, None);

    let _ = fs::remove_dir_all(REG);
    cp_cr(&repo.scratch.path.join("reg"), Path::new(REG));
    write_cargo_home_config();

    fs::create_dir_all(GOLDEN).unwrap();
    cp_cr(&repo.main, &golden_main);
    cp_cr(&wt, &golden_wt);
    repair_worktree(&golden_main, &golden_wt);

    preserve_path(&mut hive.home);
    preserve_path(&mut hive.cargo_home);
}

fn reset_work_and_home() {
    let _ = fs::remove_dir_all(WORK);
    let _ = fs::remove_dir_all(HOME);
    cp_cr(Path::new(GOLDEN), Path::new(WORK));
    repair_worktree(&Path::new(WORK).join("main"), &Path::new(WORK).join("wt"));
    fs::create_dir_all(HOME).unwrap();
}

fn run_sequence(bin: &Path) -> Vec<(String, Value, i32)> {
    reset_work_and_home();
    let mut hive = capture_hive();
    hive.config(
        r#"{"cache_roots": ["/private/tmp/hive-capture/home/no-cache"], "secondary_roots": ["/private/tmp/hive-capture/work"]}"#,
    );

    let steps: &[&[&str]] = &[
        &["doctor", "--json"],
        &[
            "repo",
            "add",
            "/private/tmp/hive-capture/work/main",
            "--json",
        ],
        &["status", "--json"],
        &[
            "status",
            "/private/tmp/hive-capture/work/main",
            "--no-sizes",
            "--json",
        ],
        &["explain", "/private/tmp/hive-capture/work/main", "--json"],
        &["gc", "--plan", "--json"],
        &["seed", "/private/tmp/hive-capture/work/wt", "--json"],
        &["repo", "list", "--json"],
    ];

    let mut out = Vec::new();
    for args in steps {
        let child = hive
            .command_with(bin, args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_within(child, COMMAND_DEADLINE);
        let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        let code = output.status.code().unwrap_or(-1);
        out.push((args.join(" "), value, code));
    }

    preserve_path(&mut hive.home);
    preserve_path(&mut hive.cargo_home);
    out
}

fn move_home(dest: &Path) {
    let _ = fs::remove_dir_all(dest);
    fs::rename(HOME, dest).unwrap();
}

fn is_json_state(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("json") | Some("jsonl")
    ) || path.file_name().and_then(|n| n.to_str()) == Some("store.db")
}

fn read_capped_bytes(path: &Path) -> Vec<u8> {
    let len = fs::metadata(path).unwrap().len();
    assert!(
        len <= FILE_READ_CAP,
        "{} is {len} bytes, over the {FILE_READ_CAP} byte cap",
        path.display()
    );
    fs::read(path).unwrap()
}

fn read_normalized_file(path: &Path) -> Value {
    let bytes = read_capped_bytes(path);
    if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        let lines: Vec<Value> = String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let v: Value = serde_json::from_str(line).unwrap();
                norm_for_compare(&v)
            })
            .collect();
        Value::Array(lines)
    } else {
        let mut v: Value = serde_json::from_slice(&bytes).unwrap();
        if let Value::Object(map) = &mut v {
            map.remove("layout");
        }
        norm_for_compare(&v)
    }
}

fn list_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut total = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        assert!(
            depth <= MAX_WALK_DEPTH,
            "walk depth bound hit at {}",
            dir.display()
        );
        let mut seen = 0usize;
        for entry in fs::read_dir(&dir).unwrap() {
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
            let entry = entry.unwrap();
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if ft.is_file() {
                files.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    files.sort();
    files
}

fn assert_homes_equal(a: &Path, b: &Path) {
    let files_a = list_files(a);
    let files_b = list_files(b);
    assert_eq!(files_a, files_b, "state file sets differ");
    for rel in files_a {
        let pa = a.join(&rel);
        let pb = b.join(&rel);
        if is_json_state(&pa) {
            assert_eq!(
                read_normalized_file(&pa),
                read_normalized_file(&pb),
                "normalized JSON differs for {}",
                rel.display()
            );
        } else {
            assert_eq!(
                read_capped_bytes(&pa),
                read_capped_bytes(&pb),
                "bytes differ for {}",
                rel.display()
            );
        }
    }
}

fn run_on_home_json(bin: &Path, home: &Path, args: &[&str]) -> (i32, Value) {
    let mut hive = Hive::new();
    let _ = fs::remove_dir_all(&hive.home.path);
    let _ = fs::remove_dir_all(&hive.cargo_home.path);
    hive.home.path = home.to_path_buf();
    hive.cargo_home.path = PathBuf::from(CARGO_HOME);
    let child = hive
        .command_with(bin, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = wait_within(child, COMMAND_DEADLINE);
    let code = output.status.code().unwrap_or(-1);
    let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    preserve_path(&mut hive.home);
    preserve_path(&mut hive.cargo_home);
    (code, value)
}

fn run_on_home(bin: &Path, home: &Path, args: &[&str]) -> i32 {
    run_on_home_json(bin, home, args).0
}

fn bin_version(bin: &Path) -> String {
    let child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, COMMAND_DEADLINE);
    assert!(
        out.status.success(),
        "{} --version failed: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("no version word from {}", bin.display()))
        .to_string()
}

#[test]
#[ignore]
fn same_inputs_give_the_same_output_and_state() {
    let _lock = capture_lock();
    let old_bin = v010_bin();
    let new_bin = new_bin();
    ensure_golden();

    let new_seq = run_sequence(&new_bin);
    let home_new = Path::new(CAPTURE).join("home-new");
    move_home(&home_new);

    let old_seq = run_sequence(&old_bin);
    let home_old = Path::new(CAPTURE).join("home-old");
    move_home(&home_old);

    assert_eq!(new_seq.len(), old_seq.len());
    for ((new_label, new_val, new_code), (old_label, old_val, old_code)) in
        new_seq.iter().zip(old_seq.iter())
    {
        assert_eq!(new_label, old_label);
        assert_eq!(
            norm_for_compare(new_val),
            norm_for_compare(old_val),
            "receipt differs for {new_label}"
        );
        assert_eq!(new_code, old_code, "exit code differs for {new_label}");
    }
    assert_homes_equal(&home_new, &home_old);
}

#[test]
#[ignore]
fn state_written_by_0_1_0_reads_unchanged() {
    let _lock = capture_lock();
    let old_bin = v010_bin();
    let new_bin = new_bin();
    ensure_golden();
    let _ = run_sequence(&old_bin);

    let home = Path::new(HOME);
    let home_a = Path::new(CAPTURE).join("home-a");
    let home_b = Path::new(CAPTURE).join("home-b");
    let _ = fs::remove_dir_all(&home_a);
    let _ = fs::remove_dir_all(&home_b);
    cp_cr(home, &home_a);
    cp_cr(home, &home_b);

    assert_eq!(run_on_home(&new_bin, &home_a, &["status", "--json"]), 0);
    assert_eq!(
        run_on_home(&new_bin, &home_a, &["gc", "--plan", "--json"]),
        0
    );
    assert_eq!(run_on_home(&old_bin, &home_b, &["status", "--json"]), 0);
    assert_eq!(
        run_on_home(&old_bin, &home_b, &["gc", "--plan", "--json"]),
        0
    );
    assert_homes_equal(&home_a, &home_b);

    let home_c = Path::new(CAPTURE).join("home-c");
    let _ = fs::remove_dir_all(&home_c);
    cp_cr(home, &home_c);
    let (seed_code, seed_receipt) = run_on_home_json(
        &new_bin,
        &home_c,
        &["seed", "/private/tmp/hive-capture/work/wt", "--json"],
    );
    assert_eq!(seed_code, 0, "seed after upgrade: {seed_receipt}");
    assert_eq!(
        seed_receipt["counts"]["fill_entries_added"], 0,
        "seed after upgrade must not refill: {seed_receipt}"
    );

    let home_d = Path::new(CAPTURE).join("home-d");
    let home_e = Path::new(CAPTURE).join("home-e");
    let _ = fs::remove_dir_all(&home_d);
    let _ = fs::remove_dir_all(&home_e);
    cp_cr(home, &home_d);
    cp_cr(home, &home_e);
    let (new_gc_code, new_gc) = run_on_home_json(&new_bin, &home_d, &["gc", "--json"]);
    let (old_gc_code, old_gc) = run_on_home_json(&old_bin, &home_e, &["gc", "--json"]);
    assert_eq!(new_gc_code, 0, "new gc --json: {new_gc}");
    assert_eq!(old_gc_code, 0, "old gc --json: {old_gc}");
    assert_eq!(
        normalized_when_versions_differ(&new_gc),
        normalized_when_versions_differ(&old_gc),
        "bare gc --json receipts differ after upgrade"
    );
    let new_version = bin_version(&new_bin);
    assert_eq!(
        new_gc["hive_version"].as_str(),
        Some(new_version.as_str()),
        "new gc receipt must carry the new binary version"
    );
}

fn fixture_bin() -> PathBuf {
    PathBuf::from(
        std::env::var("HIVE_FIXTURE_BIN")
            .expect("HIVE_FIXTURE_BIN must point at the release cargo-hive binary"),
    )
}

fn fixture_out() -> PathBuf {
    PathBuf::from(
        std::env::var("HIVE_FIXTURE_OUT")
            .expect("HIVE_FIXTURE_OUT must point at fixtures/v<release>"),
    )
}

fn run_fixture_step(hive: &Hive, bin: &Path, args: &[&str]) -> (Value, i32) {
    let child = hive
        .command_with(bin, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = wait_within(child, COMMAND_DEADLINE);
    let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(
        code,
        0,
        "{} exited {code}: {}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (value, code)
}

fn user_home_needles() -> Vec<Vec<u8>> {
    let user_home = std::env::var("HOME").expect("HOME");
    let login = Path::new(&user_home)
        .file_name()
        .and_then(|n| n.to_str())
        .expect("HOME login");
    let users_login = format!("/Users/{login}");
    let mut needles = vec![user_home.into_bytes()];
    let users_bytes = users_login.into_bytes();
    if users_bytes != needles[0] {
        needles.push(users_bytes);
    }
    needles
}

fn same_len_replacement(needle: &[u8]) -> Vec<u8> {
    let base = b"/private/tmp";
    if base.len() == needle.len() {
        return base.to_vec();
    }
    let mut out = vec![b'z'; needle.len()];
    let n = base.len().min(needle.len());
    out[..n].copy_from_slice(&base[..n]);
    if needle.first() == Some(&b'/') {
        out[0] = b'/';
    }
    out
}

fn scrub_user_paths(home: &Path) {
    let needles = user_home_needles();
    let replacements: Vec<Vec<u8>> = needles
        .iter()
        .map(|needle| same_len_replacement(needle))
        .collect();
    for rel in list_files(home) {
        let path = home.join(&rel);
        let mut bytes = read_capped_bytes(&path);
        let mut changed = false;
        for (needle, repl) in needles.iter().zip(replacements.iter()) {
            let mut start = 0usize;
            while let Some(offset) = bytes[start..]
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
            {
                let at = start + offset;
                bytes[at..at + needle.len()].copy_from_slice(repl);
                changed = true;
                start = at + needle.len();
            }
        }
        if changed {
            fs::write(&path, &bytes).unwrap();
        }
    }
}

fn assert_home_has_no_user_paths(home: &Path) {
    let needles = user_home_needles();
    for rel in list_files(home) {
        let path = home.join(&rel);
        let bytes = read_capped_bytes(&path);
        for needle in &needles {
            assert!(
                !bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_slice()),
                "{} contains a user home path",
                rel.display()
            );
        }
    }
}

fn assert_fixture_home(home: &Path) {
    let required = [
        "registry.json",
        "capabilities.json",
        "snapshot.json",
        "plans.json",
        "store.db",
        "seeds.json",
        "config.json",
    ];
    for name in required {
        let path = home.join(name);
        let meta = fs::metadata(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(meta.is_file(), "{name} is not a file");
        assert!(meta.len() > 0, "{name} is empty");
    }
    assert!(home.join("store").is_dir(), "store/ is missing");
    let plans: Value =
        serde_json::from_slice(&read_capped_bytes(&home.join("plans.json"))).unwrap();
    assert!(
        plans["plans"]
            .as_array()
            .is_some_and(|plans| !plans.is_empty()),
        "plans.json holds no plan"
    );
    let seeds: Value =
        serde_json::from_slice(&read_capped_bytes(&home.join("seeds.json"))).unwrap();
    assert!(
        seeds["targets"]
            .as_object()
            .is_some_and(|targets| !targets.is_empty()),
        "seeds.json holds no record: {seeds}"
    );
    let receipts = home.join("receipts");
    assert!(receipts.is_dir(), "receipts/ is missing");
    let receipt_files = fs::read_dir(&receipts)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    assert!(!receipt_files.is_empty(), "receipts/ holds no day file");
    assert!(
        fs::metadata(&receipt_files[0]).unwrap().len() > 0,
        "{} is empty",
        receipt_files[0].display()
    );
    assert_home_has_no_user_paths(home);
}

fn write_provenance(out: &Path) {
    let tag = std::env::var("HIVE_FIXTURE_TAG").expect("HIVE_FIXTURE_TAG");
    let commit = std::env::var("HIVE_FIXTURE_COMMIT").expect("HIVE_FIXTURE_COMMIT");
    let child = Command::new("rustc")
        .arg("-vV")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let rustc = wait_within(child, COMMAND_DEADLINE);
    assert!(
        rustc.status.success(),
        "rustc -vV failed: {}",
        String::from_utf8_lossy(&rustc.stderr)
    );
    let body = format!(
        "tag: {tag}\ncommit: {commit}\n\nrustc -vV:\n{}",
        String::from_utf8_lossy(&rustc.stdout)
    );
    fs::write(out.join("PROVENANCE"), body).unwrap();
}

#[test]
#[ignore]
fn write_fixture_home() {
    let _lock = capture_lock();
    let bin = fixture_bin();
    let out = fixture_out();
    ensure_golden();
    reset_work_and_home();
    let mut hive = capture_hive();
    hive.config(
        r#"{"cache_roots": ["/private/tmp/hive-capture/home/no-cache"], "secondary_roots": ["/private/tmp/hive-capture/work"]}"#,
    );

    let main = Path::new(WORK).join("main");
    let main_str = main.to_str().unwrap();
    run_fixture_step(&hive, &bin, &["doctor", "--json"]);
    run_fixture_step(&hive, &bin, &["repo", "add", main_str, "--json"]);
    run_fixture_step(&hive, &bin, &["status", "--json"]);

    let target = main.join("target");
    common::cargo_fixture::backdate_all(&target, common::cargo_fixture::THIRTY_DAYS);
    let manifest = main.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("version = \"0.1.0\"", "version = \"0.1.1\""),
    )
    .unwrap();
    hive.build_crate(&main, None);
    run_fixture_step(&hive, &bin, &["gc", "--plan", "--json"]);

    let wt2 = Path::new(WORK).join("wt2");
    common::git(
        &main,
        &["worktree", "add", "-q", "-b", "wt2", wt2.to_str().unwrap()],
    );
    run_fixture_step(&hive, &bin, &["seed", wt2.to_str().unwrap(), "--json"]);

    let home = Path::new(HOME);
    scrub_user_paths(home);
    assert_fixture_home(home);

    let out_home = out.join("home");
    let _ = fs::remove_dir_all(&out_home);
    fs::create_dir_all(&out).unwrap();
    cp_cr(home, &out_home);
    write_provenance(&out);

    preserve_path(&mut hive.home);
    preserve_path(&mut hive.cargo_home);
}
