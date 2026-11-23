//! Thin adapter: arguments → library calls → one receipt.

use crate::bench::command::BuildCmd;
use cargo_hive_cargo::discover::{self, Checkout, Snapshot, TargetScan, WalkFault};
use cargo_hive_cargo::registry::{self, Registry};
use cargo_hive_core::receipt::{PlanRecord, PlanTierBody, Receipt, ReceiptBuilder, Scope};
use cargo_hive_gc::checkout::{self, CheckoutInput};
use cargo_hive_platform::{self as platform, MACOS_ONLY_MESSAGE};
use cargo_hive_state::{Config, State, StateError};
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    /// Show checkouts and Cargo target dirs of registered repos
    Status {
        /// Registered repos to report; none means every registered repo
        repos: Vec<std::path::PathBuf>,
        /// Skip size accounting (sizes are then reported as not measured)
        #[arg(long)]
        no_sizes: bool,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Check which hive capabilities work on this machine
    Doctor {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Delete superseded incremental sessions and leftover rustc temp dirs. A session with a lock file is deleted under that lock. With no repo argument, also evicts from the store. Pass --checkout PATH to delete that one checkout's dead units in its own targets.
    Gc {
        /// Registered repos to clean; none means every target hive knows
        repos: Vec<std::path::PathBuf>,
        /// Report what would be deleted and freed, deleting nothing
        #[arg(long)]
        dry_run: bool,
        /// Store a plan of dead-unit paths and delete nothing
        #[arg(long, conflicts_with = "dry_run")]
        plan: bool,
        /// Checkout whose own targets are cleaned. Takes no repo argument, --plan, or --dry-run
        #[arg(long, value_name = "PATH", conflicts_with_all = ["repos", "plan", "dry_run"])]
        checkout: Option<std::path::PathBuf>,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Delete the dead units recorded in one stored plan
    Apply {
        /// Id of the plan to apply
        id: String,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Fill the store from one registered repository and clone registry units into a checkout
    Seed {
        /// Checkout to clone into. The caller's directory selects nothing
        checkout: std::path::PathBuf,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Time cold and seeded first builds of fresh checkouts
    Bench {
        /// Fresh checkouts measured without seeding
        #[arg(long, num_args = 1.., required = true)]
        cold: Vec<std::path::PathBuf>,
        /// Fresh checkouts measured after seeding from the store
        #[arg(long, num_args = 1.., required = true)]
        seeded: Vec<std::path::PathBuf>,
        /// Compile-only cargo shape for the timed build
        #[arg(long, default_value = "build")]
        build_cmd: BuildCmd,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Report copies of one crate, feature splits, and units whose last compile failed
    Explain {
        /// Registered repos to explain; none means every registered repo
        repos: Vec<std::path::PathBuf>,
        /// How many copy groups and divergences to show; 0 shows all
        #[arg(short = 'n', long = "top", default_value_t = 10)]
        top: usize,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Manage a registered repository's warm worktree pool
    Slot {
        #[command(subcommand)]
        command: SlotCommand,
    },
    /// Hold a lock until killed (used by doctor's lock check)
    #[command(name = "__hold-lock", hide = true)]
    HoldLock {
        kind: String,
        path: std::path::PathBuf,
    },
    /// Try a lock once without waiting and print `acquired` or `refused` (doctor's lock check)
    #[command(name = "__try-lock", hide = true)]
    TryLock {
        kind: String,
        path: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
pub enum SlotCommand {
    /// Establish two unused paths for a registered repository
    Init {
        /// Canonical path to the registered main checkout
        #[arg(long, value_name = "MAIN")]
        repo: PathBuf,
        /// Durable, owner-controlled root within a configured secondary root
        #[arg(long, value_name = "ROOT")]
        root: PathBuf,
        /// Read-only reviewed build profile
        #[arg(long, value_name = "FILE")]
        profile: PathBuf,
        #[command(flatten)]
        out: SlotOutputArgs,
    },
    /// Show current pool capacity, holds and next steps
    Status {
        /// Canonical path to the registered main checkout
        #[arg(long, value_name = "MAIN")]
        repo: PathBuf,
        #[command(flatten)]
        out: SlotOutputArgs,
    },
    /// Recover a held slot (unavailable in this version)
    Recover(SlotOperation),
    #[command(hide = true)]
    Key {
        /// Private output descriptor for the credential
        #[arg(long, value_name = "N")]
        out_fd: u32,
        #[command(flatten)]
        out: SlotOutputArgs,
    },
    #[command(hide = true)]
    Reserve {
        #[command(flatten)]
        scope: SlotScopeArgs,
        /// Observed vacant generation
        #[arg(long, value_name = "GEN")]
        expect: u64,
        /// Full Git object ID requested for preparation
        #[arg(long, value_name = "OID")]
        commit: String,
        /// Public operation ID
        #[arg(long, value_name = "ID")]
        op: String,
        #[command(flatten)]
        out: SlotOutputArgs,
    },
    #[command(hide = true)]
    Begin {
        #[command(flatten)]
        request: SlotLockedOperation,
        /// Private output descriptor for the first preparation permit
        #[arg(long, value_name = "N")]
        control_fd: u32,
    },
    #[command(hide = true)]
    StepCheck {
        #[command(flatten)]
        scope: SlotScopeArgs,
        /// Expected slot generation
        #[arg(long, value_name = "GEN")]
        generation: u64,
        /// Current preparation attempt ID
        #[arg(long, value_name = "ID")]
        attempt: u64,
        /// Inherited slot lock descriptor
        #[arg(long, value_name = "N")]
        lock_fd: u32,
        #[command(flatten)]
        out: SlotOutputArgs,
    },
    #[command(hide = true)]
    Producer(SlotLockedOperation),
    #[command(hide = true)]
    Ready {
        #[command(flatten)]
        request: SlotLockedOperation,
        /// Current preparation attempt ID
        #[arg(long, value_name = "ID")]
        attempt: u64,
    },
    #[command(hide = true)]
    Return(SlotOperation),
    #[command(hide = true)]
    Outcome(SlotReplay),
    #[command(hide = true)]
    Ack(SlotReplay),
}

#[derive(Args)]
pub struct SlotScopeArgs {
    /// Canonical path to the registered main checkout
    #[arg(long, value_name = "MAIN")]
    pub repo: PathBuf,
    /// Current public pool ID
    #[arg(long, value_name = "POOL")]
    pub pool: String,
    /// Slot number within the pool
    #[arg(long, value_name = "N")]
    pub slot: u8,
}

#[derive(Args)]
pub struct SlotOperation {
    #[command(flatten)]
    pub scope: SlotScopeArgs,
    /// Expected slot generation
    #[arg(long, value_name = "GEN")]
    pub generation: u64,
    /// Public operation ID
    #[arg(long, value_name = "ID")]
    pub op: String,
    #[command(flatten)]
    pub out: SlotOutputArgs,
}

#[derive(Args)]
pub struct SlotLockedOperation {
    #[command(flatten)]
    pub operation: SlotOperation,
    /// Inherited slot lock descriptor
    #[arg(long, value_name = "N")]
    pub lock_fd: u32,
}

#[derive(Args)]
pub struct SlotReplay {
    #[command(flatten)]
    pub operation: SlotOperation,
    /// Recorded preparation attempt ID, when the operation used one
    #[arg(long, value_name = "ID")]
    pub attempt: Option<u64>,
}

#[derive(Args)]
pub struct SlotOutputArgs {
    /// Print exactly one JSON slot result on stdout
    #[arg(long)]
    pub json: bool,
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
    /// Unregister repositories (any path inside a main checkout, which may be gone)
    Remove {
        paths: Vec<std::path::PathBuf>,
        /// Also remove every repository whose main checkout no longer exists
        #[arg(long)]
        missing: bool,
        #[command(flatten)]
        out: OutputArgs,
    },
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
    if let Command::HoldLock { kind, path } = &cli.command {
        return hold_lock(kind, path);
    }
    if let Command::TryLock { kind, path } = &cli.command {
        return try_lock(kind, path);
    }
    if let Command::Slot { command } = cli.command {
        return crate::slot::dispatch(command);
    }
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
        Command::Repo {
            command:
                RepoCommand::Remove {
                    paths,
                    missing,
                    out,
                },
        } => repo_remove(&state, &paths, missing, out),
        Command::Status {
            repos,
            no_sizes,
            out,
        } => status(&state, &repos, no_sizes, out),
        Command::Doctor { out } => doctor_cmd(&state, out),
        Command::Gc {
            repos,
            dry_run,
            plan,
            checkout,
            out,
        } => {
            if let Some(checkout) = checkout {
                checkout_cmd(&state, &checkout, &out)
            } else if plan {
                plan_cmd(&state, &repos, out)
            } else {
                gc_cmd(&state, &repos, dry_run, out)
            }
        }
        Command::Apply { id, out } => apply_cmd(&state, &id, out),
        Command::Seed { checkout, out } => seed_cmd(&state, &checkout, out),
        Command::Bench {
            cold,
            seeded,
            build_cmd,
            out,
        } => bench_cmd(&state, &cold, &seeded, build_cmd, out),
        Command::Explain { repos, top, out } => explain_cmd(&state, &repos, top, out),
        Command::Slot { .. } => unreachable!("handled with slot-specific state checks"),
        Command::HoldLock { .. } => unreachable!("handled before the state root is opened"),
        Command::TryLock { .. } => unreachable!("handled before the state root is opened"),
    }
}

fn hold_lock(kind: &str, path: &std::path::Path) -> i32 {
    let Some(kind) = platform::LockKind::parse(kind) else {
        eprintln!("cargo-hive: unknown lock kind {kind:?} (expected flock, fcntl or fcntl-read)");
        return 2;
    };
    match platform::hold_lock_forever(kind, path) {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!(
                "cargo-hive: could not hold the lock on {}: {e}",
                path.display()
            );
            1
        }
    }
}

fn try_lock(kind: &str, path: &std::path::Path) -> i32 {
    let Some(kind) = platform::LockKind::parse(kind) else {
        eprintln!("cargo-hive: unknown lock kind {kind:?} (expected flock, fcntl or fcntl-read)");
        return 2;
    };
    match platform::try_lock_once(kind, path) {
        Ok(platform::LockState::Free) => {
            println!("acquired");
            0
        }
        Ok(platform::LockState::Held) => {
            println!("refused");
            0
        }
        Err(e) => {
            eprintln!(
                "cargo-hive: could not try the lock on {}: {e}",
                path.display()
            );
            1
        }
    }
}

/// Logs the receipt, prints it (`--json`) or the human text, and returns `code`.
pub fn emit(state: &State, out: OutputArgs, receipt: Receipt, human: &str, code: i32) -> i32 {
    let line = serde_json::to_string(&receipt).expect("receipts always serialize");
    let cancelled = || platform::signals::cancelled();
    let logged = match state
        .locked_or_cancelled(cancelled, |tx| tx.append_receipt_line(&line).map(|_| ()))
    {
        Ok(()) => true,
        Err(StateError::Cancelled) => {
            eprintln!(
                "cargo-hive: the receipt was not logged: {}",
                StateError::Cancelled
            );
            if out.json {
                println!("{line}");
            }
            return 130;
        }
        Err(e) => {
            eprintln!("cargo-hive: could not log the receipt: {e}");
            false
        }
    };
    if out.json {
        println!("{line}");
        if logged {
            eprint!("{human}");
        }
    } else if logged {
        print!("{human}");
    }
    if logged { code } else { 1 }
}

fn emit_failure(state: &State, out: OutputArgs, command: &str, error: &str, code: i32) -> i32 {
    eprintln!("cargo-hive: {error}");
    let mut receipt = Receipt::new(command, Scope::default());
    if command == "gc --plan" || command == "apply" {
        receipt.plan_tier(PlanTierBody {
            plans: Vec::new(),
            undecided: Vec::new(),
            nothing_to_do: Vec::new(),
        });
    }
    receipt.finding("error", None, error.to_string());
    emit(state, out, receipt.finish(), &format!("{error}\n"), code)
}

fn begin_plan_tier(b: &mut ReceiptBuilder) {
    b.plan_tier(PlanTierBody {
        plans: Vec::new(),
        undecided: Vec::new(),
        nothing_to_do: Vec::new(),
    });
}

fn repo_add(state: &State, path: &std::path::Path, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::registry::{RegisterOutcome, register};
    let result = state.locked(|tx| register(path, tx));
    let mut b = Receipt::new("repo add", Scope::default());
    match result {
        Err(e) => emit_failure(state, out, "repo add", &e.to_string(), 1),
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

fn repo_remove(state: &State, paths: &[std::path::PathBuf], missing: bool, out: OutputArgs) -> i32 {
    let result = state.locked(|tx| registry::unregister(paths, missing, tx));
    let mut b = Receipt::new("repo remove", Scope::default());
    match result {
        Err(e) => emit_failure(state, out, "repo remove", &e.to_string(), 1),
        Ok(Ok(removed)) => {
            b.count("removed", removed.len() as u64);
            let mut human = String::new();
            if removed.is_empty() {
                human.push_str("nothing to remove\n");
            }
            for repo in &removed {
                b.scope_mut().repos.push(repo.main.clone());
                human.push_str(&format!("removed {}\n", repo.main.display()));
            }
            emit(state, out, b.finish(), &human, 0)
        }
        Ok(Err(refusal)) => {
            b.count("removed", 0);
            let path = match &refusal {
                registry::UnregisterRefusal::NotRegistered(path) => Some(path.clone()),
                registry::UnregisterRefusal::NothingNamed => None,
            };
            b.finding("refusal", path, refusal.to_string());
            emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1)
        }
    }
}

fn repo_list(state: &State, out: OutputArgs) -> i32 {
    let reg: Registry = match state.read(registry::FILE) {
        Ok(r) => r,
        Err(e) => return emit_failure(state, out, "repo list", &e.to_string(), 1),
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

fn measured_text(
    m: &std::collections::BTreeMap<String, cargo_hive_core::receipt::Measured>,
) -> String {
    use cargo_hive_core::receipt::Measured;
    m.iter()
        .map(|(k, v)| match v {
            Measured::Int(n) => format!("{k}={n}"),
            Measured::Bool(b) => format!("{k}={b}"),
            Measured::Text(t) => format!("{k}={t:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn doctor_cmd(state: &State, out: OutputArgs) -> i32 {
    use cargo_hive_core::receipt::CheckRecord;
    use cargo_hive_doctor::{self as doctor, DoctorEnv, Verdict};
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let macos = platform::os_version();
    let env = DoctorEnv {
        tmp_parent: std::env::temp_dir(),
        macos: macos.clone(),
        toolchain: cargo_hive_cargo::toolchain::resolve_toolchain(state.root()),
        cancel: doctor::process_cancel,
    };
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "doctor",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let report = match doctor::run(&*platform.fs, &env) {
        Ok(r) => r,
        Err(doctor::DoctorError::Cancelled) => {
            return emit_failure(
                state,
                out,
                "doctor",
                &format!("doctor {}", doctor::DoctorError::Cancelled),
                130,
            );
        }
        Err(e) => return emit_failure(state, out, "doctor", &e.to_string(), 1),
    };
    let caps = doctor::capabilities_for(&report);
    match state.locked_or_cancelled(doctor::process_cancel, |tx| {
        tx.write(doctor::CAPABILITIES_FILE, &caps)
    }) {
        Ok(()) => {}
        Err(StateError::Cancelled) => {
            return emit_failure(
                state,
                out,
                "doctor",
                &format!("doctor {}", StateError::Cancelled),
                130,
            );
        }
        Err(e) => return emit_failure(state, out, "doctor", &e.to_string(), 1),
    }
    let mut b = Receipt::with_os("doctor", Scope::default(), macos);
    b.toolchain(report.toolchain_release.clone());
    if let Err(reason) = &env.toolchain {
        b.unmeasured("toolchain", reason.clone());
    }
    let mut human = String::new();
    let (mut pass, mut fail, mut unavailable) = (0u64, 0u64, 0u64);
    for c in &report.checks {
        match c.verdict {
            Verdict::Pass => pass += 1,
            Verdict::Fail => fail += 1,
            Verdict::Unavailable => unavailable += 1,
        }
        human.push_str(&format!(
            "({}) {:<11} {}\n",
            c.id.letter(),
            c.verdict.as_str(),
            measured_text(&c.measured)
        ));
        b.check(CheckRecord {
            id: c.id.letter().into(),
            verdict: c.verdict.as_str().into(),
            measured: c.measured.clone(),
        });
    }
    for (cap, checks) in &caps.disabled {
        let line = if checks.contains(&doctor::CheckId::F) {
            doctor::check_f_disabled_line().to_string()
        } else {
            let ids: Vec<String> = checks.iter().map(|c| format!("({})", c.letter())).collect();
            format!(
                "{} is disabled: doctor check {} did not pass",
                cap.describe(),
                ids.join(", ")
            )
        };
        human.push_str(&format!("{line}\n"));
        b.finding("capability.disabled", None, line);
    }
    if let Some(line) = doctor::floor_note(&report) {
        human.push_str(&format!("{line}\n"));
        b.note(line);
    }
    b.count("pass", pass)
        .count("fail", fail)
        .count("unavailable", unavailable)
        .count("stale_temp_dirs_removed", report.stale_removed);
    emit(state, out, b.finish(), &human, i32::from(fail > 0))
}

/// The registered repository that `path` lies inside, or whose linked worktree it is.
fn owning_repo<'a>(reg: &'a Registry, path: &std::path::Path) -> Option<&'a registry::Repo> {
    if let Some(inside) = reg
        .repos
        .iter()
        .filter(|r| path.starts_with(&r.main))
        .max_by_key(|r| r.main.components().count())
    {
        return Some(inside);
    }
    let common = cargo_hive_cargo::git::Git::at(path).common_dir().ok()?;
    let common = common.canonicalize().unwrap_or(common);
    let main = common.parent()?;
    reg.repos.iter().find(|r| r.main == main)
}

struct Selection {
    repos: Vec<registry::Repo>,
    all_repos: bool,
    refused: bool,
}

fn select(reg: &Registry, args: &[std::path::PathBuf], b: &mut ReceiptBuilder) -> Selection {
    let mut refused = false;
    let mut repos = Vec::new();
    if args.is_empty() {
        repos = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        let found = match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => Some(r),
            None => {
                let owner = owning_repo(reg, &real);
                if let Some(r) = owner {
                    b.note(format!(
                        "{} is a checkout of the registered repository {}, which this command covers",
                        real.display(),
                        r.main.display()
                    ));
                }
                owner
            }
        };
        match found {
            Some(r) => {
                if !repos.iter().any(|s| s.main == r.main) {
                    repos.push(r.clone());
                }
            }
            None => {
                b.finding(
                    "refusal",
                    Some(arg.clone()),
                    "not a registered repository; run `cargo hive repo add` first",
                );
                refused = true;
            }
        }
    }
    let all_repos = repos.len() == reg.repos.len()
        && repos
            .iter()
            .all(|s| reg.repos.iter().any(|r| r.main == s.main));
    Selection {
        repos,
        all_repos,
        refused,
    }
}

fn live_checkouts(
    repos: &[registry::Repo],
    registered_mains: &[std::path::PathBuf],
    cfg: &cargo_hive_state::Config,
    b: &mut ReceiptBuilder,
) -> (
    Vec<cargo_hive_cargo::discover::Checkout>,
    Vec<cargo_hive_cargo::discover::WalkFault>,
) {
    let mut checkouts = Vec::new();
    let mut walk_faults = Vec::new();
    for repo in repos {
        b.scope_mut().repos.push(repo.main.clone());
        let (rows, faults) = cargo_hive_cargo::discover::checkouts(repo, registered_mains, cfg);
        for fault in &faults {
            b.finding(
                "walk.unreadable",
                Some(fault.path.clone()),
                fault.detail.clone(),
            );
        }
        walk_faults.extend(faults);
        for c in rows {
            if !c.missing {
                b.scope_mut().checkouts.push(c.path.clone());
            }
            checkouts.push(c);
        }
    }
    (checkouts, walk_faults)
}

fn status(state: &State, args: &[std::path::PathBuf], no_sizes: bool, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::discover::{self, SNAPSHOT_FILE, Snapshot};
    use cargo_hive_core::classify::Class;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    let loaded = (|| -> Result<_, cargo_hive_state::StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
            state.read::<Snapshot>(SNAPSHOT_FILE)?,
        ))
    })();
    let (reg, cfg, caps, mut snapshot) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "status", &e.to_string(), 1),
    };
    let mut b = Receipt::new("status", Scope::default());
    let list = |v: &[std::path::PathBuf]| {
        v.iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    b.note(format!("secondary_roots: {}", list(&cfg.secondary_roots)));
    b.note(format!("cache_roots: {}", list(&cfg.cache_roots)));
    let release = cargo_hive_cargo::toolchain::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = doctor::require(&caps, Capability::FingerprintReading, release.as_deref())
    {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
    let selection = select(&reg, args, &mut b);
    let code = i32::from(selection.refused);
    let selected = selection.repos;
    let all_repos = selection.all_repos;
    let mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (mut total, mut protected, mut secondary, mut missing) = (0u64, 0u64, 0u64, 0u64);
    let mut human = String::new();
    let mut all_checkouts = Vec::new();
    for repo in &selected {
        b.scope_mut().repos.push(repo.main.clone());
        human.push_str(&format!("{}\n", repo.main.display()));
        let (rows, faults) = discover::checkouts(repo, &mains, &cfg);
        for fault in faults {
            b.finding("walk.unreadable", Some(fault.path), fault.detail);
        }
        for c in rows {
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
            b.scope_mut().checkouts.push(c.path.clone());
            all_checkouts.push(c);
        }
    }
    b.count("checkouts", total)
        .count("protected", protected)
        .count("secondary", secondary)
        .count("missing", missing);
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "status",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let mut scan = discover::targets(&all_checkouts, &cfg, all_repos, &mut snapshot, cancelled);
    if cancelled() {
        b.finding("error", None, "status was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    for fault in &scan.walk_faults {
        b.finding(
            "walk.unreadable",
            Some(fault.path.clone()),
            fault.detail.clone(),
        );
    }
    if no_sizes {
        b.unmeasured("sizes", "--no-sizes was given");
    } else {
        let mut bytes = 0u64;
        for t in &mut scan.targets {
            if cancelled() {
                b.finding("error", None, "status was cancelled");
                return emit(state, out, b.finish(), &human, 130);
            }
            let key = format!("bytes:{}", t.path.display());
            if let Some(why) = &t.unreadable {
                b.unmeasured(&key, format!("the target dir is unreadable: {why}"));
                continue;
            }
            match cargo_hive_core::size::allocated_bytes(&t.path, cargo_hive_core::size::MAX_DEPTH)
            {
                Ok(n) => {
                    t.size = Some(n);
                    bytes += n;
                    b.count(&key, n);
                }
                Err(e) => {
                    b.unmeasured(&key, e.to_string());
                }
            }
        }
        b.count("bytes", bytes);
    }
    let seeds = match cargo_hive_seed::record::read(state) {
        Ok(file) => Some(file),
        Err(err) => {
            b.note(err.to_string());
            None
        }
    };
    report_targets(&mut b, &mut human, &scan, seeds.as_ref());
    let wrote = match state.locked_or_cancelled(cancelled, |tx| {
        if cancelled() {
            return Ok(false);
        }
        let disk = tx.read(SNAPSHOT_FILE)?;
        tx.write(
            SNAPSHOT_FILE,
            &discover::merge_snapshot(snapshot, disk, &scan.cleared_profiles),
        )?;
        Ok(true)
    }) {
        Ok(wrote) => wrote,
        Err(StateError::Cancelled) => false,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            b.finding("error", None, e.to_string());
            return emit(state, out, b.finish(), &human, 1);
        }
    };
    if !wrote {
        b.finding("error", None, "status was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    emit(state, out, b.finish(), &human, code)
}

fn report_targets(
    b: &mut ReceiptBuilder,
    human: &mut String,
    scan: &cargo_hive_cargo::discover::TargetScan,
    seeds: Option<&cargo_hive_seed::record::SeedsFile>,
) {
    use cargo_hive_cargo::discover::{status_layout_suffix, unit_layout_counts};
    use cargo_hive_core::fingerprint::Format;
    use cargo_hive_core::target::Owner;
    let (mut owned, mut orphans, mut unresolved, mut unreadable, mut unsupported) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut units, mut resolved, mut dangling, mut ambiguous) = (0u64, 0u64, 0u64, 0u64);
    human.push_str("targets\n");
    for t in &scan.targets {
        let (kind, detail) = match &t.owner {
            Owner::Owned { checkout } => {
                owned += 1;
                ("target.owned", format!("owned by {}", checkout.display()))
            }
            Owner::OrphanCandidate {
                owned_under_caller_env,
            } => {
                orphans += 1;
                let base = "no known checkout resolves to this dir".to_string();
                (
                    "target.orphan_candidate",
                    match owned_under_caller_env {
                        Some(var) => format!("{base}; it would be owned under the caller's {var}"),
                        None => base,
                    },
                )
            }
            Owner::Unresolved { reason } => {
                unresolved += 1;
                ("target.unresolved", reason.clone())
            }
        };
        let size = t.size.map(|n| format!("  {n} bytes")).unwrap_or_default();
        human.push_str(&format!("  {}{size}  {detail}\n", t.path.display()));
        b.finding(kind, Some(t.path.clone()), detail);
        if let Some(why) = &t.unreadable {
            unreadable += 1;
            human.push_str(&format!("    unreadable: {why}\n"));
            b.finding("target.unreadable", Some(t.path.clone()), why.clone());
        }
        for p in &t.profiles {
            let key = p.path.display().to_string();
            b.count(&format!("units:{key}"), p.units as u64);
            units += p.units as u64;
            match (&p.format, &p.edges) {
                (Format::Supported, Some(e)) => {
                    b.count(&format!("edges_resolved:{key}"), e.resolved)
                        .count(&format!("edges_unresolved:{key}"), e.unresolved)
                        .count(&format!("edges_ambiguous:{key}"), e.ambiguous);
                    resolved += e.resolved;
                    dangling += e.unresolved;
                    ambiguous += e.ambiguous;
                    let (flat, unit_dir) = unit_layout_counts(p);
                    if flat > 0 {
                        b.count(&format!("units_flat:{key}"), flat);
                    }
                    if unit_dir > 0 {
                        b.count(&format!("units_unit_dir:{key}"), unit_dir);
                    }
                    if p.partial_unit_dirs > 0 {
                        b.count(
                            &format!("partial_unit_dirs:{key}"),
                            p.partial_unit_dirs as u64,
                        );
                    }
                    let suffix = status_layout_suffix(p);
                    human.push_str(&format!(
                        "    {key}  {} units, edges {} resolved / {} unresolved / {} ambiguous{suffix}\n",
                        p.units, e.resolved, e.unresolved, e.ambiguous
                    ));
                }
                (Format::Unsupported(why), _) => {
                    unsupported += 1;
                    human.push_str(&format!("    {key}  unsupported: {why}\n"));
                    b.finding("profile.unsupported", Some(p.path.clone()), why.clone());
                }
                (Format::Supported, None) => {}
            }
        }
        if let Some(seeds) = seeds
            && let Some(record) = seeds.targets.get(&t.path)
        {
            match cargo_hive_seed::record::counts(t, record, cargo_hive_core::caps::DEPS_CAP) {
                Ok(Some(counts)) => {
                    let path = t.path.display();
                    b.count(&format!("reused_fresh:{path}"), counts.fresh)
                        .count(&format!("seeded_unneeded:{path}"), counts.unneeded)
                        .count(&format!("seeded_recompiled:{path}"), counts.recompiled)
                        .count(&format!("seeded_missing:{path}"), counts.missing);
                }
                Ok(None) => {}
                Err(why) => {
                    b.note(why);
                }
            }
        }
    }
    b.count("targets", scan.targets.len() as u64)
        .count("owned", owned)
        .count("orphan_candidates", orphans)
        .count("unresolved_owners", unresolved)
        .count("unreadable", unreadable)
        .count("unsupported_profiles", unsupported)
        .count("units", units)
        .count("edges_resolved", resolved)
        .count("edges_unresolved", dangling)
        .count("edges_ambiguous", ambiguous)
        .count("metadata_calls", scan.metadata_calls)
        .count("metadata_cache_hits", scan.metadata_cache_hits);
    for var in &scan.ignored_vars {
        b.note(format!("{var} is set in the caller's environment and was ignored: ownership comes from each checkout's own Cargo configuration"));
    }
}

fn put_counts(b: &mut ReceiptBuilder, suffix: &str, c: &cargo_hive_gc::gc::Counts) {
    b.count(&format!("sessions_deleted{suffix}"), c.sessions_deleted)
        .count(
            &format!("session_lock_files_deleted{suffix}"),
            c.session_lock_files_deleted,
        )
        .count(
            &format!("temp_entries_deleted{suffix}"),
            c.temp_entries_deleted,
        )
        .count(&format!("skipped_held{suffix}"), c.skipped_held)
        .count(
            &format!("skipped_lock_unopenable{suffix}"),
            c.skipped_lock_unopenable,
        );
    if let Some(bytes) = c.bytes {
        b.count(&format!("freed_bytes{suffix}"), bytes.freed).count(
            &format!("deleted_bytes_still_shared{suffix}"),
            bytes.still_shared,
        );
    }
}

fn put_checkout_counts(b: &mut ReceiptBuilder, suffix: &str, c: &cargo_hive_gc::checkout::Counts) {
    b.count(&format!("units_deleted{suffix}"), c.units_deleted)
        .count(&format!("unit_files_deleted{suffix}"), c.unit_files_deleted)
        .count(&format!("objects_deleted{suffix}"), c.objects_deleted)
        .count(&format!("objects_referenced{suffix}"), c.objects_referenced)
        .count(&format!("binaries_read{suffix}"), c.binaries_read)
        .count(&format!("targets_shared{suffix}"), c.targets_shared)
        .count(&format!("targets_outside{suffix}"), c.targets_outside);
    for reason in [
        cargo_hive_core::locks::Reason::Held,
        cargo_hive_core::locks::Reason::Unopenable,
        cargo_hive_core::locks::Reason::Unlockable,
        cargo_hive_core::locks::Reason::Missing,
        cargo_hive_core::locks::Reason::ProfileSet,
    ] {
        let name = reason.count_name();
        b.count(
            &format!("{name}{suffix}"),
            c.skips.get(name).copied().unwrap_or(0),
        );
    }
    if let Some(bytes) = c.bytes {
        b.count(&format!("freed_bytes{suffix}"), bytes.freed).count(
            &format!("deleted_bytes_still_shared{suffix}"),
            bytes.still_shared,
        );
    }
}

fn gc_cmd(state: &State, args: &[std::path::PathBuf], dry_run: bool, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::discover::{self, SNAPSHOT_FILE, Snapshot};
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_gc::gc;
    use cargo_hive_seed::evict::{self, EvictCtx, Limits};
    use cargo_hive_state::window_days;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::time::Duration;
    const CAP_BYTES: u64 = 40_000_000_000;
    const MAX_AGE_SECS: u64 = 30 * 86_400;
    let command = if dry_run { "gc --dry-run" } else { "gc" };
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            command,
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
            state.read::<Snapshot>(SNAPSHOT_FILE)?,
        ))
    })();
    let (reg, cfg, caps, mut snapshot) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, command, &e.to_string(), 1),
    };
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let mut b = Receipt::new(command, Scope::default());
    let caller_toolchain = cargo_hive_cargo::toolchain::resolve_toolchain(state.root()).ok();
    let release = caller_toolchain.as_ref().map(|t| t.release.clone());
    b.toolchain(release.clone());
    for cap in [Capability::FingerprintReading, Capability::Deletion] {
        if let Err(refusal) = doctor::require(&caps, cap, release.as_deref()) {
            b.finding("refusal", None, refusal.to_string());
            return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
        }
    }
    let selection = select(&reg, args, &mut b);
    let code = i32::from(selection.refused);
    let registered_mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (checkouts, checkout_faults) =
        live_checkouts(&selection.repos, &registered_mains, &cfg, &mut b);
    let all_repos = selection.all_repos;
    let scan = discover::targets(&checkouts, &cfg, all_repos, &mut snapshot, cancelled);
    if cancelled() {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), "gc was cancelled\n", 130);
    }
    for fault in &scan.walk_faults {
        b.finding(
            "walk.unreadable",
            Some(fault.path.clone()),
            fault.detail.clone(),
        );
    }
    let scope = gc::Scope::from_paths(
        args.is_empty(),
        checkouts
            .iter()
            .filter(|c| !c.missing)
            .map(|c| c.path.clone()),
    );
    let account_bytes = !caps.disabled.contains_key(&Capability::EvictionAccounting);
    let volumes = |fs: &dyn platform::Fs| -> BTreeMap<String, (std::path::PathBuf, u64)> {
        scan.targets
            .iter()
            .filter_map(|t| fs.volume(&t.path).ok())
            .map(|v| (v.container, (v.mount, v.available_bytes)))
            .collect()
    };
    let before = if dry_run {
        BTreeMap::new()
    } else {
        volumes(&*platform.fs)
    };
    let opts = gc::RunOpts {
        dry_run,
        now: std::time::SystemTime::now(),
        account_bytes,
    };
    let report = gc::run(&*platform.fs, &scan.targets, &scope, &opts, &cancelled);
    if !dry_run {
        let after = volumes(&*platform.fs);
        let touched: BTreeSet<String> = report
            .per_target
            .iter()
            .filter(|(_, c)| {
                c.sessions_deleted + c.session_lock_files_deleted + c.temp_entries_deleted > 0
            })
            .filter_map(|(p, _)| platform.fs.volume(p).ok().map(|v| v.container))
            .collect();
        for note in cargo_hive_gc::plan_tier::apply::cross_check_notes(&before, &after, &touched) {
            b.note(note);
        }
    }
    let mut store_cancelled = false;
    if !args.is_empty() {
        b.note("store.skipped_scoped");
    } else if !report.cancelled {
        match window_days(cfg.gc_window_days) {
            Err(err) => {
                b.finding("store.window_invalid", None, err.to_string());
            }
            Ok(days) => {
                let outcome = evict::run(
                    &EvictCtx {
                        state,
                        fs: &*platform.fs,
                        targets: &scan.targets,
                        checkouts: &checkouts,
                        checkout_faults: &checkout_faults,
                        scan_faults: &scan.walk_faults,
                        cache_roots: &cfg.cache_roots,
                        window: Duration::from_secs(days.saturating_mul(86_400)),
                        account_bytes,
                        now: opts.now,
                        caller_toolchain,
                        caller_toolchain_kept: cargo_hive_cargo::toolchain::caller_toolchain_kept(),
                        rustup: Path::new("rustup"),
                        liveness: cargo_hive_gc::liveness::liveness,
                    },
                    Limits {
                        cap_bytes: CAP_BYTES,
                        max_age_secs: MAX_AGE_SECS,
                    },
                    dry_run,
                    &cancelled,
                );
                store_cancelled = outcome.cancelled;
                for (name, n) in outcome.counts {
                    b.count(&name, n);
                }
                for finding in outcome.findings {
                    b.finding(&finding.kind, finding.path, finding.detail);
                }
                for note in outcome.notes {
                    b.note(note);
                }
                for (field, reason) in outcome.unmeasured {
                    b.unmeasured(&field, reason);
                }
            }
        }
    }
    put_counts(&mut b, "", &report.totals);
    let mut human = String::new();
    for (path, c) in &report.per_target {
        put_counts(&mut b, &format!(":{}", path.display()), c);
        human.push_str(&format!(
            "{}  {} sessions, {} temp entries{}\n",
            path.display(),
            c.sessions_deleted,
            c.temp_entries_deleted,
            c.bytes
                .map(|bytes| format!(", {} bytes freed", bytes.freed))
                .unwrap_or_default()
        ));
    }
    if !account_bytes {
        for name in ["freed_bytes", "deleted_bytes_still_shared"] {
            b.unmeasured(name, "doctor check (c) did not pass");
        }
    }
    for skip in &report.skipped {
        b.finding("gc.skipped", Some(skip.path.clone()), skip.reason.clone());
    }
    for (path, why) in &report.unmeasured {
        b.unmeasured(&path.display().to_string(), why.clone());
    }
    if report.unmeasured_more > 0 {
        b.unmeasured(
            "unmeasured_paths",
            format!(
                "{} more paths could not be measured and are not listed",
                report.unmeasured_more
            ),
        );
    }
    if report.cancelled || store_cancelled {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    let wrote = match state.locked_or_cancelled(cancelled, |tx| {
        if cancelled() {
            return Ok(false);
        }
        let disk = tx.read(SNAPSHOT_FILE)?;
        tx.write(
            SNAPSHOT_FILE,
            &discover::merge_snapshot(snapshot, disk, &scan.cleared_profiles),
        )?;
        Ok(true)
    }) {
        Ok(wrote) => wrote,
        Err(StateError::Cancelled) => false,
        Err(e) => {
            b.finding("error", None, e.to_string());
            return emit(state, out, b.finish(), &human, 1);
        }
    };
    if !wrote {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    emit(state, out, b.finish(), &human, code)
}

enum CheckoutGcError {
    State(StateError),
    Discovery(Vec<WalkFault>),
    Unsupported,
    Refused {
        release: Option<String>,
        reason: Refusal,
    },
}

struct CheckoutGc {
    fs: Box<dyn platform::Fs>,
    cfg: Config,
    snapshot: Snapshot,
    scope: Scope,
    checkout: PathBuf,
    live: Vec<Checkout>,
    nested: Vec<PathBuf>,
    checkout_faults: Vec<WalkFault>,
    release: Option<String>,
    window: Duration,
    account_bytes: bool,
}

fn find_checkout_complete(
    registry: &Registry,
    cfg: &Config,
    path: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Found, CheckoutGcError> {
    let mains: Vec<_> = registry
        .repos
        .iter()
        .map(|repo| repo.main.clone())
        .collect();
    let mut selected = None;
    let mut rows = Vec::new();
    let mut faults = Vec::new();
    for repo in &registry.repos {
        if cancel() {
            break;
        }
        let (mut checkouts, mut unreadable) =
            discover::checkouts_complete(repo, &mains, cfg, cancel);
        if selected.is_none()
            && let Some(row) = checkouts.iter().find(|candidate| {
                !candidate.missing
                    && candidate.path == path
                    && candidate.class == cargo_hive_core::classify::Class::Secondary
            })
        {
            selected = Some((repo.clone(), row.clone()));
        }
        rows.append(&mut checkouts);
        faults.append(&mut unreadable);
    }
    if !faults.is_empty() {
        return Err(CheckoutGcError::Discovery(faults));
    }
    let Some((repo, row)) = selected else {
        return Err(CheckoutGcError::Refused {
            release: None,
            reason: Refusal {
                kind: "refusal",
                path: None,
                detail: "could not verify the exact secondary checkout for cleanup".into(),
                code: 1,
            },
        });
    };
    Ok(Found {
        repo,
        row,
        rows,
        faults,
    })
}

impl CheckoutGc {
    fn prepare(
        state: &State,
        path: &Path,
        strict_cancel: Option<&(dyn Fn() -> bool + Sync)>,
    ) -> Result<Self, CheckoutGcError> {
        use cargo_hive_doctor::{self as doctor, Capabilities, Capability};

        let loaded = (|| -> Result<_, StateError> {
            if let Some(cancel) = strict_cancel {
                let check_cancel = || {
                    if cancel() {
                        Err(StateError::Cancelled)
                    } else {
                        Ok(())
                    }
                };
                let max_bytes = cargo_hive_state::slots::MAX_BYTES;
                check_cancel()?;
                let registry: Registry = state.read_regular_bounded(registry::FILE, max_bytes)?;
                check_cancel()?;
                let config = state.slot_config()?;
                check_cancel()?;
                let capabilities: Capabilities =
                    state.read_regular_bounded(doctor::CAPABILITIES_FILE, max_bytes)?;
                check_cancel()?;
                return Ok((registry, config, capabilities, Snapshot::default()));
            }
            Ok((
                state.read::<Registry>(registry::FILE)?,
                state.config()?,
                state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
                state.read::<Snapshot>(discover::SNAPSHOT_FILE)?,
            ))
        })();
        let (reg, cfg, caps, snapshot) = loaded.map_err(CheckoutGcError::State)?;
        let platform = platform::current().map_err(|_| CheckoutGcError::Unsupported)?;
        let days = cargo_hive_state::window_days(cfg.gc_window_days).map_err(|err| {
            CheckoutGcError::Refused {
                release: None,
                reason: Refusal {
                    kind: "refusal",
                    path: None,
                    detail: err.to_string(),
                    code: 1,
                },
            }
        })?;
        let toolchain = match strict_cancel {
            Some(cancel) => {
                cargo_hive_cargo::toolchain::resolve_toolchain_until(state.root(), cancel)
            }
            None => cargo_hive_cargo::toolchain::resolve_toolchain(state.root()),
        };
        let release = toolchain.ok().map(|t| t.release);
        for cap in [Capability::FingerprintReading, Capability::Deletion] {
            if let Err(refusal) = doctor::require(&caps, cap, release.as_deref()) {
                return Err(CheckoutGcError::Refused {
                    release,
                    reason: Refusal {
                        kind: "refusal",
                        path: None,
                        detail: refusal.to_string(),
                        code: 1,
                    },
                });
            }
        }
        let Found {
            repo,
            row,
            rows,
            faults,
        } = if let Some(cancel) = strict_cancel {
            find_checkout_complete(&reg, &cfg, path, cancel)
        } else {
            find_checkout(&reg, &cfg, path).map_err(|mut reason| {
                if let Some((p, owner)) = reason
                    .path
                    .as_ref()
                    .and_then(|p| owning_repo(&reg, p).map(|owner| (p, owner)))
                {
                    reason.detail = format!(
                        "{path} is not a checkout of {main}; name the checkout's root directory",
                        path = p.display(),
                        main = owner.main.display(),
                    );
                }
                CheckoutGcError::Refused {
                    release: release.clone(),
                    reason,
                }
            })
        }?;
        let live: Vec<_> = rows.into_iter().filter(|c| !c.missing).collect();
        let nested = live
            .iter()
            .map(|c| c.path.clone())
            .filter(|p| p != &row.path && p.starts_with(&row.path))
            .collect();
        Ok(Self {
            fs: platform.fs,
            cfg,
            snapshot,
            scope: Scope {
                repos: vec![repo.main],
                checkouts: vec![row.path.clone()],
            },
            checkout: row.path,
            live,
            nested,
            checkout_faults: faults,
            release,
            window: Duration::from_secs(days.saturating_mul(86_400)),
            account_bytes: !caps.disabled.contains_key(&Capability::EvictionAccounting),
        })
    }

    fn scan(&mut self, cancel: impl Fn() -> bool + Sync) -> TargetScan {
        discover::targets(&self.live, &self.cfg, false, &mut self.snapshot, cancel)
    }

    fn input<'a>(&'a self, scan: &'a TargetScan) -> CheckoutInput<'a> {
        CheckoutInput {
            fs: &*self.fs,
            scan,
            checkout: &self.checkout,
            nested: &self.nested,
            now: SystemTime::now(),
            window: self.window,
            account_bytes: self.account_bytes,
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
#[derive(Debug)]
pub(crate) struct SlotGcReport {
    pub(crate) expected: Vec<checkout::RequiredRoot>,
    pub(crate) completed: [checkout::RequiredRoot; 2],
    pub(crate) report: checkout::CheckoutReport,
}

#[cfg(all(test, target_os = "macos"))]
#[derive(Debug)]
pub(crate) struct SlotGcRefusal {
    pub(crate) expected: Vec<checkout::RequiredRoot>,
    pub(crate) reason: Refusal,
    pub(crate) evidence: Box<SlotGcFailure>,
}

#[cfg(all(test, target_os = "macos"))]
#[derive(Debug)]
pub(crate) enum SlotGcFailure {
    Setup,
    Discovery {
        checkout_faults: Vec<WalkFault>,
        target_faults: Vec<WalkFault>,
        unresolved: Vec<(PathBuf, String)>,
    },
    Run(checkout::EvidencedCheckoutReport),
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn checkout_gc_for_slot(
    state: &State,
    path: &Path,
    expected: &[checkout::RequiredRoot],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<SlotGcReport, SlotGcRefusal> {
    let refuse = |detail: &str, evidence| SlotGcRefusal {
        expected: expected.to_vec(),
        reason: Refusal {
            kind: "refusal",
            path: None,
            detail: detail.into(),
            code: 1,
        },
        evidence: Box::new(evidence),
    };
    let cancelled = |evidence| SlotGcRefusal {
        expected: expected.to_vec(),
        reason: Refusal {
            kind: "error",
            path: None,
            detail: "gc was cancelled".into(),
            code: 130,
        },
        evidence: Box::new(evidence),
    };
    let checkout_fault = |checkout_faults| SlotGcFailure::Discovery {
        checkout_faults,
        target_faults: Vec::new(),
        unresolved: Vec::new(),
    };
    if cancel() {
        return Err(cancelled(SlotGcFailure::Setup));
    }
    let operation = match CheckoutGc::prepare(state, path, Some(cancel)) {
        Ok(operation) => operation,
        Err(CheckoutGcError::Discovery(faults)) => {
            let evidence = checkout_fault(faults);
            return Err(if cancel() {
                cancelled(evidence)
            } else {
                refuse("checkout discovery was incomplete", evidence)
            });
        }
        Err(CheckoutGcError::State(StateError::Cancelled)) => {
            return Err(cancelled(SlotGcFailure::Setup));
        }
        Err(_) if cancel() => return Err(cancelled(SlotGcFailure::Setup)),
        Err(_) => {
            return Err(refuse(
                "could not establish scoped cleanup inputs and capabilities",
                SlotGcFailure::Setup,
            ));
        }
    };
    if cancel() {
        return Err(cancelled(checkout_fault(operation.checkout_faults)));
    }
    if !operation.checkout_faults.is_empty() {
        return Err(refuse(
            "checkout discovery was incomplete",
            checkout_fault(operation.checkout_faults),
        ));
    }
    let required_paths: Vec<_> = expected.iter().map(|root| root.path.clone()).collect();
    let scan = discover::targets_for_checkout_gc(&operation.live, &required_paths, cancel);
    let unresolved = discover::failed_workspaces(&scan);
    let interrupted = cancel();
    if interrupted || !scan.walk_faults.is_empty() || !unresolved.is_empty() {
        let evidence = SlotGcFailure::Discovery {
            checkout_faults: Vec::new(),
            target_faults: scan.walk_faults,
            unresolved,
        };
        return Err(if interrupted {
            cancelled(evidence)
        } else {
            refuse(
                "Cargo target discovery or ownership was incomplete",
                evidence,
            )
        });
    }
    let mut evidence = checkout::run_with_evidence(&operation.input(&scan), expected, cancel);
    if evidence.report.cancelled || cancel() {
        evidence.report.cancelled = true;
        return Err(cancelled(SlotGcFailure::Run(evidence)));
    }
    let Some(completed) = evidence.complete_roots().cloned() else {
        return Err(refuse(
            "scoped cleanup could not prove completion for both Cargo output roots",
            SlotGcFailure::Run(evidence),
        ));
    };
    Ok(SlotGcReport {
        expected: evidence.expected().to_vec(),
        completed,
        report: evidence.report,
    })
}

fn checkout_cmd(state: &State, path: &Path, out: &OutputArgs) -> i32 {
    use cargo_hive_cargo::discover::SNAPSHOT_FILE;
    use std::collections::{BTreeMap, BTreeSet};
    let command = "gc --checkout";
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            *out,
            command,
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let mut operation = match CheckoutGc::prepare(state, path, None) {
        Ok(operation) => operation,
        Err(CheckoutGcError::State(err)) => {
            return emit_failure(state, *out, command, &err.to_string(), 1);
        }
        Err(CheckoutGcError::Discovery(faults)) => {
            let detail = faults
                .first()
                .map_or("checkout discovery was incomplete", |fault| &fault.detail);
            return emit_failure(state, *out, command, detail, 1);
        }
        Err(CheckoutGcError::Unsupported) => {
            eprintln!("{MACOS_ONLY_MESSAGE}");
            return 2;
        }
        Err(CheckoutGcError::Refused { release, reason }) => {
            let mut b = Receipt::new(command, Scope::default());
            b.toolchain(release);
            b.finding(reason.kind, reason.path, reason.detail.clone());
            return emit(
                state,
                *out,
                b.finish(),
                &format!("refused: {}\n", reason.detail),
                reason.code,
            );
        }
    };
    let mut b = Receipt::new(command, operation.scope.clone());
    b.toolchain(operation.release.clone());
    for fault in &operation.checkout_faults {
        b.finding(
            "walk.unreadable",
            Some(fault.path.clone()),
            fault.detail.clone(),
        );
    }
    let scan = operation.scan(cancelled);
    if cancelled() {
        b.finding("error", None, "gc was cancelled");
        return emit(state, *out, b.finish(), "gc was cancelled\n", 130);
    }
    for fault in &scan.walk_faults {
        b.finding(
            "walk.unreadable",
            Some(fault.path.clone()),
            fault.detail.clone(),
        );
    }
    for var in &scan.ignored_vars {
        b.note(format!("{var} is set in the caller's environment and was ignored: ownership comes from each checkout's own Cargo configuration"));
    }
    let volumes = |fs: &dyn platform::Fs| -> BTreeMap<String, (std::path::PathBuf, u64)> {
        scan.targets
            .iter()
            .filter_map(|t| fs.volume(&t.path).ok())
            .map(|v| (v.container, (v.mount, v.available_bytes)))
            .collect()
    };
    let before = volumes(&*operation.fs);
    let report = checkout::run(&operation.input(&scan), &cancelled);
    let after = volumes(&*operation.fs);
    let touched: BTreeSet<String> = report
        .per_target
        .iter()
        .filter(|(_, c)| c.units_deleted + c.unit_files_deleted + c.objects_deleted > 0)
        .filter_map(|(p, _)| operation.fs.volume(p).ok().map(|v| v.container))
        .collect();
    for note in cargo_hive_gc::plan_tier::apply::cross_check_notes(&before, &after, &touched) {
        b.note(note);
    }
    put_checkout_counts(&mut b, "", &report.totals);
    let mut human = String::new();
    for (path, c) in &report.per_target {
        put_checkout_counts(&mut b, &format!(":{}", path.display()), c);
        human.push_str(&format!(
            "{}  {} units, {} unit files{}\n",
            path.display(),
            c.units_deleted,
            c.unit_files_deleted,
            c.bytes
                .map(|bytes| format!(", {} bytes freed", bytes.freed))
                .unwrap_or_default()
        ));
    }
    if !operation.account_bytes {
        for name in ["freed_bytes", "deleted_bytes_still_shared"] {
            b.unmeasured(name, "doctor check (c) did not pass");
        }
    }
    for finding in &report.findings {
        b.finding(&finding.kind, finding.path.clone(), finding.detail.clone());
    }
    for (path, why) in &report.unmeasured {
        b.unmeasured(&path.display().to_string(), why.clone());
    }
    if report.unmeasured_more > 0 {
        b.unmeasured(
            "unmeasured_paths",
            format!(
                "{} more paths could not be measured and are not listed",
                report.unmeasured_more
            ),
        );
    }
    if report.cancelled {
        b.finding("error", None, "gc was cancelled");
        return emit(state, *out, b.finish(), &human, 130);
    }
    let wrote = match state.locked_or_cancelled(cancelled, |tx| {
        if cancelled() {
            return Ok(false);
        }
        let disk = tx.read(SNAPSHOT_FILE)?;
        tx.write(
            SNAPSHOT_FILE,
            &discover::merge_snapshot(operation.snapshot, disk, &scan.cleared_profiles),
        )?;
        Ok(true)
    }) {
        Ok(wrote) => wrote,
        Err(StateError::Cancelled) => false,
        Err(e) => {
            b.finding("error", None, e.to_string());
            return emit(state, *out, b.finish(), &human, 1);
        }
    };
    if !wrote {
        b.finding("error", None, "gc was cancelled");
        return emit(state, *out, b.finish(), &human, 130);
    }
    let code = if report.failed { 1 } else { 0 };
    emit(state, *out, b.finish(), &human, code)
}

fn apply_cmd(state: &State, id: &str, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::ctx::Ctx;
    use cargo_hive_core::receipt::Outcome;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_gc::plan_tier::apply;
    use cargo_hive_gc::plan_tier::build;
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "apply",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
        ))
    })();
    let (reg, cfg, caps) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "apply", &e.to_string(), 1),
    };
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let mut b = Receipt::new("apply", Scope::default());
    begin_plan_tier(&mut b);
    let release = cargo_hive_cargo::toolchain::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
    b.toolchain(release.clone());
    for cap in [Capability::FingerprintReading, Capability::Deletion] {
        if let Err(refusal) = doctor::require(&caps, cap, release.as_deref()) {
            b.finding("refusal", None, refusal.to_string());
            return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
        }
    }
    let registered_mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let ctx = Ctx {
        state,
        fs: &*platform.fs,
        cfg: &cfg,
        now: std::time::SystemTime::now(),
        account_bytes: !caps.disabled.contains_key(&Capability::EvictionAccounting),
        repos: &reg.repos,
        mains: &registered_mains,
        liveness: cargo_hive_gc::liveness::liveness,
    };
    let applied = match apply::apply(&ctx, id, &cancelled) {
        Ok(applied) => applied,
        Err(apply::ApplyError::State(StateError::Cancelled)) => {
            b.finding("error", None, StateError::Cancelled.to_string());
            return emit(state, out, b.finish(), "apply was cancelled\n", 130);
        }
        Err(e) => {
            b.finding("error", None, e.to_string());
            return emit(state, out, b.finish(), &format!("{e}\n"), 1);
        }
    };
    let code = apply::exit_code(&applied);
    let word = match applied.outcome {
        Outcome::Applied => "applied",
        Outcome::Partial => "partial",
        Outcome::Refused => "refused",
    };
    let human = format!(
        "{}  {}  {word}\n",
        applied.id,
        build::class_label(applied.class)
    );
    let paths = applied
        .counts
        .get("paths")
        .or_else(|| applied.counts.get("paths_deleted"))
        .copied()
        .unwrap_or(0);
    for note in &applied.notes {
        b.note(note.clone());
    }
    for item in &applied.unmeasured {
        b.unmeasured(&item.field, item.reason.clone());
    }
    for (kind, path, detail) in &applied.findings {
        b.finding(kind, path.clone(), detail.clone());
    }
    b.scope_mut().repos = applied.scope_repos.clone();
    b.scope_mut().checkouts = applied.scope_checkouts.clone();
    b.plan_tier(PlanTierBody {
        plans: vec![PlanRecord {
            id: applied.id,
            class: build::class_label(applied.class).to_string(),
            outcome: Some(applied.outcome),
            reason: applied.reason,
            targets: applied.targets,
            paths,
            counts: applied.counts,
        }],
        undecided: Vec::new(),
        nothing_to_do: Vec::new(),
    });
    emit(state, out, b.finish(), &human, code)
}

fn plan_cmd(state: &State, args: &[std::path::PathBuf], out: OutputArgs) -> i32 {
    use cargo_hive_cargo::ctx::Ctx;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_gc::plan_tier::build::{self, MakeError};
    use cargo_hive_gc::plan_tier::record;
    use std::collections::BTreeMap;
    use std::time::SystemTime;
    const ID_ATTEMPTS: u32 = 4;
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "gc --plan",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
        ))
    })();
    let (reg, cfg, caps) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "gc --plan", &e.to_string(), 1),
    };
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let mut b = Receipt::new("gc --plan", Scope::default());
    begin_plan_tier(&mut b);
    let release = cargo_hive_cargo::toolchain::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = doctor::require(&caps, Capability::FingerprintReading, release.as_deref())
    {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
    let registered_mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let mut named = Vec::new();
    if args.is_empty() {
        named.clone_from(&reg.repos);
    } else {
        for arg in args {
            let real = arg.canonicalize().unwrap_or_else(|_| arg.clone());
            if let Some(repo) = reg.repos.iter().find(|r| r.main == real)
                && !named.iter().any(|got| got.main == repo.main)
            {
                named.push(repo.clone());
            }
        }
    }
    let (_checkouts, _) = live_checkouts(&named, &registered_mains, &cfg, &mut b);
    if !args.is_empty() {
        for repo in &reg.repos {
            let (rows, _) = cargo_hive_cargo::discover::checkouts(repo, &registered_mains, &cfg);
            for arg in args {
                let real = arg.canonicalize().unwrap_or_else(|_| arg.clone());
                if rows.iter().any(|row| !row.missing && row.path == real)
                    && !b.scope_mut().checkouts.contains(&real)
                {
                    b.scope_mut().checkouts.push(real);
                }
            }
        }
    }
    let now = SystemTime::now();
    let ctx = Ctx {
        state,
        fs: &*platform.fs,
        cfg: &cfg,
        now,
        account_bytes: !caps.disabled.contains_key(&Capability::EvictionAccounting),
        repos: &reg.repos,
        mains: &registered_mains,
        liveness: cargo_hive_gc::liveness::liveness,
    };
    let made = match build::make(&ctx, args, &cancelled) {
        Ok(made) => made,
        Err(MakeError::Window(err)) => {
            b.finding("refusal", None, err.to_string());
            return emit(state, out, b.finish(), &format!("refused: {err}\n"), 1);
        }
        Err(MakeError::Cancelled) => {
            b.finding("error", None, "gc --plan was cancelled");
            return emit(state, out, b.finish(), "gc --plan was cancelled\n", 130);
        }
    };
    let code = build::command_code(&made);
    let made_at = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let unreadable = made
        .findings
        .iter()
        .any(|(kind, _, _)| kind == "plan.unreadable");
    let mut plans = made.plans;
    let mut stored = unreadable;
    if !unreadable {
        for attempt in 0..ID_ATTEMPTS {
            match record::insert(state, &cancelled, plans.clone(), made_at) {
                Ok(()) => {
                    stored = true;
                    break;
                }
                Err(StateError::DuplicatePlan { .. }) if attempt + 1 < ID_ATTEMPTS => {
                    for plan in &mut plans {
                        build::reissue(plan);
                    }
                }
                Err(StateError::Cancelled) => {
                    b.finding("error", None, StateError::Cancelled.to_string());
                    return emit(state, out, b.finish(), "gc --plan was cancelled\n", 130);
                }
                Err(e) => {
                    b.finding("error", None, e.to_string());
                    return emit(state, out, b.finish(), &format!("{e}\n"), 1);
                }
            }
        }
    }
    if !stored {
        b.finding(
            "error",
            None,
            "a plan id collided; run `cargo hive gc --plan` again",
        );
        return emit(state, out, b.finish(), "could not store the plan\n", 1);
    }
    let mut records = Vec::new();
    let mut human = String::new();
    for plan in &plans {
        let mut counts = BTreeMap::new();
        if let Some(bytes) = plan.planned {
            counts.insert("freed_bytes".to_string(), bytes.freed);
            counts.insert("deleted_bytes_still_shared".to_string(), bytes.still_shared);
        }
        let paths = plan.targets.iter().map(|t| t.entries.len() as u64).sum();
        human.push_str(&format!(
            "{}  {}  {} paths\n",
            plan.id,
            build::class_label(plan.class),
            paths
        ));
        records.push(PlanRecord {
            id: plan.id.clone(),
            class: build::class_label(plan.class).to_string(),
            outcome: None,
            reason: None,
            targets: plan.targets.iter().map(|t| t.path.clone()).collect(),
            paths,
            counts,
        });
    }
    if records.is_empty() {
        human.push_str("no plan\n");
    }
    for item in &made.unmeasured {
        b.unmeasured(&item.field, item.reason.clone());
    }
    for (kind, path, detail) in &made.findings {
        b.finding(kind, path.clone(), detail.clone());
    }
    b.plan_tier(PlanTierBody {
        plans: records,
        undecided: made.undecided,
        nothing_to_do: made.nothing_to_do,
    });
    emit(state, out, b.finish(), &human, code)
}

