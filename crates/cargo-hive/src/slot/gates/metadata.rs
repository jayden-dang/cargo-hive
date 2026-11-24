//! Verifies the pinned SQLx offline workspace-metadata child.

use cargo_hive_platform::flock::open_owner_state_file;
use cargo_hive_platform::signals::cancelled;
use cargo_hive_platform::slot::sha256_reader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

pub(super) const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const CARGO_LIMIT: u64 = 128 * 1024 * 1024;
const LOCK_LIMIT: u64 = 8 * 1024 * 1024;
const PACKAGE_LIMIT: usize = 1024;
const ARGUMENTS: [&str; 3] = ["metadata", "--format-version=1", "--no-deps"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MetadataGate {
    pub workspace_root: PathBuf,
    pub package_manifests: Vec<PathBuf>,
    pub cargo_lock_sha256: String,
    pub cargo: PathBuf,
    pub cargo_sha256: String,
}

#[derive(Deserialize)]
struct Metadata {
    version: u32,
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<Package>,
    resolve: serde_json::Value,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    manifest_path: PathBuf,
}

impl MetadataGate {
    pub(super) fn validate_request(
        &self,
        argv: &[OsString],
        cwd: &Path,
        env: &BTreeMap<OsString, OsString>,
    ) -> io::Result<()> {
        let value = |key: &str| env.get(OsStr::new(key)).map(OsString::as_os_str);
        let manifest = cwd.join("Cargo.toml");
        if argv != ARGUMENTS
            || !canonical_directory(&self.workspace_root)
            || !canonical_directory(cwd)
            || !cwd.starts_with(&self.workspace_root)
            || !(1..=PACKAGE_LIMIT).contains(&self.package_manifests.len())
            || !self
                .package_manifests
                .iter()
                .any(|path| path.as_os_str() == manifest.as_os_str())
            || value("CARGO_MANIFEST_DIR") != Some(cwd.as_os_str())
            || value("CARGO_MANIFEST_PATH") != Some(manifest.as_os_str())
            || value("CARGO_NET_OFFLINE") != Some(OsStr::new("true"))
            || value("SQLX_OFFLINE") != Some(OsStr::new("true"))
            || !value("CARGO_HOME").is_some_and(|path| Path::new(path).is_absolute())
            || [
                "__CARGO_FIX_PLZ",
                "DATABASE_URL",
                "SQLX_OFFLINE_DIR",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "DYLD_INSERT_LIBRARIES",
                "DYLD_LIBRARY_PATH",
            ]
            .iter()
            .any(|key| value(key).is_some())
        {
            return Err(unproved_request());
        }
        jobserver_descriptors(value("CARGO_MAKEFLAGS").ok_or_else(unproved_request)?)?;
        open_reviewed_file(&manifest)?;
        verify_digest(
            &self.workspace_root.join("Cargo.lock"),
            LOCK_LIMIT,
            &self.cargo_lock_sha256,
            false,
        )?;
        verify_digest(&self.cargo, CARGO_LIMIT, &self.cargo_sha256, true)
    }

    pub(super) fn validate_response(
        &self,
        status: ExitStatus,
        stdout: &[u8],
        stderr: &[u8],
    ) -> io::Result<()> {
        let unproved = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected SQLx metadata response",
            )
        };
        if !status.success() || !stderr.is_empty() || stdout.len() > OUTPUT_LIMIT {
            return Err(unproved());
        }
        let response: Metadata = serde_json::from_slice(stdout).map_err(|_| unproved())?;
        let expected = self
            .package_manifests
            .iter()
            .map(|path| path.as_os_str())
            .collect::<BTreeSet<_>>();
        let manifests = response
            .packages
            .iter()
            .map(|package| package.manifest_path.as_os_str())
            .collect::<BTreeSet<_>>();
        let ids = response
            .packages
            .iter()
            .map(|package| package.id.as_str())
            .collect::<BTreeSet<_>>();
        let members = response
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if response.version != 1
            || response.workspace_root.as_os_str() != self.workspace_root.as_os_str()
            || !response.resolve.is_null()
            || expected.is_empty()
            || expected.len() != self.package_manifests.len()
            || response.packages.len() != expected.len()
            || manifests != expected
            || ids.len() != response.packages.len()
            || members.len() != response.workspace_members.len()
            || ids != members
        {
            return Err(unproved());
        }
        Ok(())
    }
}

