//! The installed command tree keeps its shipped commands and adds the checkout and slot surfaces.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
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

fn tree() -> String {
    let mut out = String::new();
    dump(&Cli::command(), 0, &mut out);
    out
}

#[test]
fn the_command_tree_matches_the_current_fixture() {
    let out = tree();
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/current/cli-tree.txt"
    );
    if let Ok(path) = std::env::var("HIVE_WRITE_CLI_TREE") {
        std::fs::write(path, &out).unwrap();
        return;
    }
    assert_eq!(out, std::fs::read_to_string(fixture).unwrap());
}

#[test]
fn the_slot_help_lists_only_operator_commands() {
    let cli = Cli::command();
    let slot = cli.find_subcommand("slot").expect("slot command");
    let visible: Vec<_> = slot
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(clap::Command::get_name)
        .collect();
    assert_eq!(visible, ["init", "status", "recover"]);
    let help = slot.clone().render_long_help().to_string();
    for private in [
        "key",
        "reserve",
        "begin",
        "step-check",
        "producer",
        "ready",
        "return",
        "outcome",
        "ack",
    ] {
        assert!(slot.find_subcommand(private).unwrap().is_hide_set());
        assert!(
            !help
                .lines()
                .any(|line| line.split_whitespace().next() == Some(private))
        );
    }
}

#[test]
fn the_tree_differs_from_0_1_0_only_in_gc_and_the_additive_slot_branch() {
    let full = tree();
    let mut inside_slot = false;
    let current = full
        .split_inclusive('\n')
        .filter(|line| {
            if let Some(command) = line.strip_prefix("  cmd ") {
                inside_slot = command.starts_with("slot ");
            }
            !inside_slot && !line.starts_with("  slot ")
        })
        .collect::<String>();
    let old = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/v0.1.0/cli-tree.txt"
    ))
    .unwrap();
    let (old_gc, old_rest) = split_gc(&old);
    let (cur_gc, cur_rest) = split_gc(&current);
    assert_eq!(
        old_rest, cur_rest,
        "lines outside the gc block and root gc help must match v0.1.0"
    );
    let old_args: Vec<&str> = old_gc.lines().filter(|l| l.contains("arg ")).collect();
    let cur_args: Vec<&str> = cur_gc.lines().filter(|l| l.contains("arg ")).collect();
    let mut old_set: Vec<&str> = old_args.clone();
    let mut new_only = Vec::new();
    for arg in &cur_args {
        if let Some(i) = old_set.iter().position(|a| a == arg) {
            old_set.remove(i);
        } else {
            new_only.push(*arg);
        }
    }
    assert!(old_set.is_empty(), "gc lost arguments: {old_set:?}");
    assert_eq!(
        new_only.len(),
        1,
        "only checkout may be a new gc arg: {new_only:?}"
    );
    assert!(
        new_only[0].contains("arg checkout "),
        "new gc arg must be checkout: {}",
        new_only[0]
    );
}

fn split_gc(tree: &str) -> (String, String) {
    let lines: Vec<&str> = tree.lines().collect();
    let mut gc_start = None;
    let mut gc_end = lines.len();
    for (n, line) in lines.iter().enumerate() {
        if line.starts_with("  cmd gc ") {
            gc_start = Some(n);
        } else if gc_start.is_some() && line.starts_with("  cmd ") {
            gc_end = n;
            break;
        }
    }
    let start = gc_start.expect("gc command present");
    let gc: String = lines[start..gc_end].join("\n");
    let mut rest: Vec<String> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if n >= start && n < gc_end {
            continue;
        }
        if n < start && (line.contains("Delete superseded") || line.contains("Pass --checkout")) {
            continue;
        }
        rest.push((*line).to_string());
    }
    (gc, rest.join("\n"))
}
