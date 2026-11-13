//! Read-only liveness report for comparing releases against a fixed target.

use cargo_hive::cargo_target::{Owner, TargetDir, identify};
use cargo_hive::fingerprint::read_profile;
use cargo_hive::gc::liveness::liveness;
use cargo_hive::liveness::Liveness;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[test]
#[ignore]
fn report() {
    let target_path =
        PathBuf::from(std::env::var("HIVE_REPORT_TARGET").expect("HIVE_REPORT_TARGET"));
    let now_secs: u64 = std::env::var("HIVE_REPORT_NOW")
        .expect("HIVE_REPORT_NOW")
        .parse()
        .expect("HIVE_REPORT_NOW must be unix seconds");
    let window_days: u64 = std::env::var("HIVE_REPORT_WINDOW_DAYS")
        .expect("HIVE_REPORT_WINDOW_DAYS")
        .parse()
        .expect("HIVE_REPORT_WINDOW_DAYS must be a number of days");
    let out = PathBuf::from(std::env::var("HIVE_REPORT_OUT").expect("HIVE_REPORT_OUT"));
    let out_str = out.to_str().expect("HIVE_REPORT_OUT must be UTF-8");
    assert!(
        out_str.starts_with("/private/tmp/"),
        "HIVE_REPORT_OUT must start with /private/tmp/, got {out_str}"
    );

    let profiles = match identify(&target_path) {
        None => panic!("{}: not a Cargo target dir", target_path.display()),
        Some(Err(why)) => panic!("{}: {why}", target_path.display()),
        Some(Ok(profiles)) => profiles,
    };

    let target = TargetDir {
        path: target_path,
        owner: Owner::Unresolved {
            reason: "report".into(),
        },
        profiles: profiles.iter().map(|(dir, _)| read_profile(dir)).collect(),
        size: None,
        unreadable: None,
    };

    let now = UNIX_EPOCH + Duration::from_secs(now_secs);
    let window = Duration::from_secs(window_days.saturating_mul(86_400));
    let platform = cargo_hive::platform::current().unwrap();
    let verdict = liveness(&*platform.fs, &target, now, window);

    let (mut live, mut dead, mut undecided) = match verdict {
        Liveness::Decided { live, dead } => {
            let live: Vec<(String, String)> = live
                .into_iter()
                .map(|u| pair(&u.profile, &u.dir_name))
                .collect();
            let dead: Vec<(String, String)> = dead
                .into_iter()
                .map(|d| pair(&d.unit.profile, &d.unit.dir_name))
                .collect();
            (live, dead, Vec::new())
        }
        Liveness::Undecided { reasons } => {
            let undecided: Vec<String> = reasons.into_iter().map(|r| r.to_string()).collect();
            (Vec::new(), Vec::new(), undecided)
        }
    };
    live.sort();
    dead.sort();
    undecided.sort();

    let body = json!({
        "live": pairs_json(&live),
        "dead": pairs_json(&dead),
        "undecided": undecided,
    });
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("create HIVE_REPORT_OUT parent");
    }
    fs::write(&out, format!("{body}\n")).expect("write HIVE_REPORT_OUT");
}

fn pair(profile: &Path, dir_name: &str) -> (String, String) {
    (profile.to_string_lossy().into_owned(), dir_name.to_owned())
}

fn pairs_json(pairs: &[(String, String)]) -> Vec<Value> {
    pairs
        .iter()
        .map(|(profile, dir_name)| json!([profile, dir_name]))
        .collect()
}
