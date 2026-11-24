//! Exact two-child proof for the named native archive fixture.

use super::serde_core::{artifact_bytes, capture_witness};
use super::{ReviewedUnit, UnitRole, unproved, verify};
use crate::slot::{
    Refusal,
    admission::{Material, NativePackage, native_output},
    profile::{digest, refused},
};
use cargo_hive_core::{
    fingerprint::{Format, StrictProfileReader, Unit, UnitHash, UnitId, UnitKind},
    unit_path::{self, UnitLayout},
};
use cargo_hive_platform::signals::cancelled;
use cargo_hive_state::slots::{
    self, CargoLayout, EligibleProducer, GeneratedInputWitness, VerifiedRoot,
};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(in crate::slot) const PACKAGE: &str = "named-native-archive-leaf";
pub(in crate::slot) const LIB_TARGET: &str = "named_native_archive_leaf";
pub(in crate::slot) const LIB_PATH: &str = "src/lib.rs";
// The reviewed SDKROOT contributes the SDK version in LC_BUILD_VERSION.
const OBJECT_SHA256: &str = "f0795339e1a6fb81170c5b95b7ad332705a3fe95e9f79f11fcbb818fb34e6919";
const NATIVE_SOURCE: &[u8] = b"int hive_archived_value(void) {\n    return 7;\n}\n";
const LIB_SOURCE: &[u8] = b"unsafe extern \"C\" {\n    fn hive_archived_value() -> i32;\n}\n\npub fn value() -> i32 {\n    unsafe { hive_archived_value() }\n}\n";
pub(in crate::slot) const CONSUMER_SOURCE: &[u8] =
    b"fn main() {\n    assert_eq!(fixture::answer(), 7);\n}\n";
pub(in crate::slot) const WORKSPACE_LIB: &[u8] =
    b"pub fn answer() -> u32 { named_native_archive_leaf::value() as u32 }\n";
pub(in crate::slot) const INDEPENDENT_LIB: &[u8] = b"pub fn independent() {}\n";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::slot) struct NativeArchive {
    pub producer: ReviewedUnit,
    pub package_material_sha256: String,
    pub consumer: ArchiveConsumer,
    native_source: String,
    pub archiver: Material,
    leaf: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::slot) struct ArchiveConsumer {
    pub package: String,
    pub target: String,
    pub source: PathBuf,
}

impl NativeArchive {
    pub fn valid_for(&self, name: &str, version: &str, inventory: &str, native: &str) -> bool {
        name == PACKAGE
            && version == "0.1.0"
            && self.producer.package_id.starts_with("git+file://")
            && self.package_material_sha256 == inventory
            && slots::hex(inventory, 64)
            && self.producer.target == "build-script-build"
            && self.producer.source.as_os_str() == "build.rs"
            && self.producer.role == UnitRole::HostBuildScript
            && self.producer.profile == "dev"
            && self.producer.triple == native
            && native == "aarch64-apple-darwin"
            && self.producer.features.is_empty()
            && self.consumer.package == "fixture"
            && self.consumer.target == "link-proof"
            && self.consumer.source.as_os_str() == "src/main.rs"
            && self.native_source == "native.c"
            && self.leaf == "libhive_named.a"
            && self.archiver.path.is_absolute()
            && self.archiver.path.file_name() == Some(OsStr::new("ar"))
            && slots::hex(&self.archiver.sha256, 64)
    }

    pub fn verifies_source(
        &self,
        root: &Path,
        clang: &Path,
        script: &[u8],
    ) -> Result<bool, Refusal> {
        Ok(archive_script(clang, &self.archiver.path)
            .is_some_and(|reviewed| script == reviewed.as_bytes())
            && artifact_bytes(&root.join(&self.native_source), 4096)? == NATIVE_SOURCE
            && artifact_bytes(&root.join(LIB_PATH), 4096)? == LIB_SOURCE)
    }

