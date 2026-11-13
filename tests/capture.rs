//! Compare this binary's receipts, exit codes and state with the 0.1.0 binary.
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
    hive.config(r#"{"cache_roots": ["/private/tmp/hive-capture/home/no-cache"]}"#);

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
        let v: Value = serde_json::from_slice(&bytes).unwrap();
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

fn run_on_home(bin: &Path, home: &Path, args: &[&str]) -> i32 {
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
    preserve_path(&mut hive.home);
    preserve_path(&mut hive.cargo_home);
    code
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
}
