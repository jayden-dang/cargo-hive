//! Groups fingerprint units that are copies of one crate, and totals every target.
//!
//! `analyse` only reads units and dependency names already in memory. `Explanation::human`
//! stays empty for the caller to fill.

#![cfg_attr(not(test), allow(dead_code))]

use crate::fingerprint::{kind_of, NotCurrentUnit, Unit};
use crate::receipt::{Finding, Measured};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

struct ProfileInput {
    target_dir: PathBuf,
    profile_dir: PathBuf,
    units: Vec<Unit>,
    deps: Option<Vec<String>>,
    not_current: Vec<NotCurrentInput>,
}

struct NotCurrentInput {
    #[allow(dead_code)]
    unit: NotCurrentUnit,
    #[allow(dead_code)]
    kind_from_output: Option<String>,
    #[allow(dead_code)]
    outputs: Result<Vec<String>, String>,
    #[allow(dead_code)]
    attempted_at: Option<SystemTime>,
}

pub enum FailedUnits {
    Report,
    Off(String),
}

pub struct Limits {
    pub top: Option<usize>,
}

pub struct Explanation {
    pub findings: Vec<Finding>,
    pub counts: Vec<(&'static str, u64)>,
    pub unmeasured: Vec<(String, String)>,
    pub human: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Build,
    Check,
    Unknown,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Check => "check",
            Mode::Unknown => "unknown",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Mode::Build => 0,
            Mode::Check => 1,
            Mode::Unknown => 2,
        }
    }
}

struct Group<'a> {
    target_dir: &'a Path,
    profile_dir: &'a Path,
    dir_crate: String,
    stem: String,
    units: Vec<&'a Unit>,
    deps: Option<&'a [String]>,
}

struct FeatureRow<'a> {
    mode: Option<Mode>,
    features: &'a str,
    units: Vec<&'a Unit>,
}

#[derive(Default)]
struct TargetTotals {
    groups: u64,
    extra_units: u64,
    groups_shown: u64,
}

fn analyse(inputs: &[ProfileInput], limits: Limits, failed: &FailedUnits) -> Explanation {
    let mut groups = Vec::new();
    for input in inputs {
        groups.extend(groups_in(input));
    }
    groups.sort_by(rank_groups);

    let keep = match limits.top {
        Some(top) => top,
        None => groups.len(),
    };

    let mut by_target: BTreeMap<PathBuf, TargetTotals> = BTreeMap::new();
    for input in inputs {
        by_target.entry(input.target_dir.clone()).or_default();
    }

    let mut findings = Vec::new();
    let mut unmeasured = Vec::new();
    let mut deps_noted = BTreeSet::new();
    for (index, group) in groups.iter().enumerate() {
        let totals = by_target.entry(group.target_dir.to_path_buf()).or_default();
        totals.groups += 1;
        totals.extra_units += as_u64(group.units.len() - 1);
        if index < keep {
            totals.groups_shown += 1;
            findings.push(group_finding(group, &mut unmeasured, &mut deps_noted));
            push_feature_findings(group, &mut findings, &mut unmeasured);
        }
    }

    for (path, totals) in &by_target {
        findings.push(totals_finding(path, totals, failed));
    }

    Explanation {
        findings,
        counts: vec![
            (
                "groups",
                by_target.values().map(|totals| totals.groups).sum(),
            ),
            (
                "extra_units",
                by_target.values().map(|totals| totals.extra_units).sum(),
            ),
            (
                "groups_shown",
                by_target.values().map(|totals| totals.groups_shown).sum(),
            ),
            ("divergences", 0),
            ("divergences_shown", 0),
        ],
        unmeasured,
        human: String::new(),
    }
}

fn groups_in(input: &ProfileInput) -> Vec<Group<'_>> {
    let mut buckets: BTreeMap<(&str, &str, u64, u64), Vec<&Unit>> = BTreeMap::new();
    for unit in &input.units {
        let dir_crate = directory_crate(&unit.dir_name, &unit.hash_hex);
        buckets
            .entry((dir_crate, unit.stem.as_str(), unit.target, unit.path))
            .or_default()
            .push(unit);
    }
    buckets
        .into_iter()
        .filter(|(_, units)| units.len() >= 2)
        .map(|((dir_crate, stem, _, _), units)| Group {
            target_dir: &input.target_dir,
            profile_dir: &input.profile_dir,
            dir_crate: dir_crate.to_string(),
            stem: stem.to_string(),
            units,
            deps: input.deps.as_deref(),
        })
        .collect()
}