    pub fn matches_workspace(&self, packages: &[NativePackage]) -> bool {
        packages.len() == 2
            && packages.iter().any(|package| {
                package.name == self.consumer.package
                    && package.version == "0.1.0"
                    && package.manifest.as_os_str() == "Cargo.toml"
                    && package.dependencies == [PACKAGE]
                    && matches!(package.targets.as_slice(), [lib, bin]
                        if lib.name == self.consumer.package && lib.kind == "lib"
                            && lib.source.as_os_str() == LIB_PATH
                            && bin.name == self.consumer.target && bin.kind == "bin"
                            && bin.source == self.consumer.source)
            })
            && packages.iter().any(|package| {
                package.name == "independent"
                    && package.version == "0.1.0"
                    && package.manifest.as_os_str() == "independent/Cargo.toml"
                    && package.dependencies.is_empty()
                    && matches!(package.targets.as_slice(), [lib]
                        if lib.name == "independent" && lib.kind == "lib"
                            && lib.source.as_os_str() == "independent/src/lib.rs")
            })
    }

    pub fn matches_witnesses(&self, inputs: &[GeneratedInputWitness]) -> Result<bool, Refusal> {
        let hash = digest(&serde_json::to_vec(self).map_err(|_| unproved())?)?;
        Ok(matches!(inputs, [input] if input.declaration_sha256 == hash
            && input.relative_path.file_name().is_some_and(|name| name == self.leaf.as_str())
            && input.bytes == 728))
    }
}

fn archive_script(clang: &Path, ar: &Path) -> Option<String> {
    let portable = |path: &Path| {
        path.is_absolute()
            && path.to_str().is_some_and(|name| {
                name.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
            })
    };
    if !portable(clang) || !portable(ar) {
        return None;
    }
    Some(format!(
        r#"use std::{{env, path::PathBuf, process::Command}};

fn main() {{
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("native.o");
    let archive = out.join("libhive_named.a");
    assert!(Command::new({clang:?})
        .args(["-c", "-g0", "native.c", "-o"])
        .arg(&object).status().unwrap().success());
    assert!(Command::new({ar:?})
        .arg("rcs").arg(&archive).arg(&object).status().unwrap().success());
    println!("cargo:rerun-if-changed=native.c");
    println!("cargo:rustc-link-search=native={{}}", out.display());
    println!("cargo:rustc-link-lib=static=hive_named");
}}
"#
    ))
}

pub(in crate::slot) fn is_named_archive_script(bytes: &[u8]) -> bool {
    let Ok(script) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut commands = script.lines().filter_map(|line| {
        let argument = line
            .strip_prefix("    assert!(Command::new(")?
            .strip_suffix(')')?;
        serde_json::from_str::<PathBuf>(argument).ok()
    });
    let (Some(clang), Some(ar), None) = (commands.next(), commands.next(), commands.next()) else {
        return false;
    };
    archive_script(&clang, &ar).as_deref() == Some(script)
}

pub(in crate::slot) struct NativeArchivePlan {
    declaration: NativeArchive,
    otool: PathBuf,
    checkout: PathBuf,
    home: PathBuf,
}

impl NativeArchivePlan {
    pub fn new(
        declaration: &NativeArchive,
        otool: PathBuf,
        checkout: PathBuf,
        home: PathBuf,
    ) -> Self {
        Self {
            declaration: declaration.clone(),
            otool,
            checkout,
            home,
        }
    }

    pub fn require_prior(
        &self,
        prior: &EligibleProducer,
        invocation: &str,
        locked: impl Fn(&Path) -> bool,
    ) -> Result<(), Refusal> {
        match prior {
            EligibleProducer::NeverBuilt { .. } => Ok(()),
            EligibleProducer::Complete {
                invocations, roots, ..
            } => {
                let record = invocations
                    .iter()
                    .rfind(|record| record.evidence.invocation == invocation)
                    .ok_or_else(unproved)?;
                if !self
                    .declaration
                    .matches_witnesses(&record.generated_inputs)?
                {
                    return Err(unproved());
                }
                let input = &record.generated_inputs[0];
                let profile = input
                    .cargo_profile(&roots.build)
                    .filter(|path| path == &roots.build.path.join("debug"))
                    .ok_or_else(unproved)?;
                if !locked(&profile) {
                    return Err(unproved());
                }
                verify(&roots.build, input)?;
                if !locked(&profile) {
                    return Err(unproved());
                }
                Ok(())
            }
            EligibleProducer::SeedOnly(_) => Err(unproved()),
        }
    }