fn explain_cmd(state: &State, args: &[std::path::PathBuf], top: usize, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::discover::{self, SNAPSHOT_FILE, Snapshot};
    use cargo_hive_core::fingerprint::Format;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_gc::explain::{self, FailedUnits, Limits};
    use cargo_hive_platform::Availability;
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
            state.read::<Snapshot>(SNAPSHOT_FILE)?,
        ))
    })();
    let (reg, cfg, caps, mut snapshot) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "explain", &e.to_string(), 1),
    };
    let mut b = Receipt::new("explain", Scope::default());
    let release = cargo_hive_cargo::toolchain::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = doctor::require(&caps, Capability::FingerprintReading, release.as_deref())
    {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
    let selection = select(&reg, args, &mut b);
    let code = i32::from(selection.refused);
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "explain",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let registered_mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (checkouts, _) = live_checkouts(&selection.repos, &registered_mains, &cfg, &mut b);
    let scan = discover::targets(
        &checkouts,
        &cfg,
        selection.all_repos,
        &mut snapshot,
        cancelled,
    );
    if cancelled() {
        b.finding("error", None, "explain was cancelled");
        return emit(state, out, b.finish(), "explain was cancelled\n", 130);
    }
    for fault in &scan.walk_faults {
        b.finding(
            "walk.unreadable",
            Some(fault.path.clone()),
            fault.detail.clone(),
        );
    }
    let wrote = match state.locked_or_cancelled(cancelled, |tx| {
        if cancelled() {
            return Ok(false);
        }
        let disk = tx.read(SNAPSHOT_FILE)?;
        tx.write(
            SNAPSHOT_FILE,
            &discover::merge_snapshot(snapshot, disk, &scan.cleared_profiles),
        )?;
        Ok(true)
    }) {
        Ok(wrote) => wrote,
        Err(StateError::Cancelled) => false,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            b.finding("error", None, e.to_string());
            return emit(state, out, b.finish(), &format!("{e}\n"), 1);
        }
    };
    if !wrote {
        b.finding("error", None, "explain was cancelled");
        return emit(state, out, b.finish(), "explain was cancelled\n", 130);
    }
    let scope = discover::Scope::from_paths(
        args.is_empty(),
        checkouts
            .iter()
            .filter(|checkout| !checkout.missing)
            .map(|checkout| checkout.path.clone()),
    );
    let kept: Vec<_> = scan
        .targets
        .iter()
        .filter(|target| scope.contains(target))
        .collect();
    for target in &kept {
        if let Some(why) = &target.unreadable {
            b.finding("target.unreadable", Some(target.path.clone()), why.clone());
            continue;
        }
        for profile in &target.profiles {
            if let Format::Unsupported(why) = &profile.format {
                b.finding(
                    "profile.unsupported",
                    Some(profile.path.clone()),
                    why.clone(),
                );
            }
        }
    }
    let failed = match doctor::failed_unit_reporting(&caps) {
        Availability::Available => FailedUnits::Report,
        Availability::Unavailable(reason) => FailedUnits::Off(reason),
    };
    let limits = if top == 0 {
        Limits { top: None }
    } else {
        Limits { top: Some(top) }
    };
    let explanation = match explain::run(&kept, limits, failed, &cancelled) {
        Ok(explanation) => explanation,
        Err(explain::Cancelled) => {
            b.finding("error", None, "explain was cancelled");
            return emit(state, out, b.finish(), "explain was cancelled\n", 130);
        }
    };
    for finding in explanation.findings {
        b.finding_measured(
            &finding.kind,
            finding.path,
            finding.detail,
            finding.measured,
        );
    }
    for (name, n) in explanation.counts {
        b.count(name, n);
    }
    for (field, reason) in explanation.unmeasured {
        b.unmeasured(&field, reason);
    }
    emit(state, out, b.finish(), &explanation.human, code)
}

