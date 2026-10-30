use std::fs;

fn repo_file(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

struct Frontmatter {
    keys: Vec<(String, String)>,
    metadata: Vec<(String, String)>,
}

fn split_skill(text: &str) -> Result<(Frontmatter, String), String> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Err("unsupported line 1: the file must open with ---".to_string());
    }
    let mut keys = Vec::new();
    let mut metadata = Vec::new();
    let mut in_metadata = false;
    let mut seen_metadata = false;
    let mut closed = false;
    for (i, line) in lines.by_ref().enumerate() {
        let n = i + 2;
        if line == "---" {
            closed = true;
            break;
        }
        if let Some(rest) = line.strip_prefix("  ") {
            match rest.split_once(": ") {
                Some((k, v))
                    if in_metadata
                        && !k.is_empty()
                        && !k.starts_with(' ')
                        && v.len() >= 2
                        && v.starts_with('"')
                        && v.ends_with('"') =>
                {
                    metadata.push((k.to_string(), v[1..v.len() - 1].to_string()));
                }
                _ => return Err(format!("unsupported line {n}: {line:?}")),
            }
            continue;
        }
        if line == "metadata:" {
            if seen_metadata {
                return Err(format!("unsupported line {n}: second metadata block"));
            }
            seen_metadata = true;
            in_metadata = true;
            continue;
        }
        match line.split_once(": ") {
            Some((k, v)) if !k.is_empty() && !k.contains(' ') && !v.is_empty() => {
                if keys.iter().any(|(existing, _)| existing == k) {
                    return Err(format!("unsupported line {n}: repeated key {k:?}"));
                }
                in_metadata = false;
                keys.push((k.to_string(), v.to_string()));
            }
            _ => return Err(format!("unsupported line {n}: {line:?}")),
        }
    }
    if !closed {
        return Err("unsupported frontmatter: no closing ---".to_string());
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    Ok((Frontmatter { keys, metadata }, body))
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn check_against_spec(dir: &str, text: &str) -> Result<(), String> {
    let (fm, _) = split_skill(text)?;
    for (k, _) in &fm.keys {
        if !["name", "description", "license", "compatibility"].contains(&k.as_str()) {
            return Err(format!("frontmatter key {k:?} is not in the allowed keys"));
        }
    }
    let name = get(&fm.keys, "name").ok_or("name is missing")?;
    if name != dir {
        return Err(format!(
            "name {name:?} must equal the directory name {dir:?}"
        ));
    }
    if name.len() > 64 {
        return Err(format!("name {name:?} must be at most 64 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("name {name:?} must use only [a-z0-9] and hyphens"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!("name {name:?} must not start or end with a hyphen"));
    }
    if name.contains("--") {
        return Err(format!("name {name:?} must use single hyphens"));
    }
    let description = get(&fm.keys, "description").ok_or("description is missing")?;
    if description.chars().count() > 1024 {
        return Err("description is longer than 1024 characters".to_string());
    }
    for needle in ["target", "cargo clean"] {
        if !description.contains(needle) {
            return Err(format!("description must contain {needle:?}"));
        }
    }
    if let Some(c) = get(&fm.keys, "compatibility") {
        if c.chars().count() > 500 {
            return Err("compatibility is longer than 500 characters".to_string());
        }
    }
    Ok(())
}

#[test]
fn skill_frontmatter_follows_the_spec() {
    let text = repo_file("skills/hive/SKILL.md");
    check_against_spec("hive", &text).unwrap();
    let (_, body) = split_skill(&text).unwrap();
    assert!(!body.trim().is_empty(), "the skill body is empty");

    let ok_desc = "description: Reclaim a large target dir instead of cargo clean";
    let long = "x".repeat(1025);
    let long_name = "a".repeat(65);
    let long_compat = "y".repeat(501);
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "hive",
            format!("---\nname: other\n{ok_desc}\n---\nbody"),
            "directory name",
        ),
        (
            "hive-",
            format!("---\nname: hive-\n{ok_desc}\n---\nbody"),
            "start or end with a hyphen",
        ),
        (
            "-hive",
            format!("---\nname: -hive\n{ok_desc}\n---\nbody"),
            "start or end with a hyphen",
        ),
        (
            "hi_ve",
            format!("---\nname: hi_ve\n{ok_desc}\n---\nbody"),
            "[a-z0-9]",
        ),
        (
            "Hive",
            format!("---\nname: Hive\n{ok_desc}\n---\nbody"),
            "[a-z0-9]",
        ),
        (
            long_name.as_str(),
            format!("---\nname: {long_name}\n{ok_desc}\n---\nbody"),
            "at most 64 characters",
        ),
        (
            "hive",
            format!("---\nmetadata:\n  a: \"1\"\nname: hive\nmetadata:\n  b: \"2\"\n{ok_desc}\n---\nbody"),
            "second metadata",
        ),
        (
            "hive",
            format!("---\nname: hive\nname: hive\n{ok_desc}\n---\nbody"),
            "repeated key",
        ),
        (
            "hi--ve",
            format!("---\nname: hi--ve\n{ok_desc}\n---\nbody"),
            "single hyphens",
        ),
        (
            "hive",
            format!("---\nname: hive\ndescription: target cargo clean {long}\n---\nbody"),
            "1024",
        ),
        (
            "hive",
            "---\nname: hive\ndescription: cargo clean only\n---\nbody".to_string(),
            "\"target\"",
        ),
        (
            "hive",
            "---\nname: hive\ndescription: target only\n---\nbody".to_string(),
            "\"cargo clean\"",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\ncompatibility: {long_compat}\n---\nbody"),
            "500",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\nallowed-tools: Bash\n---\nbody"),
            "allowed keys",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\n- a list item\n---\nbody"),
            "unsupported line 4",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\nmetadata:\n  hive-version: 0.1.0\n---\nbody"),
            "unsupported line 5",
        ),
        (
            "hive",
            format!("name: hive\n{ok_desc}\n"),
            "unsupported line 1",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\n"),
            "no closing ---",
        ),
    ];
    for (dir, crafted, rule) in &cases {
        let err = check_against_spec(dir, crafted)
            .expect_err(&format!("accepted a frontmatter that breaks {rule}"));
        assert!(err.contains(rule), "error {err:?} does not name {rule:?}");
    }
}

fn manifest(rel: &str) -> serde_json::Value {
    serde_json::from_str(&repo_file(rel)).unwrap_or_else(|e| panic!("{rel} is not JSON: {e}"))
}

#[test]
fn manifest_versions_match_the_crate() {
    let crate_version = env!("CARGO_PKG_VERSION");
    let (fm, _) = split_skill(&repo_file("skills/hive/SKILL.md")).unwrap();
    assert_eq!(
        get(&fm.metadata, "hive-version"),
        Some(crate_version),
        "skills/hive/SKILL.md metadata.hive-version must equal the crate version"
    );

    for rel in [".claude-plugin/plugin.json", ".codex-plugin/plugin.json"] {
        let plugin = manifest(rel);
        assert_eq!(plugin["name"], "cargo-hive", "{rel} field name");
        assert_eq!(
            plugin["version"], crate_version,
            "{rel} field version must equal the crate version"
        );
    }
    assert_eq!(
        manifest(".claude-plugin/plugin.json")["skills"],
        serde_json::json!(["./skills/hive"]),
        ".claude-plugin/plugin.json field skills"
    );
    assert_eq!(
        manifest(".codex-plugin/plugin.json")["skills"],
        "./skills/",
        ".codex-plugin/plugin.json field skills"
    );

    let market = manifest(".claude-plugin/marketplace.json");
    assert_eq!(
        market["name"], "cargo-hive",
        ".claude-plugin/marketplace.json field name"
    );
    let plugins = market["plugins"]
        .as_array()
        .unwrap_or_else(|| panic!(".claude-plugin/marketplace.json field plugins is not a list"));
    assert_eq!(
        plugins.len(),
        1,
        ".claude-plugin/marketplace.json field plugins must hold exactly one plugin"
    );
    let entry = &plugins[0];
    assert_eq!(
        entry["name"], "cargo-hive",
        ".claude-plugin/marketplace.json field plugins[0].name"
    );
    assert_eq!(
        entry["source"], "./",
        ".claude-plugin/marketplace.json field plugins[0].source"
    );
    assert!(
        entry.get("version").is_none(),
        ".claude-plugin/marketplace.json field plugins[0].version must be absent"
    );
    assert!(
        entry.get("skills").is_none(),
        ".claude-plugin/marketplace.json field plugins[0].skills must be absent"
    );
}

struct CommandLine {
    line_no: usize,
    text: String,
    words: Vec<String>,
}

const PREFIXES: [&str; 2] = ["cargo hive ", "cargo-hive "];

fn mentions(text: &str) -> usize {
    PREFIXES.iter().map(|p| text.matches(p).count()).sum()
}

fn command_words(text: &str) -> Option<Vec<String>> {
    if text.trim_end().ends_with('\\') {
        return None;
    }
    let rest = PREFIXES.iter().find_map(|p| text.strip_prefix(p))?;
    Some(
        rest.split_whitespace()
            .take_while(|w| !(w.starts_with('|') || w.starts_with("&&") || w.starts_with('#')))
            .take_while(|w| !w.contains('>'))
            .map(str::to_string)
            .collect(),
    )
}

fn scan(body: &str) -> (Vec<CommandLine>, Vec<(usize, String)>) {
    let mut commands = Vec::new();
    let mut strays = Vec::new();
    let mut fenced = false;
    for (i, line) in body.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let texts: Vec<&str> = if fenced {
            vec![line]
        } else {
            line.split('`').skip(1).step_by(2).collect()
        };
        let mut found = 0;
        for text in texts {
            if let Some(words) = command_words(text) {
                found += 1;
                commands.push(CommandLine {
                    line_no: i + 1,
                    text: text.to_string(),
                    words,
                });
            }
        }
        if mentions(line) > found {
            strays.push((i + 1, line.to_string()));
        }
    }
    (commands, strays)
}

