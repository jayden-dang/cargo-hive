//! Fixed project configuration and output paths for the sealed Nextest recipes.

use super::{ENTRY_LIMIT, Refusal, digest, read_input_bounded, refused};
use crate::slot::canonical_worktree_path;
use cargo_hive_platform::flock::open_owner_state_file;
use cargo_hive_state::slots::{self, GraphExpectation, GraphInput, InputIdentity, Locator};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

const NEXTEST_CONFIG: &str = ".config/nextest.toml";
const NEXTEST_CONFIG_LIMIT: usize = 64 * 1024;
const NEXTEST_STORE: &str = "target/nextest";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectConfig {
    sha256: String,
    profile: String,
    store: PathBuf,
}

impl ProjectConfig {
    pub(super) fn valid(&self) -> bool {
        slots::hex(&self.sha256, 64)
            && self.profile == "default"
            && self.store.as_os_str() == NEXTEST_STORE
    }

    pub(super) fn fixed_in(&self, inputs: &[GraphInput]) -> bool {
        inputs.iter().any(|input| {
            matches!(&input.locator, Locator::Checkout { path } if path.as_os_str() == NEXTEST_CONFIG)
                && match &input.expectation {
                    GraphExpectation::Present {
                        identity: InputIdentity::ReviewedBytes { sha256 },
                    } => sha256 == &self.sha256,
                    GraphExpectation::Present {
                        identity: InputIdentity::GitObject { .. },
                    } => true,
                    _ => false,
                }
        })
    }

    fn verify_store(&self, checkout: &Path) -> Result<(), Refusal> {
        let owner = slots::owner_directory(checkout).map_err(|_| nextest_config_refusal())?;
        let store = checkout.join(&self.store);
        let profile = store.join(&self.profile);
        let report = profile.join("junit.xml");
        for directory in [checkout.join("target"), store, profile] {
            verify_nextest_path(&directory, owner.dev(), true)?;
        }
        verify_nextest_path(&report, owner.dev(), false)
    }
}

fn nextest_config_refusal() -> Refusal {
    refused("The reviewed Nextest project configuration or default store could not be verified.")
}