#[derive(Debug)]
pub(crate) struct Refusal {
    pub kind: &'static str,
    pub path: Option<std::path::PathBuf>,
    pub detail: String,
    pub code: i32,
}

/// The named checkout, its repo, and the repo's checkouts and walk faults as listed to find it,
/// so a caller needs no second listing (CHECKOUTGC-5.6).
pub(crate) struct Found {
    pub repo: registry::Repo,
    pub row: cargo_hive_cargo::discover::Checkout,
    pub rows: Vec<cargo_hive_cargo::discover::Checkout>,
    pub faults: Vec<cargo_hive_cargo::discover::WalkFault>,
}

pub(crate) fn find_checkout(
    reg: &Registry,
    cfg: &cargo_hive_state::Config,
    path: &std::path::Path,
) -> Result<Found, Refusal> {
    let real = match path.canonicalize() {
        Ok(path) => path,
        Err(err) => {
            let detail = format!(
                "{path}: {err}; pass a checkout of a registered repository",
                path = path.display()
            );
            return Err(Refusal {
                kind: "refusal",
                path: Some(path.to_path_buf()),
                detail,
                code: 1,
            });
        }
    };
    let registered_mains: Vec<_> = reg.repos.iter().map(|repo| repo.main.clone()).collect();
    let mut matched = None;
    for repo in &reg.repos {
        let (rows, faults) = cargo_hive_cargo::discover::checkouts(repo, &registered_mains, cfg);
        if let Some(row) = rows.iter().find(|row| row.path == real && !row.missing) {
            matched = Some(Found {
                repo: repo.clone(),
                row: row.clone(),
                rows,
                faults,
            });
            break;
        }
    }
    let Some(found) = matched else {
        let detail = format!(
            "{path} is not a checkout of a registered repository; register it with `cargo hive repo add` and run the command again",
            path = real.display()
        );
        return Err(Refusal {
            kind: "refusal",
            path: Some(real),
            detail,
            code: 1,
        });
    };
    Ok(found)
}