fn canonical_directory(path: &Path) -> bool {
    fs::canonicalize(path).is_ok_and(|canonical| canonical.as_os_str() == path.as_os_str())
        && path.is_dir()
}

fn open_reviewed_file(path: &Path) -> io::Result<File> {
    let canonical = fs::canonicalize(path).map_err(|_| unproved_request())?;
    if canonical.as_os_str() != path.as_os_str() {
        return Err(unproved_request());
    }
    open_owner_state_file(path).map_err(|_| unproved_request())
}

fn verify_digest(path: &Path, limit: u64, expected: &str, executable: bool) -> io::Result<()> {
    use super::super::directory_stamp;

    let mut file = open_reviewed_file(path)?;
    let before = file.metadata()?;
    if before.len() > limit || (executable && before.mode() & 0o111 == 0) {
        return Err(unproved_request());
    }
    let digest = sha256_reader(&mut file, limit, cancelled)?;
    let after = fs::symlink_metadata(path)?;
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if expected != actual
        || !after.is_file()
        || before.len() != after.len()
        || directory_stamp(&before) != directory_stamp(&after)
    {
        return Err(unproved_request());
    }
    Ok(())
}

pub(super) fn jobserver_descriptors(value: &OsStr) -> io::Result<(i32, i32)> {
    let mut pieces = value.to_str().ok_or_else(unproved_request)?.split(' ');
    let (Some("-j"), Some(fds), Some(auth), None) =
        (pieces.next(), pieces.next(), pieces.next(), pieces.next())
    else {
        return Err(unproved_request());
    };
    let fds = fds
        .strip_prefix("--jobserver-fds=")
        .filter(|fds| auth.strip_prefix("--jobserver-auth=") == Some(*fds))
        .ok_or_else(unproved_request)?;
    let (read, write) = fds.split_once(',').ok_or_else(unproved_request)?;
    let parse = |fd: &str| {
        fd.parse::<i32>()
            .ok()
            .filter(|value| *value > 2 && value.to_string() == fd)
            .ok_or_else(unproved_request)
    };
    let (read, write) = (parse(read)?, parse(write)?);
    if read == write {
        return Err(unproved_request());
    }
    Ok((read, write))
}

fn unproved_request() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unproved SQLx metadata request",
    )
}

#[cfg(test)]
mod tests {
    use super::MetadataGate;
    use serde_json::json;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    fn reviewed() -> MetadataGate {
        let root = PathBuf::from("/scratch/workspace");
        MetadataGate {
            package_manifests: vec![root.join("pkg/Cargo.toml"), root.join("tools/Cargo.toml")],
            workspace_root: root,
            cargo_lock_sha256: "a".repeat(64),
            cargo: PathBuf::from("/scratch/bin/cargo"),
            cargo_sha256: "b".repeat(64),
        }
    }

    fn response() -> serde_json::Value {
        json!({
            "version": 1,
            "workspace_root": "/scratch/workspace",
            "target_directory": "/scratch/target",
            "workspace_members": ["pkg", "tools"],
            "workspace_default_members": ["pkg"],
            "packages": [
                { "id": "tools", "manifest_path": "/scratch/workspace/tools/Cargo.toml" },
                { "id": "pkg", "manifest_path": "/scratch/workspace/pkg/Cargo.toml" }
            ],
            "resolve": null
        })
    }