    pub fn collect(
        &self,
        root: &VerifiedRoot,
        layout: CargoLayout,
    ) -> Result<GeneratedInputWitness, Refusal> {
        let profile = root.path.join("debug");
        let fs = cargo_hive_platform::host_fs();
        let report = StrictProfileReader::new(fs.as_ref(), &cancelled).read_profile(&profile);
        if report.format != Format::Supported
            || report.partial_unit_dirs != 0
            || report.not_current != 0
        {
            return Err(unproved());
        }
        let graph = report.graph.ok_or_else(unproved)?;
        let layout = match layout {
            CargoLayout::Flat => UnitLayout::Flat,
            CargoLayout::UnitDir => UnitLayout::UnitDir,
        };
        let exact = |package: &str, kind, stem: &str| {
            let mut matches = graph.units.iter().enumerate().filter(|(_, unit)| {
                unit.loc.package() == package
                    && unit.loc.layout == layout
                    && unit.kind == kind
                    && unit.stem == stem
            });
            let (index, unit) = matches.next().ok_or_else(|| {
                refused(
                    "The Cargo producer or linked consumer unit is absent from the archive proof.",
                )
            })?;
            if matches.next().is_some() {
                return Err(unproved());
            }
            Ok((UnitId(u32::try_from(index).map_err(|_| unproved())?), unit))
        };
        let (library_id, library) = exact(PACKAGE, UnitKind::Lib, "lib-named_native_archive_leaf")?;
        let (_, run) = exact(
            PACKAGE,
            UnitKind::RunBuildScript,
            "run-build-script-build-script-build",
        )?;
        let (_, compiled) = exact(
            PACKAGE,
            UnitKind::BuildScript,
            "build-script-build-script-build",
        )?;
        let (consumer, consumer_unit) = exact(
            &self.declaration.consumer.package,
            UnitKind::Bin,
            "bin-link-proof",
        )?;
        let (workspace_library, _) = exact("fixture", UnitKind::Lib, "lib-fixture")?;
        let (independent, independent_unit) =
            exact("independent", UnitKind::Lib, "lib-independent")?;
        let linked = |from: &Unit, to: &Unit| matches!(from.deps.as_slice(), [edge] if UnitHash::from_fingerprint(edge.fingerprint) == to.hash);
        if !linked(library, run)
            || !linked(run, compiled)
            || !compiled.deps.is_empty()
            || library.deps[0].package != run.deps[0].package
            || library.toolchain_value != run.toolchain_value
            || run.toolchain_value != compiled.toolchain_value
        {
            return Err(refused(
                "The archive producer's compiled and executed units are not linked.",
            ));
        }
        let consumer_dependencies = graph.successors(consumer);
        if graph.units.len() != 6
            || graph.successors(workspace_library) != [library_id]
            || !graph.successors(independent).is_empty()
            || !independent_unit.deps.is_empty()
            || !consumer_dependencies.contains(&workspace_library)
            || consumer_dependencies
                .iter()
                .any(|dependency| *dependency != workspace_library && *dependency != library_id)
            || consumer_unit.deps.len() != consumer_dependencies.len()
        {
            return Err(refused(
                "The native archive output has a different Cargo consumer graph than the reviewed fixture.",
            ));
        }

        let (run_data, out, stdout) = match layout {
            UnitLayout::Flat => {
                let data = profile.join("build").join(&run.loc.name_meta);
                (data.clone(), data.join("out"), "output")
            }
            UnitLayout::UnitDir => (
                unit_path::run_dir(&profile, &run.loc).ok_or_else(unproved)?,
                unit_path::outputs(&profile, &run.loc),
                "stdout",
            ),
        };
        let directives = format!(
            "cargo:rerun-if-changed=native.c\ncargo:rustc-link-search=native={}\ncargo:rustc-link-lib=static=hive_named\n",
            out.to_str().ok_or_else(unproved)?
        );
        let output = run_data.join(stdout);
        let fingerprint =
            unit_path::fingerprint_dir(&profile, &run.loc).join(format!("{}.json", run.stem));
        let record: serde_json::Value =
            serde_json::from_slice(&artifact_bytes(&fingerprint, 64 * 1024)?)
                .map_err(|_| unproved())?;
        if record["deps"][0][1] != "build_script_build"
            || record["local"]
                != serde_json::json!([{"RerunIfChanged": {
                    "output": output.strip_prefix(&root.path).map_err(|_| unproved())?, "paths": ["native.c"],
                }}])
            || artifact_bytes(&run_data.join("root-output"), 4096)?
                != out.as_os_str().as_encoded_bytes()
            || artifact_bytes(&output, 4096)? != directives.as_bytes()
            || !artifact_bytes(&run_data.join("stderr"), 4096)?.is_empty()
        {
            return Err(refused(
                "The retained Cargo run directives do not prove the reviewed archive link.",
            ));
        }
        let object = artifact_bytes(&out.join("native.o"), 4096)?;
        let archive_path = out.join(&self.declaration.leaf);
        let archive = artifact_bytes(&archive_path, 8192)?;
        if digest(&object)? != OBJECT_SHA256 {
            return Err(refused(
                "The compiled native object differs from the reviewed fixture.",
            ));
        }
        if !archive_matches(&archive, &object) {
            return Err(refused(
                "The named archive does not contain the exact compiled object and symbol table.",
            ));
        }
        let stem = self.declaration.consumer.target.replace('-', "_");
        let name = match layout {
            UnitLayout::Flat => format!("{stem}-{}", consumer_unit.loc.hex()),
            UnitLayout::UnitDir => stem,
        };
        let binary = unit_path::outputs(&profile, &consumer_unit.loc).join(name);
        let published = profile.join(&self.declaration.consumer.target);
        let before = artifact_bytes(&binary, 2 * 1024 * 1024)?;
        let listing = native_output(
            &self.otool,
            [OsStr::new("-tvV"), binary.as_os_str()],
            &self.checkout,
            self.home.as_os_str(),
            4 * 1024 * 1024,
        )?;
        if !linked_symbol(&listing)
            || artifact_bytes(&binary, 2 * 1024 * 1024)? != before
            || artifact_bytes(&published, 2 * 1024 * 1024)? != before
        {
            return Err(refused(
                "The Rust binary does not contain the exact linked native function.",
            ));
        }
        capture_witness(
            root,
            &archive_path,
            digest(&serde_json::to_vec(&self.declaration).map_err(|_| unproved())?)?,
            archive.len() as u64,
            digest(&archive)?,
        )
    }
}

