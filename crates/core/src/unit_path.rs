//! Path joins and predicates for a unit in either Cargo layout.

use crate::fingerprint::{self, UnitKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How Cargo laid out a unit's files under a profile dir.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum UnitLayout {
    #[default]
    Flat,
    UnitDir,
}

impl UnitLayout {
    pub fn is_flat(&self) -> bool {
        *self == Self::Flat
    }
}

/// Where a unit lives: its layout and the shared `<package>-<16 hex>` name-meta.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitLoc {
    pub layout: UnitLayout,
    pub name_meta: String,
}

impl UnitLoc {
    /// The package name before the last `-<16 hex>` of `name_meta`.
    pub fn package(&self) -> &str {
        split_name_meta(&self.name_meta).0
    }

    /// The 16-hex unit hash at the end of `name_meta`.
    pub fn hex(&self) -> &str {
        split_name_meta(&self.name_meta).1
    }
}

/// The unit's fingerprint directory.
pub fn fingerprint_dir(profile: &Path, loc: &UnitLoc) -> PathBuf {
    match loc.layout {
        UnitLayout::Flat => profile.join(".fingerprint").join(&loc.name_meta),
        UnitLayout::UnitDir => unit_root(profile, loc).join("fingerprint"),
    }
}

/// The hash file for `stem` inside the unit's fingerprint directory.
pub fn hash_file(profile: &Path, loc: &UnitLoc, stem: &str) -> PathBuf {
    fingerprint_dir(profile, loc).join(stem)
}

/// The `invoked.timestamp` that dates the unit.
pub fn stamp(profile: &Path, loc: &UnitLoc, kind: UnitKind) -> PathBuf {
    match (loc.layout, kind) {
        (UnitLayout::Flat, UnitKind::RunBuildScript) => profile
            .join("build")
            .join(&loc.name_meta)
            .join(fingerprint::INVOKED_TIMESTAMP),
        (UnitLayout::Flat, _) => fingerprint_dir(profile, loc).join(fingerprint::INVOKED_TIMESTAMP),
        (UnitLayout::UnitDir, UnitKind::RunBuildScript) => unit_root(profile, loc)
            .join("run")
            .join(fingerprint::INVOKED_TIMESTAMP),
        (UnitLayout::UnitDir, _) => {
            fingerprint_dir(profile, loc).join(fingerprint::INVOKED_TIMESTAMP)
        }
    }
}

/// The unit directory for a unit-dir unit; `None` for flat.
pub fn unit_dir(profile: &Path, loc: &UnitLoc) -> Option<PathBuf> {
    match loc.layout {
        UnitLayout::Flat => None,
        UnitLayout::UnitDir => Some(unit_root(profile, loc)),
    }
}

/// Where the unit's outputs live: `deps/` for flat, `out/` for unit-dir.
pub fn outputs(profile: &Path, loc: &UnitLoc) -> PathBuf {
    match loc.layout {
        UnitLayout::Flat => flat_deps_dir(profile),
        UnitLayout::UnitDir => unit_root(profile, loc).join("out"),
    }
}

/// The `run/` directory for a unit-dir unit; `None` for flat.
pub fn run_dir(profile: &Path, loc: &UnitLoc) -> Option<PathBuf> {
    match loc.layout {
        UnitLayout::Flat => None,
        UnitLayout::UnitDir => Some(unit_root(profile, loc).join("run")),
    }
}

/// The profile-wide flat `deps/` directory.
pub fn flat_deps_dir(profile: &Path) -> PathBuf {
    profile.join("deps")
}

/// The unit dir that contains `path`, when `path` lies under `build/<package>/<16 hex>/`.
pub fn unit_dir_of(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let comps: Vec<Component<'_>> = path.components().collect();
    for i in 0..comps.len().saturating_sub(2) {
        let (Component::Normal(build), Component::Normal(pkg), Component::Normal(hex)) =
            (&comps[i], &comps[i + 1], &comps[i + 2])
        else {
            continue;
        };
        if build == &"build" && !pkg.is_empty() && is_hex_component(hex) {
            return Some(comps[..=i + 2].iter().collect());
        }
    }
    None
}

/// True for a flat `.fingerprint/<d>` or a unit-dir `<hex>/fingerprint` under `build/<pkg>/`.
pub fn is_fingerprint_dir(path: &Path) -> bool {
    let parent = path.parent();
    if parent
        .and_then(|p| p.file_name())
        .is_some_and(|name| name == ".fingerprint")
    {
        return true;
    }
    path.file_name().is_some_and(|name| name == "fingerprint")
        && parent.is_some_and(|p| unit_dir_of(p) == Some(p.to_path_buf()))
}

fn unit_root(profile: &Path, loc: &UnitLoc) -> PathBuf {
    profile.join("build").join(loc.package()).join(loc.hex())
}

fn split_name_meta(name_meta: &str) -> (&str, &str) {
    match name_meta.rsplit_once('-') {
        Some((pkg, hex)) if fingerprint::unit_hash(name_meta).is_some() => (pkg, hex),
        _ => (name_meta, ""),
    }
}

fn is_hex_component(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|text| fingerprint::UnitHash::parse(text).is_some())
}

#[cfg(test)]
mod tests {
    use super::{
        UnitLayout, UnitLoc, fingerprint_dir, flat_deps_dir, hash_file, is_fingerprint_dir,
        outputs, stamp, unit_dir, unit_dir_of,
    };
    use crate::fingerprint::UnitKind;
    use std::path::{Path, PathBuf};

    const NAME_META: &str = "probe-0123456789abcdef";
    const PROFILE: &str = "/t/debug";

