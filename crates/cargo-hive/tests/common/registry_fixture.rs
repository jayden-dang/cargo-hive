//! A local-registry crate with a build script under the scratch `CARGO_HOME`.
//! `cargo_json` sends cargo's stdout to a file: `wait_within` does not drain a pipe.

use super::{Hive, Scratch, git, register, wait_within};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const COMMAND_DEADLINE: Duration = Duration::from_secs(300);
const OUTPUT_CAP: u64 = 256 * 1024 * 1024;
const LINE_CAP: usize = 100_000;

const LEAF_TOML: &str = "\
[package]
name = \"leafcrate\"
version = \"1.0.0\"
edition = \"2021\"
";

const LEAF_LIB: &str = "pub fn leaf() -> u32 { 1 }\n";

const DEP_TOML: &str = "\
[package]
name = \"depcrate\"
version = \"1.0.0\"
edition = \"2021\"
build = \"build.rs\"

[dependencies]
leafcrate = \"1\"
";

const DEP_BUILD: &str = "\
fn main() {
    let out = std::env::var(\"OUT_DIR\").unwrap();
    std::fs::write(
        std::path::Path::new(&out).join(\"generated.rs\"),
        \"pub const G: u32 = 7;\\n\",
    )
    .unwrap();
}
";

const DEP_LIB: &str = "\
include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));
pub fn dep() -> u32 { leafcrate::leaf() + G }
";

const DEP_DEPS: &str = "\
[{\"name\":\"leafcrate\",\"req\":\"^1\",\"features\":[],\"optional\":false,\"default_features\":true,\"target\":null,\"kind\":\"normal\"}]\
";

const TOP_TOML: &str = "\
[package]
name = \"topcrate\"
version = \"1.0.0\"
edition = \"2021\"

[dependencies]
depcrate = \"1\"
";

const TOP_LIB: &str = "pub fn top() -> u32 { depcrate::dep() + 1 }\n";

const TOP_DEPS: &str = "\
[{\"name\":\"depcrate\",\"req\":\"^1\",\"features\":[],\"optional\":false,\"default_features\":true,\"target\":null,\"kind\":\"normal\"}]\
";

pub struct RegistryRepo {
    pub scratch: Scratch,
    pub main: PathBuf,
}

impl RegistryRepo {
    pub fn new(hive: &Hive) -> RegistryRepo {
        RegistryRepo::build(hive, false)
    }

    /// The same repo, with a third registry crate `topcrate` on top of `depcrate`, and
    /// the app depending on `topcrate`. `depcrate` includes a file from its `OUT_DIR`.
    #[allow(dead_code)]
    pub fn with_top(hive: &Hive) -> RegistryRepo {
        RegistryRepo::build(hive, true)
    }

    fn build(hive: &Hive, top: bool) -> RegistryRepo {
        let scratch = Scratch::new("registry");
        let reg = scratch.path.join("reg");
        fs::create_dir_all(&reg).unwrap();
        let leaf_cksum = pack(&scratch, &reg, "leafcrate", LEAF_TOML, LEAF_LIB, None);
        let dep_cksum = pack(
            &scratch,
            &reg,
            "depcrate",
            DEP_TOML,
            DEP_LIB,
            Some(DEP_BUILD),
        );
        write_index(&reg, "leafcrate", "[]", &leaf_cksum);
        write_index(&reg, "depcrate", DEP_DEPS, &dep_cksum);
        if top {
            let top_cksum = pack(&scratch, &reg, "topcrate", TOP_TOML, TOP_LIB, None);
            write_index(&reg, "topcrate", TOP_DEPS, &top_cksum);
        }
        let (app_dep, app_lib) = if top {
            ("topcrate", "pub fn app() -> u32 { topcrate::top() }\n")
        } else {
            ("depcrate", "pub fn app() -> u32 { depcrate::dep() }\n")
        };
        fs::write(
            hive.cargo_home.path.join("config.toml"),
            format!(
                "[source.crates-io]\nreplace-with = \"fixture\"\n\n[source.fixture]\nlocal-registry = \"{}\"\n",
                reg.display()
            ),
        )
        .unwrap();

        let main = scratch.path.join("main");
        fs::create_dir_all(main.join("src")).unwrap();
        git(&main, &["init", "-q"]);
        fs::write(
            main.join("Cargo.toml"),
            format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{app_dep} = \"1\"\n"),
        )
        .unwrap();
        fs::write(main.join("src/lib.rs"), app_lib).unwrap();
        fs::write(main.join(".gitignore"), "target\n").unwrap();
        let mut lock = Command::new("cargo");
        lock.args(["generate-lockfile", "--offline"])
            .current_dir(&main)
            .env("CARGO_HOME", &hive.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        checked(&scratch.path, "generate-lockfile", &mut lock);
        git(&main, &["add", "-A"]);
        git(
            &main,
            &["commit", "-q", "-m", &format!("init {}", main.display())],
        );
        let main = main.canonicalize().unwrap();
        hive.config(&format!(
            r#"{{"secondary_roots": ["{}"]}}"#,
            scratch.path.display()
        ));
        register(hive, &main);
        RegistryRepo { scratch, main }
    }

    pub fn worktree(&self, name: &str) -> PathBuf {
        let path = self.scratch.path.join(name);
        git(
            &self.main,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }
}

pub struct Compiled {
    pub registry_fresh: Vec<String>,
    pub registry_recompiled: Vec<String>,
    pub workspace_recompiled: Vec<String>,
}

pub fn cargo_json(hive: &Hive, dir: &Path, args: &[&str]) -> Compiled {
    let mut cmd = Command::new("cargo");
    cmd.args(args)
        .arg("--offline")
        .arg("--message-format=json-render-diagnostics")
        .current_dir(dir)
        .env("CARGO_HOME", &hive.cargo_home.path)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR");
    let text = checked(&hive.home.path, "cargo-json", &mut cmd);
    parse_compiled(&text)
}

fn pack(
    scratch: &Scratch,
    reg: &Path,
    name: &str,
    manifest: &str,
    lib: &str,
    build: Option<&str>,
) -> String {
    let parent = scratch.path.join("pack");
    let dir_name = format!("{name}-1.0.0");
    let dir = parent.join(&dir_name);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(dir.join("src/lib.rs"), lib).unwrap();
    if let Some(build) = build {
        fs::write(dir.join("build.rs"), build).unwrap();
    }
    let archive = reg.join(format!("{name}-1.0.0.crate"));
    let mut cmd = Command::new("tar");
    cmd.args(["-czf", archive.to_str().unwrap(), &dir_name])
        .current_dir(&parent)
        .env("COPYFILE_DISABLE", "1");
    checked(&scratch.path, &format!("tar-{name}"), &mut cmd);
    let sum = checked(
        &scratch.path,
        &format!("shasum-{name}"),
        Command::new("shasum").args(["-a", "256", archive.to_str().unwrap()]),
    );
    let cksum = sum
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("shasum wrote nothing for {name}"))
        .to_string();
    assert_eq!(cksum.len(), 64, "{cksum}");
    cksum
}

fn write_index(reg: &Path, name: &str, deps: &str, cksum: &str) {
    assert!(name.len() >= 4, "{name}");
    let path = reg
        .join("index")
        .join(&name[..2])
        .join(&name[2..4])
        .join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "{{\"name\":\"{name}\",\"vers\":\"1.0.0\",\"deps\":{deps},\"cksum\":\"{cksum}\",\"features\":{{}},\"yanked\":false}}\n"
        ),
    )
    .unwrap();
}

