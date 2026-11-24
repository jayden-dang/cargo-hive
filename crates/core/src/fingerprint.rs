//! Cargo `.fingerprint/` reader (ADR-0001). A unit dir holds exactly one `<H>.json`
//! and a sibling `<H>` with 16 lowercase hex chars. Its JSON must have exactly the 11
//! known keys with known types. Any deviation makes the whole profile dir unsupported.

use crate::unit_path::{UnitLayout, UnitLoc};
use cargo_hive_platform::{EntryKind, Fs};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The file Cargo touches when it starts compiling a unit.
#[doc(hidden)]
pub const INVOKED_TIMESTAMP: &str = "invoked.timestamp";

const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

pub const KEYS: [&str; 11] = [
    "rustc",
    "features",
    "declared_features",
    "target",
    "profile",
    "path",
    "deps",
    "local",
    "rustflags",
    "config",
    "compile_kind",
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitKind {
    RunBuildScript,
    BuildScript,
    DocLib,
    DocBin,
    Test,
    Lib,
    Bin,
    Example,
    Bench,
    Other,
}

const KINDS: [(&str, UnitKind); 9] = [
    ("run-build-script", UnitKind::RunBuildScript),
    ("build-script", UnitKind::BuildScript),
    ("doc-lib", UnitKind::DocLib),
    ("doc-bin", UnitKind::DocBin),
    ("test", UnitKind::Test),
    ("lib", UnitKind::Lib),
    ("bin", UnitKind::Bin),
    ("example", UnitKind::Example),
    ("bench", UnitKind::Bench),
];

impl UnitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunBuildScript => "run-build-script",
            Self::BuildScript => "build-script",
            Self::DocLib => "doc-lib",
            Self::DocBin => "doc-bin",
            Self::Test => "test",
            Self::Lib => "lib",
            Self::Bin => "bin",
            Self::Example => "example",
            Self::Bench => "bench",
            Self::Other => "other",
        }
    }

    pub fn of(stem: &str) -> UnitKind {
        KINDS
            .iter()
            .find(|(name, _)| {
                stem == *name
                    || stem
                        .strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with('-'))
            })
            .map(|(_, kind)| *kind)
            .unwrap_or(Self::Other)
    }
}

impl fmt::Display for UnitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for UnitKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "lowercase")]
pub enum Format {
    Supported,
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    NotRecorded,
    None,
    AllDeclared,
    Partial,
    Unreadable,
}

impl Coverage {
    pub fn as_str(&self) -> Option<&'static str> {
        match self {
            Coverage::NotRecorded => Some("not_recorded"),
            Coverage::None => Some("none"),
            Coverage::AllDeclared => Some("all_declared"),
            Coverage::Partial => Some("partial"),
            Coverage::Unreadable => None,
        }
    }
}

/// One `deps` entry of a fingerprint report: the dependency's package id and its fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DepEdge {
    pub package: u64,
    pub fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitHash(u64);

impl UnitHash {
    pub const HEX_LEN: usize = 16;

    /// The unit hash of a fingerprint integer. Equal to parsing `le_hex(value)`.
    pub fn from_fingerprint(value: u64) -> UnitHash {
        UnitHash(value.swap_bytes())
    }

    pub fn parse(text: &str) -> Option<UnitHash> {
        if !is_hash(text.as_bytes()) {
            return None;
        }
        u64::from_str_radix(text, 16).ok().map(UnitHash)
    }

    pub fn hex_digits(self) -> [u8; 16] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut buf = [0u8; 16];
        let mut n = self.0;
        for slot in buf.iter_mut().rev() {
            *slot = HEX[(n & 0xf) as usize];
            n >>= 4;
        }
        buf
    }
}

impl fmt::Display for UnitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl Serialize for UnitHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let buf = self.hex_digits();
        let text = std::str::from_utf8(&buf).expect("hex digits are utf-8");
        serializer.serialize_str(text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub loc: UnitLoc,
    pub kind: UnitKind,
    pub hash: UnitHash,
    pub deps: Vec<DepEdge>,
    pub stem: String,
    pub features: Arc<str>,
    pub coverage: Coverage,
    pub profile: u64,
    pub target: u64,
    pub path: u64,
    pub toolchain_value: u64,
}

