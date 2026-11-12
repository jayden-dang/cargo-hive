//! Name check for the hive skill: every single-word backtick span outside a
//! fence must name something the binary defines (a flag, subcommand, bench
//! shape or series, plan outcome or class, receipt key, skill metadata key, or
//! a string literal in `src/`), or be a recognised shape, file name or other
//! tool. Nothing here runs hive or writes outside `target/`.

use cargo_hive::bench::command::BuildCmd;
use cargo_hive::cli::Cli;
use cargo_hive::plan_tier::build::class_label;
use cargo_hive::plan_tier::record::PlanClass;
use cargo_hive::receipt::{Outcome, PlanTierBody, Receipt, Scope};
use clap::CommandFactory;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 2000;
const BENCH_SERIES: [&str; 4] = [
    "cold_build_ms",
    "seeded_build_ms",
    "seeded_seed_ms",
    "seeded_total_ms",
];
const BENCH_SUFFIXES: [&str; 5] = ["_median", "_p90", "_min", "_max", "_spread"];
const FILE_SUFFIXES: [&str; 7] = [".md", ".rs", ".toml", ".lock", ".json", ".jsonl", ".svg"];
const OTHER_TOOLS: [&str; 6] = ["cargo", "rustc", "git", "rustup", "hive", "cargo-hive"];

struct Context {
    literals: Vec<String>,
    bench_literals: Vec<String>,
    flags: HashSet<String>,
    subcommands: HashSet<String>,
    envelope_keys: HashSet<String>,
    metadata_keys: Vec<String>,
    config_keys: HashSet<String>,
}

/// Backtick spans outside fences, as 1-based line and span text.
pub fn inline_spans(text: &str) -> Result<Vec<(usize, String)>, String> {
    let mut spans = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if line.matches('`').count() % 2 == 1 {
            return Err(format!("line {}: odd number of backticks", i + 1));
        }
        spans.extend(
            line.split('`')
                .skip(1)
                .step_by(2)
                .map(|s| (i + 1, s.to_string())),
        );
    }
    Ok(spans)
}

fn quoted(c: &[char], open: usize) -> (String, usize) {
    let mut j = open + 1;
    while j < c.len() && c[j] != '"' {
        j += if c[j] == '\\' { 2 } else { 1 };
    }
    let j = j.min(c.len());
    (c[open + 1..j].iter().collect(), j + 1)
}

fn raw(c: &[char], at: usize) -> Option<(String, usize)> {
    let hashes = c[at..].iter().take_while(|&&ch| ch == '#').count();
    if c.get(at + hashes) != Some(&'"') {
        return None;
    }
    let start = at + hashes + 1;
    let close: Vec<char> = std::iter::once('"')
        .chain(std::iter::repeat_n('#', hashes))
        .collect();
    let len = (start..c.len()).find(|&j| c[j..].starts_with(&close))?;
    Some((c[start..len].iter().collect(), len + close.len()))
}

fn char_end(c: &[char], at: usize) -> usize {
    if c.get(at + 1) == Some(&'\\') {
        let mut j = at + 3;
        while j < c.len() && c[j] != '\'' {
            j += 1;
        }
        j + 1
    } else if c.get(at + 2) == Some(&'\'') {
        at + 3
    } else {
        at + 1
    }
}

fn attribute(c: &[char], at: usize) -> Option<(String, usize)> {
    let mut j = at + 1;
    if c.get(j) == Some(&'!') {
        j += 1;
    }
    if c.get(j) != Some(&'[') {
        return None;
    }
    let (mut depth, mut text) = (0usize, String::new());
    while j < c.len() {
        match c[j] {
            '"' => j = quoted(c, j).1,
            '\'' => j = char_end(c, j),
            '[' | ']' => {
                depth = if c[j] == '[' { depth + 1 } else { depth - 1 };
                j += 1;
                if depth == 0 {
                    break;
                }
            }
            ch => {
                text.push(ch);
                j += 1;
            }
        }
    }
    Some((text, j))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Raw text of each string literal outside attributes and test modules.
pub fn string_literals(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut depth) = (0, 0usize);
    let (mut skipping, mut test_attr, mut want_body) = (false, false, false);
    while i < c.len() {
        match c[i] {
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let (text, end) = quoted(&c, i);
                if !skipping {
                    out.push(text);
                }
                i = end;
            }
            '\'' => i = char_end(&c, i),
            '#' => match attribute(&c, i) {
                Some((text, end)) => {
                    test_attr |= text.split(|ch| !is_word_char(ch)).any(|w| w == "test");
                    i = end;
                }
                None => i += 1,
            },
            '{' => {
                if want_body {
                    (skipping, depth, want_body) = (true, 0, false);
                }
                depth += usize::from(skipping);
                i += 1;
            }
            '}' => {
                if skipping {
                    depth -= 1;
                    skipping = depth > 0;
                }
                i += 1;
            }
            ';' => {
                want_body = false;
                i += 1;
            }
            ch if is_word_char(ch) => {
                let start = i;
                while i < c.len() && is_word_char(c[i]) {
                    i += 1;
                }
                let word: String = c[start..i].iter().collect();
                if matches!(word.as_str(), "r" | "br") && matches!(c.get(i), Some('"' | '#')) {
                    if let Some((text, end)) = raw(&c, i) {
                        if !skipping {
                            out.push(text);
                        }
                        i = end;
                        continue;
                    }
                }
                want_body |= test_attr && word == "mod";
                test_attr &= word == "pub";
            }
            _ => i += 1,
        }
    }
    out
}

