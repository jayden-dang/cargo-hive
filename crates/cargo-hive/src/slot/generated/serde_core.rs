//! Checked Cargo artifact equivalent for the reviewed serde_core generated input.

use super::{GeneratedRust, unproved, verify};
use crate::slot::{
    Refusal, check_cancelled,
    profile::{digest, read_input_bounded},
};
use cargo_hive_core::{
    fingerprint::{Format, StrictProfileReader, Unit, UnitHash, UnitKind},
    unit_path::{self, UnitLayout},
};
use cargo_hive_platform::{
    flock::{open_owner_directory, open_owner_directory_at, open_owner_file_at},
    signals::cancelled,
};
use cargo_hive_state::slots::{
    CargoLayout, EligibleProducer, FileIdentity, GeneratedInputWitness, VerifiedRoot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt as _,
    path::{Component, Path, PathBuf},
};

const MAX_COMPILER_UNITS: usize = 512;
const MAX_COMPILER_ENTRIES: usize = 65_536;
const MAX_DEP_INFO_FILE: usize = 1024 * 1024;
const MAX_DEP_INFO_BYTES: usize = 16 * MAX_DEP_INFO_FILE;

pub(in crate::slot) struct GeneratedRustPlan {
    declaration: GeneratedRust,
    package_root: PathBuf,
}

// The pinned build.rs emits these declarations after its synchronous probe of
// the sealed RUSTC with --version. Cargo retains the selected branch's output.
const SERDE_CORE_DIRECTIVES: &[u8] = concat!(
    "cargo:rerun-if-changed=build.rs\n",
    "cargo:rustc-check-cfg=cfg(if_docsrs_then_no_serde_core)\n",
    "cargo:rustc-check-cfg=cfg(no_core_cstr)\n",
    "cargo:rustc-check-cfg=cfg(no_core_error)\n",
    "cargo:rustc-check-cfg=cfg(no_core_net)\n",
    "cargo:rustc-check-cfg=cfg(no_core_num_saturating)\n",
    "cargo:rustc-check-cfg=cfg(no_diagnostic_namespace)\n",
    "cargo:rustc-check-cfg=cfg(no_serde_derive)\n",
    "cargo:rustc-check-cfg=cfg(no_std_atomic)\n",
    "cargo:rustc-check-cfg=cfg(no_std_atomic64)\n",
    "cargo:rustc-check-cfg=cfg(no_target_has_atomic)\n",
)
.as_bytes();

impl GeneratedRustPlan {
    pub fn new(declaration: &GeneratedRust, package_root: PathBuf) -> Self {
        Self {
            declaration: declaration.clone(),
            package_root,
        }
    }

    pub fn profile_directory(root: &VerifiedRoot) -> PathBuf {
        root.path.join("debug")
    }