pub(crate) fn locate_checkout(
    reg: &Registry,
    cfg: &cargo_hive_state::Config,
    caps: &cargo_hive_doctor::Capabilities,
    path: &std::path::Path,
) -> Result<
    (
        registry::Repo,
        cargo_hive_cargo::discover::Checkout,
        cargo_hive_cargo::toolchain::Toolchain,
    ),
    Refusal,
> {
    use cargo_hive_core::classify::Class;
    use cargo_hive_doctor::{self as doctor, Capability};

    let Found { repo, row, .. } = find_checkout(reg, cfg, path)?;
    let real = row.path.clone();
    if row.class == Class::Protected {
        let detail = format!(
            "{path} is protected ({reason}); hive leaves a protected checkout untouched",
            path = real.display(),
            reason = row.reason
        );
        return Err(Refusal {
            kind: "refusal",
            path: Some(real),
            detail,
            code: 1,
        });
    }
    let toolchain = match cargo_hive_cargo::toolchain::resolve_toolchain(&real) {
        Ok(toolchain) => toolchain,
        Err(err) => {
            let code = doctor::toolchain_failure_exit(&err);
            let detail = if code == 130 {
                format!("{path}: {err}", path = real.display())
            } else {
                format!(
                    "{path}: {err}; install the toolchain that checkout selects and run the command again",
                    path = real.display()
                )
            };
            let kind = if code == 130 { "error" } else { "refusal" };
            return Err(Refusal {
                kind,
                path: Some(real),
                detail,
                code,
            });
        }
    };
    for cap in [Capability::Seeding, Capability::Deletion] {
        if let Err(refusal) = doctor::require(caps, cap, Some(toolchain.release.as_str())) {
            let detail = refusal.to_string();
            return Err(Refusal {
                kind: "refusal",
                path: Some(real.clone()),
                detail,
                code: 1,
            });
        }
    }
    Ok((repo, row, toolchain))
}