struct Member<'a> {
    name: &'a [u8],
    contents: &'a [u8],
    rest: &'a [u8],
}

fn member(bytes: &[u8]) -> Option<Member<'_>> {
    let (header, body) = bytes.split_at_checked(60)?;
    if &header[58..] != b"`\n" {
        return None;
    }
    let size: usize = std::str::from_utf8(&header[48..58])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let name_size: usize = std::str::from_utf8(&header[..16])
        .ok()?
        .trim()
        .strip_prefix("#1/")?
        .parse()
        .ok()?;
    let (member, rest) = body.split_at_checked(size)?;
    let (name, contents) = member.split_at_checked(name_size)?;
    let name = name.split(|byte| *byte == 0).next()?;
    Some(Member {
        name,
        contents,
        rest: rest.get(size % 2..)?,
    })
}

fn archive_matches(archive: &[u8], object: &[u8]) -> bool {
    if archive.len() != 728 {
        return false;
    }
    let Some(symbols) = archive.strip_prefix(b"!<arch>\n").and_then(member) else {
        return false;
    };
    let Some(archive_object) = member(symbols.rest) else {
        return false;
    };
    symbols.name == b"__.SYMDEF SORTED"
        && symbols
            .contents
            .windows(21)
            .any(|name| name == b"_hive_archived_value\0")
        && archive_object.name == b"native.o"
        && archive_object.contents == object
        && archive_object.rest.is_empty()
}

fn linked_symbol(listing: &[u8]) -> bool {
    let Ok(listing) = std::str::from_utf8(listing) else {
        return false;
    };
    if listing
        .lines()
        .filter(|line| *line == "_hive_archived_value:")
        .count()
        != 1
    {
        return false;
    }
    let Some((_, code)) = listing.split_once("\n_hive_archived_value:\n") else {
        return false;
    };
    let mut lines = code.lines();
    let mut instructions = lines.next().unwrap_or("").split_whitespace();
    let address = instructions
        .next()
        .and_then(|value| u64::from_str_radix(value, 16).ok());
    let mut ret = lines.next().unwrap_or("").split_whitespace();
    let next = ret
        .next()
        .and_then(|value| u64::from_str_radix(value, 16).ok());
    address.and_then(|value| value.checked_add(4)) == next
        && next.is_some()
        && instructions.eq(["mov", "w0,", "#0x7"])
        && ret.eq(["ret"])
}