fn command_lines(body: &str) -> Vec<CommandLine> {
    scan(body).0
}

fn stray_mentions(body: &str) -> Vec<(usize, String)> {
    scan(body).1
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Help,
    Doctor,
    RepoAdd,
    RepoList,
    Status,
    Gc { dry_run: bool },
    Explain,
}

fn parse(words: &[String]) -> Result<Parsed, String> {
    use cargo_hive::cli::{Cli, Command, RepoCommand};
    use clap::error::ErrorKind;
    use clap::Parser;

    let argv = std::iter::once("cargo-hive".to_string()).chain(words.iter().cloned());
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            return Ok(Parsed::Help)
        }
        Err(e) => return Err(e.to_string()),
    };
    match cli.command {
        Command::Doctor { .. } => Ok(Parsed::Doctor),
        Command::Repo { command } => match command {
            RepoCommand::Add { .. } => Ok(Parsed::RepoAdd),
            RepoCommand::List(_) => Ok(Parsed::RepoList),
        },
        Command::Status { .. } => Ok(Parsed::Status),
        Command::Gc { dry_run, .. } => Ok(Parsed::Gc { dry_run }),
        Command::Explain { .. } => Ok(Parsed::Explain),
        Command::HoldLock { .. } | Command::TryLock { .. } => {
            Err("the internal lock subcommands are not part of the skill".to_string())
        }
    }
}