    #[test]
    fn cargo_metadata_matches_the_reviewed_root_and_complete_member_set() {
        let mut response = response();
        let gate = reviewed();
        let exit = ExitStatus::from_raw(0);
        for build_directory in [false, true] {
            if build_directory {
                response["build_directory"] = json!("/scratch/build");
            }
            let raw = serde_json::to_vec(&response).unwrap();
            gate.validate_response(exit, &raw, b"").unwrap();
        }

        for (field, substitution) in [
            ("version", json!(2)),
            ("workspace_root", json!("/scratch/other")),
            ("resolve", json!({"nodes": []})),
            (
                "packages",
                json!([{ "id": "pkg", "manifest_path": "/scratch/workspace/pkg/Cargo.toml" }]),
            ),
            ("workspace_members", json!(["pkg", "unexpected"])),
        ] {
            let mut unreviewed = response.clone();
            unreviewed[field] = substitution;
            let raw = serde_json::to_vec(&unreviewed).unwrap();
            assert!(gate.validate_response(exit, &raw, b"").is_err(), "{field}");
        }
        let raw = serde_json::to_vec(&response).unwrap();
        assert!(
            gate.validate_response(ExitStatus::from_raw(1 << 8), &raw, b"")
                .is_err()
        );
        assert!(
            gate.validate_response(exit, &raw, b"unexpected stderr")
                .is_err()
        );
        response.as_object_mut().unwrap().remove("resolve");
        let raw = serde_json::to_vec(&response).unwrap();
        assert!(gate.validate_response(exit, &raw, b"").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_the_pinned_offline_child_from_a_reviewed_manifest_is_admitted() {
        use cargo_hive_platform::host_fs;
        use cargo_hive_platform::slot::sha256;
        use std::collections::BTreeMap;
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::fs::{PermissionsExt, symlink};
        use std::path::Path;

        fn hex(bytes: &[u8]) -> String {
            sha256(bytes)
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }

        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-metadata-gate")
            .unwrap();
        let workspace = scratch.path().join("workspace");
        let cwd = workspace.join("pkg");
        fs::create_dir_all(&cwd).unwrap();
        let manifest = cwd.join("Cargo.toml");
        fs::write(&manifest, "[package]").unwrap();
        let lockfile = workspace.join("Cargo.lock");
        fs::write(&lockfile, "locked").unwrap();
        let cargo = scratch.path().join("cargo");
        fs::write(&cargo, "pinned Cargo").unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
        let mut gate = MetadataGate {
            workspace_root: workspace,
            package_manifests: vec![manifest.clone()],
            cargo_lock_sha256: hex(b"locked"),
            cargo: cargo.clone(),
            cargo_sha256: hex(b"pinned Cargo"),
        };
        let argv = ["metadata", "--format-version=1", "--no-deps"].map(OsString::from);
        let mut env = BTreeMap::from([
            ("CARGO_NET_OFFLINE".into(), "true".into()),
            ("SQLX_OFFLINE".into(), "true".into()),
            ("CARGO_MANIFEST_DIR".into(), cwd.as_os_str().into()),
            ("CARGO_MANIFEST_PATH".into(), manifest.as_os_str().into()),
            (
                "CARGO_HOME".into(),
                scratch.path().join("cargo-home").into_os_string(),
            ),
            (
                "CARGO_MAKEFLAGS".into(),
                "-j --jobserver-fds=3,5 --jobserver-auth=3,5".into(),
            ),
        ]);
        gate.validate_request(&argv, &cwd, &env).unwrap();

        let mut extra_args = argv.to_vec();
        extra_args.push("--all-features".into());
        assert!(gate.validate_request(&extra_args, &cwd, &env).is_err());

        let alias = scratch.path().join("aliased-pkg");
        symlink(&cwd, &alias).unwrap();
        assert!(gate.validate_request(&argv, &alias, &env).is_err());
        env.insert("CARGO_NET_OFFLINE".into(), "false".into());
        assert!(gate.validate_request(&argv, &cwd, &env).is_err());
        env.insert("CARGO_NET_OFFLINE".into(), "true".into());
        env.insert(
            "CARGO_MAKEFLAGS".into(),
            "-j --jobserver-fds=3,5 --jobserver-auth=3,4".into(),
        );
        assert!(gate.validate_request(&argv, &cwd, &env).is_err());
        env.insert(
            "CARGO_MAKEFLAGS".into(),
            "-j --jobserver-fds=3,5 --jobserver-auth=3,5".into(),
        );

        fs::write(&lockfile, "edited").unwrap();
        assert!(gate.validate_request(&argv, &cwd, &env).is_err());
        fs::write(&lockfile, "locked").unwrap();

        let cargo_alias = scratch.path().join("cargo-alias");
        symlink(&cargo, &cargo_alias).unwrap();
        gate.cargo = cargo_alias;
        assert!(gate.validate_request(&argv, &cwd, &env).is_err());
        gate.cargo = cargo.clone();
        fs::write(cargo, "substituted Cargo").unwrap();
        assert!(gate.validate_request(&argv, &cwd, &env).is_err());
    }
}