fn rank_groups(left: &Group<'_>, right: &Group<'_>) -> std::cmp::Ordering {
    right
        .units
        .len()
        .cmp(&left.units.len())
        .then_with(|| {
            left.profile_dir
                .to_string_lossy()
                .cmp(&right.profile_dir.to_string_lossy())
        })
        .then_with(|| left.dir_crate.cmp(&right.dir_crate))
        .then_with(|| left.stem.cmp(&right.stem))
}

fn directory_crate<'a>(dir_name: &'a str, hash_hex: &str) -> &'a str {
    match dir_name.strip_suffix(hash_hex) {
        Some(head) => head.strip_suffix('-').unwrap_or(dir_name),
        None => dir_name,
    }
}

fn crate_and_kind(stem: &str) -> (String, String) {
    let kind = kind_of(stem);
    let crate_name = if kind == "other" {
        stem.to_string()
    } else {
        let prefix = format!("{kind}-");
        stem.strip_prefix(&prefix).unwrap_or(stem).to_string()
    };
    (crate_name, kind)
}

fn unit_mode(hash_hex: &str, deps: &[String]) -> Mode {
    let mut saw_rmeta = false;
    for name in deps {
        let Some(ext) = matched_extension(name, hash_hex) else {
            continue;
        };
        if ext.is_empty() || ext == "rlib" || ext == "dylib" {
            return Mode::Build;
        }
        if ext == "rmeta" {
            saw_rmeta = true;
        }
    }
    if saw_rmeta {
        Mode::Check
    } else {
        Mode::Unknown
    }
}

fn matched_extension<'a>(name: &'a str, hash_hex: &str) -> Option<&'a str> {
    let segment = match name.rsplit_once('-') {
        Some((_, tail)) => tail,
        None => name,
    };
    let rest = segment.strip_prefix(hash_hex)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('.')
    }
}

fn group_finding(
    group: &Group<'_>,
    unmeasured: &mut Vec<(String, String)>,
    deps_noted: &mut BTreeSet<PathBuf>,
) -> Finding {
    let (crate_name, kind) = crate_and_kind(&group.stem);
    let mut measured = BTreeMap::new();
    insert_text(&mut measured, "crate", &crate_name);
    insert_text(&mut measured, "target_kind", &kind);
    insert_int(&mut measured, "copies", as_u64(group.units.len()));
    let feature_sets = group
        .units
        .iter()
        .map(|unit| unit.features.as_ref())
        .collect::<BTreeSet<_>>()
        .len();
    insert_int(&mut measured, "feature_sets", as_u64(feature_sets));
    let profile_hashes = group
        .units
        .iter()
        .map(|unit| unit.profile)
        .collect::<BTreeSet<_>>()
        .len();
    insert_int(&mut measured, "profile_hashes", as_u64(profile_hashes));
    if let Some(deps) = group.deps {
        let mut build = 0_u64;
        let mut check = 0_u64;
        let mut unknown = 0_u64;
        for unit in &group.units {
            match unit_mode(&unit.hash_hex, deps) {
                Mode::Build => build += 1,
                Mode::Check => check += 1,
                Mode::Unknown => unknown += 1,
            }
        }
        insert_int(&mut measured, "mode_build", build);
        insert_int(&mut measured, "mode_check", check);
        insert_int(&mut measured, "mode_unknown", unknown);
    } else if deps_noted.insert(group.profile_dir.to_path_buf()) {
        unmeasured.push((
            group.profile_dir.display().to_string(),
            "deps listing failed or was over its cap".to_string(),
        ));
    }
    let pairs = group
        .units
        .iter()
        .map(|unit| (unit.features.as_ref(), unit.profile))
        .collect::<BTreeSet<_>>()
        .len();
    insert_int(
        &mut measured,
        "copies_sharing_features_and_profile",
        as_u64(group.units.len() - pairs),
    );
    finding("copies.group", group.profile_dir, measured)
}