fn skill_body() -> String {
    split_skill(&repo_file("skills/hive/SKILL.md")).unwrap().1
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

#[test]
fn skill_command_lines_parse_with_the_cli() {
    let fenced = command_lines("```\ncargo hive status /p --json\n```");
    assert_eq!(fenced.len(), 1);
    assert_eq!(fenced[0].line_no, 2);
    assert_eq!(fenced[0].words, words("status /p --json"));

    let inline = command_lines("Run `cargo-hive doctor --json` first.");
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0].text, "cargo-hive doctor --json");
    assert_eq!(inline[0].words, words("doctor --json"));

    let bare = "Use `cargo hive` here.";
    assert!(command_lines(bare).is_empty());
    assert!(stray_mentions(bare).is_empty());

    let cut = command_lines("```\ncargo hive gc /p --json 2>&1 | jq\n```");
    assert_eq!(cut[0].words, words("gc /p --json"));
    assert!(stray_mentions("```\ncargo hive gc /p --json 2>&1 | jq\n```").is_empty());

    for escaped in [
        "```\n$ cargo hive status\n```",
        "```\nHIVE_HOME=/x cargo hive status\n```",
        "```\n  cargo hive status\n```",
        "```\ncargo hive status \\\n  --json\n```",
        "Plain prose: cargo hive status.",
    ] {
        assert_eq!(
            stray_mentions(escaped).len(),
            1,
            "mention escaped the extractor: {escaped:?}"
        );
        assert!(command_lines(escaped).is_empty(), "{escaped:?}");
    }

    let hold = command_lines("`cargo hive __hold-lock a b`");
    assert!(parse(&hold[0].words).is_err());
    assert_eq!(parse(&words("--help")), Ok(Parsed::Help));
    assert_eq!(parse(&words("--version")), Ok(Parsed::Help));
    assert!(parse(&words("status --no-such-flag")).is_err());

    let body = skill_body();
    for line in command_lines(&body) {
        if let Err(e) = parse(&line.words) {
            panic!(
                "skill body line {} does not parse: {}\n{e}",
                line.line_no, line.text
            );
        }
    }
    let strays = stray_mentions(&body);
    assert!(
        strays.is_empty(),
        "skill body mentions the CLI outside a command line: {strays:?}"
    );
}