impl Serialize for Unit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Unit", 5)?;
        state.serialize_field("name_meta", &self.loc.name_meta)?;
        state.serialize_field("unit_layout", &self.loc.layout)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("hash_hex", &self.hash)?;
        state.serialize_field("deps", &self.deps)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotCurrentUnit {
    pub dir: PathBuf,
    pub loc: UnitLoc,
    pub stem: Option<String>,
    pub features: Option<Arc<str>>,
    pub outputs: Vec<String>,
    pub deps: Vec<DepEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EdgeCounts {
    pub resolved: u64,
    pub unresolved: u64,
    pub ambiguous: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UnitId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitGraph {
    pub units: Vec<Unit>,
    pub offsets: Vec<u32>,
    pub targets: Vec<UnitId>,
}

impl UnitGraph {
    pub fn successors(&self, id: UnitId) -> &[UnitId] {
        let i = id.0 as usize;
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.targets[start..end]
    }

    pub fn edge_count(&self) -> usize {
        self.targets.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileReport {
    pub path: PathBuf,
    pub format: Format,
    pub units: usize,
    /// Unit dirs Cargo left mid-rebuild or after a failed compile: counted, never read as units.
    pub not_current: usize,
    #[serde(skip)]
    pub not_current_units: Vec<NotCurrentUnit>,
    pub edges: Option<EdgeCounts>,
    #[serde(skip)]
    pub graph: Option<UnitGraph>,
    /// Unit layouts that contributed units or not-current units in this profile.
    pub layouts: BTreeSet<UnitLayout>,
    /// Hex unit dirs under package dirs with no or empty `fingerprint/`.
    pub partial_unit_dirs: usize,
}

/// Cargo's `to_hex`: the u64's 8 bytes in little-endian order, as lowercase hex.
pub fn le_hex(value: u64) -> String {
    value
        .to_le_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_hash(bytes: &[u8]) -> bool {
    bytes.len() == 16
        && bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// The `<hash>` of a `<name>-<hash>` unit dir name, when it is exactly 16 lowercase hex digits.
pub fn unit_hash(dir_name: &str) -> Option<UnitHash> {
    let (_, tail) = dir_name.rsplit_once('-')?;
    UnitHash::parse(tail)
}

/// The crate name of dependency `dep` in the unit's fingerprint report, when it can be read.
#[doc(hidden)]
pub fn dep_name(fingerprint_dir: &Path, stem: &str, dep: u64) -> Option<String> {
    let mut file = fs::File::open(fingerprint_dir.join(format!("{stem}.json"))).ok()?;
    let bytes = read_capped(&mut file).ok()?;
    dependency_name(&bytes, dep)
}

fn dependency_name(bytes: &[u8], dep: u64) -> Option<String> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    value["deps"]
        .as_array()?
        .iter()
        .find_map(|d| match d.as_array().map(Vec::as_slice) {
            Some([_, name, _, hash]) if hash.as_u64() == Some(dep) => {
                name.as_str().map(String::from)
            }
            _ => None,
        })
}

fn check_types(obj: &Map<String, Value>) -> Result<Vec<DepEdge>, String> {
    if obj.len() != KEYS.len() || !KEYS.iter().all(|k| obj.contains_key(*k)) {
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        return Err(format!("unexpected key set {keys:?}"));
    }
    for k in ["rustc", "target", "profile", "path", "config"] {
        obj[k]
            .as_u64()
            .ok_or_else(|| format!("`{k}` is not an unsigned integer"))?;
    }
    if !(obj["compile_kind"].is_u64() || obj["compile_kind"].is_i64()) {
        return Err("`compile_kind` is not an integer".into());
    }
    for k in ["features", "declared_features"] {
        if !obj[k].is_string() {
            return Err(format!("`{k}` is not a string"));
        }
    }
    for k in ["local", "rustflags"] {
        if !obj[k].is_array() {
            return Err(format!("`{k}` is not an array"));
        }
    }
    let deps = obj["deps"].as_array().ok_or("`deps` is not an array")?;
    deps.iter()
        .map(|d| match d.as_array().map(Vec::as_slice) {
            Some([a, b, c, f]) if a.is_u64() && b.is_string() && c.is_boolean() && f.is_u64() => {
                Ok(DepEdge {
                    package: a.as_u64().unwrap(),
                    fingerprint: f.as_u64().unwrap(),
                })
            }
            _ => Err(format!("`deps` entry {d} is not [u64, string, bool, u64]")),
        })
        .collect()
}

#[doc(hidden)]
pub fn read_capped(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("larger than 4 MiB".into());
    }
    Ok(bytes)
}

enum ParsedUnit {
    Current(Unit),
    NotCurrent {
        stem: String,
        features: Arc<str>,
        deps: Vec<DepEdge>,
    },
}

fn read_hash(reader: &mut impl Read, stem: &str) -> Result<Option<UnitHash>, String> {
    let mut bytes = Vec::with_capacity(UnitHash::HEX_LEN + 1);
    reader
        .take((UnitHash::HEX_LEN + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("hash file `{stem}`: {e}"))?;
    if bytes.len() > UnitHash::HEX_LEN {
        return Err(format!(
            "hash file `{stem}` is larger than {} bytes; rebuild the unit and try again",
            UnitHash::HEX_LEN
        ));
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(UnitHash::parse)
        .map(Some)
        .ok_or_else(|| format!("hash file `{stem}` is not 16 lowercase hex characters"))
}

const STRICT_PROFILE_BYTES: usize = 128 * 1024 * 1024;

/// Shares strict fingerprint entry and byte limits across all profiles in one target.
pub struct StrictProfileReader<'a> {
    fs: &'a dyn Fs,
    cancel: &'a dyn Fn() -> bool,
    entries: Cell<usize>,
    bytes: Cell<usize>,
}

impl<'a> StrictProfileReader<'a> {
    pub fn new(fs: &'a dyn Fs, cancel: &'a dyn Fn() -> bool) -> Self {
        Self {
            fs,
            cancel,
            entries: Cell::new(crate::caps::CRATE_DIR_CAP),
            bytes: Cell::new(STRICT_PROFILE_BYTES),
        }
    }

    pub fn read_profile(&self, profile: &Path) -> ProfileReport {
        read_profile_with(profile, crate::caps::CRATE_DIR_CAP, Some(self), false)
    }

    /// A completed Cargo test build can leave un-fingerprinted unit directories
    /// that contain only an empty `out` directory.
    pub fn read_profile_with_empty_unit_outputs(&self, profile: &Path) -> ProfileReport {
        read_profile_with(profile, crate::caps::CRATE_DIR_CAP, Some(self), true)
    }

    fn check_cancel(&self) -> Result<(), String> {
        if (self.cancel)() {
            Err("fingerprint reading was cancelled".into())
        } else {
            Ok(())
        }
    }

    fn charge_entries(&self, count: usize) -> Result<(), String> {
        self.check_cancel()?;
        let left = self
            .entries
            .get()
            .checked_sub(count)
            .ok_or("fingerprint entry limit exceeded")?;
        self.entries.set(left);
        Ok(())
    }

    fn read_file(&self, path: &Path, max: usize) -> Result<Vec<u8>, String> {
        self.check_cancel()?;
        let limit = max.min(self.bytes.get());
        let bytes = self
            .fs
            .read_regular_bounded(path, limit)
            .map_err(|err| err.to_string())?;
        let left = self
            .bytes
            .get()
            .checked_sub(bytes.len())
            .ok_or("fingerprint byte limit exceeded")?;
        self.bytes.set(left);
        Ok(bytes)
    }

    fn is_dir(&self, path: &Path) -> Result<bool, String> {
        self.check_cancel()?;
        match self.fs.entry_stat(path) {
            Ok(stat) if stat.kind == EntryKind::Dir => Ok(true),
            Ok(stat) if stat.kind == EntryKind::File => Ok(false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(format!("{}: {err}", path.display())),
            _ => Err(format!("{} has an unsupported entry type", path.display())),
        }
    }

    fn list_names(&self, dir: &Path) -> Result<Vec<String>, String> {
        if !self.is_dir(dir)? {
            return Err(format!("{} is not a directory", dir.display()));
        }
        let entries = self
            .fs
            .read_dir_bounded(dir, self.entries.get())
            .map_err(|err| format!("{}: {err}", dir.display()))?;
        self.charge_entries(entries.len())?;
        if let Some((name, _)) = entries
            .iter()
            .find(|(_, kind)| !matches!(kind, EntryKind::File | EntryKind::Dir))
        {
            return Err(format!(
                "{} has an unsupported entry type",
                dir.join(name).display()
            ));
        }
        Ok(entries.into_iter().map(|(name, _)| name).collect())
    }

    fn has_only_empty_unit_output(&self, unit: &Path) -> Result<bool, String> {
        if self.list_names(unit)? != ["out"] {
            return Ok(false);
        }
        let output = unit.join("out");
        Ok(self.is_dir(&output)? && self.list_names(&output)?.is_empty())
    }
}

fn fingerprint_directory(
    path: &Path,
    strict: Option<&StrictProfileReader<'_>>,
) -> Result<bool, String> {
    match strict {
        Some(reader) => reader.is_dir(path),
        None => Ok(path.is_dir()),
    }
}

/// An emptied hash is a unit Cargo is rebuilding. The JSON beside it is stale, so the unit is
/// not current; its stem and features are still the ones the last successful build wrote.
fn read_unit(
    dir: &Path,
    json_name: &str,
    loc: UnitLoc,
    shared_features: &mut HashMap<String, Arc<str>>,
    strict: Option<&StrictProfileReader<'_>>,
) -> Result<ParsedUnit, String> {
    let stem = json_name.trim_end_matches(".json");
    let hash_path = dir.join(stem);
    let hash = if let Some(reader) = strict {
        let bytes = reader
            .read_file(&hash_path, UnitHash::HEX_LEN)
            .map_err(|err| format!("hash file `{stem}`: {err}"))?;
        read_hash(&mut bytes.as_slice(), stem)?
    } else {
        let mut file =
            fs::File::open(hash_path).map_err(|err| format!("hash file `{stem}`: {err}"))?;
        read_hash(&mut file, stem)?
    };
    let json_path = dir.join(json_name);
    let bytes = if let Some(reader) = strict {
        reader
            .read_file(&json_path, MAX_JSON_BYTES as usize)
            .map_err(|err| format!("{json_name}: {err}"))?
    } else {
        let len = fs::metadata(&json_path)
            .map_err(|e| format!("{json_name}: {e}"))?
            .len();
        if len > MAX_JSON_BYTES {
            return Err(format!("{json_name}: larger than 4 MiB"));
        }
        let mut file = fs::File::open(&json_path).map_err(|e| format!("{json_name}: {e}"))?;
        read_capped(&mut file).map_err(|e| format!("{json_name}: {e}"))?
    };
    decode_unit(&bytes, hash, stem, loc, shared_features)
}

fn decode_unit(
    bytes: &[u8],
    hash: Option<UnitHash>,
    stem: &str,
    loc: UnitLoc,
    shared_features: &mut HashMap<String, Arc<str>>,
) -> Result<ParsedUnit, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("{stem}.json: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("{stem}.json is not a JSON object"))?;
    let deps = check_types(obj)?;
    let features_raw = obj["features"]
        .as_str()
        .ok_or("`features` is not a string")?;
    let declared_raw = obj["declared_features"]
        .as_str()
        .ok_or("`declared_features` is not a string")?;
    let features = intern(shared_features, features_raw);
    let stem = stem.to_string();
    let Some(hash) = hash else {
        return Ok(ParsedUnit::NotCurrent {
            stem,
            features,
            deps,
        });
    };
    // Both layouts key a unit by its hash file: a dep edge carries that value. A unit dir's own
    // name is a different hex in Cargo 1.100, so it only has to be well-formed.
    if loc.layout == UnitLayout::UnitDir && UnitHash::parse(loc.hex()).is_none() {
        return Err("unit dir name is not 16 lowercase hex".to_string());
    }
    Ok(ParsedUnit::Current(Unit {
        loc,
        kind: UnitKind::of(&stem),
        hash,
        deps,
        stem,
        features,
        coverage: coverage_of(features_raw, declared_raw),
        profile: obj["profile"]
            .as_u64()
            .ok_or("`profile` is not an unsigned integer")?,
        target: obj["target"]
            .as_u64()
            .ok_or("`target` is not an unsigned integer")?,
        path: obj["path"]
            .as_u64()
            .ok_or("`path` is not an unsigned integer")?,
        toolchain_value: obj["rustc"].as_u64().expect("`rustc` is a u64"),
    }))
}

fn intern(shared: &mut HashMap<String, Arc<str>>, text: &str) -> Arc<str> {
    if let Some(hit) = shared.get(text) {
        return Arc::clone(hit);
    }
    let hit: Arc<str> = Arc::from(text);
    shared.insert(text.to_string(), Arc::clone(&hit));
    hit
}

fn coverage_of(features: &str, declared: &str) -> Coverage {
    if features.is_empty() {
        return Coverage::NotRecorded;
    }
    let Some(enabled) = json_string_array(features) else {
        return Coverage::Unreadable;
    };
    if enabled.is_empty() {
        return Coverage::None;
    }
    let Some(declared) = json_string_array(declared) else {
        return Coverage::Unreadable;
    };
    if declared
        .iter()
        .all(|name| enabled.iter().any(|on| on == name))
    {
        Coverage::AllDeclared
    } else {
        Coverage::Partial
    }
}

fn json_string_array(raw: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let items = value.as_array()?;
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        names.push(item.as_str()?.to_string());
    }
    Some(names)
}

#[derive(Deserialize)]
struct OutputLine {
    level: Option<String>,
    code: Option<OutputCode>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct OutputCode {
    code: Option<String>,
}

fn output_line(line: &str) -> Option<OutputLine> {
    serde_json::from_str(line).ok()
}

pub fn is_summary_line(line: &str) -> Option<bool> {
    let parsed = output_line(line)?;
    Some(
        parsed.level.as_deref() == Some("error")
            && parsed.code.is_none()
            && parsed
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("aborting due to")),
    )
}

pub fn error_code(line: &str) -> Option<String> {
    let parsed = output_line(line)?;
    if parsed.level.as_deref() != Some("error") {
        return None;
    }
    parsed.code.and_then(|code| code.code)
}

/// The dir Cargo leaves for a unit whose first compile failed: its start stamp and its cached
/// compiler messages, with no fingerprint written yet.
fn never_compiled(names: &[String]) -> bool {
    names.iter().any(|n| n == INVOKED_TIMESTAMP)
        && names.iter().any(|n| n.starts_with("output-"))
        && names
            .iter()
            .all(|n| n == INVOKED_TIMESTAMP || n.starts_with("output-"))
}

fn flat_loc_for(dir: &Path) -> UnitLoc {
    UnitLoc {
        layout: UnitLayout::Flat,
        name_meta: dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

fn list_names(
    dir: &Path,
    problem: &mut Option<String>,
    cap: usize,
    strict: Option<&StrictProfileReader<'_>>,
) -> Option<Vec<String>> {
    if let Some(reader) = strict {
        return reader
            .list_names(dir)
            .map_err(|why| problem.get_or_insert(why))
            .ok();
    }
    match fs::read_dir(dir) {
        Ok(entries) => {
            let mut names = Vec::new();
            for (seen, entry) in entries.enumerate() {
                if seen >= cap {
                    problem.get_or_insert(format!("{} has more than {cap} entries", dir.display()));
                    return None;
                }
                match entry {
                    Ok(entry) => names.push(entry.file_name().to_string_lossy().to_string()),
                    Err(e) => {
                        problem.get_or_insert(format!("{}: {e}", dir.display()));
                    }
                }
            }
            Some(names)
        }
        Err(e) => {
            problem.get_or_insert(format!("{}: {e}", dir.display()));
            None
        }
    }
}

struct ProfileAccum<'a> {
    units: &'a mut Vec<Unit>,
    counted: &'a mut usize,
    not_current_units: &'a mut Vec<NotCurrentUnit>,
    layouts: &'a mut BTreeSet<UnitLayout>,
    shared_features: &'a mut HashMap<String, Arc<str>>,
    problem: &'a mut Option<String>,
}

fn output_names(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| n.starts_with("output-"))
        .cloned()
        .collect()
}

fn absorb_fingerprint_dir(
    dir: &Path,
    loc: UnitLoc,
    names: &[String],
    acc: &mut ProfileAccum<'_>,
    strict: Option<&StrictProfileReader<'_>>,
) {
    if names.is_empty() {
        return;
    }
    if never_compiled(names) {
        acc.layouts.insert(loc.layout);
        acc.not_current_units.push(NotCurrentUnit {
            dir: dir.to_path_buf(),
            loc,
            stem: None,
            features: None,
            outputs: output_names(names),
            deps: Vec::new(),
        });
        return;
    }
    let jsons: Vec<&String> = names.iter().filter(|n| n.ends_with(".json")).collect();
    if jsons.len() != 1 {
        acc.problem.get_or_insert(format!(
            "{}: {} fingerprint JSON files",
            dir.display(),
            jsons.len()
        ));
        return;
    }
    match read_unit(dir, jsons[0], loc.clone(), acc.shared_features, strict) {
        Ok(ParsedUnit::Current(u)) => {
            acc.layouts.insert(u.loc.layout);
            *acc.counted += 1;
            acc.units.push(u);
        }
        Ok(ParsedUnit::NotCurrent {
            stem,
            features,
            deps,
        }) => {
            acc.layouts.insert(loc.layout);
            acc.not_current_units.push(NotCurrentUnit {
                dir: dir.to_path_buf(),
                loc,
                stem: Some(stem),
                features: Some(features),
                outputs: output_names(names),
                deps,
            });
        }
        Err(reason) => {
            *acc.counted += 1;
            acc.problem
                .get_or_insert(format!("{}: {reason}", dir.display()));
        }
    }
}

fn is_package_dir_entries(
    names: &[String],
    paths: &[PathBuf],
    strict: Option<&StrictProfileReader<'_>>,
) -> Result<bool, String> {
    for (name, path) in names.iter().zip(paths) {
        if UnitHash::parse(name).is_none() || !fingerprint_directory(path, strict)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn read_profile(profile_dir: &Path) -> ProfileReport {
    read_profile_capped(profile_dir, crate::caps::CRATE_DIR_CAP)
}

fn read_profile_capped(profile_dir: &Path, dir_cap: usize) -> ProfileReport {
    read_profile_with(profile_dir, dir_cap, None, false)
}

fn read_profile_with(
    profile_dir: &Path,
    dir_cap: usize,
    strict: Option<&StrictProfileReader<'_>>,
    allow_empty_unit_outputs: bool,
) -> ProfileReport {
    let path = profile_dir.to_path_buf();
    let unsupported = |units: usize, reason: String| ProfileReport {
        path: path.clone(),
        format: Format::Unsupported(reason),
        units,
        not_current: 0,
        not_current_units: Vec::new(),
        edges: None,
        graph: None,
        layouts: BTreeSet::new(),
        partial_unit_dirs: 0,
    };
    let mut units = Vec::new();
    let mut counted = 0usize;
    let mut not_current_units = Vec::new();
    let mut layouts = BTreeSet::new();
    let mut partial_unit_dirs = 0usize;
    let mut shared_features: HashMap<String, Arc<str>> = HashMap::new();
    let mut problem: Option<String> = None;
    let mut saw_fingerprint = false;
    let mut saw_package_dir = false;

    let fp = profile_dir.join(".fingerprint");
    let fp_present = match fingerprint_directory(&fp, strict) {
        Ok(present) => present,
        Err(why) => return unsupported(counted, why),
    };
    if fp_present {
        saw_fingerprint = true;
        let entries = match fs::read_dir(&fp) {
            Ok(e) => e,
            Err(e) => return unsupported(0, format!("cannot read .fingerprint/: {e}")),
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        for (seen, entry) in entries.enumerate() {
            if let Some(reader) = strict
                && let Err(why) = reader.charge_entries(1)
            {
                return unsupported(counted, why);
            }
            if seen >= dir_cap {
                return unsupported(
                    0,
                    format!("{} has more than {dir_cap} entries", fp.display()),
                );
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    problem.get_or_insert(format!("cannot read .fingerprint/: {e}"));
                    continue;
                }
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
                Ok(kind) if strict.is_some() && !kind.is_file() => {
                    problem.get_or_insert(format!(
                        "{} has an unsupported entry type",
                        entry.path().display()
                    ));
                }
                Ok(_) => {}
                Err(e) => {
                    problem.get_or_insert(format!("{}: {e}", entry.path().display()));
                }
            }
        }
        dirs.sort();
        for dir in dirs {
            let Some(names) = list_names(&dir, &mut problem, dir_cap, strict) else {
                continue;
            };
            absorb_fingerprint_dir(
                &dir,
                flat_loc_for(&dir),
                &names,
                &mut ProfileAccum {
                    units: &mut units,
                    counted: &mut counted,
                    not_current_units: &mut not_current_units,
                    layouts: &mut layouts,
                    shared_features: &mut shared_features,
                    problem: &mut problem,
                },
                strict,
            );
        }
    }

    let build = profile_dir.join("build");
    let build_present = match fingerprint_directory(&build, strict) {
        Ok(present) => present,
        Err(why) => return unsupported(counted, why),
    };
    if build_present {
        let mut children: Vec<PathBuf> = Vec::new();
        match fs::read_dir(&build) {
            Ok(entries) => {
                for (seen, entry) in entries.enumerate() {
                    if let Some(reader) = strict
                        && let Err(why) = reader.charge_entries(1)
                    {
                        return unsupported(counted, why);
                    }
                    if seen >= dir_cap {
                        return unsupported(
                            counted,
                            format!("{} has more than {dir_cap} entries", build.display()),
                        );
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            problem.get_or_insert(format!("cannot read build/: {e}"));
                            continue;
                        }
                    };
                    match entry.file_type() {
                        Ok(kind) if kind.is_dir() => children.push(entry.path()),
                        Ok(_) => {
                            problem.get_or_insert(format!(
                                "{}: not a package dir or flat build-script dir",
                                entry.path().display()
                            ));
                        }
                        Err(e) => {
                            problem.get_or_insert(format!("{}: {e}", entry.path().display()));
                        }
                    }
                }
            }
            Err(e) => {
                problem.get_or_insert(format!("cannot read build/: {e}"));
            }
        }
        children.sort();
        for pkg_path in children {
            let pkg_name = pkg_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let Some(child_names) = list_names(&pkg_path, &mut problem, dir_cap, strict) else {
                continue;
            };
            let child_paths: Vec<PathBuf> =
                child_names.iter().map(|name| pkg_path.join(name)).collect();
            let is_package = match is_package_dir_entries(&child_names, &child_paths, strict) {
                Ok(is_package) => is_package,
                Err(why) => return unsupported(counted, why),
            };
            if is_package {
                saw_package_dir = true;
                let mut hexes = child_names.clone();
                hexes.sort();
                for hex in hexes {
                    let unit_root = pkg_path.join(&hex);
                    let fp_dir = unit_root.join("fingerprint");
                    let loc = UnitLoc {
                        layout: UnitLayout::UnitDir,
                        name_meta: format!("{pkg_name}-{hex}"),
                    };
                    match fingerprint_directory(&fp_dir, strict) {
                        Ok(true) => {}
                        Ok(false) => {
                            let empty_output = match strict.filter(|_| allow_empty_unit_outputs) {
                                Some(reader) => match reader.has_only_empty_unit_output(&unit_root)
                                {
                                    Ok(empty) => empty,
                                    Err(why) => return unsupported(counted, why),
                                },
                                None => false,
                            };
                            if !empty_output {
                                partial_unit_dirs += 1;
                            }
                            continue;
                        }
                        Err(why) => return unsupported(counted, why),
                    }
                    let Some(names) = list_names(&fp_dir, &mut problem, dir_cap, strict) else {
                        continue;
                    };
                    if names.is_empty() {
                        partial_unit_dirs += 1;
                        continue;
                    }
                    absorb_fingerprint_dir(
                        &fp_dir,
                        loc,
                        &names,
                        &mut ProfileAccum {
                            units: &mut units,
                            counted: &mut counted,
                            not_current_units: &mut not_current_units,
                            layouts: &mut layouts,
                            shared_features: &mut shared_features,
                            problem: &mut problem,
                        },
                        strict,
                    );
                }
            } else if unit_hash(&pkg_name).is_some() {
                // Flat build-script dir under build/; units live in .fingerprint/.
            } else {
                problem.get_or_insert(format!(
                    "{}: not a package dir or flat build-script dir",
                    pkg_path.display()
                ));
            }
        }
    }

    if let Some(reader) = strict
        && let Err(why) = reader.check_cancel()
    {
        return unsupported(counted, why);
    }
    if let Some(reason) = problem {
        return unsupported(counted, reason);
    }
    if !saw_fingerprint && !saw_package_dir {
        return unsupported(0, "no .fingerprint/ directory and no unit dirs".into());
    }
    let mut index: HashMap<UnitHash, Vec<UnitId>> = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        let id = UnitId(u32::try_from(i).expect("profile cannot exceed u32::MAX units"));
        index.entry(u.hash).or_default().push(id);
    }
    let mut counts = EdgeCounts::default();
    let mut offsets = Vec::with_capacity(units.len() + 1);
    let mut targets = Vec::new();
    offsets.push(0);
    for u in &units {
        for dep in &u.deps {
            let key = UnitHash::from_fingerprint(dep.fingerprint);
            match index.get(&key).map(Vec::as_slice) {
                Some([j]) => {
                    counts.resolved += 1;
                    targets.push(*j);
                }
                Some(_) => counts.ambiguous += 1,
                None => counts.unresolved += 1,
            }
        }
        offsets.push(u32::try_from(targets.len()).expect("edge count fits u32"));
    }
    ProfileReport {
        path,
        format: Format::Supported,
        units: counted,
        not_current: not_current_units.len(),
        not_current_units,
        edges: Some(counts),
        graph: Some(UnitGraph {
            units,
            offsets,
            targets,
        }),
        layouts,
        partial_unit_dirs,
    }
}

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;

    pub fn fingerprint(data: &[u8]) {
        let loc = UnitLoc {
            layout: UnitLayout::UnitDir,
            name_meta: "probe-ffffffffffffffff".into(),
        };
        let mut shared = HashMap::new();
        let _ = read_hash(&mut &data[..], "lib-probe");
        let _ = read_capped(&mut &data[..]);
        let _ = decode_unit(
            data,
            Some(UnitHash(1)),
            "lib-probe",
            loc.clone(),
            &mut shared,
        );
        let _ = dependency_name(data, 1);
        if let Ok(text) = std::str::from_utf8(data) {
            let _ = coverage_of(text, text);
        }
        let n = data.first().copied().unwrap_or(0) as u64;
        let value = serde_json::json!({"rustc":n,"features":"[\"a\"]","declared_features":"[\"a\"]","target":2,"profile":3,"path":4,"deps":[[5,"neighbor",false,n]],"local":[],"rustflags":[],"config":6,"compile_kind":0});
        let bytes = serde_json::to_vec(&value).unwrap();
        let hash_text = format!("{n:016x}");
        let hash = read_hash(&mut hash_text.as_bytes(), "lib-probe").unwrap();
        for layout in [UnitLayout::Flat, UnitLayout::UnitDir] {
            let loc = UnitLoc {
                layout,
                ..loc.clone()
            };
            let ParsedUnit::Current(unit) =
                decode_unit(&bytes, hash, "lib-probe", loc.clone(), &mut shared).unwrap()
            else {
                panic!("valid current unit")
            };
            assert_eq!(unit.hash.to_string(), hash_text);
            assert_eq!(
                unit.deps,
                vec![DepEdge {
                    package: 5,
                    fingerprint: n
                }]
            );
            assert_eq!(unit.coverage, Coverage::AllDeclared);
            assert_eq!(unit.toolchain_value, n);
            assert!(matches!(
                decode_unit(&bytes, None, "lib-probe", loc, &mut shared),
                Ok(ParsedUnit::NotCurrent { .. })
            ));
        }
        assert_eq!(dependency_name(&bytes, n).as_deref(), Some("neighbor"));
        let mut invalid = value;
        invalid["deps"] = serde_json::json!([[5, "neighbor", 0, n]]);
        assert!(
            decode_unit(
                &serde_json::to_vec(&invalid).unwrap(),
                hash,
                "lib-probe",
                loc,
                &mut shared
            )
            .is_err()
        );
    }

    pub fn diagnostics(data: &[u8]) {
        if let Ok(text) = std::str::from_utf8(data) {
            for line in text.lines() {
                let _ = is_summary_line(line);
                let _ = error_code(line);
            }
        }
        let message = String::from_utf8_lossy(data);
        let line = serde_json::json!({"level":"error","code":{"code":"E0123"},"message":message})
            .to_string();
        assert_eq!(is_summary_line(&line), Some(false));
        assert_eq!(error_code(&line).as_deref(), Some("E0123"));
        let summary = serde_json::json!({"level":"error","code":null,"message":"aborting due to previous error"}).to_string();
        assert_eq!(is_summary_line(&summary), Some(true));
        assert!(error_code(&summary).is_none());
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        // Tests run in parallel threads of one process: the counter keeps their dirs apart.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("hive-fp-{}-{k}-{n}", std::process::id()));
        fs::create_dir_all(p.join(".fingerprint")).unwrap();
        p
    }

    #[test]
    fn reader_models_cover_valid_and_invalid_compiler_data() {
        for data in [
            &b""[..],
            &b"\xff\x00garbled"[..],
            &b"[]"[..],
            &b"abcdef"[..],
        ] {
            fuzzing::fingerprint(data);
            fuzzing::diagnostics(data);
        }
        let bytes = unit_json(&[7]).into_bytes();
        let loc = UnitLoc {
            layout: UnitLayout::Flat,
            name_meta: "q-0000000000000001".into(),
        };
        for end in 0..bytes.len() {
            assert!(
                decode_unit(
                    &bytes[..end],
                    Some(UnitHash(1)),
                    "lib-q",
                    loc.clone(),
                    &mut HashMap::new()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn hash_prefixes_distinguish_rebuilding_from_incomplete_hashes() {
        let bytes = b"0123456789abcdef";
        for end in 0..=bytes.len() {
            let parsed = read_hash(&mut &bytes[..end], "lib-probe");
            match end {
                0 => assert_eq!(parsed.unwrap(), None),
                16 => assert_eq!(parsed.unwrap().unwrap().to_string(), "0123456789abcdef"),
                _ => assert!(parsed.unwrap_err().contains("16 lowercase hex")),
            }
        }
    }

    fn unit_json(deps: &[u64]) -> String {
        let deps: Vec<String> = deps
            .iter()
            .map(|d| format!("[1,\"dep\",false,{d}]"))
            .collect();
        format!(
            "{{\"rustc\":11722972603611519644,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}",
            deps.join(",")
        )
    }

    fn unit_json_named(deps: &[(u64, &str)]) -> String {
        let deps: Vec<String> = deps
            .iter()
            .map(|(d, name)| format!("[1,\"{name}\",false,{d}]"))
            .collect();
        format!(
            "{{\"rustc\":11722972603611519644,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}",
            deps.join(",")
        )
    }

    #[test]
    fn the_unit_hash_is_the_sixteen_hex_tail_of_the_dir_name() {
        assert_eq!(
            unit_hash("dep-4360aef667e9273e").map(|h| h.to_string()),
            Some("4360aef667e9273e".into())
        );
        assert_eq!(
            unit_hash("build_script_build-9a4d89fe046fe1ba").map(|h| h.to_string()),
            Some("9a4d89fe046fe1ba".into())
        );
        assert_eq!(unit_hash("dep-4360AEF667E9273E"), None);
        assert_eq!(unit_hash("dep-4360aef667e9273"), None);
        assert_eq!(unit_hash("nodash"), None);
    }

    #[test]
    fn a_unit_hash_round_trips_as_sixteen_lowercase_hex() {
        assert_eq!(
            UnitHash::parse("00000000000000ab").unwrap().to_string(),
            "00000000000000ab"
        );
    }

    #[test]
    fn unit_hash_parse_rejects_uppercase_and_wrong_lengths() {
        assert!(UnitHash::parse("00000000000000AB").is_none());
        assert!(UnitHash::parse("00000000000000a").is_none());
        assert!(UnitHash::parse("00000000000000abc").is_none());
    }

    #[test]
    fn a_units_hash_serializes_under_hash_hex_as_the_sixteen_hex_string() {
        let p = scratch();
        write_unit(&p, "dep-0000000000000001", "lib-dep", 0xab, &[]);
        let report = read_profile(&p);
        let unit = &report.graph.as_ref().unwrap().units[0];
        let value = serde_json::to_value(unit).unwrap();
        assert_eq!(value["hash_hex"], "ab00000000000000");
        assert_eq!(value["kind"], "lib");
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_kind_matches_the_stem_prefix_table_and_falls_back_to_other() {
        let cases = [
            (
                "run-build-script",
                UnitKind::RunBuildScript,
                "run-build-script",
            ),
            ("build-script", UnitKind::BuildScript, "build-script"),
            ("doc-lib", UnitKind::DocLib, "doc-lib"),
            ("doc-bin", UnitKind::DocBin, "doc-bin"),
            ("test", UnitKind::Test, "test"),
            ("lib", UnitKind::Lib, "lib"),
            ("bin", UnitKind::Bin, "bin"),
            ("example", UnitKind::Example, "example"),
            ("bench", UnitKind::Bench, "bench"),
            ("build-script-build", UnitKind::BuildScript, "build-script"),
            ("lib-foo", UnitKind::Lib, "lib"),
            ("weird", UnitKind::Other, "other"),
        ];
        for (stem, want, printed) in cases {
            let kind = UnitKind::of(stem);
            assert_eq!(kind, want, "stem {stem}");
            assert_eq!(kind.as_str(), printed, "stem {stem}");
            assert_eq!(kind.to_string(), printed, "stem {stem}");
        }
    }

    #[test]
    fn a_dependency_name_is_read_back_from_the_unit_json() {
        let p = scratch();
        let dir = p.join(".fingerprint").join("dep-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("lib-q.json"),
            unit_json_named(&[(7, "alpha"), (9, "beta")]),
        )
        .unwrap();
        assert_eq!(dep_name(&dir, "lib-q", 9), Some("beta".to_string()));
        assert_eq!(dep_name(&dir, "lib-q", 7), Some("alpha".to_string()));
        assert_eq!(dep_name(&dir, "lib-q", 8), None);
        assert_eq!(dep_name(&dir, "lib-missing", 7), None);
    }

    fn unit_json_with(deps: &[u64], features: &str, declared: &str) -> String {
        let deps: Vec<String> = deps
            .iter()
            .map(|d| format!("[1,\"dep\",false,{d}]"))
            .collect();
        format!(
            "{{\"rustc\":11722972603611519644,\"features\":{},\"declared_features\":{},\"target\":1,\"profile\":2,\"path\":3,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}",
            serde_json::to_string(features).unwrap(),
            serde_json::to_string(declared).unwrap(),
            deps.join(",")
        )
    }

    fn write_unit_with(
        profile: &Path,
        dir: &str,
        stem: &str,
        value: u64,
        deps: &[u64],
        features: &str,
        declared: &str,
    ) {
        let d = profile.join(".fingerprint").join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(stem), le_hex(value)).unwrap();
        fs::write(
            d.join(format!("{stem}.json")),
            unit_json_with(deps, features, declared),
        )
        .unwrap();
    }

    fn write_unit(profile: &Path, dir: &str, stem: &str, value: u64, deps: &[u64]) {
        let d = profile.join(".fingerprint").join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(stem), le_hex(value)).unwrap();
        fs::write(d.join(format!("{stem}.json")), unit_json(deps)).unwrap();
        fs::write(
            d.join("invoked.timestamp"),
            "This file has an mtime of when this was started.",
        )
        .unwrap();
    }

    #[test]
    fn a_unit_keeps_the_toolchain_value_of_its_json() {
        let p = scratch();
        write_unit(&p, "dep-0000000000000001", "lib-dep", 1, &[]);
        let json = p.join(".fingerprint/dep-0000000000000001/lib-dep.json");
        let body = fs::read_to_string(&json).unwrap().replacen(
            "11722972603611519644",
            "776363091828236830",
            1,
        );
        fs::write(&json, body).unwrap();
        let report = read_profile(&p);
        let graph = report.graph.expect("a known unit stays readable");
        assert_eq!(graph.units[0].toolchain_value, 776363091828236830);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn edges_resolve_through_little_endian_hex_of_the_dependency_fingerprint() {
        assert_eq!(le_hex(0x0102_0304_0506_0708), "0807060504030201");
        let p = scratch();
        let (a, b, c) = (
            0x0102_0304_0506_0708u64,
            17_112_352_188_342_185_299u64,
            42u64,
        );
        write_unit(&p, "b-0000000000000001", "lib-b", b, &[]);
        write_unit(&p, "a-0000000000000002", "lib-a", a, &[b]);
        write_unit(&p, "c-0000000000000003", "bin-c", c, &[a, b]);
        fs::create_dir_all(p.join(".fingerprint/empty-0000000000000004")).unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 3, "the empty unit dir is ignored");
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 3,
                unresolved: 0,
                ambiguous: 0
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn unresolved_and_ambiguous_edges_are_counted() {
        let p = scratch();
        write_unit(&p, "x-0000000000000001", "lib-x", 7, &[]);
        write_unit(&p, "y-0000000000000002", "lib-y", 7, &[]);
        write_unit(&p, "z-0000000000000003", "test-z", 9, &[7, 1234]);
        let r = read_profile(&p);
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 0,
                unresolved: 1,
                ambiguous: 1
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn doc_and_run_build_script_units_are_accepted() {
        let p = scratch();
        write_unit(&p, "d-0000000000000001", "doc-lib-d", 5, &[]);
        write_unit(
            &p,
            "e-0000000000000002",
            "run-build-script-build-script-build",
            6,
            &[5],
        );
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        let kinds: Vec<UnitKind> = r.graph.unwrap().units.iter().map(|u| u.kind).collect();
        assert!(
            kinds.contains(&UnitKind::DocLib) && kinds.contains(&UnitKind::RunBuildScript),
            "{kinds:?}"
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn any_deviation_from_the_signature_makes_the_whole_profile_unsupported() {
        type Corrupt = Box<dyn Fn(&Path)>;
        let cases: Vec<(&str, Corrupt)> = vec![
            (
                "extra key",
                Box::new(|d: &Path| {
                    fs::write(
                        d.join("lib-q.json"),
                        unit_json(&[]).replacen('{', "{\"version\":1,", 1),
                    )
                    .unwrap()
                }),
            ),
            (
                "missing key",
                Box::new(|d: &Path| {
                    fs::write(
                        d.join("lib-q.json"),
                        unit_json(&[]).replace(",\"compile_kind\":0", ""),
                    )
                    .unwrap()
                }),
            ),
            (
                "deps entry shape",
                Box::new(|d: &Path| {
                    fs::write(
                        d.join("lib-q.json"),
                        unit_json(&[]).replace("\"deps\":[]", "\"deps\":[[1,\"x\",false]]"),
                    )
                    .unwrap()
                }),
            ),
            (
                "short hash",
                Box::new(|d: &Path| fs::write(d.join("lib-q"), "abc").unwrap()),
            ),
            (
                "uppercase hash",
                Box::new(|d: &Path| fs::write(d.join("lib-q"), "ABCDEF0123456789").unwrap()),
            ),
            (
                "two json files",
                Box::new(|d: &Path| fs::write(d.join("lib-r.json"), unit_json(&[])).unwrap()),
            ),
        ];
        for (name, corrupt) in cases {
            let p = scratch();
            write_unit(&p, "ok-0000000000000001", "lib-ok", 1, &[]);
            write_unit(&p, "q-0000000000000002", "lib-q", 2, &[]);
            corrupt(&p.join(".fingerprint/q-0000000000000002"));
            let r = read_profile(&p);
            assert!(
                matches!(r.format, Format::Unsupported(_)),
                "{name}: {:?}",
                r.format
            );
            assert_eq!(r.edges, None, "{name}");
            fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn a_fingerprint_json_larger_than_four_mebibytes_is_unsupported() {
        let p = scratch();
        let dir = p.join(".fingerprint/huge-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib-huge"), le_hex(1)).unwrap();
        let features = "x".repeat(4 * 1024 * 1024);
        fs::write(
            dir.join("lib-huge.json"),
            format!(
                "{{\"rustc\":1,\"features\":\"{features}\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}"
            ),
        )
        .unwrap();
        let r = read_profile(&p);
        assert!(
            matches!(r.format, Format::Unsupported(ref why) if why.contains("4 MiB")),
            "{:?}",
            r.format
        );
        assert_eq!(r.edges, None);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_fingerprint_json_of_exactly_four_mebibytes_is_not_rejected_for_size() {
        let p = scratch();
        let dir = p.join(".fingerprint/edge-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib-edge"), le_hex(1)).unwrap();
        fs::write(dir.join("lib-edge.json"), vec![b'x'; 4 * 1024 * 1024]).unwrap();
        let r = read_profile(&p);
        assert!(
            matches!(r.format, Format::Unsupported(ref why) if !why.contains("4 MiB")),
            "exact 4 MiB must reach the parser: {:?}",
            r.format
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_fingerprint_json_one_byte_over_four_mebibytes_is_unsupported() {
        let p = scratch();
        let dir = p.join(".fingerprint/over-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib-over"), le_hex(1)).unwrap();
        fs::write(dir.join("lib-over.json"), vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
        let r = read_profile(&p);
        assert!(
            matches!(r.format, Format::Unsupported(ref why) if why.contains("4 MiB")),
            "{:?}",
            r.format
        );
        assert_eq!(r.edges, None);
        fs::remove_dir_all(p).unwrap();
    }

    struct CountingReader {
        remaining: u64,
        pulled: u64,
    }

    impl Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 || buf.is_empty() {
                return Ok(0);
            }
            let n = std::cmp::min(self.remaining, buf.len() as u64) as usize;
            buf[..n].fill(b'x');
            self.remaining -= n as u64;
            self.pulled += n as u64;
            Ok(n)
        }
    }

    #[test]
    fn a_capped_body_read_rejects_an_oversize_stream_without_pulling_past_the_cap() {
        let mut source = CountingReader {
            remaining: MAX_JSON_BYTES + 100,
            pulled: 0,
        };
        let err = read_capped(&mut source).expect_err("oversize body");
        assert!(err.contains("4 MiB"), "expected a size error, got {err:?}");
        assert!(
            source.pulled <= MAX_JSON_BYTES + 1,
            "pulled {} bytes past the cap",
            source.pulled
        );
    }

    #[test]
    fn a_hash_stream_stops_after_the_one_overflow_sentinel() {
        let mut source = CountingReader {
            remaining: 1024 * 1024,
            pulled: 0,
        };
        assert!(
            read_hash(&mut source, "lib-probe")
                .unwrap_err()
                .contains("larger than 16 bytes")
        );
        assert_eq!(source.pulled, 17);
    }

    #[test]
    fn a_hash_file_at_the_limit_is_current_and_one_byte_past_is_a_size_refusal() {
        let profile = scratch();
        let name = "probe-0000000000000001";
        let stem = "lib-probe";
        write_unit(&profile, name, stem, 1, &[]);
        let hash_path = profile.join(".fingerprint").join(name).join(stem);
        let exact = read_profile(&profile);
        fs::write(&hash_path, format!("{}0", le_hex(1))).unwrap();
        let over = read_profile(&profile);
        fs::write(&hash_path, b"").unwrap();
        let empty = read_profile(&profile);
        fs::remove_dir_all(profile).unwrap();

        assert_eq!(exact.format, Format::Supported);
        assert_eq!(exact.units, 1);
        assert_eq!(empty.format, Format::Supported);
        assert_eq!(empty.not_current, 1);
        assert!(
            matches!(&over.format, Format::Unsupported(why) if why.contains(stem) && why.contains("larger than 16 bytes") && why.contains("rebuild")),
            "{:?}",
            over.format
        );
    }

    #[test]
    fn a_fingerprint_stream_longer_than_its_size_is_cut_off_at_four_mebibytes() {
        let p = scratch();
        let dir = p.join(".fingerprint/fifo-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib-fifo"), le_hex(1)).unwrap();
        let fifo = dir.join("lib-fifo.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        // A FIFO reports length 0, so only the capped read stands between hive and
        // an unbounded body. The writer stops on its own after 12 MiB.
        let writer = std::thread::spawn({
            let fifo = fifo.clone();
            move || {
                let mut w = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
                let _ = w.write_all(b"{\"rustc\":1,\"features\":\"");
                let chunk = vec![b'x'; 1024 * 1024];
                for _ in 0..12 {
                    if w.write_all(&chunk).is_err() {
                        return; // the reader stopped at the cap
                    }
                }
            }
        });
        let r = read_profile(&p);
        writer.join().unwrap();
        assert!(
            matches!(r.format, Format::Unsupported(ref why) if why.contains("4 MiB")),
            "{:?}",
            r.format
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn an_unreadable_unit_directory_makes_the_profile_unsupported() {
        let p = scratch();
        write_unit(&p, "ok-0000000000000001", "lib-ok", 1, &[]);
        let blocked = p.join(".fingerprint/blocked-0000000000000002");
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let r = read_profile(&p);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(r.format, Format::Unsupported(_)), "{:?}", r.format);
        assert_eq!(r.edges, None);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_that_never_compiled_is_not_current_and_keeps_the_profile_supported() {
        let p = scratch();
        write_unit(&p, "ok-0000000000000001", "lib-ok", 1, &[]);
        let failed = p.join(".fingerprint/failed-0000000000000002");
        fs::create_dir_all(&failed).unwrap();
        fs::write(failed.join("invoked.timestamp"), "").unwrap();
        fs::write(
            failed.join("output-lib-failed"),
            "{\"$message_type\":\"diagnostic\"}",
        )
        .unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!((r.units, r.not_current), (1, 1));
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_with_an_emptied_hash_file_is_not_current_and_keeps_the_profile_supported() {
        for with_output in [false, true] {
            let p = scratch();
            write_unit(&p, "dep-0000000000000001", "lib-dep", 1, &[]);
            write_unit(&p, "user-0000000000000002", "lib-user", 2, &[1]);
            let dep = p.join(".fingerprint/dep-0000000000000001");
            fs::write(dep.join("lib-dep"), "").unwrap();
            fs::write(dep.join("dep-lib-dep"), "dep-info").unwrap();
            if with_output {
                fs::write(dep.join("output-lib-dep"), "{}").unwrap();
            }
            let r = read_profile(&p);
            assert_eq!(r.format, Format::Supported, "output-*: {with_output}");
            assert_eq!((r.units, r.not_current), (1, 1), "output-*: {with_output}");
            assert_eq!(
                r.edges,
                Some(EdgeCounts {
                    resolved: 0,
                    unresolved: 1,
                    ambiguous: 0
                }),
                "output-*: {with_output}"
            );
            fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn near_misses_of_a_not_current_unit_still_make_the_profile_unsupported() {
        type Shape = Box<dyn Fn(&Path)>;
        let cases: Vec<(&str, Shape)> = vec![
            (
                "start stamp only",
                Box::new(|d: &Path| fs::write(d.join("invoked.timestamp"), "").unwrap()),
            ),
            (
                "cached messages beside an unknown file",
                Box::new(|d: &Path| {
                    fs::write(d.join("invoked.timestamp"), "").unwrap();
                    fs::write(d.join("output-lib-q"), "{}").unwrap();
                    fs::write(d.join("dep-lib-q"), "dep-info").unwrap();
                }),
            ),
            (
                "emptied hash beside a JSON off the signature",
                Box::new(|d: &Path| {
                    fs::write(d.join("lib-q"), "").unwrap();
                    fs::write(
                        d.join("lib-q.json"),
                        unit_json(&[]).replace(",\"compile_kind\":0", ""),
                    )
                    .unwrap();
                }),
            ),
            (
                "JSON with no hash file",
                Box::new(|d: &Path| fs::write(d.join("lib-q.json"), unit_json(&[])).unwrap()),
            ),
        ];
        for (name, shape) in cases {
            let p = scratch();
            write_unit(&p, "ok-0000000000000001", "lib-ok", 1, &[]);
            let d = p.join(".fingerprint/q-0000000000000002");
            fs::create_dir_all(&d).unwrap();
            shape(&d);
            let r = read_profile(&p);
            assert!(
                matches!(r.format, Format::Unsupported(_)),
                "{name}: {:?}",
                r.format
            );
            fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn a_profile_with_a_unit_dir_and_no_fingerprint_dir_is_supported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        fs::create_dir_all(p.join("build/pkg/0123456789abcdef/fingerprint")).unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!((r.units, r.partial_unit_dirs), (0, 1));
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 0,
                unresolved: 0,
                ambiguous: 0
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    fn write_unit_dir(
        profile: &Path,
        package: &str,
        hex: &str,
        stem: &str,
        value: u64,
        deps: &[u64],
    ) {
        let fp = profile
            .join("build")
            .join(package)
            .join(hex)
            .join("fingerprint");
        fs::create_dir_all(&fp).unwrap();
        fs::write(fp.join(stem), le_hex(value)).unwrap();
        fs::write(fp.join(format!("{stem}.json")), unit_json(deps)).unwrap();
        fs::write(
            fp.join("invoked.timestamp"),
            "This file has an mtime of when this was started.",
        )
        .unwrap();
    }

    #[test]
    fn a_unit_dir_profile_reads_its_units_and_resolves_an_edge_between_packages() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        let (a, b) = (0x0102_0304_0506_0708u64, 17_112_352_188_342_185_299u64);
        write_unit_dir(&p, "dep", &le_hex(b), "lib-dep", b, &[]);
        write_unit_dir(&p, "app", &le_hex(a), "lib-app", a, &[b]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 2);
        assert_eq!(r.layouts, BTreeSet::from([UnitLayout::UnitDir]));
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 1,
                unresolved: 0,
                ambiguous: 0
            })
        );
        let graph = r.graph.as_ref().unwrap();
        assert!(
            graph
                .units
                .iter()
                .all(|u| u.loc.layout == UnitLayout::UnitDir)
        );
        assert!(
            graph
                .units
                .iter()
                .any(|u| u.loc.name_meta == format!("dep-{}", le_hex(b)))
        );
        assert!(
            graph
                .units
                .iter()
                .any(|u| u.loc.name_meta == format!("app-{}", le_hex(a)))
        );
        fs::remove_dir_all(p).unwrap();
    }

    /// Cargo 1.100 names a unit dir with one hex and writes another in the hash file; a dep edge
    /// carries the hash file's value, never the directory's.
    #[test]
    fn a_unit_dir_edge_resolves_by_the_hash_file_when_the_dir_name_differs() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        // Values read from a real Cargo 1.100.0 build: edge c968045be28830c3 is the little-endian
        // reading of depcrate's hash file c33088e25b0468c9 under dir 5c0d43421562aa29.
        let dep = 0xc968_045b_e288_30c3u64;
        assert_eq!(le_hex(dep), "c33088e25b0468c9");
        write_unit_dir(&p, "depcrate", "5c0d43421562aa29", "lib-depcrate", dep, &[]);
        write_unit_dir(&p, "app", "2ec6ba8b5b9de2ba", "lib-app", 7, &[dep]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 1,
                unresolved: 0,
                ambiguous: 0
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_mixed_profile_resolves_an_edge_from_a_unit_dir_unit_to_a_flat_unit() {
        let p = scratch();
        let (a, b) = (0x0102_0304_0506_0708u64, 17_112_352_188_342_185_299u64);
        write_unit(&p, &format!("dep-{}", le_hex(b)), "lib-dep", b, &[]);
        write_unit_dir(&p, "app", &le_hex(a), "lib-app", a, &[b]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 2);
        assert_eq!(
            r.layouts,
            BTreeSet::from([UnitLayout::Flat, UnitLayout::UnitDir])
        );
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 1,
                unresolved: 0,
                ambiguous: 0
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_dir_with_only_a_start_stamp_and_cached_messages_is_not_current() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        let failed = p.join("build/failed/fedcba9876543210/fingerprint");
        fs::create_dir_all(&failed).unwrap();
        fs::write(failed.join("invoked.timestamp"), "").unwrap();
        fs::write(failed.join("output-lib-failed"), "{}").unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!((r.units, r.not_current), (1, 1));
        assert_eq!(r.layouts, BTreeSet::from([UnitLayout::UnitDir]));
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_dir_with_an_empty_hash_file_is_not_current() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "dep", "0123456789abcdef", "lib-dep", 1, &[]);
        write_unit_dir(&p, "user", "1123456789abcdef", "lib-user", 2, &[1]);
        let dep = p.join("build/dep/0123456789abcdef/fingerprint");
        fs::write(dep.join("lib-dep"), "").unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!((r.units, r.not_current), (1, 1));
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 0,
                unresolved: 1,
                ambiguous: 0
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_hex_dir_with_no_or_empty_fingerprint_is_a_partial_unit_dir() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        fs::create_dir_all(p.join("build/pkg/0123456789abcdef")).unwrap();
        fs::create_dir_all(p.join("build/pkg/1123456789abcdef/fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "2123456789abcdef", "lib-ok", 1, &[]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!((r.units, r.partial_unit_dirs), (1, 2));
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn the_empty_unit_output_reader_still_counts_incomplete_units_with_artifacts() {
        let p = scratch();
        let output = p.join("build/lib/0123456789abcdef/out");
        fs::create_dir_all(&output).unwrap();
        let host = cargo_hive_platform::host_fs();
        let reader = StrictProfileReader::new(host.as_ref(), &|| false);
        assert_eq!(reader.read_profile(&p).partial_unit_dirs, 1);
        let empty = reader.read_profile_with_empty_unit_outputs(&p);
        assert_eq!(empty.format, Format::Supported);
        assert_eq!(empty.partial_unit_dirs, 0);

        fs::write(output.join("lib.rlib"), b"unproved output").unwrap();
        let occupied = reader.read_profile_with_empty_unit_outputs(&p);
        assert_eq!(occupied.format, Format::Supported);
        assert_eq!(occupied.partial_unit_dirs, 1);

        fs::remove_file(output.join("lib.rlib")).unwrap();
        fs::create_dir(p.join("build/lib/0123456789abcdef/fingerprint")).unwrap();
        let fingerprint = reader.read_profile_with_empty_unit_outputs(&p);
        assert_eq!(fingerprint.partial_unit_dirs, 1);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn two_jsons_in_a_unit_dir_fingerprint_make_the_profile_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        write_unit_dir(&p, "q", "1123456789abcdef", "lib-q", 2, &[]);
        fs::write(
            p.join("build/q/1123456789abcdef/fingerprint/lib-r.json"),
            unit_json(&[]),
        )
        .unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(_)), "{:?}", r.format);
        assert_eq!(r.edges, None);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_file_directly_in_build_makes_the_profile_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        fs::write(p.join("build/stray"), b"x").unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(_)), "{:?}", r.format);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_package_dir_holding_a_non_hex_dir_makes_the_profile_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        fs::create_dir_all(p.join("build/pkg/not-a-hash")).unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(_)), "{:?}", r.format);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_build_dir_that_is_neither_a_package_nor_a_flat_build_script_is_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        fs::create_dir_all(p.join("build/weird-name/out")).unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(_)), "{:?}", r.format);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn an_empty_package_dir_is_accepted() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        write_unit_dir(&p, "ok", "0123456789abcdef", "lib-ok", 1, &[]);
        fs::create_dir_all(p.join("build/empty-pkg")).unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 1);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_flat_and_unit_dir_unit_with_the_same_hash_make_an_edge_ambiguous() {
        let p = scratch();
        let shared = 7u64;
        write_unit(&p, "flat-0000000000000001", "lib-flat", shared, &[]);
        write_unit_dir(&p, "udir", &le_hex(shared), "lib-udir", shared, &[]);
        write_unit(&p, "user-0000000000000003", "lib-user", 9, &[shared]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 3);
        assert_eq!(
            r.edges,
            Some(EdgeCounts {
                resolved: 0,
                unresolved: 0,
                ambiguous: 1
            })
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn layouts_names_flat_unit_dir_or_both() {
        let flat = scratch();
        write_unit(&flat, "a-0000000000000001", "lib-a", 1, &[]);
        let r = read_profile(&flat);
        assert_eq!(r.layouts, BTreeSet::from([UnitLayout::Flat]));
        fs::remove_dir_all(flat).unwrap();

        let unit = scratch();
        fs::remove_dir_all(unit.join(".fingerprint")).unwrap();
        write_unit_dir(&unit, "a", "0123456789abcdef", "lib-a", 1, &[]);
        let r = read_profile(&unit);
        assert_eq!(r.layouts, BTreeSet::from([UnitLayout::UnitDir]));
        fs::remove_dir_all(unit).unwrap();

        let mixed = scratch();
        write_unit(&mixed, "a-0000000000000001", "lib-a", 1, &[]);
        write_unit_dir(&mixed, "b", "1123456789abcdef", "lib-b", 2, &[]);
        let r = read_profile(&mixed);
        assert_eq!(
            r.layouts,
            BTreeSet::from([UnitLayout::Flat, UnitLayout::UnitDir])
        );
        fs::remove_dir_all(mixed).unwrap();
    }

    #[test]
    fn a_unit_keeps_its_stem_features_coverage_and_hashes() {
        let cases = [
            (
                r#"["testkit"]"#,
                r#"["testkit"]"#,
                Coverage::AllDeclared,
                Some("all_declared"),
            ),
            ("[]", "[]", Coverage::None, Some("none")),
            (
                r#"["a"]"#,
                r#"["a","b"]"#,
                Coverage::Partial,
                Some("partial"),
            ),
            ("", "", Coverage::NotRecorded, Some("not_recorded")),
            ("not json", "[]", Coverage::Unreadable, None),
        ];
        let p = scratch();
        for (i, (features, declared, _, _)) in cases.iter().enumerate() {
            write_unit_with(
                &p,
                &format!("q{i}-000000000000000{}", i + 1),
                "lib-q",
                (i as u64) + 1,
                &[],
                features,
                declared,
            );
        }
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, cases.len());
        let units = r.graph.expect("a supported profile keeps its units").units;
        for (features, _, coverage, label) in cases {
            let unit = units
                .iter()
                .find(|u| u.features.as_ref() == features)
                .unwrap_or_else(|| panic!("no unit with features {features:?}"));
            assert_eq!(unit.coverage, coverage, "{features:?}");
            assert_eq!(unit.coverage.as_str(), label, "{features:?}");
            assert_eq!(unit.stem, "lib-q", "{features:?}");
            assert_eq!(
                (unit.profile, unit.target, unit.path),
                (2, 1, 3),
                "{features:?}"
            );
        }
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn units_with_the_same_features_share_one_string() {
        let p = scratch();
        write_unit(&p, "a-0000000000000001", "lib-a", 1, &[]);
        write_unit(&p, "b-0000000000000002", "lib-b", 2, &[]);
        let r = read_profile(&p);
        let units = r.graph.expect("supported").units;
        assert!(
            Arc::ptr_eq(&units[0].features, &units[1].features),
            "both units are features \"[]\""
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn not_current_units_are_listed_with_what_explain_needs() {
        let never = scratch();
        write_unit(&never, "ok-0000000000000001", "lib-ok", 1, &[]);
        let failed = never.join(".fingerprint/failed-0000000000000002");
        fs::create_dir_all(&failed).unwrap();
        fs::write(failed.join("invoked.timestamp"), "").unwrap();
        fs::write(failed.join("output-lib-failed"), "{}").unwrap();
        let r = read_profile(&never);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.not_current, r.not_current_units.len());
        assert_eq!(
            r.not_current_units,
            vec![NotCurrentUnit {
                dir: failed,
                loc: UnitLoc {
                    layout: UnitLayout::Flat,
                    name_meta: "failed-0000000000000002".into(),
                },
                stem: None,
                features: None,
                outputs: vec!["output-lib-failed".to_string()],
                deps: Vec::new(),
            }]
        );
        fs::remove_dir_all(never).unwrap();

        let emptied = scratch();
        write_unit(&emptied, "dep-0000000000000001", "lib-dep", 1, &[]);
        write_unit(&emptied, "user-0000000000000002", "lib-user", 2, &[1]);
        let dep = emptied.join(".fingerprint/dep-0000000000000001");
        fs::write(dep.join("lib-dep"), "").unwrap();
        fs::write(dep.join("dep-lib-dep"), "dep-info").unwrap();
        let r = read_profile(&emptied);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.not_current, r.not_current_units.len());
        assert_eq!(
            r.not_current_units,
            vec![NotCurrentUnit {
                dir: dep,
                loc: UnitLoc {
                    layout: UnitLayout::Flat,
                    name_meta: "dep-0000000000000001".into(),
                },
                stem: Some("lib-dep".to_string()),
                features: Some(Arc::from("[]")),
                outputs: vec![],
                deps: vec![],
            }]
        );
        fs::remove_dir_all(emptied).unwrap();

        let unsupported = scratch();
        fs::remove_dir_all(unsupported.join(".fingerprint")).unwrap();
        let r = read_profile(&unsupported);
        assert!(matches!(r.format, Format::Unsupported(_)));
        assert_eq!(r.not_current, 0);
        assert!(r.not_current_units.is_empty());
        fs::remove_dir_all(unsupported).unwrap();
    }

    fn unit_json_edges(edges: &[(u64, u64)]) -> String {
        let deps: Vec<String> = edges
            .iter()
            .map(|(package, fingerprint)| format!("[{package},\"dep\",false,{fingerprint}]"))
            .collect();
        unit_json(&[]).replace("\"deps\":[]", &format!("\"deps\":[{}]", deps.join(",")))
    }

    fn write_unit_edges(profile: &Path, dir: &str, stem: &str, value: u64, edges: &[(u64, u64)]) {
        write_unit(profile, dir, stem, value, &[]);
        let d = profile.join(".fingerprint").join(dir);
        fs::write(d.join(format!("{stem}.json")), unit_json_edges(edges)).unwrap();
    }

    #[test]
    fn a_unit_keeps_the_package_id_of_each_of_its_edges() {
        let p = scratch();
        write_unit_edges(&p, "u-0000000000000001", "lib-u", 1, &[(11, 7), (12, 9)]);
        let r = read_profile(&p);
        let graph = r.graph.unwrap();
        assert_eq!(
            graph.units[0].deps,
            vec![
                DepEdge {
                    package: 11,
                    fingerprint: 7
                },
                DepEdge {
                    package: 12,
                    fingerprint: 9
                }
            ]
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_with_an_emptied_hash_file_lists_the_edges_of_its_stale_json() {
        let p = scratch();
        write_unit_edges(&p, "u-0000000000000001", "lib-u", 1, &[(11, 7), (12, 9)]);
        fs::write(p.join(".fingerprint/u-0000000000000001/lib-u"), "").unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(
            r.not_current_units[0].deps,
            vec![
                DepEdge {
                    package: 11,
                    fingerprint: 7
                },
                DepEdge {
                    package: 12,
                    fingerprint: 9
                }
            ]
        );
        assert_eq!(
            r.edges,
            Some(EdgeCounts::default()),
            "a not-current unit's edges are not counted"
        );
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_unit_that_never_compiled_has_no_edges() {
        let p = scratch();
        let failed = p.join(".fingerprint/failed-0000000000000002");
        fs::create_dir_all(&failed).unwrap();
        fs::write(failed.join("invoked.timestamp"), "").unwrap();
        fs::write(failed.join("output-lib-failed"), "{}").unwrap();
        let r = read_profile(&p);
        assert!(r.not_current_units[0].deps.is_empty());
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_profile_listing_past_the_entry_cap_is_unsupported() {
        let p = scratch();
        write_unit(&p, "a-0000000000000001", "lib-a", 1, &[]);
        write_unit(&p, "b-0000000000000002", "lib-b", 2, &[]);
        write_unit(&p, "c-0000000000000003", "lib-c", 3, &[]);
        let r = read_profile_capped(&p, 2);
        assert!(
            matches!(
                r.format,
                Format::Unsupported(ref why) if why.contains("2") && why.contains("entries")
            ),
            "{:?}",
            r.format
        );
        assert_eq!(r.edges, None);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn a_profile_with_neither_fingerprint_nor_unit_dirs_names_both() {
        let unsupported = scratch();
        fs::remove_dir_all(unsupported.join(".fingerprint")).unwrap();
        let r = read_profile(&unsupported);
        assert!(
            matches!(
                r.format,
                Format::Unsupported(ref why)
                    if why.starts_with("no .fingerprint/")
                        && why.contains("no unit dirs")
                        && !why.contains("newer")
            ),
            "{:?}",
            r.format
        );
        fs::remove_dir_all(unsupported).unwrap();
    }

    #[test]
    fn the_summary_line_is_recognized_anywhere_and_only_in_its_exact_form() {
        let summary = r#"{"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","spans":[]}"#;
        let coded = summary.replacen("\"code\":null", "\"code\":{\"code\":\"E0308\"}", 1);
        let failure_note = r#"{"$message_type":"diagnostic","message":"For more information about this error, try `rustc --explain E0308`.","code":null,"level":"failure-note","spans":[]}"#;
        let dead_code = r#"{"$message_type":"diagnostic","message":"function is never used","code":{"code":"dead_code","explanation":null},"level":"error","spans":[]}"#;
        assert_eq!(is_summary_line(summary), Some(true));
        assert_eq!(is_summary_line(&coded), Some(false));
        assert_eq!(is_summary_line(failure_note), Some(false));
        assert_eq!(
            is_summary_line(r#"{"$message_type":"future_incompat"}"#),
            Some(false)
        );
        assert_eq!(is_summary_line("not json"), None);
        assert_eq!(error_code(&coded), Some("E0308".to_string()));
        assert_eq!(error_code(dead_code), Some("dead_code".to_string()));
        assert_eq!(error_code(summary), None);
    }
}
