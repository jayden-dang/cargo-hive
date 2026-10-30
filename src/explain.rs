//! Groups fingerprint units that are copies of one crate, totals every target,
//! and reports units whose last compile failed.
//!
//! `analyse` only reads values already in memory. `run` does the bounded
//! reads and fills `Explanation::human`.

use crate::cargo_target::TargetDir;
use crate::fingerprint::{
    error_code, is_summary_line, kind_of, read_capped, Format, NotCurrentUnit, Unit,
};
use crate::gc::DEPS_CAP;
use crate::platform::{
    CompressOutcome, EntryKind, EntryStat, Fs, LockHolder, LockKind, LockState, Sharing, TempDir,
    TryLock, VolumeInfo,
};
use crate::receipt::{Finding, Measured};
use crate::state::utc_timestamp;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const DIVERGENCE_DETAIL: &str = "Feature sets in one mode can differ because invocations pass different feature flags, or because dev-dependency feature unification differs between build and test.";

struct ProfileInput {
    target_dir: PathBuf,
    profile_dir: PathBuf,
    units: Vec<Unit>,
    deps: Option<Vec<String>>,
    not_current: Vec<NotCurrentInput>,
}

struct NotCurrentInput {
    unit: NotCurrentUnit,
    kind_from_output: Option<String>,
    outputs: Result<Vec<String>, String>,
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    deps: Option<&'a ModeIndex>,
}

struct FeatureRow<'a> {
    mode: Option<Mode>,
    features: &'a str,
    units: Vec<&'a Unit>,
}

struct Divergence {
    group_index: usize,
    mode: Mode,
    feature_sets: usize,
}

#[derive(Default)]
struct TargetTotals {
    groups: u64,
    extra_units: u64,
    groups_shown: u64,
    divergences: u64,
    divergences_shown: u64,
    failed_units: u64,
    compiling_or_unknown_units: u64,
}

fn analyse(inputs: &[ProfileInput], limits: Limits, failed: &FailedUnits) -> Explanation {
    let indexes: Vec<Option<ModeIndex>> = inputs
        .iter()
        .map(|input| input.deps.as_deref().map(mode_index))
        .collect();
    let mut groups = Vec::new();
    for (input, index) in inputs.iter().zip(&indexes) {
        groups.extend(groups_in(input, index.as_ref()));
    }
    groups.sort_by(rank_groups);

    let mut divergences = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        for (mode, feature_sets) in divergent_modes(group) {
            divergences.push(Divergence {
                group_index: index,
                mode,
                feature_sets,
            });
        }
    }
    divergences.sort_by(|left, right| rank_divergences(&groups, left, right));

    let groups_kept = match limits.top {
        Some(top) => top,
        None => groups.len(),
    };
    let divergences_kept = match limits.top {
        Some(top) => top,
        None => divergences.len(),
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
        if index < groups_kept {
            totals.groups_shown += 1;
            findings.push(group_finding(group, &mut unmeasured, &mut deps_noted));
            push_feature_findings(group, &mut findings, &mut unmeasured, None);
        }
    }

    for (index, divergence) in divergences.iter().enumerate() {
        let group = &groups[divergence.group_index];
        let totals = by_target.entry(group.target_dir.to_path_buf()).or_default();
        totals.divergences += 1;
        if index < divergences_kept {
            totals.divergences_shown += 1;
            findings.push(divergence_finding(
                group,
                divergence.mode,
                divergence.feature_sets,
            ));
            if divergence.group_index >= groups_kept {
                push_feature_findings(group, &mut findings, &mut unmeasured, Some(divergence.mode));
            }
        }
    }

    apply_failed_units(
        inputs,
        failed,
        &mut by_target,
        &mut findings,
        &mut unmeasured,
    );

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
            (
                "divergences",
                by_target.values().map(|totals| totals.divergences).sum(),
            ),
            (
                "divergences_shown",
                by_target
                    .values()
                    .map(|totals| totals.divergences_shown)
                    .sum(),
            ),
        ],
        unmeasured,
        human: String::new(),
    }
}

fn groups_in<'a>(input: &'a ProfileInput, index: Option<&'a ModeIndex>) -> Vec<Group<'a>> {
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
            deps: index,
        })
        .collect()
}

fn divergent_modes(group: &Group<'_>) -> Vec<(Mode, usize)> {
    let Some(deps) = group.deps else {
        return Vec::new();
    };
    let mut features_by_mode: BTreeMap<Mode, BTreeSet<&str>> = BTreeMap::new();
    for unit in &group.units {
        features_by_mode
            .entry(unit_mode(&unit.dir_name, &unit.hash_hex, deps))
            .or_default()
            .insert(unit.features.as_ref());
    }
    features_by_mode
        .into_iter()
        .filter(|(_, features)| features.len() >= 2)
        .map(|(mode, features)| (mode, features.len()))
        .collect()
}