fn push_feature_findings(
    group: &Group<'_>,
    findings: &mut Vec<Finding>,
    unmeasured: &mut Vec<(String, String)>,
) {
    let (crate_name, kind) = crate_and_kind(&group.stem);
    for row in feature_rows(group) {
        let mut measured = BTreeMap::new();
        insert_text(&mut measured, "crate", &crate_name);
        insert_text(&mut measured, "target_kind", &kind);
        if let Some(mode) = row.mode {
            insert_text(&mut measured, "mode", mode.as_str());
        }
        insert_text(&mut measured, "features", row.features);
        match uniform_coverage(&row.units) {
            Some(label) => insert_text(&mut measured, "coverage", label),
            None => unmeasured.push((
                group.profile_dir.display().to_string(),
                "coverage is unreadable".to_string(),
            )),
        }
        insert_int(&mut measured, "units", as_u64(row.units.len()));
        findings.push(finding("copies.features", group.profile_dir, measured));
    }
}

fn feature_rows<'a>(group: &'a Group<'a>) -> Vec<FeatureRow<'a>> {
    let mut buckets: BTreeMap<(u8, &'a str), FeatureRow<'a>> = BTreeMap::new();
    for unit in &group.units {
        let mode = group.deps.map(|deps| unit_mode(&unit.hash_hex, deps));
        let rank = mode.map(Mode::rank).unwrap_or(3);
        buckets
            .entry((rank, unit.features.as_ref()))
            .or_insert_with(|| FeatureRow {
                mode,
                features: unit.features.as_ref(),
                units: Vec::new(),
            })
            .units
            .push(unit);
    }
    buckets.into_values().collect()
}

fn uniform_coverage(units: &[&Unit]) -> Option<&'static str> {
    let first = units.first()?;
    let label = first.coverage.as_str();
    if units.iter().all(|unit| unit.coverage.as_str() == label) {
        label
    } else {
        None
    }
}

fn totals_finding(path: &Path, totals: &TargetTotals, failed: &FailedUnits) -> Finding {
    let mut measured = BTreeMap::new();
    insert_int(&mut measured, "groups", totals.groups);
    insert_int(&mut measured, "extra_units", totals.extra_units);
    insert_int(&mut measured, "groups_shown", totals.groups_shown);
    insert_int(&mut measured, "divergences", 0);
    insert_int(&mut measured, "divergences_shown", 0);
    if matches!(failed, FailedUnits::Report) {
        insert_int(&mut measured, "failed_units", 0);
    }
    insert_int(&mut measured, "compiling_or_unknown_units", 0);
    finding("copies.totals", path, measured)
}

fn finding(kind: &str, path: &Path, measured: BTreeMap<String, Measured>) -> Finding {
    Finding {
        kind: kind.to_string(),
        path: Some(path.to_path_buf()),
        detail: String::new(),
        measured,
    }
}

fn insert_text(measured: &mut BTreeMap<String, Measured>, key: &str, value: &str) {
    measured.insert(key.to_string(), Measured::Text(value.to_string()));
}

