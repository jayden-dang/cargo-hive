mod common;
use common::prove::{check_command, prove, ProveInput, WindowOutcome};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const VARS: [&str; 3] = [
    "HIVE_PROVE_TARGET",
    "HIVE_PROVE_CHECKOUT",
    "HIVE_PROVE_CMDS",
];

fn missing_vars(get: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
    VARS.into_iter()
        .filter(|name| get(name).is_none_or(|v| v.is_empty()))
        .collect()
}

#[test]
fn missing_proof_variables_are_named() {
    let table: [(&[&str], Vec<&str>); 6] = [
        (&VARS, vec![]),
        (
            &["HIVE_PROVE_TARGET", "HIVE_PROVE_CMDS"],
            vec!["HIVE_PROVE_CHECKOUT"],
        ),
        (
            &["HIVE_PROVE_CHECKOUT"],
            vec!["HIVE_PROVE_TARGET", "HIVE_PROVE_CMDS"],
        ),
        (&[], VARS.to_vec()),
        (
            &["HIVE_PROVE_TARGET"],
            vec!["HIVE_PROVE_CHECKOUT", "HIVE_PROVE_CMDS"],
        ),
        (&["OTHER"], VARS.to_vec()),
    ];
    for (set, expected) in table {
        let got = missing_vars(|name| set.contains(&name).then(|| "x".to_string()));
        assert_eq!(got, expected, "set: {set:?}");
    }
    let empty = missing_vars(|name| {
        Some(if name == VARS[1] {
            String::new()
        } else {
            "x".into()
        })
    });
    assert_eq!(
        empty,
        vec!["HIVE_PROVE_CHECKOUT"],
        "an empty value counts as unset"
    );
}

#[test]
#[ignore = "needs HIVE_PROVE_TARGET, HIVE_PROVE_CHECKOUT and HIVE_PROVE_CMDS"]
fn deleting_dead_units_rebuilds_no_live_unit_on_a_real_target() {
    let get = |name: &str| std::env::var(name).ok();
    let missing = missing_vars(get);
    assert!(
        missing.is_empty(),
        "set these variables to run the proof: {}",
        missing.join(", ")
    );
    let cmds: Vec<Vec<String>> = serde_json::from_str(&get(VARS[2]).unwrap())
        .unwrap_or_else(|e| panic!("HIVE_PROVE_CMDS must be a JSON array of string arrays: {e}"));
    let refused: Vec<String> = cmds
        .iter()
        .filter_map(|cmd| {
            check_command(cmd)
                .err()
                .map(|why| format!("{cmd:?}: {why}"))
        })
        .collect();
    assert!(
        refused.is_empty(),
        "commands refused:\n{}",
        refused.join("\n")
    );
    let hour = 3600;
    let input = ProveInput {
        target: PathBuf::from(get(VARS[0]).unwrap()),
        checkout: PathBuf::from(get(VARS[1]).unwrap()),
        cmds,
        windows: [6 * hour, 24 * hour, 7 * 24 * hour]
            .map(Duration::from_secs)
            .to_vec(),
        now: SystemTime::now(),
    };

    let report = prove(&input).unwrap_or_else(|e| panic!("the proof could not run: {e}"));

    println!("{}", report.render());
    for window in &report.windows {
        if let WindowOutcome::RebuiltLive(units) = &window.outcome {
            panic!(
                "window {:?} rebuilt live units: {}",
                window.window,
                units.join(", ")
            );
        }
    }
    assert!(
        !report.all_vacuous(),
        "every window was vacuous, so nothing was proved"
    );
}