fn rust_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    assert!(depth <= MAX_DEPTH, "{} is nested too deep", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap())
        .collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            rust_files(&path, depth + 1, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
    assert!(out.len() <= MAX_FILES, "more than {MAX_FILES} source files");
}

fn collect_commands(cmd: &clap::Command, ctx: &mut Context) {
    for arg in cmd.get_arguments() {
        ctx.flags.extend(arg.get_long().map(str::to_string));
    }
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        ctx.subcommands.insert(sub.get_name().to_string());
        collect_commands(sub, ctx);
    }
}

fn plan_classes() -> Vec<&'static str> {
    // The match fails to compile when a class is added, so this list cannot drift.
    let all = [
        PlanClass::Secondary,
        PlanClass::Protected,
        PlanClass::Orphan,
    ];
    for class in all {
        match class {
            PlanClass::Secondary | PlanClass::Protected | PlanClass::Orphan => {}
        }
    }
    all.into_iter().map(class_label).collect()
}

fn readme_config_keys() -> HashSet<String> {
    let readme = super::repo_file("README.md");
    let section = readme.split("\n## Configuration").nth(1).unwrap_or("");
    let section = section.split("\n## ").next().unwrap_or("");
    section
        .lines()
        .filter(|l| l.starts_with('|'))
        .filter_map(|l| l.split('|').nth(1))
        .map(|cell| cell.trim().trim_matches('`').to_string())
        .collect()
}

fn context() -> &'static Context {
    static CONTEXT: OnceLock<Context> = OnceLock::new();
    CONTEXT.get_or_init(|| {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, 0, &mut files);
        let mut ctx = Context {
            literals: Vec::new(),
            bench_literals: Vec::new(),
            flags: HashSet::new(),
            subcommands: HashSet::new(),
            envelope_keys: HashSet::new(),
            metadata_keys: Vec::new(),
            config_keys: readme_config_keys(),
        };
        for file in files {
            let lits = string_literals(&std::fs::read_to_string(&file).unwrap());
            if file == src.join("bench/mod.rs") {
                ctx.bench_literals.extend(lits.iter().cloned());
            }
            ctx.literals.extend(lits);
        }
        let mut cli = Cli::command();
        cli.build();
        collect_commands(&cli, &mut ctx);
        let mut builder = Receipt::with_os("x", Scope::default(), None);
        builder.plan_tier(PlanTierBody {
            plans: vec![],
            undecided: vec![],
            nothing_to_do: vec![],
        });
        let envelope = serde_json::to_value(builder.finish()).unwrap();
        ctx.envelope_keys = envelope.as_object().unwrap().keys().cloned().collect();
        let (front, _) = super::split_skill(&super::repo_file(super::SKILL)).unwrap();
        ctx.metadata_keys = front.metadata.into_iter().map(|(k, _)| k).collect();
        ctx
    })
}

fn is_shape(span: &str) -> bool {
    span.contains(['/', '~', '$', '*'])
        || span.starts_with('<')
        || span.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
}

fn strip_placeholder(lit: &str) -> &str {
    match lit.strip_suffix('}').and_then(|l| l.rsplit_once('{')) {
        Some((head, _)) => head.strip_suffix(':').unwrap_or(head),
        None => lit,
    }
}

/// `None` when `span` is not a bench series name; otherwise whether `bench_literals` still define it.
pub fn bench_series(bench_literals: &[String], span: &str) -> Option<Result<(), String>> {
    let series = BENCH_SERIES.iter().find(|s| span.starts_with(**s))?;
    let suffix = BENCH_SUFFIXES
        .iter()
        .find(|x| span[series.len()..] == ***x)?;
    let has = |lit: &str| bench_literals.iter().any(|l| l == lit);
    let template = format!("{{series}}{suffix}");
    Some(if has(series) && has(&template) {
        Ok(())
    } else {
        Err(format!(
            "is a bench series that src/bench/mod.rs no longer defines (`{series}` or `{template}`)"
        ))
    })
}

