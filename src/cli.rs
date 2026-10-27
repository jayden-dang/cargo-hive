//! Thin adapter: arguments → library calls → one receipt.

use crate::platform::{self, MACOS_ONLY_MESSAGE};
use crate::receipt::{Receipt, Scope};
use crate::registry::{self, Registry};
use crate::state::State;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;

#[derive(Parser)]
#[command(
    name = "cargo-hive",
    bin_name = "cargo hive",
    version,
    about = "Cargo build artifacts across worktrees, with measured receipts"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Manage registered repositories
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Show checkouts (and, from Task 13, Cargo target dirs) of registered repos
    Status {
        /// Registered repos to report; none means every registered repo
        repos: Vec<std::path::PathBuf>,
        #[command(flatten)]
        out: OutputArgs,
    },
}

#[derive(Subcommand)]
pub enum RepoCommand {
    /// Register a repository by its main checkout (any path inside it)
    Add {
        path: std::path::PathBuf,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// List registered repositories
    List(OutputArgs),
}

#[derive(Args, Clone, Copy, Debug)]
pub struct OutputArgs {
    /// Print exactly one JSON receipt on stdout (prose goes to stderr)
    #[arg(long)]
    pub json: bool,
}

/// `cargo hive …` runs `cargo-hive hive …`; drop that inserted word.
fn strip_cargo_subcommand(mut args: Vec<OsString>) -> Vec<OsString> {
    if args.get(1).is_some_and(|a| a == "hive") {
        args.remove(1);
    }
    args
}

pub fn main_with(args: Vec<OsString>) -> i32 {
    if platform::ensure_supported().is_err() {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    }
    let cli = match Cli::try_parse_from(strip_cargo_subcommand(args)) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let state = match State::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    match cli.command {
        Command::Repo {
            command: RepoCommand::Add { path, out },
        } => repo_add(&state, &path, out),
        Command::Repo {
            command: RepoCommand::List(out),
        } => repo_list(&state, out),
        Command::Status { repos, out } => status(&state, &repos, out),
    }
}

/// Logs the receipt, prints it (`--json`) or the human text, and returns `code`.
pub fn emit(state: &State, out: OutputArgs, receipt: Receipt, human: &str, code: i32) -> i32 {
    let line = serde_json::to_string(&receipt).expect("receipts always serialize");
    if let Err(e) = state.locked(|tx| tx.append_receipt_line(&line).map(|_| ())) {
        eprintln!("cargo-hive: could not log the receipt: {e}");
        return 1;
    }
    if out.json {
        println!("{line}");
        eprint!("{human}");
    } else {
        print!("{human}");
    }
    code
}

fn repo_add(state: &State, path: &std::path::Path, out: OutputArgs) -> i32 {
    use crate::registry::{register, RegisterOutcome};
    let result = state.locked(|tx| register(path, tx));
    let mut b = Receipt::new("repo add", Scope::default());
    match result {
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            1
        }
        Ok(Ok(RegisterOutcome::Registered(repo))) => {
            b.count("registered", 1);
            b.scope_mut().repos.push(repo.main.clone());
            emit(
                state,
                out,
                b.finish(),
                &format!("registered {}\n", repo.main.display()),
                0,
            )
        }
        Ok(Ok(RegisterOutcome::AlreadyRegistered(repo))) => {
            b.count("registered", 0);
            b.scope_mut().repos.push(repo.main.clone());
            b.note(format!("{} is already registered", repo.main.display()));
            emit(
                state,
                out,
                b.finish(),
                &format!("{} is already registered\n", repo.main.display()),
                0,
            )
        }
        Ok(Err(refusal)) => {
            b.count("registered", 0);
            b.finding("refusal", Some(path.to_path_buf()), refusal.to_string());
            emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1)
        }
    }
}

fn repo_list(state: &State, out: OutputArgs) -> i32 {
    let reg: Registry = match state.read(registry::FILE) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let mut b = Receipt::new(
        "repo list",
        Scope {
            repos: reg.repos.iter().map(|r| r.main.clone()).collect(),
            checkouts: Vec::new(),
        },
    );
    b.count("repos", reg.repos.len() as u64);
    let mut human = String::new();
    if reg.repos.is_empty() {
        human.push_str("no repositories registered\n");
    }
    for repo in &reg.repos {
        let roots: Vec<&str> = repo.root_commits.iter().map(String::as_str).collect();
        human.push_str(&format!(
            "{}  root {}\n",
            repo.main.display(),
            roots.join(",")
        ));
    }
    emit(state, out, b.finish(), &human, 0)
}

fn status(state: &State, args: &[std::path::PathBuf], out: OutputArgs) -> i32 {
    use crate::classify::Class;
    use crate::discover;
    let (reg, cfg) = match (state.read::<Registry>(registry::FILE), state.config()) {
        (Ok(r), Ok(c)) => (r, c),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let mut b = Receipt::new("status", Scope::default());
    b.note(format!(
        "secondary_roots: {}",
        cfg.secondary_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    b.note(format!(
        "cache_roots: {}",
        cfg.cache_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let mut code = 0;
    let mut selected = Vec::new();
    if args.is_empty() {
        selected = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => selected.push(r.clone()),
            None => {
                b.finding(
                    "refusal",
                    Some(arg.clone()),
                    "not a registered repository; run `cargo hive repo add` first",
                );
                code = 1;
            }
        }
    }
    let mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (mut total, mut protected, mut secondary, mut missing) = (0u64, 0u64, 0u64, 0u64);
    let mut human = String::new();
    for repo in &selected {
        b.scope_mut().repos.push(repo.main.clone());
        human.push_str(&format!("{}\n", repo.main.display()));
        for c in discover::checkouts(repo, &mains, &cfg) {
            total += 1;
            let label = if c.missing {
                missing += 1;
                "missing"
            } else if c.class == Class::Secondary {
                secondary += 1;
                "secondary"
            } else {
                protected += 1;
                "protected"
            };
            human.push_str(&format!(
                "  {label:<9} {}  ({})\n",
                c.path.display(),
                c.reason
            ));
            b.finding(
                &format!("checkout.{label}"),
                Some(c.path.clone()),
                c.reason.clone(),
            );
            b.scope_mut().checkouts.push(c.path);
        }
    }
    b.count("checkouts", total)
        .count("protected", protected)
        .count("secondary", secondary)
        .count("missing", missing);
    emit(state, out, b.finish(), &human, code)
}