fn verify_nextest_path(path: &Path, device: u64, directory: bool) -> Result<(), Refusal> {
    if canonical_worktree_path(path)
        .ok()
        .is_none_or(|canonical| canonical.as_os_str() != path.as_os_str())
    {
        return Err(nextest_config_refusal());
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let metadata = if directory {
                slots::owner_directory(path)
            } else {
                open_owner_state_file(path).and_then(|file| file.metadata())
            }
            .map_err(|_| nextest_config_refusal())?;
            if metadata.dev() != device {
                return Err(nextest_config_refusal());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(nextest_config_refusal()),
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NextestSection {
    Default,
    DefaultJunit,
    Ci,
    CiJunit,
    Fast,
    Groups,
    Override,
}

impl NextestSection {
    fn read(header: &str) -> Option<Self> {
        Some(match header {
            "[profile.default]" => Self::Default,
            "[profile.default.junit]" => Self::DefaultJunit,
            "[profile.ci]" => Self::Ci,
            "[profile.ci.junit]" => Self::CiJunit,
            "[profile.fast]" => Self::Fast,
            "[test-groups]" => Self::Groups,
            "[[profile.default.overrides]]" => Self::Override,
            _ => return None,
        })
    }
}

fn positive_decimal(value: &str) -> bool {
    !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u16>().is_ok_and(|number| number > 0)
}

fn seconds(value: &str) -> bool {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix("s\""))
        .is_some_and(positive_decimal)
}

fn inline_settings<'a, const N: usize>(source: &'a str, keys: [&str; N]) -> Option<[&'a str; N]> {
    let body = source.strip_prefix('{')?.strip_suffix('}')?;
    let mut values = [""; N];
    for field in body.split(',') {
        let (key, value) = field.split_once('=')?;
        let index = keys.iter().position(|candidate| *candidate == key.trim())?;
        if !values[index].is_empty() || value.trim().is_empty() {
            return None;
        }
        values[index] = value.trim();
    }
    values
        .iter()
        .all(|value| !value.is_empty())
        .then_some(values)
}

fn nextest_literal(value: &str) -> Option<&str> {
    value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .filter(|value| !value.trim().is_empty() && !value.contains('\''))
}

fn nextest_group(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

// This closed TOML subset recognizes only the reviewed scheduling and reporting fields.
// Quoted filter expressions are data, and cannot end their literal except at its checked delimiter.
fn reviewed_nextest_config(source: &[u8]) -> bool {
    if !source
        .iter()
        .all(|byte| matches!(byte, b'\t' | b'\n' | b' '..=b'~'))
    {
        return false;
    }
    let Ok(text) = std::str::from_utf8(source) else {
        return false;
    };
    if text.lines().count() > ENTRY_LIMIT {
        return false;
    }
    let mut lines = text.lines();
    let mut section = None;
    let mut headers = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut referenced_groups = BTreeSet::new();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let Some(next) = NextestSection::read(line) else {
                return false;
            };
            if (section == Some(NextestSection::Override) && !keys.contains("filter"))
                || (next != NextestSection::Override && !headers.insert(next))
            {
                return false;
            }
            section = Some(next);
            keys.clear();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        let (key, value) = (key.trim(), value.trim());
        if !keys.insert(key) {
            return false;
        }
        use NextestSection::{Ci, CiJunit, Default, DefaultJunit, Fast, Groups, Override};
        let allowed = match (section, key) {
            (Some(Default), "test-threads") => positive_decimal(value),
            (Some(Default), "slow-timeout") => {
                inline_settings(value, ["period", "terminate-after"])
                    .is_some_and(|[period, after]| seconds(period) && positive_decimal(after))
            }
            (Some(DefaultJunit | CiJunit), "path") => value == "\"junit.xml\"",
            (Some(Ci | Fast), "inherits") => value == "\"default\"",
            (Some(Ci), "fail-fast") => matches!(value, "true" | "false"),
            (Some(Ci), "retries") => inline_settings(value, ["backoff", "count", "delay"])
                .is_some_and(|[backoff, count, delay]| {
                    backoff == "\"fixed\"" && positive_decimal(count) && seconds(delay)
                }),
            (Some(Fast), "default-filter") if value == "'''" => {
                let mut populated = false;
                loop {
                    match lines.next() {
                        Some(line) if line.trim() == "'''" => break populated,
                        Some(line) if !line.contains('\'') => populated |= !line.trim().is_empty(),
                        _ => break false,
                    }
                }
            }
            (Some(Fast), "default-filter") | (Some(Override), "filter") => {
                nextest_literal(value).is_some()
            }
            (Some(Groups), key) => {
                nextest_group(key)
                    && inline_settings(value, ["max-threads"])
                        .is_some_and(|[threads]| positive_decimal(threads))
                    && groups.insert(key)
            }
            (Some(Override), "test-group") => nextest_literal(value)
                .filter(|group| nextest_group(group))
                .is_some_and(|group| {
                    referenced_groups.insert(group);
                    true
                }),
            (Some(Override), "priority") => positive_decimal(value) && value.parse::<i8>().is_ok(),
            _ => false,
        };
        if !allowed {
            return false;
        }
    }
    headers.contains(&NextestSection::Default)
        && (section != Some(NextestSection::Override) || keys.contains("filter"))
        && referenced_groups.is_subset(&groups)
}

pub(super) fn overrides_configuration(name: &str) -> bool {
    name == "NEXTEST" || name.starts_with("NEXTEST_")
}

pub(super) fn verify_project(
    checkout: &Path,
    expected: Option<&ProjectConfig>,
) -> Result<(), Refusal> {
    let bytes = read_input_bounded(&checkout.join(NEXTEST_CONFIG), NEXTEST_CONFIG_LIMIT)?;
    match (expected, bytes) {
        (None, None) => Ok(()),
        (Some(expected), Some(bytes))
            if expected.valid()
                && digest(&bytes)? == expected.sha256
                && reviewed_nextest_config(&bytes) =>
        {
            expected.verify_store(checkout)
        }
        _ => Err(nextest_config_refusal()),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{NEXTEST_STORE, ProjectConfig, digest, overrides_configuration, verify_project};
    use cargo_hive_state::slots::GraphInput;
    use serde_json::json;

    const PROJECT_CONFIG: &str = r#"
[profile.default]
test-threads = 8
slow-timeout = { period = "30s", terminate-after = 6 }

[profile.default.junit]
path = "junit.xml"

[profile.ci]
inherits = "default"
fail-fast = false
retries = { backoff = "fixed", count = 2, delay = "1s" }
[profile.ci.junit]
path = "junit.xml"

[profile.fast]
inherits = "default"
default-filter = '''
not (
    package(example_worker)
    or (binary_id(gateways::it) and test(/^slow::/))
)
'''

[test-groups]
live-faults = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'binary_id(gateways::it) and test(/^slow::/)'
test-group = 'live-faults'

[[profile.default.overrides]]
filter = 'package(example_worker)'
priority = 100
"#;

    fn project(source: &str) -> ProjectConfig {
        ProjectConfig {
            sha256: digest(source.as_bytes()).unwrap_or_else(|_| panic!("fixture hash")),
            profile: "default".into(),
            store: NEXTEST_STORE.into(),
        }
    }

    #[test]
    fn reviewed_nextest_config_data_requires_exact_bytes_and_confined_default_store() {
        use cargo_hive_platform::host_fs;
        use std::{fs, os::unix::fs::symlink, path::Path};

        let fixture = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-nextest-config-")
            .unwrap();
        let checkout = fixture.path().join("checkout");
        fs::create_dir_all(checkout.join(".config")).unwrap();
        let path = checkout.join(".config/nextest.toml");
        let candidate = project(PROJECT_CONFIG);
        assert!(verify_project(&checkout, None).is_ok());
        assert!(verify_project(&checkout, Some(&candidate)).is_err());
        {
            fs::write(&path, PROJECT_CONFIG).unwrap();
            assert!(verify_project(&checkout, None).is_err());
            assert!(
                verify_project(&checkout, Some(&candidate)).is_ok(),
                "the verified project configuration should qualify as data"
            );

            let changed = PROJECT_CONFIG.replace("threads = 8", "threads = 9");
            fs::write(&path, changed).unwrap();
            assert!(
                verify_project(&checkout, Some(&candidate)).is_err(),
                "reviewed bytes changed"
            );

            for changed in [
                format!("{PROJECT_CONFIG}\n[scripts.danger]\ncommand = 'example'\n"),
                format!(
                    "{PROJECT_CONFIG}\n[target.x86_64-unknown-linux-gnu]\nrunner = 'example'\n"
                ),
                PROJECT_CONFIG.replace("priority = 100", "script = 'example'"),
                PROJECT_CONFIG.replace("priority = 100", "unknown = 1"),
                PROJECT_CONFIG.replace("path = \"junit.xml\"", "path = \"../../foreign.xml\""),
                format!("{PROJECT_CONFIG}\n[store]\ndir = '../outside'\n"),
                PROJECT_CONFIG.replace("inherits = \"default\"", "inherits = \"alternate\""),
                PROJECT_CONFIG.replace("test-threads = 8", "test-threads = 8\ntest-threads = 2"),
                PROJECT_CONFIG.replace(
                    "    package(example_worker)",
                    "    package(example_worker) \'\'\'",
                ),
                PROJECT_CONFIG
                    .replace("test-group = 'live-faults'", "test-group = 'unknown-group'"),
            ] {
                fs::write(&path, &changed).unwrap();
                assert!(
                    verify_project(&checkout, Some(&project(&changed))).is_err(),
                    "a matching digest cannot approve out-of-scope config syntax"
                );
            }

            let mut override_profile = project(PROJECT_CONFIG);
            override_profile.profile = "ci".into();
            fs::write(&path, PROJECT_CONFIG).unwrap();
            assert!(verify_project(&checkout, Some(&override_profile)).is_err());
            let mut override_store = project(PROJECT_CONFIG);
            override_store.store = "../outside".into();
            assert!(verify_project(&checkout, Some(&override_store)).is_err());
        }

        let store = checkout.join("target/nextest");
        let default = store.join("default");
        fs::create_dir_all(&default).unwrap();
        let report = default.join("junit.xml");
        fs::write(&report, b"prior report").unwrap();
        assert!(verify_project(&checkout, Some(&candidate)).is_ok());

        let outside = fixture.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let external_report = outside.join("report.xml");
        fs::write(&external_report, b"outside report").unwrap();
        fs::remove_file(&report).unwrap();
        symlink(&external_report, &report).unwrap();
        assert!(
            verify_project(&checkout, Some(&candidate)).is_err(),
            "report escaped physically"
        );

        fs::rename(&store, fixture.path().join("prior-store")).unwrap();
        symlink(&outside, &store).unwrap();
        assert!(
            verify_project(&checkout, Some(&candidate)).is_err(),
            "store escaped physically"
        );
    }

    #[test]
    fn a_reviewed_nextest_project_has_fixed_bytes_and_rejects_profile_overrides() {
        let candidate = project(PROJECT_CONFIG);
        let input = |sha256: &str| {
            serde_json::from_value::<GraphInput>(json!({
                "locator": { "kind": "checkout", "path": ".config/nextest.toml" },
                "expectation": {
                    "kind": "present",
                    "identity": { "kind": "reviewed_bytes", "sha256": sha256 }
                }
            }))
            .unwrap()
        };
        assert!(candidate.fixed_in(&[input(&candidate.sha256)]));
        assert!(!candidate.fixed_in(&[input(&"a".repeat(64))]));
        assert!(!candidate.fixed_in(&[]));
        assert!(overrides_configuration("NEXTEST_PROFILE"));
        assert!(overrides_configuration("NEXTEST_USER_CONFIG_FILE"));
        assert!(!overrides_configuration("CARGO_HOME"));
    }
}
