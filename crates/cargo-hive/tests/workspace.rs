//! Workspace structure guards: layer order, edition, pins, and the virtual root.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LAYERS: &[(&str, u8)] = &[
    ("cargo-hive-platform", 0),
    ("cargo-hive-core", 1),
    ("cargo-hive", 5),
];
const FEATURES: &[&str] = &[];

fn metadata() -> serde_json::Value {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("spawn cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("cargo metadata json")
}

fn member_packages(meta: &serde_json::Value) -> Vec<&serde_json::Value> {
    let members: HashSet<&str> = meta["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|v| v.as_str().expect("member id"))
        .collect();
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| members.contains(p["id"].as_str().expect("package id")))
        .collect()
}

fn layer_of(name: &str) -> u8 {
    LAYERS
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("package {name} is not in LAYERS"))
        .1
}

fn package_named<'a>(packages: &[&'a serde_json::Value], name: &str) -> &'a serde_json::Value {
    packages
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("workspace member {name} is missing"))
}

#[test]
fn every_member_is_edition_2024() {
    let meta = metadata();
    let packages = member_packages(&meta);
    let layer_names: HashSet<&str> = LAYERS.iter().map(|(n, _)| *n).collect();
    for (name, _) in LAYERS {
        let pkg = package_named(&packages, name);
        assert_eq!(pkg["edition"].as_str(), Some("2024"), "{name} edition");
    }
    for pkg in &packages {
        let name = pkg["name"].as_str().expect("name");
        assert!(
            layer_names.contains(name),
            "unexpected workspace member {name}"
        );
    }
}

#[test]
fn members_depend_only_on_lower_layers_and_no_feature_on_another() {
    let meta = metadata();
    let packages = member_packages(&meta);
    let member_names: HashSet<&str> = LAYERS.iter().map(|(n, _)| *n).collect();
    for pkg in packages {
        let name = pkg["name"].as_str().expect("name");
        let from = layer_of(name);
        let is_feature = FEATURES.contains(&name);
        for dep in pkg["dependencies"].as_array().expect("dependencies") {
            let dep_name = dep["name"].as_str().expect("dep name");
            if !member_names.contains(dep_name) {
                continue;
            }
            let to = layer_of(dep_name);
            assert!(
                to < from,
                "{name} (layer {from}) depends on {dep_name} (layer {to})"
            );
            if is_feature {
                assert!(
                    !FEATURES.contains(&dep_name),
                    "feature {name} depends on feature {dep_name}"
                );
            }
        }
    }
}

#[test]
fn member_dependencies_are_pinned_exactly() {
    let meta = metadata();
    let packages = member_packages(&meta);
    let member_names: HashSet<&str> = LAYERS.iter().map(|(n, _)| *n).collect();
    let versions: HashMap<&str, &str> = packages
        .iter()
        .map(|p| {
            (
                p["name"].as_str().expect("name"),
                p["version"].as_str().expect("version"),
            )
        })
        .collect();
    for pkg in packages {
        let name = pkg["name"].as_str().expect("name");
        for dep in pkg["dependencies"].as_array().expect("dependencies") {
            let dep_name = dep["name"].as_str().expect("dep name");
            if !member_names.contains(dep_name) {
                continue;
            }
            let want = format!("={}", versions[dep_name]);
            assert_eq!(
                dep["req"].as_str(),
                Some(want.as_str()),
                "{name} → {dep_name} req"
            );
        }
    }
}

#[test]
fn only_cargo_hive_has_a_binary() {
    let meta = metadata();
    for pkg in member_packages(&meta) {
        let name = pkg["name"].as_str().expect("name");
        let has_bin = pkg["targets"].as_array().expect("targets").iter().any(|t| {
            t["kind"]
                .as_array()
                .expect("kind")
                .iter()
                .any(|k| k.as_str() == Some("bin"))
        });
        if name == "cargo-hive" {
            assert!(has_bin, "cargo-hive must have a binary target");
        } else {
            assert!(!has_bin, "{name} must not have a binary target");
        }
    }
}

#[test]
fn the_root_is_a_virtual_workspace() {
    let meta = metadata();
    let root = PathBuf::from(meta["workspace_root"].as_str().expect("workspace_root"));
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml");
    assert!(
        text.lines().any(|l| l.trim() == "[workspace]"),
        "root Cargo.toml must have a [workspace] table"
    );
    assert!(
        text.lines().all(|l| l.trim() != "[package]"),
        "root Cargo.toml must not have a [package] table"
    );
}

#[test]
fn every_member_inherits_version_and_edition() {
    let meta = metadata();
    for pkg in member_packages(&meta) {
        let name = pkg["name"].as_str().expect("name");
        let path = pkg["manifest_path"].as_str().expect("manifest_path");
        let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            text.contains("version.workspace = true"),
            "{name} must inherit version from the workspace"
        );
        assert!(
            text.contains("edition.workspace = true"),
            "{name} must inherit edition from the workspace"
        );
    }
}