    fn flat() -> UnitLoc {
        UnitLoc {
            layout: UnitLayout::Flat,
            name_meta: NAME_META.into(),
        }
    }

    fn unit_dir_loc() -> UnitLoc {
        UnitLoc {
            layout: UnitLayout::UnitDir,
            name_meta: NAME_META.into(),
        }
    }

    #[test]
    fn a_flat_unit_joins_fingerprint_hash_stamp_and_deps() {
        let loc = flat();
        let profile = Path::new(PROFILE);
        assert_eq!(
            fingerprint_dir(profile, &loc),
            PathBuf::from("/t/debug/.fingerprint/probe-0123456789abcdef")
        );
        assert_eq!(
            hash_file(profile, &loc, "lib-probe"),
            PathBuf::from("/t/debug/.fingerprint/probe-0123456789abcdef/lib-probe")
        );
        assert_eq!(
            stamp(profile, &loc, UnitKind::Lib),
            PathBuf::from("/t/debug/.fingerprint/probe-0123456789abcdef/invoked.timestamp")
        );
        assert_eq!(
            stamp(profile, &loc, UnitKind::RunBuildScript),
            PathBuf::from("/t/debug/build/probe-0123456789abcdef/invoked.timestamp")
        );
        assert_eq!(unit_dir(profile, &loc), None);
        assert_eq!(outputs(profile, &loc), PathBuf::from("/t/debug/deps"));
    }

    #[test]
    fn a_unit_dir_unit_joins_fingerprint_hash_stamp_and_out() {
        let loc = unit_dir_loc();
        let profile = Path::new(PROFILE);
        assert_eq!(
            fingerprint_dir(profile, &loc),
            PathBuf::from("/t/debug/build/probe/0123456789abcdef/fingerprint")
        );
        assert_eq!(
            hash_file(profile, &loc, "lib-probe"),
            PathBuf::from("/t/debug/build/probe/0123456789abcdef/fingerprint/lib-probe")
        );
        assert_eq!(
            stamp(profile, &loc, UnitKind::Lib),
            PathBuf::from("/t/debug/build/probe/0123456789abcdef/fingerprint/invoked.timestamp")
        );
        assert_eq!(
            stamp(profile, &loc, UnitKind::RunBuildScript),
            PathBuf::from("/t/debug/build/probe/0123456789abcdef/run/invoked.timestamp")
        );
        assert_eq!(
            unit_dir(profile, &loc),
            Some(PathBuf::from("/t/debug/build/probe/0123456789abcdef"))
        );
        assert_eq!(
            outputs(profile, &loc),
            PathBuf::from("/t/debug/build/probe/0123456789abcdef/out")
        );
    }

    #[test]
    fn name_meta_splits_into_package_and_hex() {
        let loc = flat();
        assert_eq!(loc.package(), "probe");
        assert_eq!(loc.hex(), "0123456789abcdef");
    }

    #[test]
    fn flat_deps_dir_is_the_profile_deps_directory() {
        assert_eq!(
            flat_deps_dir(Path::new(PROFILE)),
            PathBuf::from("/t/debug/deps")
        );
    }

    #[test]
    fn unit_dir_of_recognises_paths_inside_a_unit_dir_including_a_package_named_out() {
        let inside = Path::new("/t/debug/build/out/0123456789abcdef/out/foo.d");
        assert_eq!(
            unit_dir_of(inside),
            Some(PathBuf::from("/t/debug/build/out/0123456789abcdef"))
        );
        let fingerprint = Path::new("/t/debug/build/out/0123456789abcdef/fingerprint/lib-out");
        assert_eq!(
            unit_dir_of(fingerprint),
            Some(PathBuf::from("/t/debug/build/out/0123456789abcdef"))
        );
        let unit_root = Path::new("/t/debug/build/out/0123456789abcdef");
        assert_eq!(
            unit_dir_of(unit_root),
            Some(PathBuf::from("/t/debug/build/out/0123456789abcdef"))
        );
        assert_eq!(
            unit_dir_of(Path::new(
                "/t/debug/build/probe-0123456789abcdef/invoked.timestamp"
            )),
            None
        );
        assert_eq!(unit_dir_of(Path::new("/t/debug/deps/libfoo.rlib")), None);
        assert_eq!(
            unit_dir_of(Path::new("/t/debug/.fingerprint/probe-0123456789abcdef")),
            None
        );
    }

    #[test]
    fn is_fingerprint_dir_accepts_flat_and_unit_dir_shapes_only() {
        assert!(is_fingerprint_dir(Path::new(
            "/t/debug/.fingerprint/x-0123456789abcdef"
        )));
        assert!(is_fingerprint_dir(Path::new(
            "/t/debug/build/probe/0123456789abcdef/fingerprint"
        )));
        assert!(!is_fingerprint_dir(Path::new("/t/debug/fingerprint")));
        assert!(!is_fingerprint_dir(Path::new("/t/debug/build/fingerprint")));
        assert!(!is_fingerprint_dir(Path::new(
            "/t/debug/build/probe/fingerprint"
        )));
        assert!(!is_fingerprint_dir(Path::new(
            "/t/debug/deps/x-0123456789abcdef"
        )));
    }

    #[test]
    fn unit_layout_defaults_to_flat_and_serde_names_are_stable() {
        assert_eq!(UnitLayout::default(), UnitLayout::Flat);
        assert_eq!(
            serde_json::to_string(&UnitLayout::Flat).unwrap(),
            "\"flat\""
        );
        assert_eq!(
            serde_json::to_string(&UnitLayout::UnitDir).unwrap(),
            "\"unit-dir\""
        );
    }
}