fn insert_int(measured: &mut BTreeMap<String, Measured>, key: &str, value: u64) {
    measured.insert(key.to_string(), Measured::Int(value));
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{analyse, FailedUnits, Limits, NotCurrentInput, ProfileInput};
    use crate::fingerprint::{kind_of, Coverage, NotCurrentUnit};
    use crate::receipt::{Finding, Measured};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn unit(
        dir_crate: &str,
        stem: &str,
        hash: &str,
        features: &str,
        coverage: Coverage,
        place: (u64, u64, u64),
    ) -> crate::fingerprint::Unit {
        let (profile, target, path) = place;
        crate::fingerprint::Unit {
            dir_name: format!("{dir_crate}-{hash}"),
            kind: kind_of(stem),
            hash_hex: hash.to_string(),
            deps: Vec::new(),
            stem: stem.to_string(),
            features: Arc::from(features),
            coverage,
            profile,
            target,
            path,
        }
    }

    fn profile(
        target: &str,
        profile_dir: &str,
        units: Vec<crate::fingerprint::Unit>,
        deps: Option<Vec<String>>,
    ) -> ProfileInput {
        ProfileInput {
            target_dir: PathBuf::from(target),
            profile_dir: PathBuf::from(profile_dir),
            units,
            deps,
            not_current: Vec::new(),
        }
    }

    fn text(value: &str) -> Measured {
        Measured::Text(value.to_string())
    }

    fn num(value: u64) -> Measured {
        Measured::Int(value)
    }

    fn measured_text(finding: &Finding, key: &str) -> String {
        match finding.measured.get(key) {
            Some(Measured::Text(value)) => value.clone(),
            other => panic!("{key} is not text: {other:?}"),
        }
    }

    fn measured_int(finding: &Finding, key: &str) -> u64 {
        match finding.measured.get(key) {
            Some(Measured::Int(value)) => *value,
            other => panic!("{key} is not an int: {other:?}"),
        }
    }

    fn groups_of(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|finding| finding.kind == "copies.group")
            .collect()
    }

    fn persistence_units() -> Vec<crate::fingerprint::Unit> {
        vec![
            unit(
                "persistence",
                "lib-persistence",
                "1111111111111111",
                "[\"testkit\"]",
                Coverage::AllDeclared,
                (2, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "2222222222222222",
                "[\"testkit\"]",
                Coverage::AllDeclared,
                (2, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "3333333333333333",
                "[]",
                Coverage::None,
                (5, 4, 8),
            ),
        ]
    }

    fn persistence_deps() -> Vec<String> {
        vec![
            "libpersistence-1111111111111111.rmeta".to_string(),
            "libpersistence-1111111111111111.rlib".to_string(),
            "libpersistence-2222222222222222".to_string(),
            "libpersistence-3333333333333333.d".to_string(),
            "libpersistence-3333333333333333.rmeta".to_string(),
            "unrelated-ffffffffffffffff.rlib".to_string(),
        ]
    }

    #[test]
    fn copies_are_grouped_by_crate_stem_target_and_path() {
        let units = vec![
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000001",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000002",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000003",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000004",
                "[]",
                Coverage::None,
                (1, 10, 21),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000005",
                "[]",
                Coverage::None,
                (1, 11, 20),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "0000000000000006",
                "[]",
                Coverage::None,
                (1, 11, 20),
            ),
            unit(
                "persistence",
                "bin-persistence",
                "0000000000000007",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "persistence",
                "bin-persistence",
                "0000000000000008",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "othercrate",
                "lib-persistence",
                "0000000000000009",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "othercrate",
                "lib-persistence",
                "000000000000000a",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
        ];
        let elsewhere = vec![
            unit(
                "persistence",
                "lib-persistence",
                "000000000000000b",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "000000000000000c",
                "[]",
                Coverage::None,
                (1, 10, 20),
            ),
        ];
        let mut primary = profile("/targets/app", "/profile/a", units, Some(vec![]));
        primary.not_current.push(NotCurrentInput {
            unit: NotCurrentUnit {
                dir: PathBuf::from("/profile/a/not-current"),
                stem: None,
                features: None,
                outputs: Vec::new(),
            },
            kind_from_output: Some("lib".to_string()),
            outputs: Err("unread".to_string()),
            attempted_at: None,
        });
        let failed = FailedUnits::Report;
        let exp = analyse(
            &[
                primary,
                profile("/targets/app", "/profile/b", elsewhere, Some(vec![])),
            ],
            Limits { top: None },
            &failed,
        );

        let groups = groups_of(&exp.findings);
        assert_eq!(groups.len(), 5);
        let main: Vec<_> = groups
            .iter()
            .filter(|finding| {
                finding.path.as_deref() == Some(Path::new("/profile/a"))
                    && measured_text(finding, "crate") == "persistence"
                    && measured_text(finding, "target_kind") == "lib"
                    && measured_int(finding, "copies") == 3
            })
            .collect();
        assert_eq!(main.len(), 1);
        let bins: Vec<_> = groups
            .iter()
            .filter(|finding| measured_text(finding, "target_kind") == "bin")
            .collect();
        assert_eq!(bins.len(), 1);
        assert_eq!(measured_int(bins[0], "copies"), 2);
        assert!(groups.iter().all(|finding| {
            let copies = measured_int(finding, "copies");
            copies >= 2 && copies != 4
        }));
        assert!(exp
            .findings
            .iter()
            .all(|finding| finding.kind != "copies.divergence" && finding.kind != "unit.failed"));
        assert!(exp.human.is_empty());
    }

    #[test]
    fn a_group_reports_its_counts_under_fixed_keys() {
        let input = profile(
            "/targets/workspace",
            "/profiles/debug",
            persistence_units(),
            Some(persistence_deps()),
        );
        let failed = FailedUnits::Report;
        let exp = analyse(&[input], Limits { top: Some(10) }, &failed);
        let groups = groups_of(&exp.findings);
        assert_eq!(groups.len(), 1);
        let mut expected = BTreeMap::new();
        expected.insert("crate".to_string(), text("persistence"));
        expected.insert("target_kind".to_string(), text("lib"));
        expected.insert("copies".to_string(), num(3));
        expected.insert("feature_sets".to_string(), num(2));
        expected.insert("profile_hashes".to_string(), num(2));
        expected.insert("mode_build".to_string(), num(2));
        expected.insert("mode_check".to_string(), num(1));
        expected.insert("mode_unknown".to_string(), num(0));
        expected.insert("copies_sharing_features_and_profile".to_string(), num(1));
        assert_eq!(groups[0].measured, expected);
        assert_eq!(
            groups[0].path.as_deref(),
            Some(Path::new("/profiles/debug"))
        );
        assert_eq!(groups[0].detail, "");
        assert!(exp.human.is_empty());
    }

    #[test]
    fn feature_sets_are_reported_per_mode_with_coverage() {
        let units = vec![
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a1",
                "[\"testkit\"]",
                Coverage::AllDeclared,
                (2, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a2",
                "[\"testkit\"]",
                Coverage::AllDeclared,
                (2, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a3",
                "[]",
                Coverage::None,
                (5, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a4",
                "",
                Coverage::NotRecorded,
                (1, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a5",
                "[]",
                Coverage::None,
                (3, 4, 8),
            ),
            unit(
                "persistence",
                "lib-persistence",
                "00000000000000a6",
                "[\"other\"]",
                Coverage::Unreadable,
                (9, 4, 8),
            ),
        ];
        let deps = vec![
            "libpersistence-00000000000000a1.rmeta".to_string(),
            "libpersistence-00000000000000a1.rlib".to_string(),
            "libpersistence-00000000000000a2.dylib".to_string(),
            "libpersistence-00000000000000a5".to_string(),
            "libpersistence-00000000000000a3.d".to_string(),
            "libpersistence-00000000000000a3.rmeta".to_string(),
            "libpersistence-00000000000000a4ffff.rlib".to_string(),
            "libpersistence-00000000000000a6.rlib".to_string(),
        ];
        let failed = FailedUnits::Report;
        let exp = analyse(
            &[profile(
                "/targets/workspace",
                "/profiles/debug",
                units,
                Some(deps),
            )],
            Limits { top: None },
            &failed,
        );
        let features: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.features")
            .collect();
        let rows: Vec<_> = features
            .iter()
            .map(|finding| {
                (
                    measured_text(finding, "mode"),
                    measured_text(finding, "features"),
                    finding.measured.get("coverage").map(|value| match value {
                        Measured::Text(text) => text.clone(),
                        other => panic!("coverage is not text: {other:?}"),
                    }),
                    measured_int(finding, "units"),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("build".to_string(), "[\"other\"]".to_string(), None, 1),
                (
                    "build".to_string(),
                    "[\"testkit\"]".to_string(),
                    Some("all_declared".to_string()),
                    2
                ),
                (
                    "build".to_string(),
                    "[]".to_string(),
                    Some("none".to_string()),
                    1
                ),
                (
                    "check".to_string(),
                    "[]".to_string(),
                    Some("none".to_string()),
                    1
                ),
                (
                    "unknown".to_string(),
                    String::new(),
                    Some("not_recorded".to_string()),
                    1
                ),
            ]
        );
        assert!(features.iter().all(|finding| {
            finding.path.as_deref() == Some(Path::new("/profiles/debug"))
                && finding.detail.is_empty()
                && measured_text(finding, "crate") == "persistence"
                && measured_text(finding, "target_kind") == "lib"
        }));
        assert_eq!(exp.findings[0].kind, "copies.group");
        assert!(exp.findings[1..=5]
            .iter()
            .all(|finding| finding.kind == "copies.features"));
        assert_eq!(
            exp.unmeasured,
            vec![(
                "/profiles/debug".to_string(),
                "coverage is unreadable".to_string()
            )]
        );
    }

    #[test]
    fn the_most_copied_groups_are_listed_first_and_cut_at_the_limit() {
        let plan = [
            (8_usize, "/p/a", "solo", "lib-solo"),
            (4, "/p/a", "aaa", "lib-zzz"),
            (4, "/p/a", "mmm", "lib-aaa"),
            (4, "/p/b", "aaa", "lib-aaa"),
            (3, "/p/a", "tie", "bin-tie"),
            (3, "/p/a", "tie", "lib-tie"),
            (2, "/p/a", "c1", "lib-c1"),
            (2, "/p/a", "c2", "lib-c2"),
            (2, "/p/b", "c3", "lib-c3"),
            (2, "/p/b", "c4", "lib-c4"),
            (2, "/p/b", "c5", "lib-c5"),
            (2, "/p/b", "c6", "lib-c6"),
        ];
        let expected: Vec<(u64, String, String, String)> = [
            (8_u64, "/p/a", "solo", "lib"),
            (4, "/p/a", "zzz", "lib"),
            (4, "/p/a", "aaa", "lib"),
            (4, "/p/b", "aaa", "lib"),
            (3, "/p/a", "tie", "bin"),
            (3, "/p/a", "tie", "lib"),
            (2, "/p/a", "c1", "lib"),
            (2, "/p/a", "c2", "lib"),
            (2, "/p/b", "c3", "lib"),
            (2, "/p/b", "c4", "lib"),
            (2, "/p/b", "c5", "lib"),
            (2, "/p/b", "c6", "lib"),
        ]
        .into_iter()
        .map(|(copies, path, crate_name, kind)| {
            (
                copies,
                path.to_string(),
                crate_name.to_string(),
                kind.to_string(),
            )
        })
        .collect();
        let mut by_profile: BTreeMap<&str, Vec<crate::fingerprint::Unit>> = BTreeMap::new();
        let mut n = 1_u32;
        for (copies, profile_dir, dir_crate, stem) in plan.into_iter().rev() {
            for _ in 0..copies {
                let hash = format!("{n:016x}");
                n += 1;
                by_profile.entry(profile_dir).or_default().push(unit(
                    dir_crate,
                    stem,
                    &hash,
                    "[]",
                    Coverage::None,
                    (1, 1, 1),
                ));
            }
        }
        let inputs: Vec<_> = by_profile
            .into_iter()
            .map(|(profile_dir, units)| profile("/t", profile_dir, units, Some(vec![])))
            .collect();
        let failed = FailedUnits::Report;
        let identity = |finding: &Finding| {
            (
                measured_int(finding, "copies"),
                finding.path.as_ref().unwrap().display().to_string(),
                measured_text(finding, "crate"),
                measured_text(finding, "target_kind"),
            )
        };
        let listed = analyse(&inputs, Limits { top: Some(10) }, &failed);
        let got: Vec<_> = groups_of(&listed.findings)
            .into_iter()
            .map(identity)
            .collect();
        assert_eq!(got, expected[..10]);
        let all = analyse(&inputs, Limits { top: None }, &failed);
        let got_all: Vec<_> = groups_of(&all.findings).into_iter().map(identity).collect();
        assert_eq!(got_all, expected);
        let none = analyse(&inputs, Limits { top: Some(0) }, &failed);
        assert!(none
            .findings
            .iter()
            .all(|finding| finding.kind != "copies.group" && finding.kind != "copies.features"));
        assert!(none
            .findings
            .iter()
            .any(|finding| finding.kind == "copies.totals"));
    }

    fn target_sample() -> Vec<ProfileInput> {
        let mut high = Vec::new();
        for n in 1..=5 {
            high.push(unit(
                "shown",
                "lib-shown",
                &format!("{n:016x}"),
                "[]",
                Coverage::None,
                (1, 1, 1),
            ));
        }
        high.push(unit(
            "shown",
            "lib-shown",
            "00000000000000aa",
            "[]",
            Coverage::None,
            (1, 1, 2),
        ));
        let low_a = vec![
            unit(
                "low",
                "lib-low",
                "00000000000000b1",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "low",
                "lib-low",
                "00000000000000b2",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let low_b = vec![
            unit(
                "low",
                "lib-low",
                "00000000000000c1",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "low",
                "lib-low",
                "00000000000000c2",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let lone = vec![unit(
            "lone",
            "lib-lone",
            "00000000000000d1",
            "[]",
            Coverage::None,
            (1, 1, 1),
        )];
        vec![
            profile("/t/empty", "/p/empty", lone, Some(vec![])),
            profile("/t/low", "/p/low-b", low_b, Some(vec![])),
            profile("/t/high", "/p/high", high, Some(vec![])),
            profile("/t/low", "/p/low-a", low_a, Some(vec![])),
        ]
    }

    fn totals_map(
        groups: u64,
        extra: u64,
        shown: u64,
        failed_units: Option<u64>,
    ) -> BTreeMap<String, Measured> {
        let mut map = BTreeMap::new();
        map.insert("groups".to_string(), num(groups));
        map.insert("extra_units".to_string(), num(extra));
        map.insert("groups_shown".to_string(), num(shown));
        map.insert("divergences".to_string(), num(0));
        map.insert("divergences_shown".to_string(), num(0));
        if let Some(failed_units) = failed_units {
            map.insert("failed_units".to_string(), num(failed_units));
        }
        map.insert("compiling_or_unknown_units".to_string(), num(0));
        map
    }

    #[test]
    fn every_target_gets_totals_even_with_nothing_listed() {
        let inputs = target_sample();
        let failed = FailedUnits::Report;
        let exp = analyse(&inputs, Limits { top: Some(1) }, &failed);
        let totals: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.totals")
            .collect();
        assert_eq!(totals.len(), 3);
        assert_eq!(totals[0].path.as_deref(), Some(Path::new("/t/empty")));
        assert_eq!(totals[0].measured, totals_map(0, 0, 0, Some(0)));
        assert_eq!(totals[1].path.as_deref(), Some(Path::new("/t/high")));
        assert_eq!(totals[1].measured, totals_map(1, 4, 1, Some(0)));
        assert_eq!(totals[2].path.as_deref(), Some(Path::new("/t/low")));
        assert_eq!(totals[2].measured, totals_map(2, 2, 0, Some(0)));
        assert!(totals.iter().all(|finding| finding.detail.is_empty()));
        let first_totals = exp
            .findings
            .iter()
            .position(|finding| finding.kind == "copies.totals")
            .unwrap();
        assert!(exp.findings[..first_totals]
            .iter()
            .any(|finding| finding.kind == "copies.group"));
        assert!(exp.findings[first_totals..]
            .iter()
            .all(|finding| finding.kind == "copies.totals"));
        assert_eq!(
            exp.counts,
            vec![
                ("groups", 3),
                ("extra_units", 6),
                ("groups_shown", 1),
                ("divergences", 0),
                ("divergences_shown", 0),
            ]
        );
    }

    #[test]
    fn totals_omit_failed_units_when_reporting_is_off() {
        let exp = analyse(
            &target_sample(),
            Limits { top: Some(1) },
            &FailedUnits::Off("doctor check (g) has not run".to_string()),
        );
        let totals: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.totals")
            .collect();
        assert!(totals
            .iter()
            .all(|finding| !finding.measured.contains_key("failed_units")));
        assert_eq!(totals[1].measured, totals_map(1, 4, 1, None));
        assert!(exp.counts.iter().all(|(name, _)| *name != "failed_units"));
    }

    #[test]
    fn names_never_appear_in_keys() {
        let input = profile(
            "/targets/workspace",
            "/profiles/debug",
            persistence_units(),
            Some(persistence_deps()),
        );
        let failed = FailedUnits::Report;
        let exp = analyse(&[input], Limits { top: Some(10) }, &failed);
        let allowed_keys = [
            "crate",
            "target_kind",
            "copies",
            "feature_sets",
            "profile_hashes",
            "mode_build",
            "mode_check",
            "mode_unknown",
            "copies_sharing_features_and_profile",
            "mode",
            "features",
            "coverage",
            "units",
            "groups",
            "extra_units",
            "groups_shown",
            "divergences",
            "divergences_shown",
            "failed_units",
            "compiling_or_unknown_units",
        ];
        let mut saw_persistence = false;
        let mut saw_testkit = false;
        for finding in &exp.findings {
            for (key, value) in &finding.measured {
                assert!(
                    allowed_keys.contains(&key.as_str()),
                    "unexpected measured key {key}"
                );
                assert!(!key.contains("persistence") && !key.contains("testkit"));
                if let Measured::Text(value) = value {
                    saw_persistence |= value == "persistence";
                    saw_testkit |= value.contains("testkit");
                }
            }
        }
        assert!(saw_persistence);
        assert!(saw_testkit);
        let count_names: Vec<_> = exp.counts.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            count_names,
            [
                "groups",
                "extra_units",
                "groups_shown",
                "divergences",
                "divergences_shown",
            ]
        );
        assert!(count_names
            .iter()
            .all(|name| !name.contains("persistence") && !name.contains("testkit")));
    }

    #[test]
    fn a_failed_deps_listing_omits_modes_and_says_why() {
        let mut listed = profile(
            "/targets/workspace",
            "/profiles/debug",
            persistence_units(),
            None,
        );
        listed.units.push(unit(
            "other",
            "lib-other",
            "4444444444444444",
            "[]",
            Coverage::None,
            (1, 4, 8),
        ));
        listed.units.push(unit(
            "other",
            "lib-other",
            "5555555555555555",
            "[]",
            Coverage::None,
            (1, 4, 8),
        ));
        let empty_listing = profile(
            "/targets/workspace",
            "/profiles/empty",
            vec![
                unit(
                    "empty",
                    "lib-empty",
                    "6666666666666666",
                    "[]",
                    Coverage::None,
                    (1, 1, 1),
                ),
                unit(
                    "empty",
                    "lib-empty",
                    "7777777777777777",
                    "[]",
                    Coverage::None,
                    (1, 1, 1),
                ),
            ],
            Some(vec![]),
        );
        let unlisted = profile(
            "/targets/workspace",
            "/profiles/unread",
            vec![unit(
                "unread",
                "lib-unread",
                "8888888888888888",
                "[]",
                Coverage::None,
                (1, 1, 1),
            )],
            None,
        );
        let failed = FailedUnits::Report;
        let exp = analyse(
            &[listed, empty_listing, unlisted],
            Limits { top: None },
            &failed,
        );
        let groups = groups_of(&exp.findings);
        let missing: Vec<_> = groups
            .iter()
            .filter(|finding| finding.path.as_deref() == Some(Path::new("/profiles/debug")))
            .collect();
        assert_eq!(missing.len(), 2);
        for finding in &missing {
            assert!(!finding.measured.contains_key("mode_build"));
            assert!(!finding.measured.contains_key("mode_check"));
            assert!(!finding.measured.contains_key("mode_unknown"));
        }
        let persistence = missing
            .iter()
            .find(|finding| measured_text(finding, "crate") == "persistence")
            .unwrap();
        assert_eq!(measured_int(persistence, "copies"), 3);
        assert_eq!(measured_int(persistence, "feature_sets"), 2);
        assert_eq!(measured_int(persistence, "profile_hashes"), 2);
        assert_eq!(
            measured_int(persistence, "copies_sharing_features_and_profile"),
            1
        );
        let listed_features: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| {
                finding.kind == "copies.features"
                    && finding.path.as_deref() == Some(Path::new("/profiles/debug"))
                    && measured_text(finding, "crate") == "persistence"
            })
            .collect();
        assert!(listed_features
            .iter()
            .all(|finding| !finding.measured.contains_key("mode")));
        assert_eq!(
            listed_features
                .iter()
                .map(|finding| (
                    measured_text(finding, "features"),
                    measured_int(finding, "units")
                ))
                .collect::<Vec<_>>(),
            vec![("[\"testkit\"]".to_string(), 2), ("[]".to_string(), 1),]
        );
        let empty = groups
            .iter()
            .find(|finding| finding.path.as_deref() == Some(Path::new("/profiles/empty")))
            .unwrap();
        assert_eq!(measured_int(empty, "mode_build"), 0);
        assert_eq!(measured_int(empty, "mode_check"), 0);
        assert_eq!(measured_int(empty, "mode_unknown"), 2);
        assert_eq!(
            exp.unmeasured,
            vec![(
                "/profiles/debug".to_string(),
                "deps listing failed or was over its cap".to_string()
            )]
        );
    }
}