#[test]
fn skill_documents_every_mile1_command() {
    let body = skill_body();
    let mut seen: Vec<Parsed> = Vec::new();
    for line in command_lines(&body) {
        let parsed = parse(&line.words).unwrap_or_else(|e| {
            panic!(
                "skill body line {} does not parse: {}\n{e}",
                line.line_no, line.text
            )
        });
        if parsed == Parsed::Help {
            continue;
        }
        assert!(
            line.words.iter().any(|w| w == "--json"),
            "skill body line {} lacks --json: {}",
            line.line_no,
            line.text
        );
        let skip = if parsed == Parsed::RepoAdd || parsed == Parsed::RepoList {
            2
        } else {
            1
        };
        let mut path_words = 0;
        let mut rest = line.words.iter().skip(skip);
        while let Some(w) = rest.next() {
            if w == "--top" || w == "-n" {
                rest.next();
            } else if !w.starts_with('-') {
                path_words += 1;
            }
        }
        if matches!(
            parsed,
            Parsed::RepoAdd | Parsed::Status | Parsed::Gc { .. } | Parsed::Explain
        ) {
            assert!(
                path_words >= 1,
                "skill body line {} carries no path: {}",
                line.line_no,
                line.text
            );
        }
        seen.push(parsed);
    }
    for want in [
        Parsed::Doctor,
        Parsed::RepoAdd,
        Parsed::RepoList,
        Parsed::Status,
        Parsed::Gc { dry_run: true },
        Parsed::Gc { dry_run: false },
        Parsed::Explain,
    ] {
        assert!(
            seen.contains(&want),
            "the skill documents no command line for {want:?}"
        );
    }
}
