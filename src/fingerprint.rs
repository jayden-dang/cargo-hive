//! Cargo `.fingerprint/` reader (ADR-0001). A unit dir holds exactly one `<H>.json`
//! and a sibling `<H>` with 16 lowercase hex chars. Its JSON must have exactly the 11
//! known keys with known types. Any deviation makes the whole profile dir unsupported.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
const KINDS: [&str; 9] = [
    "run-build-script",
    "build-script",
    "doc-lib",
    "doc-bin",
    "test",
    "lib",
    "bin",
    "example",
    "bench",
];

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub dir_name: String,
    pub kind: String,
    pub hash_hex: String,
    pub deps: Vec<u64>,
    #[serde(skip)]
    pub stem: String,
    #[serde(skip)]
    pub features: Arc<str>,
    #[serde(skip)]
    pub coverage: Coverage,
    #[serde(skip)]
    pub profile: u64,
    #[serde(skip)]
    pub target: u64,
    #[serde(skip)]
    pub path: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotCurrentUnit {
    pub dir: PathBuf,
    pub stem: Option<String>,
    pub features: Option<Arc<str>>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EdgeCounts {
    pub resolved: u64,
    pub unresolved: u64,
    pub ambiguous: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitGraph {
    pub units: Vec<Unit>,
    pub edges: Vec<(usize, usize)>,
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
}

/// Cargo's `to_hex`: the u64's 8 bytes in little-endian order, as lowercase hex.
pub fn le_hex(value: u64) -> String {
    value
        .to_le_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn kind_of(stem: &str) -> String {
    KINDS
        .iter()
        .find(|k| stem == **k || stem.starts_with(&format!("{k}-")))
        .map(|k| k.to_string())
        .unwrap_or_else(|| "other".into())
}

fn is_hash(bytes: &[u8]) -> bool {
    bytes.len() == 16
        && bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

pub fn unit_hash(dir_name: &str) -> Option<&str> {
    let (_, tail) = dir_name.rsplit_once('-')?;
    is_hash(tail.as_bytes()).then_some(tail)
}

pub fn dep_name(fingerprint_dir: &Path, stem: &str, dep: u64) -> Option<String> {
    let mut file = fs::File::open(fingerprint_dir.join(format!("{stem}.json"))).ok()?;
    let bytes = read_capped(&mut file).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
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

fn check_types(obj: &Map<String, Value>) -> Result<Vec<u64>, String> {
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
                Ok(f.as_u64().unwrap())
            }
            _ => Err(format!("`deps` entry {d} is not [u64, string, bool, u64]")),
        })
        .collect()
}

pub(crate) fn read_capped(reader: &mut impl Read) -> Result<Vec<u8>, String> {
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
    NotCurrent { stem: String, features: Arc<str> },
}

/// An emptied hash is a unit Cargo is rebuilding. The JSON beside it is stale, so the unit is
/// not current; its stem and features are still the ones the last successful build wrote.
fn read_unit(
    dir: &Path,
    json_name: &str,
    shared_features: &mut HashMap<String, Arc<str>>,
) -> Result<ParsedUnit, String> {
    let stem = json_name.trim_end_matches(".json");
    let hash = fs::read(dir.join(stem)).map_err(|e| format!("hash file `{stem}`: {e}"))?;
    if !hash.is_empty() && !is_hash(&hash) {
        return Err(format!(
            "hash file `{stem}` is not 16 lowercase hex characters"
        ));
    }
    let json_path = dir.join(json_name);
    let len = fs::metadata(&json_path)
        .map_err(|e| format!("{json_name}: {e}"))?
        .len();
    if len > MAX_JSON_BYTES {
        return Err(format!("{json_name}: larger than 4 MiB"));
    }
    let mut file = fs::File::open(&json_path).map_err(|e| format!("{json_name}: {e}"))?;
    let bytes = read_capped(&mut file).map_err(|e| format!("{json_name}: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{json_name}: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("{json_name} is not a JSON object"))?;
    let deps = check_types(obj)?;
    let features_raw = obj["features"]
        .as_str()
        .ok_or("`features` is not a string")?;
    let declared_raw = obj["declared_features"]
        .as_str()
        .ok_or("`declared_features` is not a string")?;
    let features = intern(shared_features, features_raw);
    let stem = stem.to_string();
    if hash.is_empty() {
        return Ok(ParsedUnit::NotCurrent { stem, features });
    }
    Ok(ParsedUnit::Current(Unit {
        dir_name: dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        kind: kind_of(&stem),
        hash_hex: String::from_utf8(hash).expect("checked ASCII hex"),
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
    names.iter().any(|n| n == "invoked.timestamp")
        && names.iter().any(|n| n.starts_with("output-"))
        && names
            .iter()
            .all(|n| n == "invoked.timestamp" || n.starts_with("output-"))
}

pub fn read_profile(profile_dir: &Path) -> ProfileReport {
    let path = profile_dir.to_path_buf();
    let unsupported = |units: usize, reason: String| ProfileReport {
        path: path.clone(),
        format: Format::Unsupported(reason),
        units,
        not_current: 0,
        not_current_units: Vec::new(),
        edges: None,
        graph: None,
    };
    let fp = profile_dir.join(".fingerprint");
    if !fp.is_dir() {
        return unsupported(
            0,
            "no .fingerprint/ directory (newer or unknown Cargo layout)".into(),
        );
    }
    let entries = match fs::read_dir(&fp) {
        Ok(e) => e,
        Err(e) => return unsupported(0, format!("cannot read .fingerprint/: {e}")),
    };
    let mut units = Vec::new();
    let mut counted = 0usize;
    let mut not_current_units = Vec::new();
    let mut shared_features: HashMap<String, Arc<str>> = HashMap::new();
    let mut problem: Option<String> = None;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                problem.get_or_insert(format!("cannot read .fingerprint/: {e}"));
                continue;
            }
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(e) => {
                problem.get_or_insert(format!("{}: {e}", entry.path().display()));
            }
        }
    }
    dirs.sort();
    for dir in dirs {
        let names: Vec<String> = match fs::read_dir(&dir) {
            Ok(entries) => {
                let mut names = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(entry) => names.push(entry.file_name().to_string_lossy().to_string()),
                        Err(e) => {
                            problem.get_or_insert(format!("{}: {e}", dir.display()));
                        }
                    }
                }
                names
            }
            Err(e) => {
                problem.get_or_insert(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        if names.is_empty() {
            continue;
        }
        if never_compiled(&names) {
            not_current_units.push(NotCurrentUnit {
                dir: dir.clone(),
                stem: None,
                features: None,
                outputs: names
                    .iter()
                    .filter(|n| n.starts_with("output-"))
                    .cloned()
                    .collect(),
            });
            continue;
        }
        let jsons: Vec<&String> = names.iter().filter(|n| n.ends_with(".json")).collect();
        if jsons.len() != 1 {
            problem.get_or_insert(format!(
                "{}: {} fingerprint JSON files",
                dir.display(),
                jsons.len()
            ));
            continue;
        }
        match read_unit(&dir, jsons[0], &mut shared_features) {
            Ok(ParsedUnit::Current(u)) => {
                counted += 1;
                units.push(u);
            }
            Ok(ParsedUnit::NotCurrent { stem, features }) => {
                not_current_units.push(NotCurrentUnit {
                    dir: dir.clone(),
                    stem: Some(stem),
                    features: Some(features),
                    outputs: names
                        .iter()
                        .filter(|n| n.starts_with("output-"))
                        .cloned()
                        .collect(),
                });
            }
            Err(reason) => {
                counted += 1;
                problem.get_or_insert(format!("{}: {reason}", dir.display()));
            }
        }
    }
    if let Some(reason) = problem {
        return unsupported(counted, reason);
    }
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        index.entry(u.hash_hex.as_str()).or_default().push(i);
    }
    let mut counts = EdgeCounts::default();
    let mut edges = Vec::new();
    for (i, u) in units.iter().enumerate() {
        for dep in &u.deps {
            match index.get(le_hex(*dep).as_str()).map(Vec::as_slice) {
                Some([j]) => {
                    counts.resolved += 1;
                    edges.push((i, *j));
                }
                Some(_) => counts.ambiguous += 1,
                None => counts.unresolved += 1,
            }
        }
    }
    ProfileReport {
        path,
        format: Format::Supported,
        units: counted,
        not_current: not_current_units.len(),
        not_current_units,
        edges: Some(counts),
        graph: Some(UnitGraph { units, edges }),
    }
}

#[cfg(test)]
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
        assert_eq!(unit_hash("dep-4360aef667e9273e"), Some("4360aef667e9273e"));
        assert_eq!(
            unit_hash("build_script_build-9a4d89fe046fe1ba"),
            Some("9a4d89fe046fe1ba")
        );
        assert_eq!(unit_hash("dep-4360AEF667E9273E"), None);
        assert_eq!(unit_hash("dep-4360aef667e9273"), None);
        assert_eq!(unit_hash("nodash"), None);
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
        let kinds: Vec<String> = r
            .graph
            .unwrap()
            .units
            .iter()
            .map(|u| u.kind.clone())
            .collect();
        assert!(
            kinds.contains(&"doc-lib".to_string())
                && kinds.contains(&"run-build-script".to_string()),
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
    fn a_fingerprint_stream_longer_than_its_size_is_cut_off_at_four_mebibytes() {
        let p = scratch();
        let dir = p.join(".fingerprint/fifo-0000000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib-fifo"), le_hex(1)).unwrap();
        let fifo = dir.join("lib-fifo.json");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
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
    fn a_profile_without_a_fingerprint_dir_is_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        fs::create_dir_all(p.join("build/pkg/0123456789abcdef/fingerprint")).unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(ref why) if why.contains(".fingerprint")));
        assert_eq!((r.units, r.edges), (0, None));
        fs::remove_dir_all(p).unwrap();
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
                stem: None,
                features: None,
                outputs: vec!["output-lib-failed".to_string()],
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
                stem: Some("lib-dep".to_string()),
                features: Some(Arc::from("[]")),
                outputs: vec![],
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
