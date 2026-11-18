//! Synthetic profile-dir builders for flat and unit-dir layouts.

use cargo_hive_core::fingerprint::le_hex;
use cargo_hive_core::unit_path::{self, UnitLayout, UnitLoc};
use std::fs;
use std::path::Path;

fn unit_json(deps: &[(u64, &str)]) -> String {
    let deps: Vec<String> = deps
        .iter()
        .map(|(d, name)| format!("[1,\"{name}\",false,{d}]"))
        .collect();
    format!(
        "{{\"rustc\":11722972603611519644,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}",
        deps.join(",")
    )
}

fn name_meta(package: &str, hex: &str) -> String {
    format!("{package}-{hex}")
}

/// Writes a current unit in either layout: JSON, hash file, stamp, and named outputs.
#[allow(clippy::too_many_arguments)]
pub fn plant_unit(
    profile: &Path,
    layout: UnitLayout,
    package: &str,
    hex: &str,
    stem: &str,
    fingerprint: u64,
    deps: &[(u64, &str)],
    outputs: &[&str],
) -> UnitLoc {
    let loc = UnitLoc {
        layout,
        name_meta: name_meta(package, hex),
    };
    let fp = unit_path::fingerprint_dir(profile, &loc);
    fs::create_dir_all(&fp).unwrap();
    fs::write(
        unit_path::hash_file(profile, &loc, stem),
        le_hex(fingerprint),
    )
    .unwrap();
    fs::write(fp.join(format!("{stem}.json")), unit_json(deps)).unwrap();
    let stamp = unit_path::stamp(
        profile,
        &loc,
        cargo_hive_core::fingerprint::UnitKind::of(stem),
    );
    if let Some(parent) = stamp.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&stamp, "This file has an mtime of when this was started.").unwrap();
    let out_dir = unit_path::outputs(profile, &loc);
    if !outputs.is_empty() {
        fs::create_dir_all(&out_dir).unwrap();
    }
    for name in outputs {
        fs::write(out_dir.join(name), b"").unwrap();
    }
    loc
}

/// A 16-hex dir under a package dir with no `fingerprint/`.
pub fn plant_partial(profile: &Path, package: &str, hex: &str) {
    fs::create_dir_all(profile.join("build").join(package).join(hex)).unwrap();
}

/// A unit fingerprint dir that never compiled: start stamp and one `output-*` only.
pub fn plant_not_current(profile: &Path, layout: UnitLayout, package: &str, hex: &str, stem: &str) {
    let loc = UnitLoc {
        layout,
        name_meta: name_meta(package, hex),
    };
    let fp = unit_path::fingerprint_dir(profile, &loc);
    fs::create_dir_all(&fp).unwrap();
    fs::write(fp.join(cargo_hive_core::fingerprint::INVOKED_TIMESTAMP), "").unwrap();
    fs::write(fp.join(format!("output-{stem}")), "{}").unwrap();
}
