//! Read-only input qualification for the pinned SQLx 0.9.0 query macro.

use super::{
    Invocation, NativePackage, digest, native_environment, read_input_bounded, refused, relative,
};
use crate::slot::{Refusal, canonical_worktree_path, check_cancelled, exists};
use cargo_hive_cargo::discover::output_paths_may_overlap;
use cargo_hive_state::slots::{self, EnvironmentExpectation};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const PACKAGE_LIMIT: usize = 128;
const ANCESTOR_LIMIT: usize = 64;
const QUERY_LIMIT: usize = 4096;
const AUX_FILE_LIMIT: usize = 64 * 1024;
const QUERY_FILE_LIMIT: usize = 256 * 1024;
const TOTAL_INPUT_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OfflineInputs {
    packages: Vec<PackageInputs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageInputs {
    manifest: PathBuf,
    config_sha256: Option<String>,
    #[serde(default)]
    dotenv: Vec<DotenvInput>,
    queries: Vec<QueryInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DotenvInput {
    // Zero is the package directory. Unlisted ancestor probes must be absent.
    ancestor: usize,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryInput {
    query_sha256: String,
    data_sha256: String,
    cache: Cache,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cache {
    Override,
    Package,
    Workspace,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryData {
    db_name: String,
    query: String,
    hash: String,
    #[serde(rename = "describe")]
    description: serde_json::Map<String, serde_json::Value>,
}

impl OfflineInputs {
    pub(super) fn valid(&self, workspace: &[NativePackage]) -> bool {
        let mut manifests = BTreeSet::new();
        let mut queries_left = QUERY_LIMIT;
        (1..=PACKAGE_LIMIT).contains(&self.packages.len())
            && self.packages.iter().all(|package| {
                let Some(left) = queries_left.checked_sub(package.queries.len()) else {
                    return false;
                };
                queries_left = left;
                relative(&package.manifest)
                    && package
                        .manifest
                        .file_name()
                        .is_some_and(|name| name == "Cargo.toml")
                    && manifests.insert(&package.manifest)
                    && workspace
                        .iter()
                        .any(|record| record.manifest == package.manifest)
                    && package.valid()
            })
    }

    pub(super) fn verify(&self, checkout: &Path, invocation: &Invocation) -> Result<(), Refusal> {
        if !native_environment(&invocation.environment, true) {
            return Err(unproved());
        }
        let process_override = invocation.environment.iter().find_map(|entry| {
            match (&*entry.name, &entry.expectation) {
                ("SQLX_OFFLINE_DIR", EnvironmentExpectation::Present { value }) => {
                    Some(Path::new(value))
                }
                _ => None,
            }
        });
        let mut remaining = TOTAL_INPUT_LIMIT;
        for package in &self.packages {
            check_cancelled()?;
            package.verify(checkout, invocation, process_override, &mut remaining)?;
        }
        Ok(())
    }
}

impl PackageInputs {
    fn valid(&self) -> bool {
        let mut ancestors = BTreeSet::new();
        let mut queries = BTreeSet::new();
        self.config_sha256
            .as_ref()
            .is_none_or(|hash| slots::hex(hash, 64))
            && self.dotenv.len() <= ANCESTOR_LIMIT
            && self.dotenv.iter().all(|input| {
                input.ancestor < ANCESTOR_LIMIT
                    && ancestors.insert(input.ancestor)
                    && slots::hex(&input.sha256, 64)
            })
            && !self.queries.is_empty()
            && self.queries.iter().all(|query| {
                slots::hex(&query.query_sha256, 64)
                    && slots::hex(&query.data_sha256, 64)
                    && queries.insert(&query.query_sha256)
            })
    }

    fn verify(
        &self,
        checkout: &Path,
        invocation: &Invocation,
        process_override: Option<&Path>,
        remaining: &mut usize,
    ) -> Result<(), Refusal> {
        let manifest = checkout.join(&self.manifest);
        let package = manifest.parent().ok_or_else(unproved)?;
        slots::owner_directory(package).map_err(|_| unproved())?;
        checked_file(
            &package.join("sqlx.toml"),
            self.config_sha256.as_deref(),
            remaining,
        )?;
        let dotenv_override = self.check_dotenv(package, remaining)?;
        let override_dir = process_override.or(dotenv_override.as_deref());
        let package_cache = package.join(".sqlx");
        let workspace_cache = checkout.join(".sqlx");
        let candidates = [
            override_dir.map(|path| (Cache::Override, path)),
            Some((Cache::Package, package_cache.as_path())),
            Some((Cache::Workspace, workspace_cache.as_path())),
        ];
        let output_roots = [
            checkout.join(&invocation.target_root),
            checkout.join(&invocation.build_root),
        ];
        for (_, path) in candidates.iter().flatten() {
            if !confined_cache(path, checkout, &output_roots) {
                return Err(unproved());
            }
        }
        for query in &self.queries {
            check_cancelled()?;
            query.verify(&candidates, remaining)?;
        }
        Ok(())
    }

    fn check_dotenv(
        &self,
        package: &Path,
        remaining: &mut usize,
    ) -> Result<Option<PathBuf>, Refusal> {
        let mut observed = 0;
        let mut offline_dir = None;
        // SQLx 0.9.0 visits nearest to farthest and overwrites earlier dotenv values.
        for (ancestor, dir) in package.ancestors().enumerate() {
            check_cancelled()?;
            if ancestor >= ANCESTOR_LIMIT {
                return Err(unproved());
            }
            let expected = self.dotenv.iter().find(|entry| entry.ancestor == ancestor);
            let bytes = checked_file(
                &dir.join(".env"),
                expected.map(|entry| entry.sha256.as_str()),
                remaining,
            )?;
            if let Some(bytes) = bytes {
                observed += 1;
                if let Some(path) = dotenv_offline_dir(&bytes)? {
                    offline_dir = Some(path);
                }
            }
        }
        if observed != self.dotenv.len() {
            return Err(unproved());
        }
        Ok(offline_dir)
    }
}

impl QueryInput {
    fn verify(
        &self,
        candidates: &[Option<(Cache, &Path)>; 3],
        remaining: &mut usize,
    ) -> Result<(), Refusal> {
        let name = format!("query-{}.json", self.query_sha256);
        for (kind, dir) in candidates.iter().flatten() {
            let path = dir.join(&name);
            // SQLx selects the first present query file, not the first present directory.
            if *kind != self.cache {
                require_absent(&path)?;
                continue;
            }
            let bytes = read_budgeted(&path, QUERY_FILE_LIMIT, remaining)?.ok_or_else(unproved)?;
            if digest(&bytes)? != self.data_sha256 {
                return Err(unproved());
            }
            let data: QueryData = serde_json::from_slice(&bytes).map_err(|_| unproved())?;
            if data.db_name.is_empty()
                || data.description.is_empty()
                || data.hash != self.query_sha256
                || digest(data.query.as_bytes())? != self.query_sha256
            {
                return Err(unproved());
            }
            return Ok(());
        }
        Err(unproved())
    }
}

fn confined_cache(path: &Path, checkout: &Path, output_roots: &[PathBuf; 2]) -> bool {
    path != checkout
        && path.starts_with(checkout)
        && path.strip_prefix(checkout).is_ok_and(relative)
        && canonical_worktree_path(path)
            .is_ok_and(|canonical| canonical.as_os_str() == path.as_os_str())
        && output_roots
            .iter()
            .all(|root| !output_paths_may_overlap(root, path))
}

fn checked_file(
    path: &Path,
    expected: Option<&str>,
    remaining: &mut usize,
) -> Result<Option<Vec<u8>>, Refusal> {
    if let Some(expected) = expected {
        let bytes = read_budgeted(path, AUX_FILE_LIMIT, remaining)?.ok_or_else(unproved)?;
        if digest(&bytes)? != expected {
            return Err(unproved());
        }
        Ok(Some(bytes))
    } else {
        require_absent(path)?;
        Ok(None)
    }
}

fn require_absent(path: &Path) -> Result<(), Refusal> {
    if canonical_worktree_path(path)
        .is_ok_and(|canonical| canonical.as_os_str() == path.as_os_str())
        && matches!(exists(path), Ok(false))
    {
        Ok(())
    } else {
        Err(unproved())
    }
}

fn read_budgeted(
    path: &Path,
    cap: usize,
    remaining: &mut usize,
) -> Result<Option<Vec<u8>>, Refusal> {
    let bytes = read_input_bounded(path, cap.min(*remaining))?;
    if let Some(bytes) = &bytes {
        *remaining -= bytes.len();
    }
    Ok(bytes)
}

fn dotenv_offline_dir(bytes: &[u8]) -> Result<Option<PathBuf>, Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| unproved())?;
    let mut seen_offline = false;
    let mut offline_dir = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line.split_once('=').ok_or_else(unproved)?;
        // This subset requires literal values and excludes dotenv interpolation and URL variables.
        match (name, value) {
            ("SQLX_OFFLINE", "true" | "false" | "1" | "0") if !seen_offline => {
                seen_offline = true;
            }
            ("SQLX_OFFLINE_DIR", value)
                if offline_dir.is_none()
                    && value.starts_with('/')
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"/_.-".contains(&byte)) =>
            {
                offline_dir = Some(PathBuf::from(value));
            }
            _ => return Err(unproved()),
        }
    }
    Ok(offline_dir)
}

fn unproved() -> Refusal {
    refused("Could not verify the reviewed SQLx offline inputs and cache selection.")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use cargo_hive_platform::{TempDir, host_fs};
    use cargo_hive_state::slots::{CargoLayout, Environment};
    use serde_json::json;
    use std::fs;

    const MANIFEST: &str = "crates/persistence/Cargo.toml";

    struct Fixture {
        scratch: TempDir,
        root: PathBuf,
        invocation: Invocation,
        review: OfflineInputs,
        query: String,
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            let scratch = host_fs()
                .temp_dir(Path::new("/private/tmp"), "hive-sqlx-preflight")
                .unwrap();
            let root = scratch.path().join("workspace");
            // Never inspect an owner dotenv file while driving this disposable fixture.
            for path in scratch.path().ancestors().map(|dir| dir.join(".env")) {
                assert!(
                    fs::symlink_metadata(&path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                );
            }
            fs::create_dir_all(root.join("crates/persistence")).unwrap();
            let root = root.canonicalize().unwrap();
            fs::write(root.join(MANIFEST), b"[package]\nname = \"persistence\"\n").unwrap();
            let query = hash(b"SELECT 1");
            let bytes = serde_json::to_vec(&json!({
                "db_name": "PostgreSQL",
                "query": "SELECT 1",
                "describe": {"columns": [], "parameters": {"Left": []}, "nullable": []},
                "hash": query,
            }))
            .unwrap();
            let review = serde_json::from_value(json!({
                "packages": [{
                    "manifest": MANIFEST,
                    "queries": [{
                        "query_sha256": query,
                        "data_sha256": hash(&bytes),
                        "cache": "workspace",
                    }],
                }],
            }))
            .unwrap();
            let invocation = Invocation {
                id: "clippy".into(),
                cargo: "cargo pinned".into(),
                rustc: "rustc pinned".into(),
                layout: CargoLayout::Flat,
                argv: Vec::new(),
                cwd: ".".into(),
                environment: vec![
                    set("CARGO_HOME", scratch.path().join("cargo").to_str().unwrap()),
                    set("CARGO_NET_OFFLINE", "true"),
                    set("SQLX_OFFLINE", "true"),
                ],
                target_root: "target".into(),
                build_root: "build-output".into(),
            };
            let fixture = Self {
                scratch,
                root,
                invocation,
                review,
                query,
                bytes,
            };
            fixture.write_query(".sqlx", &fixture.bytes);
            fixture
        }

        fn query_in(&self, directory: &str) -> PathBuf {
            self.root
                .join(directory)
                .join(format!("query-{}.json", self.query))
        }

        fn write_query(&self, directory: &str, bytes: &[u8]) {
            let path = self.query_in(directory);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        fn write_dotenv(&mut self, rel: &str, ancestor: usize, bytes: &[u8]) {
            fs::write(self.root.join(rel), bytes).unwrap();
            self.review.packages[0].dotenv.push(DotenvInput {
                ancestor,
                sha256: hash(bytes),
            });
        }

        fn verifies(&self) -> bool {
            self.review.verify(&self.root, &self.invocation).is_ok()
        }
    }

    fn set(name: &str, value: &str) -> Environment {
        Environment {
            name: name.into(),
            expectation: EnvironmentExpectation::Present {
                value: value.into(),
            },
        }
    }

    fn hash(bytes: &[u8]) -> String {
        digest(bytes).unwrap_or_else(|_| panic!("could not hash scratch fixture bytes"))
    }

    #[test]
    fn offline_workspace_cache_requires_the_exact_query_and_no_earlier_hit() {
        let mut fixture = Fixture::new();
        assert!(fixture.verifies());
        fs::create_dir_all(fixture.root.join("crates/persistence/.sqlx")).unwrap();
        assert!(
            fixture.verifies(),
            "an empty package directory is not a query hit"
        );

        fixture.write_query("crates/persistence/.sqlx", &fixture.bytes);
        assert!(
            !fixture.verifies(),
            "the competing query shadows the workspace"
        );
        fixture.review.packages[0].queries[0].cache = Cache::Package;
        assert!(fixture.verifies());
        let selected = fixture.query_in("crates/persistence/.sqlx");
        let mtime = fs::metadata(&selected).unwrap().modified().unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(&fixture.bytes).unwrap();
        changed["query"] = json!("SELECT 2");
        let changed = serde_json::to_vec(&changed).unwrap();
        assert_eq!(changed.len(), fixture.bytes.len());
        fs::write(&selected, &changed).unwrap();
        fs::File::open(&selected)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(
            !fixture.verifies(),
            "same mtime does not admit changed bytes"
        );
        fixture.review.packages[0].queries[0].data_sha256 = hash(&changed);
        assert!(
            !fixture.verifies(),
            "a resealed file must still match the query hash"
        );
    }

    #[test]
    fn dotenv_ancestors_and_process_values_select_the_reviewed_override_in_order() {
        let mut fixture = Fixture::new();
        let near = fixture.root.join("nearest-cache");
        let far = fixture.root.join("far-cache");
        let near_dotenv = format!("SQLX_OFFLINE_DIR={}\n", near.display());
        let far_dotenv = format!("SQLX_OFFLINE=0\nSQLX_OFFLINE_DIR={}\n", far.display());
        fixture.write_dotenv("crates/persistence/.env", 0, near_dotenv.as_bytes());
        fixture.write_dotenv(".env", 2, far_dotenv.as_bytes());
        fixture.review.packages[0].queries[0].cache = Cache::Override;
        fixture.write_query("nearest-cache", b"unreviewed nearest bytes");
        fixture.write_query("far-cache", &fixture.bytes);
        assert!(
            fixture.verifies(),
            "the farther ancestor supplies the override"
        );

        fixture
            .invocation
            .environment
            .push(set("SQLX_OFFLINE_DIR", near.to_str().unwrap()));
        assert!(
            !fixture.verifies(),
            "the exact process override wins even with bad bytes"
        );
        fixture.write_query("nearest-cache", &fixture.bytes);
        fixture.write_query("far-cache", b"unreviewed farthest bytes");
        assert!(fixture.verifies());
        fs::write(
            fixture.root.join("crates/.env"),
            b"# earlier absent ancestor became present\n",
        )
        .unwrap();
        assert!(
            !fixture.verifies(),
            "even unused ancestor dotenv is read by SQLx"
        );
    }

    #[test]
    fn changing_sqlx_configuration_or_pinned_offline_mode_withholds_input_qualification() {
        let mut fixture = Fixture::new();
        let sqlx_toml = fixture.root.join("crates/persistence/sqlx.toml");
        let expected = b"[macros]\n";
        fs::write(&sqlx_toml, expected).unwrap();
        assert!(
            !fixture.verifies(),
            "a newly present package config must be reviewed"
        );
        fixture.review.packages[0].config_sha256 = Some(hash(expected));
        assert!(fixture.verifies());
        fs::write(&sqlx_toml, b"[common]\n").unwrap();
        assert!(!fixture.verifies());
        fs::write(&sqlx_toml, expected).unwrap();
        fixture.invocation.environment[2] = set("SQLX_OFFLINE", "false");
        assert!(!fixture.verifies());
        fixture.invocation.environment[2] = set("SQLX_OFFLINE", "true");
        fixture.invocation.environment[1] = set("CARGO_NET_OFFLINE", "false");
        assert!(!fixture.verifies());
    }

    #[test]
    fn sqlx_cache_paths_cannot_use_either_output_role_or_unreviewed_dotenv_syntax() {
        let mut fixture = Fixture::new();
        fixture.review.packages[0].queries[0].cache = Cache::Override;
        for directory in ["target", "build-output"] {
            let path = fixture.root.join(directory);
            fixture.write_query(directory, &fixture.bytes);
            fixture
                .invocation
                .environment
                .push(set("SQLX_OFFLINE_DIR", path.to_str().unwrap()));
            assert!(!fixture.verifies());
            fixture.invocation.environment.pop();
        }
        let outside = fixture.scratch.path().join("outside");
        fixture
            .invocation
            .environment
            .push(set("SQLX_OFFLINE_DIR", outside.to_str().unwrap()));
        assert!(!fixture.verifies());
        fixture.invocation.environment.pop();

        for text in [
            "SQLX_OFFLINE_DIR=${CARGO_HOME}/.sqlx\n",
            "DATABASE_URL=postgres://fixture\n",
        ] {
            fs::write(fixture.root.join(".env"), text).unwrap();
            fixture.review.packages[0].dotenv = vec![DotenvInput {
                ancestor: 2,
                sha256: hash(text.as_bytes()),
            }];
            assert!(!fixture.verifies());
        }
    }
}