fn parse_compiled(text: &str) -> Compiled {
    let mut compiled = Compiled {
        registry_fresh: Vec::new(),
        registry_recompiled: Vec::new(),
        workspace_recompiled: Vec::new(),
    };
    for (n, line) in text.lines().enumerate() {
        assert!(n < LINE_CAP, "walk bound hit");
        if line.is_empty() {
            continue;
        }
        let message: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("cargo stdout line {} is not JSON ({e}): {line}", n + 1));
        if message["reason"].as_str() != Some("compiler-artifact") {
            continue;
        }
        let package_id = message["package_id"]
            .as_str()
            .unwrap_or_else(|| panic!("compiler-artifact has no package_id: {line}"));
        let name = unit_name(package_id);
        let fresh = message["fresh"]
            .as_bool()
            .unwrap_or_else(|| panic!("compiler-artifact has no fresh flag: {line}"));
        if package_id.starts_with("registry+") {
            if fresh {
                compiled.registry_fresh.push(name);
            } else {
                compiled.registry_recompiled.push(name);
            }
        } else if !fresh {
            compiled.workspace_recompiled.push(name);
        }
    }
    compiled
}

fn unit_name(package_id: &str) -> String {
    let hash = package_id
        .find('#')
        .unwrap_or_else(|| panic!("no # in {package_id}"));
    let at = package_id[hash + 1..]
        .find('@')
        .unwrap_or_else(|| panic!("no @ in {package_id}"))
        + hash
        + 1;
    package_id[hash + 1..at].to_string()
}

fn checked(dir: &Path, label: &str, cmd: &mut Command) -> String {
    let stdout = dir.join(format!("{label}.out"));
    let stderr = dir.join(format!("{label}.err"));
    let child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {label}: {e}"));
    let output = wait_within(child, COMMAND_DEADLINE);
    let err = read_capped(&stderr);
    assert!(output.status.success(), "{label} failed: {err}");
    read_capped(&stdout)
}

fn read_capped(path: &Path) -> String {
    let len = fs::metadata(path).unwrap().len();
    assert!(
        len <= OUTPUT_CAP,
        "{} is {len} bytes, over the {OUTPUT_CAP} byte cap",
        path.display()
    );
    fs::read_to_string(path).unwrap()
}
