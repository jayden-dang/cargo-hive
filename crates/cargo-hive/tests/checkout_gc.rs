#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::Hive;
use common::registry_fixture::{RegistryRepo, cargo_json};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn the_binaries_fixture_links_a_bin_a_test_an_example_a_build_script_and_a_proc_macro_offline() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let _built = cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let deps = debug.join("deps");

    assert!(debug.join("app").is_file(), "bin missing");
    assert!(
        named_under(&deps, "it-").is_some_and(|p| is_linked_binary(&p)),
        "integration test binary missing under deps/"
    );
    assert!(debug.join("examples/ex").is_file(), "example missing");
    assert!(
        build_script_under(&debug.join("build")).is_some(),
        "build-script-build missing under build/"
    );
    assert!(
        deps_dylib(&deps, "libappmacro-").is_some(),
        "proc-macro dylib missing under deps/"
    );

    let objects: Vec<_> = fs::read_dir(&deps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "o") && p.to_string_lossy().contains(".rcgu.")
        })
        .collect();
    assert!(
        !objects.is_empty(),
        "expected loose deps/*.rcgu.o beside the linked binaries"
    );
}

fn named_under(dir: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_str()?;
        (name.starts_with(prefix) && !name.contains('.')).then_some(p)
    })
}

fn deps_dylib(dir: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_str()?;
        (name.starts_with(prefix) && name.ends_with(".dylib")).then_some(p)
    })
}

fn build_script_under(build: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(build).ok()? {
        let dir = entry.ok()?.path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name()?.to_str()?;
        if !name.starts_with("app-") {
            continue;
        }
        let script = dir.join("build-script-build");
        if script.is_file() {
            return Some(script);
        }
    }
    None
}

fn is_linked_binary(path: &Path) -> bool {
    path.is_file()
        && fs::metadata(path)
            .ok()
            .is_some_and(|m| m.len() > 0 && !m.file_type().is_symlink())
}