    pub fn require_prior(
        &self,
        prior: &EligibleProducer,
        invocation: &str,
        is_locked: impl Fn(&Path) -> bool,
    ) -> Result<(), Refusal> {
        match prior {
            EligibleProducer::NeverBuilt { .. } => Ok(()),
            EligibleProducer::Complete {
                invocations, roots, ..
            } => {
                let saved = invocations
                    .iter()
                    .rev()
                    .find(|saved| saved.evidence.invocation == invocation)
                    .ok_or_else(unproved)?;
                if saved.generated_inputs.len() != 1
                    || !self
                        .declaration
                        .matches_witnesses(&saved.generated_inputs)?
                {
                    return Err(unproved());
                }
                let profile = saved.generated_inputs[0]
                    .cargo_profile(&roots.build)
                    .filter(|profile| profile == &Self::profile_directory(&roots.build))
                    .ok_or_else(unproved)?;
                if !is_locked(&profile) {
                    return Err(unproved());
                }
                if self.observe(&roots.build, None)? != saved.generated_inputs[0]
                    || !is_locked(&profile)
                {
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
        let layout = match layout {
            CargoLayout::Flat => UnitLayout::Flat,
            CargoLayout::UnitDir => UnitLayout::UnitDir,
        };
        self.observe(root, Some(layout))
    }

    fn observe(
        &self,
        root: &VerifiedRoot,
        expected_layout: Option<UnitLayout>,
    ) -> Result<GeneratedInputWitness, Refusal> {
        if root.path.file_name().is_none_or(|name| name != "target") {
            return Err(unproved());
        }
        let profile = Self::profile_directory(root);
        let platform_fs = cargo_hive_platform::host_fs();
        let report =
            StrictProfileReader::new(platform_fs.as_ref(), &cancelled).read_profile(&profile);
        if report.format != Format::Supported
            || report.partial_unit_dirs != 0
            || report.not_current != 0
        {
            return Err(unproved());
        }
        let graph = report.graph.ok_or_else(unproved)?;
        let mut layouts = report.layouts.into_iter();
        let layout = layouts.next().ok_or_else(unproved)?;
        if layouts.next().is_some()
            || expected_layout.is_some_and(|expected| expected != layout)
            || !(1..=MAX_COMPILER_UNITS).contains(&graph.units.len())
        {
            return Err(unproved());
        }
        let producer = &self.declaration.producer;
        let [consumer] = self.declaration.consumers.as_slice() else {
            return Err(unproved());
        };
        let exact_unit = |kind, stem: &str| {
            let mut units = graph.units.iter().filter(|unit| {
                unit.loc.package() == "serde_core"
                    && unit.loc.layout == layout
                    && unit.kind == kind
                    && unit.stem == stem
            });
            let unit = units.next().ok_or_else(unproved)?;
            if units.next().is_some() {
                return Err(unproved());
            }
            Ok(unit)
        };
        let library = exact_unit(UnitKind::Lib, "lib-serde_core")?;
        let run = exact_unit(
            UnitKind::RunBuildScript,
            "run-build-script-build-script-build",
        )?;
        let compiled = exact_unit(UnitKind::BuildScript, "build-script-build-script-build")?;
        let features = |unit: &Unit, expected: &[String]| {
            serde_json::from_str::<Vec<String>>(&unit.features).is_ok_and(|mut features| {
                features.sort();
                features == expected
            })
        };
        let linked = |from: &Unit, to: &Unit| matches!(from.deps.as_slice(), [edge] if UnitHash::from_fingerprint(edge.fingerprint) == to.hash);
        if !features(library, &consumer.features)
            || !features(compiled, &producer.features)
            || library.toolchain_value != run.toolchain_value
            || run.toolchain_value != compiled.toolchain_value
            || !linked(library, run)
            || !linked(run, compiled)
            || library.deps[0].package != run.deps[0].package
            || !compiled.deps.is_empty()
        {
            return Err(unproved());
        }

        let flat = |unit: &Unit| profile.join("build").join(&unit.loc.name_meta);
        let (out, run_data, stdout, compiled_dep_info) = match layout {
            UnitLayout::Flat => (
                flat(run).join("out"),
                flat(run),
                "output",
                flat(compiled).join(format!("build_script_build-{}.d", compiled.loc.hex())),
            ),
            UnitLayout::UnitDir => (
                unit_path::outputs(&profile, &run.loc),
                unit_path::run_dir(&profile, &run.loc).ok_or_else(unproved)?,
                "stdout",
                unit_path::outputs(&profile, &compiled.loc).join("build_script_build.d"),
            ),
        };
        let output = run_data.join(stdout);
        let run_fingerprint =
            unit_path::fingerprint_dir(&profile, &run.loc).join(format!("{}.json", run.stem));
        let run_record: serde_json::Value =
            serde_json::from_slice(&artifact_bytes(&run_fingerprint, 64 * 1024)?)
                .map_err(|_| unproved())?;
        let lib_fingerprint = unit_path::fingerprint_dir(&profile, &library.loc)
            .join(format!("{}.json", library.stem));
        let library_record: serde_json::Value =
            serde_json::from_slice(&artifact_bytes(&lib_fingerprint, 64 * 1024)?)
                .map_err(|_| unproved())?;
        if run_record["local"]
            != serde_json::json!([{"RerunIfChanged": {
                "output": output.strip_prefix(&root.path).map_err(|_| unproved())?,
                "paths": ["build.rs"],
            }}])
            || artifact_bytes(&run_data.join("root-output"), 4096)?
                != out.as_os_str().as_encoded_bytes()
            || artifact_bytes(&output, 4096)? != SERDE_CORE_DIRECTIVES
            || !artifact_bytes(&run_data.join("stderr"), 4096)?.is_empty()
            || run_record["deps"][0][1] != "build_script_build"
            || library_record["deps"][0][1] != "build_script_build"
        {
            return Err(unproved());
        }
        let input = out.join(&self.declaration.leaf);
        let library_dep_info = unit_dep_info(library, &profile)?;
        let mut remaining = MAX_DEP_INFO_BYTES;
        require_dep_info(
            &compiled_dep_info,
            &self.package_root,
            &self.package_root.join(&producer.source),
            None,
            &mut remaining,
        )?;
        require_dep_info(
            &library_dep_info,
            &self.package_root,
            &self.package_root.join(&consumer.source),
            Some(&input),
            &mut remaining,
        )?;
        let build_source = root.path.parent().ok_or_else(unproved)?;
        let protected = fs::symlink_metadata(&input).map_err(|_| unproved())?;
        let protected = FileIdentity::of(&protected);
        let mut inventory = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
        for unit in &graph.units {
            check_cancelled()?;
            if unit == run {
                if layout == UnitLayout::UnitDir {
                    inventory.entry(out.clone()).or_default();
                }
                continue;
            }
            let path = if unit == compiled {
                compiled_dep_info.clone()
            } else if unit == library {
                library_dep_info.clone()
            } else {
                let path = unit_dep_info(unit, &profile)?;
                let mut inputs = Vec::new();
                read_dep_info(&path, &mut remaining, &mut inputs)?;
                if !inputs
                    .iter()
                    .all(|path| workspace_source(path, build_source, &root.path, protected))
                {
                    return Err(unproved());
                }
                path
            };
            let directory = path.parent().ok_or_else(unproved)?.to_path_buf();
            if !inventory.entry(directory).or_default().insert(path) {
                return Err(unproved());
            }
        }
        verify_dep_info_inventory(platform_fs.as_ref(), inventory)?;
        capture_witness(
            root,
            &input,
            digest(&serde_json::to_vec(&self.declaration).map_err(|_| unproved())?)?,
            self.declaration.bytes,
            self.declaration.sha256.clone(),
        )
    }
}

pub(super) fn capture_witness(
    root: &VerifiedRoot,
    input: &Path,
    declaration_sha256: String,
    bytes: u64,
    sha256: String,
) -> Result<GeneratedInputWitness, Refusal> {
    let relative_path = input
        .strip_prefix(&root.path)
        .map_err(|_| unproved())?
        .to_path_buf();
    let (mut directory, metadata) = open_owner_directory(&root.path).map_err(|_| unproved())?;
    if FileIdentity::of(&metadata) != root.identity {
        return Err(unproved());
    }
    let mut parents = Vec::new();
    for component in relative_path.parent().ok_or_else(unproved)?.components() {
        let Component::Normal(name) = component else {
            return Err(unproved());
        };
        let (child, metadata) =
            open_owner_directory_at(&directory, name).map_err(|_| unproved())?;
        parents.push(FileIdentity::of(&metadata));
        directory = child;
    }
    let file = open_owner_file_at(&directory, relative_path.file_name().ok_or_else(unproved)?)
        .map_err(|_| unproved())?;
    let witness = GeneratedInputWitness {
        declaration_sha256,
        build_root: root.identity,
        relative_path,
        parents,
        file: FileIdentity::of(&file.metadata().map_err(|_| unproved())?),
        bytes,
        sha256,
    };
    verify(root, &witness)?;
    Ok(witness)
}

pub(super) fn artifact_bytes(path: &Path, cap: usize) -> Result<Vec<u8>, Refusal> {
    if path.canonicalize().as_deref().ok() != Some(path) {
        return Err(unproved());
    }
    read_input_bounded(path, cap)?.ok_or_else(unproved)
}

fn exact_dep_path(path: &str) -> bool {
    !path.is_empty()
        && !path.bytes().any(|byte| {
            byte.is_ascii_control() || byte.is_ascii_whitespace() || b":#\\$".contains(&byte)
        })
        && path
            .strip_prefix('/')
            .unwrap_or(path)
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn regular(path: &Path) -> Result<bool, Refusal> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        _ => Err(unproved()),
    }
}

fn unit_dep_info(unit: &Unit, profile: &Path) -> Result<PathBuf, Refusal> {
    let kind = match unit.kind {
        UnitKind::Lib => "lib-",
        UnitKind::Bin => "bin-",
        _ => return Err(unproved()),
    };
    let target = unit.stem.strip_prefix(kind).ok_or_else(unproved)?;
    if target.is_empty()
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(unproved());
    }
    let output = unit_path::outputs(profile, &unit.loc);
    let hashed = output.join(format!("{target}-{}.d", unit.loc.hex()));
    if unit.loc.layout == UnitLayout::Flat {
        return Ok(hashed);
    }
    let plain = output.join(format!("{target}.d"));
    match (regular(&hashed)?, regular(&plain)?) {
        (true, false) => Ok(hashed),
        (false, true) => Ok(plain),
        _ => Err(unproved()),
    }
}

fn verify_dep_info_inventory(
    fs: &dyn cargo_hive_platform::Fs,
    inventory: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
) -> Result<(), Refusal> {
    let mut remaining = MAX_COMPILER_ENTRIES;
    for (directory, expected) in inventory {
        check_cancelled()?;
        let entries = fs
            .read_dir_bounded(&directory, remaining)
            .map_err(|_| unproved())?;
        remaining = remaining.checked_sub(entries.len()).ok_or_else(unproved)?;
        let mut observed = BTreeSet::new();
        for (name, kind) in entries {
            if name.ends_with(".d")
                && (kind != cargo_hive_platform::EntryKind::File
                    || !observed.insert(directory.join(name)))
            {
                return Err(unproved());
            }
        }
        if observed != expected {
            return Err(unproved());
        }
    }
    Ok(())
}

fn read_dep_info(
    path: &Path,
    remaining: &mut usize,
    inputs: &mut Vec<PathBuf>,
) -> Result<(), Refusal> {
    let bytes = artifact_bytes(path, MAX_DEP_INFO_FILE.min(*remaining))?;
    *remaining -= bytes.len();
    let text = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.strip_suffix('\n'))
        .ok_or_else(unproved)?;
    let mut lines = text.lines();
    let (target, prerequisites) = lines
        .next()
        .and_then(|line| line.split_once(": "))
        .ok_or_else(unproved)?;
    if !exact_dep_path(target) || path.as_os_str() != target {
        return Err(unproved());
    }
    let dependencies: Vec<_> = prerequisites.split(' ').collect();
    if !dependencies.iter().all(|entry| exact_dep_path(entry)) {
        return Err(unproved());
    }
    let names: BTreeSet<_> = dependencies.iter().copied().collect();
    let mut rules = BTreeSet::from([target]);
    let mut phony = BTreeSet::new();
    for line in lines.filter(|line| !line.is_empty()) {
        if let Some(env) = line.strip_prefix("# env-dep:") {
            if env.is_empty() || env.bytes().any(|byte| byte.is_ascii_control()) {
                return Err(unproved());
            }
        } else if let Some((target, repeated)) = line.split_once(": ") {
            if !exact_dep_path(target) || repeated != prerequisites || !rules.insert(target) {
                return Err(unproved());
            }
        } else if let Some(name) = line.strip_suffix(':') {
            if !names.contains(name) || !phony.insert(name) {
                return Err(unproved());
            }
        } else {
            return Err(unproved());
        }
    }
    if phony != names {
        return Err(unproved());
    }
    inputs.extend(dependencies.into_iter().map(PathBuf::from));
    Ok(())
}

fn workspace_source(path: &Path, checkout: &Path, output: &Path, protected: FileIdentity) -> bool {
    let path = checkout.join(path);
    path.starts_with(checkout)
        && !path.starts_with(output)
        && path
            .canonicalize()
            .is_ok_and(|resolved| resolved.as_os_str() == path.as_os_str())
        && fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.is_file() && metadata.nlink() == 1 && FileIdentity::of(&metadata) != protected
        })
}

fn require_dep_info(
    dep_info: &Path,
    package_root: &Path,
    source: &Path,
    generated: Option<&Path>,
    remaining: &mut usize,
) -> Result<(), Refusal> {
    let mut inputs = Vec::new();
    read_dep_info(dep_info, remaining, &mut inputs)?;
    let mut inputs = inputs.iter().map(PathBuf::as_path);
    if inputs.next() != Some(source) {
        return Err(unproved());
    }
    let mut observed_generated = 0;
    for path in inputs {
        if Some(path) == generated {
            observed_generated += 1;
        } else if generated.is_none() || !path.starts_with(package_root) {
            return Err(unproved());
        }
    }
    if observed_generated != usize::from(generated.is_some()) {
        return Err(unproved());
    }
    Ok(())
}
