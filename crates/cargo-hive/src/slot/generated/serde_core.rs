//! Checked Cargo artifact equivalent for the reviewed serde_core generated input.

use super::{GeneratedRust, unproved, verify};
use crate::slot::{
    Refusal,
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
use std::path::{Component, Path, PathBuf};

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
                verify(&roots.build, &saved.generated_inputs[0])?;
                if !is_locked(&profile) {
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
        let profile = Self::profile_directory(root);
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
        let library_dep_info = unit_path::outputs(&profile, &library.loc)
            .join(format!("serde_core-{}.d", library.loc.hex()));
        require_dep_info(
            &compiled_dep_info,
            &self.package_root,
            &self.package_root.join(&producer.source),
            None,
        )?;
        require_dep_info(
            &library_dep_info,
            &self.package_root,
            &self.package_root.join(&consumer.source),
            Some(&input),
        )?;
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
            declaration_sha256: digest(
                &serde_json::to_vec(&self.declaration).map_err(|_| unproved())?,
            )?,
            build_root: root.identity,
            relative_path,
            parents,
            file: FileIdentity::of(&file.metadata().map_err(|_| unproved())?),
            bytes: self.declaration.bytes,
            sha256: self.declaration.sha256.clone(),
        };
        verify(root, &witness)?;
        Ok(witness)
    }
}

fn artifact_bytes(path: &Path, cap: usize) -> Result<Vec<u8>, Refusal> {
    if path.canonicalize().as_deref().ok() != Some(path) {
        return Err(unproved());
    }
    read_input_bounded(path, cap)?.ok_or_else(unproved)
}

fn require_dep_info(
    dep_info: &Path,
    package_root: &Path,
    source: &Path,
    generated: Option<&Path>,
) -> Result<(), Refusal> {
    let exact = |path: &Path| {
        path.to_str()
            .filter(|path| {
                !path
                    .bytes()
                    .any(|byte| byte.is_ascii_whitespace() || b":#\\".contains(&byte))
            })
            .ok_or_else(unproved)
            .map(str::to_owned)
    };
    let dep_info_path = exact(dep_info)?;
    let package_root = exact(package_root)?;
    let source = exact(source)?;
    let generated = generated.map(exact).transpose()?;
    let bytes = artifact_bytes(dep_info, 1024 * 1024)?;
    let data = std::str::from_utf8(&bytes).map_err(|_| unproved())?;
    let (target, dependencies) = data
        .lines()
        .next()
        .and_then(|line| line.split_once(": "))
        .ok_or_else(unproved)?;
    let mut paths = dependencies.split(' ');
    if target != dep_info_path || paths.next() != Some(source.as_str()) {
        return Err(unproved());
    }
    let mut observed_generated = 0;
    for path in paths {
        if Some(path) == generated.as_deref() {
            observed_generated += 1;
        } else if generated.is_none()
            || !path
                .strip_prefix(&package_root)
                .and_then(|path| path.strip_prefix('/'))
                .is_some_and(|relative| {
                    relative
                        .split('/')
                        .all(|part| !matches!(part, "" | "." | ".."))
                        && exact(Path::new(path)).is_ok()
                })
        {
            return Err(unproved());
        }
    }
    if observed_generated != usize::from(generated.is_some()) {
        return Err(unproved());
    }
    Ok(())
}