fn seed_cmd(state: &State, checkout: &std::path::Path, out: OutputArgs) -> i32 {
    use cargo_hive_cargo::ctx::Ctx;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_state::window_days;
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
        ))
    })();
    let (reg, cfg, caps) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "seed", &e.to_string(), 1),
    };
    let mut b = Receipt::new("seed", Scope::default());
    if let Err(err) = window_days(cfg.gc_window_days) {
        b.finding("refusal", None, err.to_string());
        return emit(state, out, b.finish(), &format!("refused: {err}\n"), 1);
    }
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "seed",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let (repo, row, toolchain) = match locate_checkout(&reg, &cfg, &caps, checkout) {
        Ok(v) => v,
        Err(r) => {
            b.finding(r.kind, r.path, r.detail.clone());
            let human = if r.code == 130 {
                format!("{}\n", r.detail)
            } else {
                format!("refused: {}\n", r.detail)
            };
            return emit(state, out, b.finish(), &human, r.code);
        }
    };
    let real = row.path.clone();
    let registered_mains: Vec<_> = reg.repos.iter().map(|repo| repo.main.clone()).collect();
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let repos = [repo];
    let ctx = Ctx {
        state,
        fs: &*platform.fs,
        cfg: &cfg,
        now: std::time::SystemTime::now(),
        account_bytes: !caps.disabled.contains_key(&Capability::EvictionAccounting),
        repos: &repos,
        mains: &registered_mains,
        liveness: cargo_hive_gc::liveness::liveness,
    };
    b.toolchain(Some(toolchain.release.clone()));
    b.scope_mut().checkouts.push(real.clone());
    let result = cargo_hive_seed::run(
        &ctx,
        &repos[0],
        &row,
        &toolchain,
        caps.unit_layout_or_flat(),
        &cancelled,
    );
    let code = cargo_hive_seed::exit_code(&result);
    cargo_hive_seed::record_seed_result(&mut b, &result);
    match result {
        Ok(outcome) => {
            let human = format!(
                "seed {path}\nfilled {added}, held {held}, cloned {cloned}\n",
                path = real.display(),
                added = outcome
                    .counts
                    .get("fill_entries_added")
                    .copied()
                    .unwrap_or(0),
                held = outcome
                    .counts
                    .get("fill_entries_held")
                    .copied()
                    .unwrap_or(0),
                cloned = outcome
                    .counts
                    .get("clone_units_cloned")
                    .copied()
                    .unwrap_or(0),
            );
            emit(state, out, b.finish(), &human, code)
        }
        Err(err) => emit(state, out, b.finish(), &format!("{err}\n"), code),
    }
}

