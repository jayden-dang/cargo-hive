use cargo_hive_platform::{host_fs, proc};
use std::{fs, time::Duration};

#[test]
fn a_cargo_gate_refuses_before_dispatch_when_the_private_plan_is_missing() {
    let scratch = host_fs()
        .temp_dir(&std::env::temp_dir(), "hive-gates-")
        .unwrap();
    let gate = scratch.path().join("hive-recipe-cargo");
    fs::copy(env!("CARGO_BIN_EXE_cargo-hive"), &gate).unwrap();
    let mut command = proc::Spec::new(gate);
    command.args = ["metadata", "--format-version=1", "--no-deps"]
        .into_iter()
        .map(Into::into)
        .collect();
    let output = proc::output_with_deadline(&command, Duration::from_secs(10), || false).unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("reviewed Nextest gate refused"),
        "{output:?}"
    );
}

#[test]
fn the_completion_gate_requires_private_recipe_evidence() {
    let scratch = host_fs()
        .temp_dir(&std::env::temp_dir(), "hive-gates-")
        .unwrap();
    let check = scratch.path().join("hive-recipe-complete");
    fs::copy(env!("CARGO_BIN_EXE_cargo-hive"), &check).unwrap();
    let mut command = proc::Spec::new(check);
    command.args.push("unexpected".into());
    let output = proc::output_with_deadline(&command, Duration::from_secs(10), || false).unwrap();
    assert_eq!(output.status.code(), Some(78));
    assert!(output.stdout.is_empty());
}
