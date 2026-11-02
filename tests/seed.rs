#![cfg(target_os = "macos")]
mod common;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::Hive;

#[test]
fn the_fixture_builds_a_registry_crate_with_a_build_script_offline() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let built = cargo_json(&hive, &repo.main, &["build"]);
    assert!(
        built.registry_recompiled.iter().any(|n| n == "depcrate"),
        "{:?}",
        built.registry_recompiled
    );
    assert!(built.registry_recompiled.iter().any(|n| n == "leafcrate"));
    let deps = repo.main.join("target/debug/deps");
    let dep_info = std::fs::read_dir(&deps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.extension().is_some_and(|x| x == "d")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("depcrate-")
        })
        .unwrap();
    let first = std::fs::read_to_string(dep_info).unwrap();
    let cargo_home = hive.cargo_home.path.join("registry/src");
    assert!(first.contains(&*cargo_home.to_string_lossy()), "{first}");
    let again = cargo_json(&hive, &repo.main, &["build"]);
    assert!(again.registry_recompiled.is_empty());
}