fn bench_cmd(
    state: &State,
    cold: &[std::path::PathBuf],
    seeded: &[std::path::PathBuf],
    build: BuildCmd,
    out: OutputArgs,
) -> i32 {
    use crate::bench::command::env_map;
    use crate::bench::{self, BUILD_DEADLINE, Limits, Plan};
    use cargo_hive_cargo::ctx::Ctx;
    use cargo_hive_core::receipt::Measured;
    use cargo_hive_doctor::{self as doctor, Capabilities, Capability};
    use cargo_hive_state::window_days;
    use std::collections::BTreeMap;

    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
        ))
    })();
    let (reg, cfg, caps) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, "bench", &e.to_string(), 1),
    };
    let mut b = Receipt::new("bench", Scope::default());
    if let Err(err) = window_days(cfg.gc_window_days) {
        b.finding("refusal", None, err.to_string());
        return emit(state, out, b.finish(), &format!("refused: {err}\n"), 1);
    }
    if let Err(e) = platform::signals::install() {
        return emit_failure(
            state,
            out,
            "bench",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || platform::signals::cancelled();
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let checked = match bench::check::check(&reg, &cfg, &caps, cold, seeded, &cancelled) {
        Ok(v) => v,
        Err(bench::check::CheckStop::Cancelled) => {
            b.finding("error", None, "bench was cancelled");
            return emit(state, out, b.finish(), "bench was cancelled\n", 130);
        }
        Err(bench::check::CheckStop::Unfair(list)) => {
            for u in &list {
                b.finding("refusal", u.path.clone(), u.reason.clone());
            }
            let detail = list
                .iter()
                .map(|u| u.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return emit(state, out, b.finish(), &format!("refused: {detail}\n"), 1);
        }
    };
    let env = env_map(std::env::vars_os(), &checked.toolchain);
    let plan = Plan {
        repo: checked.repo,
        cold: checked.cold,
        seeded: checked.seeded,
        toolchain: checked.toolchain,
        build,
        env,
    };
    let registered_mains: Vec<_> = reg.repos.iter().map(|repo| repo.main.clone()).collect();
    let repos = [plan.repo.clone()];
    let ctx = Ctx {
        state,
        fs: &*platform.fs,
        cfg: &cfg,
        now: std::time::SystemTime::now(),
        account_bytes: !caps.disabled.contains_key(&Capability::EvictionAccounting),
        repos: &repos,
        mains: &registered_mains,
        liveness: cargo_hive_gc::liveness::liveness,
    };
    let report = bench::run(
        &ctx,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        caps.unit_layout_or_flat(),
        &cancelled,
    );
    b.toolchain(Some(plan.toolchain.release.clone()));
    b.scope_mut().repos.push(plan.repo.main.clone());
    for c in plan.cold.iter().chain(plan.seeded.iter()) {
        b.scope_mut().checkouts.push(c.path.clone());
    }
    report.write_into(&mut b);
    for (kind, detail) in plan.env.findings() {
        b.finding(kind, None, detail);
    }
    let command_line = plan.build.command_line();
    b.finding_measured(
        "bench.command",
        None,
        command_line.clone(),
        BTreeMap::from([("command".into(), Measured::Text(command_line))]),
    );
    let n = plan.cold.len();
    let human = match report.end {
        bench::End::Finished => {
            let cold_ms = report
                .counts
                .get("cold_build_ms_median")
                .copied()
                .unwrap_or(0);
            let seeded_ms = report
                .counts
                .get("seeded_total_ms_median")
                .copied()
                .unwrap_or(0);
            format!("bench n={n}: cold median {cold_ms} ms, seeded-total median {seeded_ms} ms\n")
        }
        bench::End::Cancelled => "bench was cancelled\n".into(),
        bench::End::Failed => "bench failed\n".into(),
    };
    emit(state, out, b.finish(), &human, report.exit_code())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use cargo_hive_cargo::git::Git;
    use cargo_hive_doctor::Capabilities;
    use cargo_hive_state::Config;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    #[cfg(target_os = "macos")]
    fn checkout_gc_fixture() -> (platform::TempDir, State, Registry, PathBuf) {
        let fs = platform::current().unwrap().fs;
        let scratch = fs
            .temp_dir(Path::new("/private/tmp"), "slot-checkout-gc")
            .unwrap();
        let state = State::open_at(scratch.path().join("hive")).unwrap();
        let main = scratch.path().join("main");
        let lane = main.join("lane");
        let cache = scratch.path().join("missing-cache");
        std::fs::create_dir(&main).unwrap();
        fixture_git(&main, &["init", "-q"]);
        std::fs::write(
            main.join("Cargo.toml"),
            "[package]\nname = \"slot-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "").unwrap();
        fixture_git(&main, &["add", "."]);
        fixture_git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        fixture_git(
            &main,
            &["worktree", "add", "-q", "--detach", lane.to_str().unwrap()],
        );
        let roots = Git::at(&main).root_commits().unwrap();
        let reg = Registry {
            repos: vec![registry::Repo {
                main,
                root_commits: roots,
            }],
            ..Default::default()
        };
        let caps = Capabilities {
            toolchain_release: Some(
                cargo_hive_cargo::toolchain::resolve_toolchain(state.root())
                    .unwrap()
                    .release,
            ),
            lock_check: cargo_hive_doctor::LOCK_CHECK_VERSION,
            ..Capabilities::default()
        };
        state
            .locked(|tx| {
                tx.write(registry::FILE, &reg)?;
                tx.write(cargo_hive_doctor::CAPABILITIES_FILE, &caps)?;
                tx.write(
                    "config.json",
                    &serde_json::json!({ "secondary_roots": [], "cache_roots": [cache] }),
                )
            })
            .unwrap();
        (scratch, state, reg, lane)
    }

    #[cfg(target_os = "macos")]
    fn fixture_git(path: &Path, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    #[cfg(target_os = "macos")]
    fn absent_roots(lane: &Path) -> [checkout::RequiredRoot; 2] {
        [checkout::RootRole::Target, checkout::RootRole::Build].map(|role| checkout::RequiredRoot {
            path: lane.join("target"),
            role,
            presence: checkout::RootPresence::InitiallyAbsent,
        })
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_refuses_incomplete_discovery() {
        let (scratch, state, reg, lane) = checkout_gc_fixture();
        let cache = scratch.path().join("missing-cache");
        let required = absent_roots(&lane);
        use std::os::unix::fs::PermissionsExt;

        let blocked = lane.join("unreadable-source");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .err()
            .unwrap();
        assert_eq!(refusal.reason.code, 1);
        assert!(
            refusal.reason.detail.contains("target discovery"),
            "{}",
            refusal.reason.detail
        );

        let SlotGcFailure::Discovery {
            checkout_faults,
            target_faults,
            unresolved,
        } = *refusal.evidence
        else {
            panic!("expected target scan evidence");
        };
        assert!(checkout_faults.is_empty() && unresolved.is_empty());
        assert!(target_faults.iter().any(|fault| fault.path == blocked));
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();

        let positive = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .ok()
            .unwrap();
        assert_eq!(positive.completed, required);
        assert!(!state.root().join(discover::SNAPSHOT_FILE).exists());
        assert!(!required[0].path.exists());

        let clones = scratch.path().join("clones");
        let clone = clones.join("related");
        std::fs::create_dir(&clones).unwrap();
        fixture_git(
            &clones,
            &[
                "clone",
                "--quiet",
                reg.repos[0].main.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        std::fs::create_dir(clone.join(".cargo")).unwrap();
        std::fs::write(
            clone.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}", required[0].path),
        )
        .unwrap();
        state
            .locked(|tx| {
                tx.write(
                    "config.json",
                    &serde_json::json!({
                        "secondary_roots": [clones], "cache_roots": [cache]
                    }),
                )
            })
            .unwrap();
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .expect_err("the related clone resolves the slot output");
        assert_eq!(refusal.reason.code, 1);
        let SlotGcFailure::Run(evidence) = *refusal.evidence else {
            panic!("expected recognized clone ownership");
        };
        assert!(evidence.completed().is_empty());

        let missing = scratch.path().join("missing-secondary");
        state
            .locked(|tx| {
                tx.write(
                    "config.json",
                    &serde_json::json!({
                        "secondary_roots": [missing], "cache_roots": [cache]
                    }),
                )
            })
            .unwrap();
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .err()
            .unwrap();
        assert_eq!(refusal.reason.code, 1);
        assert!(
            refusal.reason.detail.contains("checkout discovery"),
            "{}",
            refusal.reason.detail
        );

        let SlotGcFailure::Discovery {
            checkout_faults,
            target_faults,
            unresolved,
        } = *refusal.evidence
        else {
            panic!("expected checkout scan evidence");
        };
        assert!(target_faults.is_empty() && unresolved.is_empty());
        assert_eq!(checkout_faults[0].path, missing);

        let other = scratch.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::create_dir(other.join("src")).unwrap();
        std::fs::create_dir(other.join(".cargo")).unwrap();
        std::fs::write(
            other.join("Cargo.toml"),
            "[package]\nname = \"separate-repository\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(other.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            other.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}", required[0].path),
        )
        .unwrap();
        fixture_git(&other, &["init", "-q"]);
        fixture_git(&other, &["add", "."]);
        fixture_git(
            &other,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "other",
            ],
        );
        let mut reg = reg;
        reg.repos.push(registry::Repo {
            root_commits: Git::at(&other).root_commits().unwrap(),
            main: other,
        });
        state
            .locked(|tx| {
                tx.write(registry::FILE, &reg)?;
                tx.write(
                    "config.json",
                    &serde_json::json!({
                        "secondary_roots": [], "cache_roots": [cache]
                    }),
                )
            })
            .unwrap();
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .err()
            .unwrap();
        assert_eq!(refusal.reason.code, 1);
        assert!(
            refusal.reason.detail.contains("prove completion"),
            "{}",
            refusal.reason.detail
        );
        assert!(!required[0].path.exists());

        let SlotGcFailure::Run(evidence) = *refusal.evidence else {
            panic!("expected incomplete ownership evidence");
        };
        assert!(evidence.completed().is_empty());
        assert!(
            evidence
                .report
                .findings
                .iter()
                .any(|finding| finding.kind == "gc.root_incomplete")
        );

        let target = required[0].path.clone();
        let debug = target.join("debug");
        let unit = debug.join(".fingerprint/fixture-aaaaaaaaaaaaaaaa");
        let foreign_output = unit.join("foreign-target");
        std::fs::create_dir_all(&foreign_output).unwrap();
        let sentinel = foreign_output.join("keep");
        std::fs::write(&sentinel, b"foreign output").unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            cargo_hive_core::target::TAG_LINE,
        )
        .unwrap();
        std::fs::write(debug.join(".cargo-build-lock"), "").unwrap();
        std::fs::write(unit.join("lib-fixture"), "0000000000000068").unwrap();
        std::fs::write(
            unit.join("lib-fixture.json"),
            r#"{"rustc":1,"features":"[]","declared_features":"[]","target":2,"profile":3,"path":4,"deps":[],"local":[],"rustflags":[],"config":5,"compile_kind":0}"#,
        )
        .unwrap();
        let stamp = unit.join("invoked.timestamp");
        std::fs::write(&stamp, b"").unwrap();
        std::fs::File::open(&stamp)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400),
            )
            .unwrap();
        std::fs::write(
            reg.repos[1].main.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}", foreign_output),
        )
        .unwrap();
        let identity = platform::current().unwrap().fs.entry_stat(&target).unwrap();
        let required = required.map(|mut root| {
            root.presence = checkout::RootPresence::Present {
                dev: identity.dev,
                ino: identity.ino,
            };
            root
        });
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .expect_err("a nested foreign output must block strict cleanup");
        let SlotGcFailure::Run(evidence) = *refusal.evidence else {
            panic!("expected typed root refusal");
        };
        assert!(evidence.completed().is_empty());
        assert!(evidence.report.per_target.is_empty(), "cleanup started");
        assert!(evidence.report.findings.iter().any(|finding| {
            finding.kind == "gc.root_incomplete" && finding.path.as_ref() == Some(&target)
        }));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"foreign output");
        assert!(stamp.is_file());

        std::fs::write(reg.repos[1].main.join("Cargo.toml"), "[broken").unwrap();
        let refusal = checkout_gc_for_slot(&state, &lane, &required, &|| false)
            .err()
            .unwrap();
        assert_eq!(refusal.reason.code, 1);
        assert!(
            refusal.reason.detail.contains("ownership"),
            "{}",
            refusal.reason.detail
        );
        let SlotGcFailure::Discovery {
            checkout_faults,
            target_faults,
            unresolved,
        } = *refusal.evidence
        else {
            panic!("expected unresolved workspace evidence");
        };
        assert!(checkout_faults.is_empty() && target_faults.is_empty());
        assert_eq!(unresolved[0].0, reg.repos[1].main);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_resolves_nested_linked_checkout_workspaces_independently() {
        assert_shared_nested_project(true);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_resolves_independent_workspaces_in_one_checkout() {
        assert_shared_nested_project(false);
    }

    #[cfg(target_os = "macos")]
    fn assert_shared_nested_project(linked: bool) {
        let (scratch, state, mut reg, lane) = checkout_gc_fixture();
        std::fs::create_dir(scratch.path().join("missing-cache")).unwrap();
        let other = scratch.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::create_dir(other.join("src")).unwrap();
        std::fs::write(
            other.join("Cargo.toml"),
            "[package]\nname = \"outer\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(other.join("src/lib.rs"), "").unwrap();
        fixture_git(&other, &["init", "-q"]);
        fixture_git(&other, &["add", "."]);
        fixture_git(
            &other,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "other",
            ],
        );
        let workspace_checkout = if linked {
            let nested = other.join("nested");
            fixture_git(
                &other,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--detach",
                    nested.to_str().unwrap(),
                ],
            );
            fixture_git(&nested, &["rm", "Cargo.toml", "src/lib.rs"]);
            nested
        } else {
            other.clone()
        };
        let project = workspace_checkout.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir(project.join(".cargo")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"nested\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(project.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            project.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}", lane.join("target")),
        )
        .unwrap();
        fixture_git(&workspace_checkout, &["add", "."]);
        fixture_git(
            &workspace_checkout,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "nested project",
            ],
        );
        reg.repos.push(registry::Repo {
            root_commits: Git::at(&other).root_commits().unwrap(),
            main: other,
        });
        state.locked(|tx| tx.write(registry::FILE, &reg)).unwrap();
        let result = checkout_gc_for_slot(&state, &lane, &absent_roots(&lane), &|| false);
        let refusal = result.expect_err("the nested checkout shares the slot output");
        assert_eq!(refusal.reason.code, 1);
        assert!(!lane.join("target").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_keeps_measured_effects_on_success_and_cancellation() {
        let (scratch, state, _, lane) = checkout_gc_fixture();
        std::fs::create_dir(scratch.path().join("missing-cache")).unwrap();
        let target = lane.join("target");
        let debug = target.join("debug");
        std::fs::create_dir_all(debug.join(".fingerprint")).unwrap();
        std::fs::create_dir_all(debug.join("deps")).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            cargo_hive_core::target::TAG_LINE,
        )
        .unwrap();
        std::fs::write(debug.join(".cargo-build-lock"), "").unwrap();
        let fs = platform::current().unwrap().fs;
        let identity = fs.entry_stat(&target).unwrap();
        let expectations = absent_roots(&lane).map(|mut root| {
            root.presence = checkout::RootPresence::Present {
                dev: identity.dev,
                ino: identity.ino,
            };
            root
        });

        let object = debug.join("deps/orphan.0.rcgu.o");
        let second = debug.join("deps/second.0.rcgu.o");
        std::fs::write(&object, "object").unwrap();
        let complete = checkout_gc_for_slot(&state, &lane, &expectations, &|| false)
            .expect("cleanup completes");
        assert_eq!(complete.expected, expectations);
        assert_eq!(complete.completed, expectations);
        assert_eq!(complete.report.totals.objects_deleted, 1);
        assert!(!object.exists());

        std::fs::write(&object, "object").unwrap();
        std::fs::write(&second, "object").unwrap();
        let refused = checkout_gc_for_slot(&state, &lane, &expectations, &|| {
            !object.exists() || !second.exists()
        })
        .expect_err("cancellation follows the first effect");
        assert_eq!(refused.reason.code, 130);
        assert_ne!(object.exists(), second.exists());
        let SlotGcFailure::Run(evidence) = *refused.evidence else {
            panic!("expected partial cleanup evidence");
        };
        assert!(evidence.report.cancelled);
        assert!(evidence.completed().is_empty());
        assert_eq!(evidence.report.totals.objects_deleted, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_refuses_unsafe_setup_artifacts() {
        let (scratch, state, _, lane) = checkout_gc_fixture();
        let expected = absent_roots(&lane);
        assert!(checkout_gc_for_slot(&state, &lane, &expected, &|| false).is_ok());
        std::fs::create_dir(scratch.path().join("missing-cache")).unwrap();
        let assert_strict_refusal = || {
            let refusal = checkout_gc_for_slot(&state, &lane, &expected, &|| false)
                .expect_err("strict setup must refuse unsafe state");
            assert_eq!(refusal.reason.code, 1);
            assert_eq!(refusal.expected, expected);
            assert!(matches!(*refusal.evidence, SlotGcFailure::Setup));
            assert!(refusal.reason.path.is_none());
            assert!(!expected[0].path.exists());
        };
        for name in [registry::FILE, cargo_hive_doctor::CAPABILITIES_FILE] {
            let file = state.root().join(name);
            let valid = std::fs::read(&file).unwrap();

            let mut oversized = valid.clone();
            oversized.resize(cargo_hive_state::slots::MAX_BYTES as usize + 1, b' ');
            std::fs::write(&file, oversized).unwrap();
            assert!(CheckoutGc::prepare(&state, &lane, None).is_ok());
            assert_strict_refusal();

            let alternate = scratch.path().join(name);
            std::fs::write(&alternate, &valid).unwrap();
            std::fs::remove_file(&file).unwrap();
            std::os::unix::fs::symlink(alternate, &file).unwrap();
            assert!(CheckoutGc::prepare(&state, &lane, None).is_ok());
            assert_strict_refusal();

            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
            assert_strict_refusal();
            std::fs::remove_dir(&file).unwrap();

            for invalid in [b"{".as_slice(), br#"{"layout":999}"#] {
                std::fs::write(&file, invalid).unwrap();
                assert_strict_refusal();
                assert!(CheckoutGc::prepare(&state, &lane, None).is_err());
            }

            std::fs::write(&file, valid).unwrap();
        }
        assert!(checkout_gc_for_slot(&state, &lane, &expected, &|| false).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_reports_cancellation_between_setup_reads() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (_scratch, state, _, lane) = checkout_gc_fixture();
        let expected = absent_roots(&lane);
        let checks = AtomicUsize::new(0);
        let cancel = || checks.fetch_add(1, Ordering::Relaxed) == 2;
        let refusal = checkout_gc_for_slot(&state, &lane, &expected, &cancel)
            .expect_err("cancellation after reading the registry stops setup");
        assert_eq!(refusal.reason.code, 130);
        assert_eq!(refusal.expected, expected);
        assert!(matches!(*refusal.evidence, SlotGcFailure::Setup));
        assert!(!expected[0].path.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_retains_expected_paths_on_a_setup_refusal() {
        let (scratch, state, _, lane) = checkout_gc_fixture();
        let missing = scratch.path().join("missing-checkout");
        let required = absent_roots(&lane);
        let refusal = checkout_gc_for_slot(&state, &missing, &required, &|| false)
            .expect_err("missing checkout refuses GC");
        assert_eq!(refusal.reason.code, 1);
        assert_eq!(refusal.expected, required);
        assert!(refusal.reason.path.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_cancels_a_bounded_clone_probe() {
        use cargo_hive_platform::proc::{self, Capture, Spec};
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ROOT: &str = "HIVE_TEST_CHECKOUT_GC_CLONE_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let state = State::open_at(root.join("hive")).unwrap();
            let lane = root.join("main/lane");
            let expected = absent_roots(&lane);
            let cancel = || root.join("clone-probe-started").is_file();
            let refusal = checkout_gc_for_slot(&state, &lane, &expected, &cancel)
                .expect_err("interrupted clone scope must refuse");
            assert!(cancel(), "the bounded Git probe was reached");
            assert_eq!(refusal.reason.code, 130, "{refusal:?}");
            assert_eq!(refusal.expected, expected);
            return;
        }

        let (scratch, state, _, _) = checkout_gc_fixture();
        let root = scratch.path();
        let secondary = root.join("clones");
        let candidate = secondary.join("fifo-clone/.git");
        std::fs::create_dir_all(candidate.join("objects")).unwrap();
        std::fs::write(candidate.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(candidate.join("config"))
                .status()
                .unwrap()
                .success()
        );
        state
            .locked(|tx| {
                tx.write(
                    "config.json",
                    &serde_json::json!({
                        "secondary_roots": [secondary], "cache_roots": [],
                    }),
                )
            })
            .unwrap();

        let shim = root.join("git");
        std::fs::write(
            &shim,
            r#"#!/bin/sh
case " $* " in
    *" rev-parse "*)
        for param in "$@"; do
            case "$param" in
                */fifo-clone)
                    : > "$HIVE_TEST_CHECKOUT_GC_CLONE_ROOT/clone-probe-started"
                    exec sleep 10
                    ;;
            esac
        done
        ;;
esac
exec /usr/bin/git "$@"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = vec![root.to_path_buf()];
        path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let mut spec = Spec::new(std::env::current_exe().unwrap());
        spec.args = [
            "--exact",
            "cli::tests::slot_checkout_gc_cancels_a_bounded_clone_probe",
        ]
        .map(Into::into)
        .into();
        spec.env_set = vec![
            (CHILD_ROOT.into(), root.as_os_str().to_owned()),
            ("PATH".into(), std::env::join_paths(path).unwrap()),
        ];
        let child = proc::run_bounded_complete(
            &spec,
            Duration::from_secs(6),
            Capture { stdout_cap: 16384, stderr_cap: 16384 },
            || false,
        )
        .expect("strict checkout discovery must bypass direct reads of clone config and stop its Git child");
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stdout)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn slot_checkout_gc_requires_complete_git_inventory() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ROOT: &str = "HIVE_TEST_CHECKOUT_GC_INVENTORY_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let state = State::open_at(root.join("hive")).unwrap();
            let lane = root.join("main/lane");
            let result = checkout_gc_for_slot(&state, &lane, &absent_roots(&lane), &|| false);
            let refusal = result.expect_err("unproved Git checkout scope allowed cleanup");
            assert_eq!(refusal.reason.code, 1);
            if std::env::var_os("HIVE_TEST_GIT_INVENTORY_CASE").as_deref()
                != Some(std::ffi::OsStr::new("covered-live-record"))
            {
                assert!(
                    refusal.reason.detail.contains("checkout discovery"),
                    "{}",
                    refusal.reason.detail
                );
            }
            assert!(!state.root().join(discover::SNAPSHOT_FILE).exists());
            return;
        }

        let (scratch, _, reg, lane) = checkout_gc_fixture();
        let root = scratch.path();
        std::fs::create_dir(root.join("missing-cache")).unwrap();
        let main = &reg.repos[0].main;
        let valid = fixture_git(main, &["worktree", "list", "--porcelain", "-z"]).stdout;
        assert!(valid.ends_with(b"\0\0"), "{valid:?}");
        let incomplete = valid[..valid.len() - 1].to_vec();
        let mut malformed = incomplete.clone();
        malformed.extend_from_slice(b"unexpected attribute\0\0");
        let mut oversize = incomplete.clone();
        oversize.extend_from_slice(b"locked ");
        oversize.resize(64 * 1024 - 1, b'x');
        oversize.extend_from_slice(b"\0\0");

        let hidden = main.join("hidden");
        fixture_git(
            main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                hidden.to_str().unwrap(),
            ],
        );
        std::fs::create_dir(hidden.join(".cargo")).unwrap();
        std::fs::write(
            hidden.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}", lane.join("target")),
        )
        .unwrap();
        let complete = fixture_git(main, &["worktree", "list", "--porcelain", "-z"]).stdout;
        assert!(complete.len() > valid.len());

        let shim = root.join("git");
        std::fs::write(
            &shim,
            r#"#!/bin/sh
case " $* " in
    *" worktree list "*)
        if [ "$1" = "--no-optional-locks" ]; then exec cat "$HIVE_TEST_STRICT_GIT"; fi
        printf 'legacy worktree listing\n' >> "$HIVE_TEST_LEGACY_GIT_CALLS"
        exec cat "$HIVE_TEST_LEGACY_GIT"
        ;;
esac
exec /usr/bin/git "$@"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = vec![root.to_path_buf()];
        path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let path = std::env::join_paths(path).unwrap();
        let legacy_data = root.join("legacy");
        let strict_data = root.join("strict");
        let legacy_calls = root.join("legacy-calls");
        let mut failures = Vec::new();
        for (label, legacy, strict) in [
            ("incomplete", &incomplete, &incomplete),
            ("malformed", &malformed, &malformed),
            ("oversize", &oversize, &oversize),
            ("covered-live-record", &valid, &complete),
        ] {
            std::fs::write(&legacy_data, legacy).unwrap();
            std::fs::write(&strict_data, strict).unwrap();
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "cli::tests::slot_checkout_gc_requires_complete_git_inventory",
                ])
                .env(CHILD_ROOT, root)
                .env("PATH", &path)
                .env("HIVE_TEST_GIT_INVENTORY_CASE", label)
                .env("HIVE_TEST_LEGACY_GIT_CALLS", &legacy_calls)
                .env("HIVE_TEST_LEGACY_GIT", &legacy_data)
                .env("HIVE_TEST_STRICT_GIT", &strict_data)
                .output()
                .unwrap();
            if !child.status.success() {
                failures.push(format!(
                    "{label}: {}",
                    String::from_utf8_lossy(&child.stdout)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(
            !legacy_calls.exists(),
            "strict discovery invoked the unbounded worktree listing"
        );
    }

    #[test]
    fn locating_a_missing_path_refuses_with_the_path_as_given() {
        let reg = Registry::default();
        let cfg = Config {
            secondary_roots: vec![],
            cache_roots: vec![],
            gc_window_days: None,
        };
        let caps = Capabilities::default();
        let given = Path::new("/private/tmp/hive-it-no-such-lane");
        let refusal = locate_checkout(&reg, &cfg, &caps, given).unwrap_err();
        assert_eq!(refusal.kind, "refusal");
        assert_eq!(refusal.path.as_deref(), Some(given));
        assert!(
            refusal
                .detail
                .contains("pass a checkout of a registered repository"),
            "{}",
            refusal.detail
        );
        assert_eq!(refusal.code, 1);
    }

    #[test]
    fn a_protected_checkout_refusal_does_not_name_seed() {
        let dir = PathBuf::from(format!(
            "/private/tmp/hive-it-protected-locate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a"), "a").unwrap();
        git(&["add", "a"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "a",
        ]);
        let main = dir.canonicalize().unwrap();
        let roots = Git::at(&main).root_commits().unwrap();
        let reg = Registry {
            repos: vec![registry::Repo {
                main: main.clone(),
                root_commits: roots,
            }],
            ..Default::default()
        };
        let cfg = Config {
            secondary_roots: vec![],
            cache_roots: vec![],
            gc_window_days: None,
        };
        let caps = Capabilities::default();
        let refusal = locate_checkout(&reg, &cfg, &caps, &main).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(refusal.kind, "refusal");
        assert_eq!(refusal.code, 1);
        assert!(
            refusal
                .detail
                .contains("hive leaves a protected checkout untouched"),
            "{}",
            refusal.detail
        );
        assert!(
            !refusal.detail.contains("seed leaves"),
            "{}",
            refusal.detail
        );
    }
}
