//! The installed command tree stays as 0.1.0 shipped it.
use cargo_hive::cli::Cli;
use clap::CommandFactory;

fn dump(cmd: &clap::Command, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    out.push_str(&format!(
        "{pad}cmd {} hidden={} about={:?} long_about={:?}\n",
        cmd.get_name(),
        cmd.is_hide_set(),
        cmd.get_about().map(|a| a.to_string()),
        cmd.get_long_about().map(|a| a.to_string())
    ));
    let mut args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| a.get_id() != "version")
        .collect();
    args.sort_by_key(|a| a.get_id().to_string());
    for a in args {
        out.push_str(&format!(
            "{pad}  arg {} long={:?} short={:?} hidden={} required={} defaults={:?} help={:?} long_help={:?}\n",
            a.get_id(), a.get_long(), a.get_short(), a.is_hide_set(), a.is_required_set(),
            a.get_default_values(), a.get_help().map(|h| h.to_string()),
            a.get_long_help().map(|h| h.to_string())));
    }
    out.push_str(&format!(
        "{pad}  long_help_render:\n{}\n",
        cmd.clone().render_long_help()
    ));
    let mut subs: Vec<_> = cmd.get_subcommands().collect();
    subs.sort_by_key(|s| s.get_name().to_string());
    for s in subs {
        dump(s, depth + 1, out);
    }
}

#[test]
fn the_command_tree_matches_0_1_0() {
    let mut out = String::new();
    dump(&Cli::command(), 0, &mut out);
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/v0.1.0/cli-tree.txt"
    );
    if let Ok(path) = std::env::var("HIVE_WRITE_CLI_TREE") {
        std::fs::write(path, &out).unwrap();
        return;
    }
    assert_eq!(out, std::fs::read_to_string(fixture).unwrap());
}
