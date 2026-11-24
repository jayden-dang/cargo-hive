//! Physical revalidation of finite, previously witnessed generated inputs.

use super::{
    Refusal,
    admission::NativeBuild,
    check_cancelled, directory_stamp,
    profile::{digest, native_target, refused},
};
use cargo_hive_platform::{
    flock::{open_owner_directory, open_owner_directory_at, open_owner_file_at},
    signals::cancelled,
    slot::sha256_reader,
};
use cargo_hive_state::{
    StateError,
    slots::{self, FileIdentity, GeneratedInputWitness, PoolProfile, Producer, VerifiedRoot},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedRust {
    pub producer: ReviewedUnit,
    pub package_material_sha256: String,
    pub consumers: Vec<ReviewedUnit>,
    pub leaf: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewedUnit {
    pub package_id: String,
    pub target: String,
    pub source: PathBuf,
    role: UnitRole,
    profile: String,
    triple: String,
    features: Vec<String>,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum UnitRole {
    HostBuildScript,
    TargetLibrary,
}

impl GeneratedRust {
    pub fn matches_witnesses(&self, witnesses: &[GeneratedInputWitness]) -> Result<bool, Refusal> {
        let reviewed = digest(&serde_json::to_vec(self).map_err(|_| unproved())?)?;
        Ok(witnesses.iter().any(|witness| {
            witness.declaration_sha256 == reviewed
                && witness
                    .relative_path
                    .file_name()
                    .is_some_and(|leaf| leaf == self.leaf.as_str())
                && witness.bytes == self.bytes
                && witness.sha256 == self.sha256
        }))
    }

    pub fn valid_for(
        &self,
        package_name: &str,
        package_version: &str,
        material_sha256: &str,
        native: &str,
    ) -> bool {
        let valid_features = |unit: &ReviewedUnit| {
            unit.features.len() <= 64
                && unit.features.windows(2).all(|pair| pair[0] < pair[1])
                && unit.features.iter().all(|name| {
                    (1..=64).contains(&name.len())
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-+".contains(&byte))
                })
        };
        package_name == "serde_core"
            && package_version == "1.0.228"
            && slots::hex(&self.package_material_sha256, 64)
            && self.package_material_sha256 == material_sha256
            && self.leaf == "private.rs"
            && self.bytes == 90
            && self.sha256 == "27ad1b5fb4eebeb1da6419cbfad3ac1922ed1d817fcbab0a1dae1bdfd38d8307"
            && self.producer.role == UnitRole::HostBuildScript
            && self.producer.target == "build-script-build"
            && self.producer.source.as_os_str() == "build.rs"
            && self.producer.profile == "dev"
            && self.producer.triple == native
            && valid_features(&self.producer)
            && (1..=slots::MAX_GENERATED_INPUTS).contains(&self.consumers.len())
            && self.consumers.iter().collect::<BTreeSet<_>>().len() == self.consumers.len()
            && self.consumers.iter().all(|consumer| {
                consumer.package_id == self.producer.package_id
                    && consumer.role == UnitRole::TargetLibrary
                    && consumer.target == "serde_core"
                    && consumer.source.as_os_str() == "src/lib.rs"
                    && consumer.profile == self.producer.profile
                    && consumer.triple == self.producer.triple
                    && consumer.features == self.producer.features
                    && valid_features(consumer)
            })
    }
}

fn unproved() -> Refusal {
    refused("The retained generated Rust input could not be verified.")
}

fn parent(root: &VerifiedRoot, witness: &GeneratedInputWitness) -> Result<File, Refusal> {
    let (mut directory, metadata) = open_owner_directory(&root.path).map_err(|_| unproved())?;
    if FileIdentity::of(&metadata) != root.identity
        || root.path.canonicalize().as_ref().ok() != Some(&root.path)
    {
        return Err(unproved());
    }
    let parent = witness.relative_path.parent().ok_or_else(unproved)?;
    for (component, identity) in parent.components().zip(&witness.parents) {
        check_cancelled()?;
        let Component::Normal(name) = component else {
            return Err(unproved());
        };
        let (child, metadata) =
            open_owner_directory_at(&directory, name).map_err(|_| unproved())?;
        if FileIdentity::of(&metadata) != *identity {
            return Err(unproved());
        }
        directory = child;
    }
    Ok(directory)
}

fn verify(root: &VerifiedRoot, witness: &GeneratedInputWitness) -> Result<(), Refusal> {
    if !witness.valid_for(root) {
        return Err(unproved());
    }
    let leaf = witness.relative_path.file_name().ok_or_else(unproved)?;
    let named = root.path.join(&witness.relative_path);
    let mut file = open_owner_file_at(&parent(root, witness)?, leaf).map_err(|_| unproved())?;
    let before = file.metadata().map_err(|_| unproved())?;
    if FileIdentity::of(&before) != witness.file
        || before.len() != witness.bytes
        || named.canonicalize().as_ref().ok() != Some(&named)
    {
        return Err(unproved());
    }
    let digest = sha256_reader(&mut file, witness.bytes, cancelled).map_err(|err| {
        if err.kind() == ErrorKind::Interrupted {
            StateError::Cancelled.into()
        } else {
            unproved()
        }
    })?;
    let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let after = file.metadata().map_err(|_| unproved())?;
    let named_again = open_owner_file_at(&parent(root, witness)?, leaf)
        .and_then(|file| file.metadata())
        .map_err(|_| unproved())?;
    if digest != witness.sha256
        || after.len() != before.len()
        || named_again.len() != before.len()
        || directory_stamp(&before) != directory_stamp(&after)
        || directory_stamp(&before) != directory_stamp(&named_again)
        || named.canonicalize().as_ref().ok() != Some(&named)
    {
        return Err(unproved());
    }
    check_cancelled()
}

/// Rechecks recorded inputs under the caller's already held checkout output locks.
pub(super) fn verify_retained(
    profile: &PoolProfile,
    producer: &Producer,
    build_is_locked: impl Fn(&Path) -> bool,
) -> Result<(), Refusal> {
    let Producer::Complete {
        roots, invocations, ..
    } = producer
    else {
        return Ok(());
    };
    if invocations
        .iter()
        .all(|invocation| invocation.generated_inputs.is_empty())
    {
        return Ok(());
    }
    if !build_is_locked(&roots.build.path) {
        return Err(unproved());
    }
    verify_named_inputs(profile, producer)?;
    if !build_is_locked(&roots.build.path) {
        return Err(unproved());
    }
    Ok(())
}

pub(super) fn retained_gc_inputs(
    producer: &Producer,
) -> Option<(&VerifiedRoot, Vec<&GeneratedInputWitness>)> {
    let Producer::Complete {
        roots, invocations, ..
    } = producer
    else {
        return None;
    };
    let inputs: Vec<_> = invocations
        .iter()
        .flat_map(|record| &record.generated_inputs)
        .collect();
    (!inputs.is_empty()).then_some((&roots.build, inputs))
}

// The strict GC caller holds the current output guards across this check.
pub(super) fn verify_named_inputs(
    profile: &PoolProfile,
    producer: &Producer,
) -> Result<(), Refusal> {
    let Producer::Complete {
        roots, invocations, ..
    } = producer
    else {
        return Ok(());
    };
    for record in invocations
        .iter()
        .filter(|invocation| !invocation.generated_inputs.is_empty())
    {
        let invocation = profile
            .invocations
            .iter()
            .find(|invocation| invocation.id == record.evidence.invocation)
            .ok_or_else(unproved)?;
        let seal =
            NativeBuild::read(profile, invocation, &native_target())?.ok_or_else(unproved)?;
        seal.verify_generated_bindings(&record.generated_inputs)?;
        for input in &record.generated_inputs {
            verify(&roots.build, input)?;
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use cargo_hive_platform::flock::try_exclusive;
    use cargo_hive_state::slots::{FileIdentity, random_pool_id};
    use std::fs::{self, File};
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    const PRIVATE_RS: &[u8] =
        b"#[doc(hidden)]\npub mod __private228 {\n    #[doc(hidden)]\n    pub use crate::private::*;\n}\n";
    const PRIVATE_SHA256: &str = "27ad1b5fb4eebeb1da6419cbfad3ac1922ed1d817fcbab0a1dae1bdfd38d8307";

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_named_generated_rust_input_checks_content_and_retained_identity() {
        let scratch = Scratch(
            Path::new("/private/tmp").join(format!("hive-generated-{}", random_pool_id().unwrap())),
        );
        let out = scratch.0.join("build/serde_core-producer/out");
        fs::create_dir_all(&out).unwrap();
        let input = out.join("private.rs");
        fs::write(&input, PRIVATE_RS).unwrap();
        let lock = File::create(scratch.0.join(".cargo-lock")).unwrap();
        let _guard = try_exclusive(&lock).unwrap().unwrap();
        let identity = |path: &Path| FileIdentity::of(&fs::symlink_metadata(path).unwrap());
        let root = VerifiedRoot {
            path: scratch.0.clone(),
            identity: identity(&scratch.0),
        };
        let witness = GeneratedInputWitness {
            declaration_sha256: "a".repeat(64),
            build_root: root.identity,
            relative_path: input.strip_prefix(&root.path).unwrap().into(),
            parents: vec![
                identity(&scratch.0.join("build")),
                identity(&scratch.0.join("build/serde_core-producer")),
                identity(&out),
            ],
            file: identity(&input),
            bytes: PRIVATE_RS.len() as u64,
            sha256: PRIVATE_SHA256.into(),
        };
        assert!(
            verify(&root, &witness).is_ok(),
            "the exact generated leaf should verify"
        );

        let file = File::open(&input).unwrap();
        let times = fs::FileTimes::new().set_modified(file.metadata().unwrap().modified().unwrap());
        let mut changed = PRIVATE_RS.to_vec();
        changed[0] = b'!';
        fs::write(&input, changed).unwrap();
        file.set_times(times).unwrap();
        assert!(verify(&root, &witness).is_err());

        fs::rename(&input, out.join("prior-private.rs")).unwrap();
        fs::write(&input, PRIVATE_RS).unwrap();
        assert!(verify(&root, &witness).is_err());

        fs::remove_file(&input).unwrap();
        let original = out.join("prior-private.rs");
        fs::write(&original, PRIVATE_RS).unwrap();
        symlink(&original, &input).unwrap();
        assert!(verify(&root, &witness).is_err());

        fs::remove_file(&input).unwrap();
        fs::rename(&original, &input).unwrap();
        assert!(verify(&root, &witness).is_ok());
        let old_out = out.with_file_name("prior-out");
        fs::rename(&out, &old_out).unwrap();
        fs::create_dir(&out).unwrap();
        fs::rename(old_out.join("private.rs"), &input).unwrap();
        assert_eq!(identity(&input), witness.file);
        assert!(verify(&root, &witness).is_err());
    }

    #[test]
    fn a_generated_declaration_requires_exact_producer_consumer_and_bytes() {
        let package = "registry+https://github.com/rust-lang/crates.io-index#serde_core@1.0.228";
        let native = "aarch64-apple-darwin";
        let material = "a".repeat(64);
        let input = serde_json::json!({
            "producer": {
                "package_id": package, "target": "build-script-build", "source": "build.rs",
                "role": "host_build_script", "profile": "dev", "triple": native,
                "features": ["alloc", "result", "std"],
            },
            "package_material_sha256": material,
            "consumers": [{
                "package_id": package, "target": "serde_core", "source": "src/lib.rs",
                "role": "target_library", "profile": "dev", "triple": native,
                "features": ["alloc", "result", "std"],
            }],
            "leaf": "private.rs", "bytes": 90, "sha256": PRIVATE_SHA256,
        });
        let valid = |value| {
            serde_json::from_value::<GeneratedRust>(value)
                .is_ok_and(|item| item.valid_for("serde_core", "1.0.228", &material, native))
        };
        assert!(valid(input.clone()));
        let declaration: GeneratedRust = serde_json::from_value(input.clone()).unwrap();
        let review = digest(&serde_json::to_vec(&declaration).unwrap())
            .ok()
            .unwrap();
        let mut observation = GeneratedInputWitness {
            declaration_sha256: review,
            build_root: FileIdentity { dev: 1, ino: 1 },
            relative_path: "unit/out/private.rs".into(),
            parents: vec![
                FileIdentity { dev: 1, ino: 2 },
                FileIdentity { dev: 1, ino: 3 },
            ],
            file: FileIdentity { dev: 1, ino: 4 },
            bytes: 90,
            sha256: PRIVATE_SHA256.into(),
        };
        assert!(matches!(
            declaration.matches_witnesses(std::slice::from_ref(&observation)),
            Ok(true)
        ));
        observation.declaration_sha256 = "b".repeat(64);
        assert!(matches!(
            declaration.matches_witnesses(&[observation]),
            Ok(false)
        ));

        let mut other = input.clone();
        other["producer"]["source"] = serde_json::json!("other/build.rs");
        assert!(!valid(other));
        let mut other = input.clone();
        other["consumers"][0]["profile"] = serde_json::json!("release");
        assert!(!valid(other));
        let mut other = input.clone();
        other["consumers"][0]["features"] = serde_json::json!(["std"]);
        assert!(!valid(other));
        let mut other = input.clone();
        other["consumers"][0]["package_id"] = serde_json::json!("somewhere-else");
        assert!(!valid(other));
        let mut other = input;
        other["sha256"] = serde_json::json!("c".repeat(64));
        assert!(!valid(other));
    }
}