fn classify(ctx: &Context, span: &str) -> Result<(), String> {
    let bench_shape = match span.split_once("=<") {
        Some((head, tail)) if tail.ends_with('>') => format!("{head}=x"),
        _ => span.to_string(),
    };
    if bench_shape.parse::<BuildCmd>().is_ok() {
        return Ok(());
    }
    if let Some(flag) = span.strip_prefix("--") {
        return if ctx.flags.contains(flag) {
            Ok(())
        } else {
            Err("is not a flag of any hive command".into())
        };
    }
    if span.starts_with('-') {
        return Err("is a short flag; write the long form".into());
    }
    let plan_outcome = serde_json::from_value::<Outcome>(span.into()).is_ok();
    if ctx.subcommands.contains(span)
        || plan_outcome
        || plan_classes().contains(&span)
        || ctx.envelope_keys.contains(span)
    {
        return Ok(());
    }
    if let Some(key) = span.strip_prefix("metadata.") {
        return if ctx.metadata_keys.iter().any(|k| k == key) {
            Ok(())
        } else {
            Err("is not a metadata key of this skill".into())
        };
    }
    if ctx.config_keys.contains(span) {
        return Ok(());
    }
    if let Some(verdict) = bench_series(&ctx.bench_literals, span) {
        return verdict;
    }
    if is_shape(span) {
        return Ok(());
    }
    if span.contains(['_', '.'])
        && ctx
            .literals
            .iter()
            .any(|l| l == span || strip_placeholder(l) == span)
    {
        return Ok(());
    }
    if FILE_SUFFIXES.iter().any(|x| span.ends_with(x)) || OTHER_TOOLS.contains(&span) {
        return Ok(());
    }
    Err("is not a hive name the binary defines; write it as prose or add a skip rule".into())
}

/// One `"<origin>:<line>: `<span>` <reason>"` per span the binary does not define.
pub fn cited_name_failures(text: &str, origin: &str) -> Vec<String> {
    let spans = match inline_spans(text) {
        Ok(spans) => spans,
        Err(e) => return vec![format!("{origin}: {e}")],
    };
    let ctx = context();
    spans
        .into_iter()
        .filter(|(_, span)| !span.is_empty() && !span.contains(char::is_whitespace))
        .filter_map(|(line, span)| {
            classify(ctx, &span)
                .err()
                .map(|reason| format!("{origin}:{line}: `{span}` {reason}"))
        })
        .collect()
}

const COMMAND_PREFIXES: [&str; 2] = ["cargo hive ", "cargo-hive "];

fn is_placeholder(word: &str) -> bool {
    word.len() > 2 && word.starts_with('<') && word.ends_with('>')
}

/// Words of a README command line, or `None` when the text does not open with
/// the command prefix. Cut at a pipe, `&&`, `#` or a redirect.
fn readme_words(text: &str) -> Option<Vec<String>> {
    if text.trim_end().ends_with('\\') {
        return None;
    }
    let rest = COMMAND_PREFIXES.iter().find_map(|p| text.strip_prefix(p))?;
    Some(
        rest.split_whitespace()
            .take_while(|w| !(w.starts_with('|') || w.starts_with("&&") || w.starts_with('#')))
            .take_while(|w| !w.contains('>') || is_placeholder(w))
            .map(str::to_string)
            .collect(),
    )
}

/// Every README command line as 1-based line and words, from fenced lines and
/// backtick spans only. A line that opens with the prefix is the command; prose
/// mentions elsewhere are ignored. A `<word>` placeholder becomes `x`. A synopsis
/// line (its subcommand word is a placeholder) is left out.
pub fn readme_command_lines(text: &str) -> Vec<(usize, Vec<String>)> {
    command_texts(text)
        .into_iter()
        .filter_map(|(line, text)| readme_words(&text).map(|words| (line, words)))
        .filter(|(_, words)| !words.first().is_some_and(|w| is_placeholder(w)))
        .map(|(line, words)| {
            let words = words
                .into_iter()
                .map(|w| if is_placeholder(&w) { "x".into() } else { w })
                .collect();
            (line, words)
        })
        .collect()
}

fn command_texts(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            out.push((i + 1, line.to_string()));
        } else {
            let spans = line.split('`').skip(1).step_by(2);
            out.extend(spans.map(|s| (i + 1, s.to_string())));
        }
    }
    out
}

/// One failure per flag on a synopsis line (`cargo hive <command> --help`) that
/// is not a long flag of some hive command.
pub fn synopsis_flag_failures(text: &str, origin: &str) -> Vec<String> {
    let ctx = context();
    let mut out = Vec::new();
    for (line, text) in command_texts(text) {
        let Some(words) = readme_words(&text) else {
            continue;
        };
        if !words.first().is_some_and(|w| is_placeholder(w)) {
            continue;
        }
        for flag in words.iter().filter(|w| w.starts_with('-')) {
            let known = flag
                .strip_prefix("--")
                .is_some_and(|f| ctx.flags.contains(f));
            if !known {
                out.push(format!(
                    "{origin}:{line}: `{flag}` in a synopsis is not a long flag of any hive command"
                ));
            }
        }
    }
    out
}