fn rank_divergences(
    groups: &[Group<'_>],
    left: &Divergence,
    right: &Divergence,
) -> std::cmp::Ordering {
    let left_group = &groups[left.group_index];
    let right_group = &groups[right.group_index];
    right_group
        .units
        .len()
        .cmp(&left_group.units.len())
        .then_with(|| {
            left_group
                .profile_dir
                .to_string_lossy()
                .cmp(&right_group.profile_dir.to_string_lossy())
        })
        .then_with(|| left_group.dir_crate.cmp(&right_group.dir_crate))
        .then_with(|| left_group.stem.cmp(&right_group.stem))
        .then_with(|| left.mode.cmp(&right.mode))
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
    if let Some(head) = dir_name
        .strip_suffix(hash_hex)
        .and_then(|head| head.strip_suffix('-'))
    {
        return head;
    }
    match directory_suffix(dir_name) {
        Some(suffix) => &dir_name[..dir_name.len() - suffix.len() - 1],
        None => dir_name,
    }
}

fn directory_suffix(dir_name: &str) -> Option<&str> {
    let (_, tail) = dir_name.rsplit_once('-')?;
    is_lower_hex_16(tail).then_some(tail)
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

fn unit_mode(dir_name: &str, hash_hex: &str, deps: &ModeIndex) -> Mode {
    let hash = directory_suffix(dir_name).unwrap_or(hash_hex);
    match deps.get(hash) {
        Some(&(true, _)) => Mode::Build,
        Some(&(false, true)) => Mode::Check,
        _ => Mode::Unknown,
    }
}

/// What `deps/` holds for each unit hash: `(has a build artifact, has an rmeta)`.
type ModeIndex = HashMap<String, (bool, bool)>;

fn mode_index(deps: &[String]) -> ModeIndex {
    let mut index = ModeIndex::new();
    for name in deps {
        let segment = name
            .rsplit_once('-')
            .map_or(name.as_str(), |(_, tail)| tail);
        let (hash, ext) = segment.split_once('.').unwrap_or((segment, ""));
        let entry = index.entry(hash.to_string()).or_default();
        if ext.is_empty() || ext == "rlib" || ext == "dylib" {
            entry.0 = true;
        } else if ext == "rmeta" {
            entry.1 = true;
        }
    }
    index
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
            match unit_mode(&unit.dir_name, &unit.hash_hex, deps) {
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
    only_mode: Option<Mode>,
) {
    let (crate_name, kind) = crate_and_kind(&group.stem);
    for row in feature_rows(group) {
        if let Some(mode) = only_mode {
            if row.mode != Some(mode) {
                continue;
            }
        }
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
        let mode = group
            .deps
            .map(|deps| unit_mode(&unit.dir_name, &unit.hash_hex, deps));
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
    insert_int(&mut measured, "divergences", totals.divergences);
    insert_int(&mut measured, "divergences_shown", totals.divergences_shown);
    if matches!(failed, FailedUnits::Report) {
        insert_int(&mut measured, "failed_units", totals.failed_units);
    }
    insert_int(
        &mut measured,
        "compiling_or_unknown_units",
        totals.compiling_or_unknown_units,
    );
    finding("copies.totals", path, measured)
}

fn apply_failed_units(
    inputs: &[ProfileInput],
    failed: &FailedUnits,
    by_target: &mut BTreeMap<PathBuf, TargetTotals>,
    findings: &mut Vec<Finding>,
    unmeasured: &mut Vec<(String, String)>,
) {
    match failed {
        FailedUnits::Off(reason) => {
            for input in inputs {
                let totals = by_target.entry(input.target_dir.clone()).or_default();
                totals.compiling_or_unknown_units += as_u64(input.not_current.len());
            }
            unmeasured.push(("failed_units".to_string(), reason.clone()));
        }
        FailedUnits::Report => {
            for input in inputs {
                for item in &input.not_current {
                    record_not_current(item, &input.target_dir, by_target, findings, unmeasured);
                }
            }
        }
    }
}

fn record_not_current(
    item: &NotCurrentInput,
    target_dir: &Path,
    by_target: &mut BTreeMap<PathBuf, TargetTotals>,
    findings: &mut Vec<Finding>,
    unmeasured: &mut Vec<(String, String)>,
) {
    let totals = by_target.entry(target_dir.to_path_buf()).or_default();
    let texts = match &item.outputs {
        Err(reason) => {
            totals.compiling_or_unknown_units += 1;
            note_unit(unmeasured, &item.unit.dir, reason.clone());
            return;
        }
        Ok(texts) => texts,
    };
    let read = read_output_texts(texts);
    if read.unparsable {
        note_unit(unmeasured, &item.unit.dir, "a line did not parse");
    }
    if !read.failed {
        totals.compiling_or_unknown_units += 1;
        return;
    }
    totals.failed_units += 1;
    let mut measured = BTreeMap::new();
    insert_text(&mut measured, "crate", &failed_crate(item));
    insert_text(&mut measured, "target_kind", &failed_kind(item));
    match &item.unit.features {
        Some(features) => insert_text(&mut measured, "features", features),
        None => note_unit(unmeasured, &item.unit.dir, "the unit has no fingerprint"),
    }
    match item.attempted_at {
        Some(time) => insert_text(&mut measured, "attempted_at", &utc_timestamp(time)),
        None => note_unit(unmeasured, &item.unit.dir, "the timestamp is missing"),
    }
    insert_text(&mut measured, "error_codes", &read.codes);
    let mut found = finding("unit.failed", &item.unit.dir, measured);
    found.detail = "Its last compile failed.".to_string();
    findings.push(found);
}

fn note_unit(unmeasured: &mut Vec<(String, String)>, dir: &Path, reason: impl Into<String>) {
    unmeasured.push((dir.display().to_string(), reason.into()));
}

fn read_output_texts(texts: &[String]) -> OutputTexts {
    let mut failed = false;
    let mut codes = Vec::new();
    let mut unparsable = false;
    for text in texts {
        for line in text.lines() {
            match is_summary_line(line) {
                None => {
                    unparsable = true;
                    continue;
                }
                Some(is_summary) => failed |= is_summary,
            }
            if let Some(code) = error_code(line) {
                if !codes.contains(&code) {
                    codes.push(code);
                }
            }
        }
    }
    OutputTexts {
        failed,
        codes: codes.join(","),
        unparsable,
    }
}

struct OutputTexts {
    failed: bool,
    codes: String,
    unparsable: bool,
}

fn failed_crate(item: &NotCurrentInput) -> String {
    if let Some(stem) = &item.unit.stem {
        return crate_and_kind(stem).0;
    }
    let name = item
        .unit
        .dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rsplit_once('-') {
        Some((head, tail)) if is_lower_hex_16(tail) => head.to_string(),
        _ => name,
    }
}

fn is_lower_hex_16(text: &str) -> bool {
    text.len() == 16
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn failed_kind(item: &NotCurrentInput) -> String {
    if let Some(stem) = &item.unit.stem {
        return kind_of(stem);
    }
    if let Some(kind) = &item.kind_from_output {
        return kind.clone();
    }
    item.unit
        .outputs
        .iter()
        .find_map(|name| name.strip_prefix("output-"))
        .map(kind_of)
        .unwrap_or_else(|| "other".to_string())
}

fn divergence_finding(group: &Group<'_>, mode: Mode, feature_sets: usize) -> Finding {
    let (crate_name, kind) = crate_and_kind(&group.stem);
    let mut measured = BTreeMap::new();
    insert_text(&mut measured, "crate", &crate_name);
    insert_text(&mut measured, "target_kind", &kind);
    insert_text(&mut measured, "mode", mode.as_str());
    insert_int(&mut measured, "feature_sets", as_u64(feature_sets));
    let mut found = finding("copies.divergence", group.profile_dir, measured);
    found.detail = DIVERGENCE_DETAIL.to_string();
    found
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

#[derive(Debug)]
pub struct Cancelled;

pub fn run(
    targets: &[&TargetDir],
    limits: Limits,
    failed: FailedUnits,
    cancelled: &dyn Fn() -> bool,
) -> Result<Explanation, Cancelled> {
    let host = host_fs();
    run_with(host.as_ref(), DEPS_CAP, targets, limits, failed, cancelled)
}

fn host_fs() -> Box<dyn Fs> {
    #[cfg(target_os = "macos")]
    if let Ok(platform) = crate::platform::current() {
        return platform.fs;
    }
    Box::new(DirListFs)
}

struct DirListFs;

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "not supported")
}

impl Fs for DirListFs {
    fn temp_dir(&self, _parent: &Path, _prefix: &str) -> io::Result<TempDir> {
        Err(unsupported())
    }

    fn write_file(&self, _path: &Path, _bytes: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    fn read(&self, _path: &Path) -> io::Result<Vec<u8>> {
        Err(unsupported())
    }

    fn hardlink(&self, _src: &Path, _dst: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    fn set_mtime(&self, _path: &Path, _t: SystemTime) -> io::Result<()> {
        Err(unsupported())
    }

    fn mtime(&self, _path: &Path) -> io::Result<SystemTime> {
        Err(unsupported())
    }

    fn inode(&self, _path: &Path) -> io::Result<u64> {
        Err(unsupported())
    }

    fn clone_file(&self, _src: &Path, _dst: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    fn sharing(&self, _path: &Path) -> io::Result<Sharing> {
        Err(unsupported())
    }

    fn compress(&self, _path: &Path) -> CompressOutcome {
        CompressOutcome::Failed("not supported".to_string())
    }

    fn hold_lock(&self, _kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
        Err(unsupported())
    }

    fn try_lock_exclusive(&self, _path: &Path) -> TryLock {
        TryLock::Unopenable(unsupported())
    }

    fn try_lock_elsewhere(&self, _kind: LockKind, _path: &Path) -> io::Result<LockState> {
        Err(unsupported())
    }

    fn entry_stat(&self, _path: &Path) -> io::Result<EntryStat> {
        Err(unsupported())
    }

    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        read_dir_capped(dir, cap)
    }

    fn remove_tree(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    fn remove_file(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    fn volume(&self, _path: &Path) -> io::Result<VolumeInfo> {
        Err(unsupported())
    }
}

fn read_dir_capped(dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
    let mut out = Vec::new();
    for (seen, entry) in std::fs::read_dir(dir)?.enumerate() {
        if seen >= cap {
            return Err(io::Error::other(format!(
                "{} has more than {cap} entries",
                dir.display()
            )));
        }
        let entry = entry?;
        let kind = crate::platform::entry_kind(entry.file_type()?);
        if let Ok(name) = entry.file_name().into_string() {
            out.push((name, kind));
        }
    }
    Ok(out)
}

fn run_with(
    fs: &dyn Fs,
    deps_cap: usize,
    targets: &[&TargetDir],
    limits: Limits,
    failed: FailedUnits,
    cancelled: &dyn Fn() -> bool,
) -> Result<Explanation, Cancelled> {
    let inputs = profile_inputs(fs, deps_cap, targets, cancelled)?;
    let mut explanation = analyse(&inputs, limits, &failed);
    explanation.human = human_text(&explanation.findings);
    Ok(explanation)
}

fn profile_inputs(
    fs: &dyn Fs,
    deps_cap: usize,
    targets: &[&TargetDir],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<ProfileInput>, Cancelled> {
    let mut inputs = Vec::new();
    for target in targets {
        if target.unreadable.is_some() {
            continue;
        }
        for profile in &target.profiles {
            if cancelled() {
                return Err(Cancelled);
            }
            let Format::Supported = &profile.format else {
                continue;
            };
            let mut input = ProfileInput {
                target_dir: target.path.clone(),
                profile_dir: profile.path.clone(),
                units: profile
                    .graph
                    .as_ref()
                    .map(|graph| graph.units.clone())
                    .unwrap_or_default(),
                deps: None,
                not_current: Vec::new(),
            };
            if !groups_in(&input, None).is_empty() {
                if cancelled() {
                    return Err(Cancelled);
                }
                input.deps = list_deps(fs, &profile.path.join("deps"), deps_cap);
            }
            input.not_current = read_not_current(&profile.not_current_units);
            inputs.push(input);
        }
    }
    Ok(inputs)
}

fn list_deps(fs: &dyn Fs, dir: &Path, cap: usize) -> Option<Vec<String>> {
    fs.read_dir_bounded(dir, cap)
        .ok()
        .map(|entries| entries.into_iter().map(|(name, _kind)| name).collect())
}

fn read_not_current(units: &[NotCurrentUnit]) -> Vec<NotCurrentInput> {
    units
        .iter()
        .map(|unit| NotCurrentInput {
            outputs: read_outputs(&unit.dir, &unit.outputs),
            attempted_at: invoked_mtime(&unit.dir),
            kind_from_output: None,
            unit: unit.clone(),
        })
        .collect()
}

fn read_outputs(dir: &Path, names: &[String]) -> Result<Vec<String>, String> {
    let mut texts = Vec::with_capacity(names.len());
    for name in names {
        texts.push(read_output(&dir.join(name))?);
    }
    Ok(texts)
}

fn read_output(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    let bytes = read_capped(&mut file)?;
    String::from_utf8(bytes).map_err(|err| err.to_string())
}

fn invoked_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join("invoked.timestamp"))
        .and_then(|meta| meta.modified())
        .ok()
}

fn human_text(findings: &[Finding]) -> String {
    let mut text = String::new();
    for finding in findings {
        let Some(line) = human_line(finding) else {
            continue;
        };
        text.push_str(&line);
        text.push('\n');
    }
    text
}

fn human_line(finding: &Finding) -> Option<String> {
    let name = subject(finding);
    match finding.kind.as_str() {
        "copies.group" => Some(format!(
            "{name}  {} copies",
            measured_count(finding, "copies")
        )),
        "copies.divergence" => Some(format!(
            "{name}  {} feature sets",
            measured_count(finding, "feature_sets")
        )),
        "unit.failed" => Some(format!("{name}  1 failed")),
        "copies.totals" => Some(format!(
            "{name}  {} groups",
            measured_count(finding, "groups")
        )),
        _ => None,
    }
}

fn subject(finding: &Finding) -> String {
    if let Some(Measured::Text(name)) = finding.measured.get("crate") {
        return name.clone();
    }
    finding
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn measured_count(finding: &Finding, key: &str) -> u64 {
    match finding.measured.get(key) {
        Some(Measured::Int(value)) => *value,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{analyse, FailedUnits, Limits, NotCurrentInput, ProfileInput, DIVERGENCE_DETAIL};
    use crate::fingerprint::{kind_of, Coverage, NotCurrentUnit};
    use crate::receipt::{Finding, Measured};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        assert!(exp
            .findings
            .iter()
            .all(|finding| finding.kind != "copies.divergence"));
    }

    fn mode_artifact(hash: &str, mode: &str) -> String {
        let ext = match mode {
            "build" => "rlib",
            "check" => "rmeta",
            "unknown" => "d",
            other => panic!("not a mode: {other}"),
        };
        format!("artifact-{hash}.{ext}")
    }

    #[test]
    fn a_crate_with_two_feature_sets_in_one_mode_diverges() {
        let units = vec![
            unit(
                "widget",
                "lib-widget",
                "1111111111111111",
                "[\"testkit\"]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "2222222222222222",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "3333333333333333",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "4444444444444444",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let deps = vec![
            "libwidget-1111111111111111.rmeta".to_string(),
            "libwidget-2222222222222222.rmeta".to_string(),
            "libwidget-3333333333333333.rlib".to_string(),
            "libwidget-4444444444444444.rlib".to_string(),
        ];
        let no_mode = vec![
            unit(
                "widget",
                "lib-widget",
                "5555555555555555",
                "[\"testkit\"]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "6666666666666666",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let failed = FailedUnits::Report;
        let exp = analyse(
            &[
                profile("/targets/app", "/profiles/measured", units, Some(deps)),
                profile("/targets/app", "/profiles/nolist", no_mode, None),
            ],
            Limits { top: Some(10) },
            &failed,
        );

        let divergences: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.divergence")
            .collect();
        assert_eq!(divergences.len(), 1);
        assert_eq!(
            divergences[0].path.as_deref(),
            Some(Path::new("/profiles/measured"))
        );
        let mut expected = BTreeMap::new();
        expected.insert("crate".to_string(), text("widget"));
        expected.insert("target_kind".to_string(), text("lib"));
        expected.insert("mode".to_string(), text("check"));
        expected.insert("feature_sets".to_string(), num(2));
        assert_eq!(divergences[0].measured, expected);

        let measured_features: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| {
                finding.kind == "copies.features"
                    && finding.path.as_deref() == Some(Path::new("/profiles/measured"))
            })
            .map(|finding| {
                (
                    measured_text(finding, "mode"),
                    measured_text(finding, "features"),
                    measured_int(finding, "units"),
                )
            })
            .collect();
        assert_eq!(
            measured_features,
            vec![
                ("build".to_string(), "[]".to_string(), 2),
                ("check".to_string(), "[\"testkit\"]".to_string(), 1),
                ("check".to_string(), "[]".to_string(), 1),
            ]
        );

        let bare: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| {
                finding.kind == "copies.features"
                    && finding.path.as_deref() == Some(Path::new("/profiles/nolist"))
            })
            .collect();
        assert!(bare
            .iter()
            .all(|finding| !finding.measured.contains_key("mode")));
        assert_eq!(
            bare.iter()
                .map(|finding| (
                    measured_text(finding, "features"),
                    measured_int(finding, "units")
                ))
                .collect::<Vec<_>>(),
            vec![("[\"testkit\"]".to_string(), 1), ("[]".to_string(), 1)]
        );
        assert_eq!(
            exp.counts,
            vec![
                ("groups", 2),
                ("extra_units", 4),
                ("groups_shown", 2),
                ("divergences", 1),
                ("divergences_shown", 1),
            ]
        );
    }

    #[test]
    fn divergences_are_ranked_and_cut_like_groups() {
        let plans = [
            (
                "/p/a",
                "solo",
                "lib-solo",
                vec![(
                    "build",
                    vec!["[]", "[\"x\"]", "[]", "[]", "[]", "[]", "[]", "[]"],
                )],
            ),
            (
                "/p/a",
                "aaa",
                "lib-zzz",
                vec![
                    ("build", vec!["[]", "[\"x\"]"]),
                    ("check", vec!["[]", "[\"z\"]"]),
                ],
            ),
            (
                "/p/a",
                "mmm",
                "lib-aaa",
                vec![
                    ("build", vec!["[\"y\"]", "[\"y\"]"]),
                    ("check", vec!["[]", "[\"x\"]"]),
                ],
            ),
            (
                "/p/b",
                "aaa",
                "lib-aaa",
                vec![("build", vec!["[]", "[\"x\"]", "[]", "[]"])],
            ),
            (
                "/p/a",
                "tie",
                "bin-tie",
                vec![("unknown", vec!["[]", "[\"x\"]", "[]"])],
            ),
            (
                "/p/a",
                "tie",
                "lib-tie",
                vec![("build", vec!["[]", "[\"x\"]", "[]"])],
            ),
            (
                "/p/a",
                "c1",
                "lib-c1",
                vec![("build", vec!["[]", "[\"x\"]"])],
            ),
            (
                "/p/a",
                "c2",
                "lib-c2",
                vec![("check", vec!["[]", "[\"x\"]"])],
            ),
            (
                "/p/b",
                "c3",
                "lib-c3",
                vec![("unknown", vec!["[]", "[\"x\"]"])],
            ),
            (
                "/p/b",
                "c4",
                "lib-c4",
                vec![("build", vec!["[]", "[\"x\"]"])],
            ),
            (
                "/p/b",
                "c5",
                "lib-c5",
                vec![("build", vec!["[]", "[\"x\"]"])],
            ),
        ];
        let mut by_profile: BTreeMap<&str, (Vec<crate::fingerprint::Unit>, Vec<String>)> =
            BTreeMap::new();
        let mut n = 1_u32;
        for (profile_dir, dir_crate, stem, arms) in plans.into_iter().rev() {
            let entry = by_profile.entry(profile_dir).or_default();
            for (mode, features) in arms {
                for feature in features {
                    let hash = format!("{n:016x}");
                    n += 1;
                    entry.0.push(unit(
                        dir_crate,
                        stem,
                        &hash,
                        feature,
                        Coverage::None,
                        (1, 1, 1),
                    ));
                    entry.1.push(mode_artifact(&hash, mode));
                }
            }
        }
        let inputs: Vec<_> = by_profile
            .into_iter()
            .map(|(profile_dir, (units, deps))| profile("/t", profile_dir, units, Some(deps)))
            .collect();
        let failed = FailedUnits::Report;
        let identity = |finding: &Finding| {
            (
                finding.path.as_ref().unwrap().display().to_string(),
                measured_text(finding, "crate"),
                measured_text(finding, "target_kind"),
                measured_text(finding, "mode"),
                measured_int(finding, "feature_sets"),
            )
        };
        let expected = [
            ("/p/a", "solo", "lib", "build", 2),
            ("/p/a", "zzz", "lib", "build", 2),
            ("/p/a", "zzz", "lib", "check", 2),
            ("/p/a", "aaa", "lib", "check", 2),
            ("/p/b", "aaa", "lib", "build", 2),
            ("/p/a", "tie", "bin", "unknown", 2),
            ("/p/a", "tie", "lib", "build", 2),
            ("/p/a", "c1", "lib", "build", 2),
            ("/p/a", "c2", "lib", "check", 2),
            ("/p/b", "c3", "lib", "unknown", 2),
            ("/p/b", "c4", "lib", "build", 2),
            ("/p/b", "c5", "lib", "build", 2),
        ]
        .into_iter()
        .map(|(path, crate_name, kind, mode, sets)| {
            (
                path.to_string(),
                crate_name.to_string(),
                kind.to_string(),
                mode.to_string(),
                sets,
            )
        })
        .collect::<Vec<_>>();

        let listed = analyse(&inputs, Limits { top: Some(10) }, &failed);
        let got: Vec<_> = listed
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.divergence")
            .map(identity)
            .collect();
        assert_eq!(got, expected[..10]);
        let totals: Vec<_> = listed
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.totals")
            .collect();
        assert_eq!(totals.len(), 1);
        assert_eq!(measured_int(totals[0], "divergences"), 12);
        assert_eq!(measured_int(totals[0], "divergences_shown"), 10);
        assert_eq!(
            listed.counts,
            vec![
                ("groups", 11),
                ("extra_units", 25),
                ("groups_shown", 10),
                ("divergences", 12),
                ("divergences_shown", 10),
            ]
        );
        let first_divergence = listed
            .findings
            .iter()
            .position(|finding| finding.kind == "copies.divergence")
            .unwrap();
        let first_totals = listed
            .findings
            .iter()
            .position(|finding| finding.kind == "copies.totals")
            .unwrap();
        assert!(listed.findings[..first_divergence]
            .iter()
            .any(|finding| finding.kind == "copies.group"));
        assert!(listed.findings[first_divergence..first_totals]
            .iter()
            .all(|finding| finding.kind == "copies.divergence"));

        let all = analyse(&inputs, Limits { top: None }, &failed);
        let got_all: Vec<_> = all
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.divergence")
            .map(identity)
            .collect();
        assert_eq!(got_all, expected);
        assert_eq!(
            all.counts,
            vec![
                ("groups", 11),
                ("extra_units", 25),
                ("groups_shown", 11),
                ("divergences", 12),
                ("divergences_shown", 12),
            ]
        );

        let none = analyse(&inputs, Limits { top: Some(0) }, &failed);
        assert!(none.findings.iter().all(|finding| {
            finding.kind != "copies.group"
                && finding.kind != "copies.features"
                && finding.kind != "copies.divergence"
        }));
        assert_eq!(
            none.counts,
            vec![
                ("groups", 11),
                ("extra_units", 25),
                ("groups_shown", 0),
                ("divergences", 12),
                ("divergences_shown", 0),
            ]
        );
    }

    #[test]
    fn a_feature_set_finding_is_reported_once() {
        let rows = |dir_crate: &str, stem: &str, spec: &[(&str, &str)], start: u32| {
            let mut units = Vec::new();
            let mut deps = Vec::new();
            for (offset, (features, mode)) in spec.iter().enumerate() {
                let hash = format!("{:016x}", start + u32::try_from(offset).unwrap());
                units.push(unit(
                    dir_crate,
                    stem,
                    &hash,
                    features,
                    Coverage::None,
                    (1, 1, 1),
                ));
                deps.push(mode_artifact(&hash, mode));
            }
            (units, deps)
        };
        let (shown, shown_deps) = rows(
            "shown",
            "lib-shown",
            &[
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[\"testkit\"]", "check"),
                ("[]", "check"),
            ],
            0x10,
        );
        let (mid, mid_deps) = rows(
            "mid",
            "lib-mid",
            &[
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
            ],
            0x30,
        );
        let (keep, keep_deps) = rows(
            "keep",
            "lib-keep",
            &[
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
                ("[]", "build"),
            ],
            0x50,
        );
        let (cut, cut_deps) = rows(
            "cut",
            "lib-cut",
            &[
                ("[\"testkit\"]", "build"),
                ("[]", "build"),
                ("[\"other\"]", "check"),
                ("[]", "check"),
                ("[]", "unknown"),
                ("[]", "unknown"),
            ],
            0x70,
        );
        let failed = FailedUnits::Report;
        let exp = analyse(
            &[
                profile("/t", "/p/cut", cut, Some(cut_deps)),
                profile("/t", "/p/keep", keep, Some(keep_deps)),
                profile("/t", "/p/shown", shown, Some(shown_deps)),
                profile("/t", "/p/mid", mid, Some(mid_deps)),
            ],
            Limits { top: Some(3) },
            &failed,
        );

        let mark = |finding: &Finding| {
            let mode = match finding.measured.get("mode") {
                Some(Measured::Text(value)) => value.clone(),
                Some(other) => panic!("mode is not text: {other:?}"),
                None => String::new(),
            };
            let features = match finding.measured.get("features") {
                Some(Measured::Text(value)) => value.clone(),
                Some(other) => panic!("features is not text: {other:?}"),
                None => String::new(),
            };
            let count_key = match finding.kind.as_str() {
                "copies.group" => "copies",
                "copies.features" => "units",
                "copies.divergence" => "feature_sets",
                other => panic!("no count on {other}"),
            };
            (
                finding.kind.clone(),
                finding.path.as_ref().unwrap().display().to_string(),
                measured_text(finding, "crate"),
                measured_text(finding, "target_kind"),
                mode,
                features,
                measured_int(finding, count_key),
            )
        };
        let got: Vec<_> = exp.findings[..exp.findings.len() - 1]
            .iter()
            .map(mark)
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "copies.group".to_string(),
                    "/p/shown".to_string(),
                    "shown".to_string(),
                    "lib".to_string(),
                    String::new(),
                    String::new(),
                    10
                ),
                (
                    "copies.features".to_string(),
                    "/p/shown".to_string(),
                    "shown".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    "[]".to_string(),
                    8
                ),
                (
                    "copies.features".to_string(),
                    "/p/shown".to_string(),
                    "shown".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    "[\"testkit\"]".to_string(),
                    1
                ),
                (
                    "copies.features".to_string(),
                    "/p/shown".to_string(),
                    "shown".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    "[]".to_string(),
                    1
                ),
                (
                    "copies.group".to_string(),
                    "/p/mid".to_string(),
                    "mid".to_string(),
                    "lib".to_string(),
                    String::new(),
                    String::new(),
                    8
                ),
                (
                    "copies.features".to_string(),
                    "/p/mid".to_string(),
                    "mid".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    "[]".to_string(),
                    8
                ),
                (
                    "copies.group".to_string(),
                    "/p/keep".to_string(),
                    "keep".to_string(),
                    "lib".to_string(),
                    String::new(),
                    String::new(),
                    7
                ),
                (
                    "copies.features".to_string(),
                    "/p/keep".to_string(),
                    "keep".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    "[]".to_string(),
                    7
                ),
                (
                    "copies.divergence".to_string(),
                    "/p/shown".to_string(),
                    "shown".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    String::new(),
                    2
                ),
                (
                    "copies.divergence".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    String::new(),
                    2
                ),
                (
                    "copies.features".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    "[\"testkit\"]".to_string(),
                    1
                ),
                (
                    "copies.features".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "build".to_string(),
                    "[]".to_string(),
                    1
                ),
                (
                    "copies.divergence".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    String::new(),
                    2
                ),
                (
                    "copies.features".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    "[\"other\"]".to_string(),
                    1
                ),
                (
                    "copies.features".to_string(),
                    "/p/cut".to_string(),
                    "cut".to_string(),
                    "lib".to_string(),
                    "check".to_string(),
                    "[]".to_string(),
                    1
                ),
            ]
        );
        assert_eq!(exp.findings.last().unwrap().kind, "copies.totals");
        assert!(exp.unmeasured.is_empty());
        assert_eq!(
            exp.counts,
            vec![
                ("groups", 4),
                ("extra_units", 27),
                ("groups_shown", 3),
                ("divergences", 3),
                ("divergences_shown", 3),
            ]
        );
    }

    #[test]
    fn the_divergence_detail_names_both_sources_and_no_tool() {
        assert_eq!(
            DIVERGENCE_DETAIL,
            "Feature sets in one mode can differ because invocations pass different feature flags, or because dev-dependency feature unification differs between build and test."
        );
        let units = vec![
            unit(
                "widget",
                "lib-widget",
                "aaaaaaaaaaaaaaaa",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "bbbbbbbbbbbbbbbb",
                "[\"a\"]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "cccccccccccccccc",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "dddddddddddddddd",
                "[\"b\"]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let deps = vec![
            "libwidget-aaaaaaaaaaaaaaaa.rlib".to_string(),
            "libwidget-bbbbbbbbbbbbbbbb.rlib".to_string(),
            "libwidget-cccccccccccccccc.rmeta".to_string(),
            "libwidget-dddddddddddddddd.rmeta".to_string(),
        ];
        let exp = analyse(
            &[profile(
                "/targets/app",
                "/profiles/measured",
                units,
                Some(deps),
            )],
            Limits { top: None },
            &FailedUnits::Report,
        );
        let divergences: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.divergence")
            .collect();
        assert!(divergences.len() >= 2);
        assert!(divergences
            .iter()
            .all(|finding| finding.detail == DIVERGENCE_DETAIL));
    }

    fn attempted_stamp() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn error_line(code: &str) -> String {
        format!(r#"{{"level":"error","code":{{"code":"{code}"}},"message":"mismatched types"}}"#)
    }

    fn summary_line() -> String {
        r#"{"level":"error","code":null,"message":"aborting due to 1 previous error"}"#.to_string()
    }

    fn note_line() -> String {
        r#"{"level":"failure-note","code":null,"message":"For more information about this error, try `rustc --explain E0308`."}"#
            .to_string()
    }

    fn warning_line() -> String {
        r#"{"level":"warning","code":null,"message":"unused variable"}"#.to_string()
    }

    fn not_current(
        dir: &str,
        stem: Option<&str>,
        features: Option<&str>,
        kind_from_output: Option<&str>,
        output_names: &[&str],
        outputs: Result<Vec<String>, &str>,
        attempted_at: Option<SystemTime>,
    ) -> NotCurrentInput {
        NotCurrentInput {
            unit: NotCurrentUnit {
                dir: PathBuf::from(dir),
                stem: stem.map(str::to_string),
                features: features.map(Arc::from),
                outputs: output_names
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
            },
            kind_from_output: kind_from_output.map(str::to_string),
            outputs: outputs.map_err(str::to_string),
            attempted_at,
        }
    }

    fn failed_of(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|finding| finding.kind == "unit.failed")
            .collect()
    }

    fn profile_with(
        target: &str,
        profile_dir: &str,
        not_current_units: Vec<NotCurrentInput>,
    ) -> ProfileInput {
        let mut input = profile(target, profile_dir, Vec::new(), Some(Vec::new()));
        input.not_current = not_current_units;
        input
    }

    fn profile_with_a_failed_compile() -> ProfileInput {
        let copies = vec![
            unit(
                "shown",
                "lib-shown",
                "1111111111111111",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "shown",
                "lib-shown",
                "2222222222222222",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
        ];
        let mut input = profile("/targets/app", "/profiles/debug", copies, Some(Vec::new()));
        input.not_current.push(not_current(
            "/profiles/debug/.fingerprint/not-the-crate-aaaaaaaaaaaaaaaa",
            Some("lib-widget"),
            Some("[\"testkit\"]"),
            None,
            &["output-lib-widget"],
            Ok(vec![format!(
                "{}\n{}\n{}\n",
                error_line("E0308"),
                summary_line(),
                note_line()
            )]),
            Some(attempted_stamp()),
        ));
        input
    }

    #[test]
    fn a_summary_line_anywhere_makes_a_unit_failed() {
        let failed = FailedUnits::Report;
        let listed = analyse(
            &[profile_with_a_failed_compile()],
            Limits { top: Some(10) },
            &failed,
        );
        let found = failed_of(&listed.findings);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].path.as_deref(),
            Some(Path::new(
                "/profiles/debug/.fingerprint/not-the-crate-aaaaaaaaaaaaaaaa"
            ))
        );
        assert_eq!(found[0].detail, "Its last compile failed.");
        let mut expected = BTreeMap::new();
        expected.insert("crate".to_string(), text("widget"));
        expected.insert("target_kind".to_string(), text("lib"));
        expected.insert("features".to_string(), text("[\"testkit\"]"));
        expected.insert("attempted_at".to_string(), text("2023-11-14T22:13:20Z"));
        expected.insert("error_codes".to_string(), text("E0308"));
        assert_eq!(found[0].measured, expected);
        assert_eq!(
            listed
                .findings
                .iter()
                .map(|finding| finding.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "copies.group",
                "copies.features",
                "unit.failed",
                "copies.totals"
            ]
        );
        assert_eq!(
            listed
                .findings
                .iter()
                .find(|finding| finding.kind == "copies.totals")
                .unwrap()
                .measured,
            totals_map(1, 1, 1, Some(1))
        );
        assert!(listed.unmeasured.is_empty());
        assert_eq!(
            listed.counts,
            vec![
                ("groups", 1),
                ("extra_units", 1),
                ("groups_shown", 1),
                ("divergences", 0),
                ("divergences_shown", 0),
            ]
        );
        let cut = analyse(
            &[profile_with_a_failed_compile()],
            Limits { top: Some(0) },
            &failed,
        );
        assert!(cut
            .findings
            .iter()
            .all(|finding| finding.kind != "copies.group" && finding.kind != "copies.features"));
        assert_eq!(failed_of(&cut.findings).len(), 1);
        assert_eq!(
            cut.findings
                .iter()
                .find(|finding| finding.kind == "copies.totals")
                .unwrap()
                .measured,
            totals_map(1, 1, 0, Some(1))
        );
    }

    #[test]
    fn error_codes_are_distinct_non_null_and_in_first_seen_order() {
        let input = profile_with(
            "/targets/app",
            "/profiles/debug",
            vec![
                not_current(
                    "/units/ordered",
                    Some("lib-ordered"),
                    Some("[\"a\"]"),
                    None,
                    &["output-lib-ordered"],
                    Ok(vec![
                        error_line("dead_code"),
                        error_line("E0308"),
                        error_line("dead_code"),
                        summary_line(),
                    ]),
                    Some(attempted_stamp()),
                ),
                not_current(
                    "/units/summary-only",
                    Some("lib-summary"),
                    Some(""),
                    None,
                    &["output-lib-summary"],
                    Ok(vec![summary_line()]),
                    Some(attempted_stamp()),
                ),
            ],
        );
        let exp = analyse(&[input], Limits { top: None }, &FailedUnits::Report);
        let found = failed_of(&exp.findings);
        assert_eq!(found.len(), 2);
        assert_eq!(measured_text(found[0], "error_codes"), "dead_code,E0308");
        assert_eq!(measured_text(found[1], "error_codes"), "");
        assert_eq!(measured_text(found[1], "features"), "");
        assert_eq!(
            found
                .iter()
                .map(|finding| finding.path.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some(Path::new("/units/ordered")),
                Some(Path::new("/units/summary-only"))
            ]
        );
    }

    #[test]
    fn a_never_compiled_unit_names_its_kind_from_the_output_file_and_omits_features() {
        let input = profile_with(
            "/targets/app",
            "/profiles/debug",
            vec![
                not_current(
                    "/fp/widget-0123456789abcdef",
                    None,
                    None,
                    None,
                    &["dep-lib-x", "output-lib-x"],
                    Ok(vec![summary_line()]),
                    Some(attempted_stamp()),
                ),
                not_current(
                    "/fp/helper",
                    None,
                    Some("[]"),
                    Some("bin"),
                    &["output-lib-x"],
                    Ok(vec![summary_line()]),
                    Some(attempted_stamp()),
                ),
                not_current(
                    "/fp/custom-tool",
                    None,
                    Some("[]"),
                    None,
                    &[],
                    Ok(vec![summary_line()]),
                    Some(attempted_stamp()),
                ),
            ],
        );
        let exp = analyse(&[input], Limits { top: None }, &FailedUnits::Report);
        let found = failed_of(&exp.findings);
        assert_eq!(found.len(), 3);
        assert_eq!(measured_text(found[0], "crate"), "widget");
        assert_eq!(measured_text(found[0], "target_kind"), "lib");
        assert!(!found[0].measured.contains_key("features"));
        assert_eq!(
            measured_text(found[0], "attempted_at"),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(measured_text(found[0], "error_codes"), "");
        assert_eq!(
            found[0].path.as_deref(),
            Some(Path::new("/fp/widget-0123456789abcdef"))
        );
        assert_eq!(measured_text(found[1], "crate"), "helper");
        assert_eq!(measured_text(found[1], "target_kind"), "bin");
        assert_eq!(measured_text(found[1], "features"), "[]");
        assert_eq!(measured_text(found[2], "crate"), "custom-tool");
        assert_eq!(measured_text(found[2], "target_kind"), "other");
        assert_eq!(
            exp.unmeasured,
            vec![(
                "/fp/widget-0123456789abcdef".to_string(),
                "the unit has no fingerprint".to_string()
            )]
        );
        assert_eq!(
            measured_int(
                exp.findings
                    .iter()
                    .find(|finding| finding.kind == "copies.totals")
                    .unwrap(),
                "failed_units"
            ),
            3
        );
    }

    #[test]
    fn a_unit_without_a_summary_line_is_only_counted() {
        let warnings = profile_with(
            "/t/warn",
            "/p/warn",
            vec![not_current(
                "/units/warning",
                Some("lib-warning"),
                Some("[]"),
                None,
                &["output-lib-warning"],
                Ok(vec![warning_line()]),
                Some(attempted_stamp()),
            )],
        );
        let empty = profile_with(
            "/t/empty",
            "/p/empty",
            vec![not_current(
                "/units/empty",
                Some("lib-empty"),
                Some("[]"),
                None,
                &[],
                Ok(vec![]),
                Some(attempted_stamp()),
            )],
        );
        let exp = analyse(
            &[warnings, empty],
            Limits { top: None },
            &FailedUnits::Report,
        );
        assert!(failed_of(&exp.findings).is_empty());
        assert!(exp.unmeasured.is_empty());
        for target in ["/t/empty", "/t/warn"] {
            let totals = exp
                .findings
                .iter()
                .find(|finding| {
                    finding.kind == "copies.totals"
                        && finding.path.as_deref() == Some(Path::new(target))
                })
                .unwrap();
            assert_eq!(measured_int(totals, "failed_units"), 0);
            assert_eq!(measured_int(totals, "compiling_or_unknown_units"), 1);
        }
    }

    #[test]
    fn an_unparsable_line_is_skipped_and_reported() {
        let dir = "/units/broken";
        let input = profile_with(
            "/targets/app",
            "/profiles/debug",
            vec![not_current(
                dir,
                Some("lib-broken"),
                Some("[]"),
                None,
                &["output-lib-broken"],
                Ok(vec!["not json".to_string(), summary_line()]),
                Some(attempted_stamp()),
            )],
        );
        let exp = analyse(&[input], Limits { top: None }, &FailedUnits::Report);
        let found = failed_of(&exp.findings);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some(Path::new(dir)));
        assert_eq!(measured_text(found[0], "error_codes"), "");
        assert_eq!(
            measured_int(
                exp.findings
                    .iter()
                    .find(|finding| finding.kind == "copies.totals")
                    .unwrap(),
                "compiling_or_unknown_units"
            ),
            0
        );
        assert_eq!(
            exp.unmeasured,
            vec![(dir.to_string(), "a line did not parse".to_string())]
        );
    }

    #[test]
    fn an_unreadable_output_or_missing_stamp_is_handled() {
        let input = profile_with(
            "/targets/app",
            "/profiles/debug",
            vec![
                not_current(
                    "/units/reading",
                    Some("lib-reading"),
                    None,
                    None,
                    &["output-lib-reading"],
                    Err("cannot read output-lib-reading"),
                    None,
                ),
                not_current(
                    "/units/stamped",
                    Some("lib-stamped"),
                    Some("[]"),
                    None,
                    &["output-lib-stamped"],
                    Ok(vec![error_line("E0308"), summary_line()]),
                    None,
                ),
            ],
        );
        let exp = analyse(&[input], Limits { top: None }, &FailedUnits::Report);
        let found = failed_of(&exp.findings);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some(Path::new("/units/stamped")));
        assert!(!found[0].measured.contains_key("attempted_at"));
        assert_eq!(measured_text(found[0], "crate"), "stamped");
        assert_eq!(measured_text(found[0], "features"), "[]");
        assert_eq!(measured_text(found[0], "error_codes"), "E0308");
        let totals = exp
            .findings
            .iter()
            .find(|finding| finding.kind == "copies.totals")
            .unwrap();
        assert_eq!(measured_int(totals, "failed_units"), 1);
        assert_eq!(measured_int(totals, "compiling_or_unknown_units"), 1);
        assert_eq!(
            exp.unmeasured,
            vec![
                (
                    "/units/reading".to_string(),
                    "cannot read output-lib-reading".to_string()
                ),
                (
                    "/units/stamped".to_string(),
                    "the timestamp is missing".to_string()
                ),
            ]
        );
    }

    #[test]
    fn failed_units_are_not_named_while_reporting_is_off() {
        let reason = "doctor check (g) has not run; the active toolchain has no result";
        let named = profile_with(
            "/targets/app",
            "/profiles/a",
            vec![not_current(
                "/units/would-fail",
                Some("lib-widget"),
                Some("[]"),
                None,
                &["output-lib-widget"],
                Ok(vec!["not json".to_string(), summary_line()]),
                Some(attempted_stamp()),
            )],
        );
        let unread = profile_with(
            "/targets/app",
            "/profiles/b",
            vec![not_current(
                "/units/unread",
                Some("lib-other"),
                None,
                None,
                &["output-lib-other"],
                Err("cannot read output-lib-other"),
                None,
            )],
        );
        let exp = analyse(
            &[named, unread],
            Limits { top: Some(0) },
            &FailedUnits::Off(reason.to_string()),
        );
        assert!(failed_of(&exp.findings).is_empty());
        let totals: Vec<_> = exp
            .findings
            .iter()
            .filter(|finding| finding.kind == "copies.totals")
            .collect();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].path.as_deref(), Some(Path::new("/targets/app")));
        assert!(!totals[0].measured.contains_key("failed_units"));
        assert_eq!(measured_int(totals[0], "compiling_or_unknown_units"), 2);
        assert_eq!(
            exp.unmeasured,
            vec![("failed_units".to_string(), reason.to_string())]
        );
        assert!(exp
            .counts
            .iter()
            .all(|(name, _)| *name != "failed_units" && *name != "compiling_or_unknown_units"));
    }

    #[test]
    fn a_failed_unit_is_described_only_by_its_last_compile() {
        let first = profile_with(
            "/targets/app",
            "/profiles/a",
            vec![not_current(
                "/units/first",
                Some("lib-first"),
                Some("[]"),
                None,
                &["output-lib-first"],
                Ok(vec![error_line("E0308"), summary_line()]),
                Some(attempted_stamp()),
            )],
        );
        let second = profile_with(
            "/targets/app",
            "/profiles/b",
            vec![not_current(
                "/units/second",
                Some("bin-second"),
                Some(""),
                None,
                &["output-bin-second"],
                Ok(vec![summary_line()]),
                Some(attempted_stamp()),
            )],
        );
        let exp = analyse(
            &[first, second],
            Limits { top: Some(0) },
            &FailedUnits::Report,
        );
        let found = failed_of(&exp.findings);
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|finding| finding.detail == "Its last compile failed."));
        assert_eq!(
            found
                .iter()
                .map(|finding| finding.path.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some(Path::new("/units/first")),
                Some(Path::new("/units/second"))
            ]
        );
        assert_eq!(
            exp.findings
                .iter()
                .map(|finding| finding.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["unit.failed", "unit.failed", "copies.totals"]
        );
        assert_eq!(measured_int(&exp.findings[2], "failed_units"), 2);
        assert_eq!(
            measured_int(&exp.findings[2], "compiling_or_unknown_units"),
            0
        );
    }

    struct DirGuard(PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sized_summary(total: usize) -> Vec<u8> {
        let prefix =
            br#"{"level":"error","message":"aborting due to 1 previous error","code":null,"pad":""#;
        let suffix = br#""}"#;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(prefix);
        bytes.resize(total - suffix.len(), b'a');
        bytes.extend_from_slice(suffix);
        bytes
    }

    struct FakeFs;

    fn unused_io() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Unsupported, "not used by explain")
    }

    impl crate::platform::Fs for FakeFs {
        fn temp_dir(&self, _: &Path, _: &str) -> std::io::Result<crate::platform::TempDir> {
            Err(unused_io())
        }
        fn write_file(&self, _: &Path, _: &[u8]) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn read(&self, _: &Path) -> std::io::Result<Vec<u8>> {
            Err(unused_io())
        }
        fn hardlink(&self, _: &Path, _: &Path) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn set_mtime(&self, _: &Path, _: SystemTime) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn mtime(&self, _: &Path) -> std::io::Result<SystemTime> {
            Err(unused_io())
        }
        fn inode(&self, _: &Path) -> std::io::Result<u64> {
            Err(unused_io())
        }
        fn clone_file(&self, _: &Path, _: &Path) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn sharing(&self, _: &Path) -> std::io::Result<crate::platform::Sharing> {
            Err(unused_io())
        }
        fn compress(&self, _: &Path) -> crate::platform::CompressOutcome {
            crate::platform::CompressOutcome::Failed("not used by explain".into())
        }
        fn hold_lock(
            &self,
            _: crate::platform::LockKind,
            _: &Path,
        ) -> std::io::Result<crate::platform::LockHolder> {
            Err(unused_io())
        }
        fn try_lock_exclusive(&self, _: &Path) -> crate::platform::TryLock {
            crate::platform::TryLock::Unopenable(unused_io())
        }
        fn try_lock_elsewhere(
            &self,
            _: crate::platform::LockKind,
            _: &Path,
        ) -> std::io::Result<crate::platform::LockState> {
            Err(unused_io())
        }
        fn entry_stat(&self, _: &Path) -> std::io::Result<crate::platform::EntryStat> {
            Err(unused_io())
        }
        fn read_dir_bounded(
            &self,
            dir: &Path,
            cap: usize,
        ) -> std::io::Result<Vec<(String, crate::platform::EntryKind)>> {
            let mut out = Vec::new();
            for (seen, entry) in std::fs::read_dir(dir)?.enumerate() {
                if seen >= cap {
                    return Err(std::io::Error::other(format!(
                        "{} has more than {cap} entries",
                        dir.display()
                    )));
                }
                let entry = entry?;
                let kind = if entry.file_type()?.is_dir() {
                    crate::platform::EntryKind::Dir
                } else {
                    crate::platform::EntryKind::File
                };
                if let Ok(name) = entry.file_name().into_string() {
                    out.push((name, kind));
                }
            }
            Ok(out)
        }
        fn remove_tree(&self, _: &Path) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn remove_file(&self, _: &Path) -> std::io::Result<()> {
            Err(unused_io())
        }
        fn volume(&self, _: &Path) -> std::io::Result<crate::platform::VolumeInfo> {
            Err(unused_io())
        }
    }

    #[test]
    fn deps_listing_and_output_reads_are_bounded() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-explain-cap-{}-{nanos}",
            std::process::id()
        ));
        let _guard = DirGuard(root.clone());
        std::fs::create_dir_all(&root).unwrap();
        let profile = root.join("debug");
        let deps = profile.join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(deps.join("libwidget-aaaaaaaaaaaaaaaa.rlib"), b"b").unwrap();
        std::fs::write(deps.join("libwidget-bbbbbbbbbbbbbbbb.rmeta"), b"c").unwrap();
        std::fs::write(deps.join("extra-cccccccccccccccc.rlib"), b"e").unwrap();
        let ok_dir = root.join("ok-unit");
        let big_dir = root.join("big-unit");
        std::fs::create_dir_all(&ok_dir).unwrap();
        std::fs::create_dir_all(&big_dir).unwrap();
        let exact = 4 * 1024 * 1024;
        std::fs::write(ok_dir.join("output-lib-widget"), sized_summary(exact)).unwrap();
        std::fs::write(ok_dir.join("invoked.timestamp"), b"stamp").unwrap();
        std::fs::write(big_dir.join("output-lib-widget"), sized_summary(exact + 1)).unwrap();
        let units = vec![
            unit(
                "widget",
                "lib-widget",
                "aaaaaaaaaaaaaaaa",
                "[]",
                Coverage::None,
                (1, 1, 1),
            ),
            unit(
                "widget",
                "lib-widget",
                "bbbbbbbbbbbbbbbb",
                "[]",
                Coverage::None,
                (2, 1, 1),
            ),
        ];
        let target = crate::cargo_target::TargetDir {
            path: root.clone(),
            owner: crate::cargo_target::Owner::Owned {
                checkout: root.clone(),
            },
            profiles: vec![crate::fingerprint::ProfileReport {
                path: profile.clone(),
                format: crate::fingerprint::Format::Supported,
                units: 2,
                not_current: 2,
                not_current_units: vec![
                    NotCurrentUnit {
                        dir: ok_dir.clone(),
                        stem: Some("lib-widget".to_string()),
                        features: Some(Arc::from("[]")),
                        outputs: vec!["output-lib-widget".to_string()],
                    },
                    NotCurrentUnit {
                        dir: big_dir.clone(),
                        stem: Some("lib-widget".to_string()),
                        features: Some(Arc::from("[]")),
                        outputs: vec!["output-lib-widget".to_string()],
                    },
                ],
                edges: None,
                graph: Some(crate::fingerprint::UnitGraph {
                    units,
                    edges: Vec::new(),
                }),
            }],
            size: None,
            unreadable: None,
        };
        let exp = super::run_with(
            &FakeFs,
            2,
            &[&target],
            Limits { top: None },
            FailedUnits::Report,
            &|| false,
        )
        .unwrap();
        let groups = groups_of(&exp.findings);
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].measured.contains_key("mode_build"));
        assert!(!groups[0].measured.contains_key("mode_check"));
        assert!(!groups[0].measured.contains_key("mode_unknown"));
        assert!(exp.unmeasured.iter().any(|(field, reason)| {
            field == &profile.display().to_string()
                && reason == "deps listing failed or was over its cap"
        }));
        let failed = failed_of(&exp.findings);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].path.as_deref(), Some(ok_dir.as_path()));
        assert!(exp.unmeasured.iter().any(|(field, reason)| {
            field == &big_dir.display().to_string() && reason == "larger than 4 MiB"
        }));
        assert_eq!(
            exp.human,
            format!(
                "widget  2 copies\nwidget  1 failed\n{}  1 groups\n",
                root.display()
            )
        );
    }
}
