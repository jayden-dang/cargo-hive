//! Cargo `.fingerprint/` reader (ADR-0001). A unit dir holds exactly one `<H>.json`
//! and a sibling `<H>` with 16 lowercase hex chars. Its JSON must have exactly the 11
//! known keys with known types. Any deviation makes the whole profile dir unsupported.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub dir_name: String,
    pub kind: String,
    pub hash_hex: String,
    pub deps: Vec<u64>,
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

fn kind_of(stem: &str) -> String {
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

fn read_unit(dir: &Path, json_name: &str) -> Result<Unit, String> {
    let stem = json_name.trim_end_matches(".json");
    let hash = fs::read(dir.join(stem)).map_err(|e| format!("hash file `{stem}`: {e}"))?;
    if !is_hash(&hash) {
        return Err(format!(
            "hash file `{stem}` is not 16 lowercase hex characters"
        ));
    }
    let json_path = dir.join(json_name);
    let len = fs::metadata(&json_path)
        .map_err(|e| format!("{json_name}: {e}"))?
        .len();
    if len > 4 * 1024 * 1024 {
        return Err(format!("{json_name}: larger than 4 MiB"));
    }
    let bytes = fs::read(&json_path).map_err(|e| format!("{json_name}: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{json_name}: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("{json_name} is not a JSON object"))?;
    let deps = check_types(obj)?;
    Ok(Unit {
        dir_name: dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        kind: kind_of(stem),
        hash_hex: String::from_utf8(hash).expect("checked ASCII hex"),
        deps,
    })
}

pub fn read_profile(profile_dir: &Path) -> ProfileReport {
    let path = profile_dir.to_path_buf();
    let unsupported = |units: usize, reason: String| ProfileReport {
        path: path.clone(),
        format: Format::Unsupported(reason),
        units,
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
        let jsons: Vec<&String> = names.iter().filter(|n| n.ends_with(".json")).collect();
        if jsons.len() != 1 {
            problem.get_or_insert(format!(
                "{}: {} fingerprint JSON files",
                dir.display(),
                jsons.len()
            ));
            continue;
        }
        counted += 1;
        match read_unit(&dir, jsons[0]) {
            Ok(u) => units.push(u),
            Err(reason) => {
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
        edges: Some(counts),
        graph: Some(UnitGraph { units, edges }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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
    fn a_profile_without_a_fingerprint_dir_is_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        fs::create_dir_all(p.join("build/pkg/0123456789abcdef/fingerprint")).unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(ref why) if why.contains(".fingerprint")));
        assert_eq!((r.units, r.edges), (0, None));
        fs::remove_dir_all(p).unwrap();
    }
}
