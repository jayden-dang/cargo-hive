//! Thin adapter: arguments → library calls → one receipt.

use crate::platform::{self, MACOS_ONLY_MESSAGE};
use crate::receipt::{Receipt, ReceiptBuilder, Scope};
use crate::registry::{self, Registry};
use crate::state::{State, StateError};
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
    /// Hold a lock until killed (used by doctor's lock check)
    #[command(name = "__hold-lock", hide = true)]
    HoldLock {
        kind: String,
        path: std::path::PathBuf,
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
    if let Command::HoldLock { kind, path } = &cli.command {
        return hold_lock(kind, path);
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
        Command::Status {
            repos,
            no_sizes,
            out,
        } => status(&state, &repos, no_sizes, out),
        Command::Doctor { out } => doctor_cmd(&state, out),
        Command::HoldLock { .. } => unreachable!("handled before the state root is opened"),
    }
}

fn hold_lock(kind: &str, path: &std::path::Path) -> i32 {
    let Some(kind) = platform::LockKind::parse(kind) else {
        eprintln!("cargo-hive: unknown lock kind {kind:?} (expected flock or fcntl)");
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

/// Logs the receipt, prints it (`--json`) or the human text, and returns `code`.
pub fn emit(state: &State, out: OutputArgs, receipt: Receipt, human: &str, code: i32) -> i32 {
    let line = serde_json::to_string(&receipt).expect("receipts always serialize");
    let cancelled = || crate::doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
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
    if logged {
        code
    } else {
        1
    }
}

fn emit_failure(state: &State, out: OutputArgs, command: &str, error: &str, code: i32) -> i32 {
    eprintln!("cargo-hive: {error}");
    let mut receipt = Receipt::new(command, Scope::default());
    receipt.finding("error", None, error.to_string());
    emit(state, out, receipt.finish(), &format!("{error}\n"), code)
}

fn repo_add(state: &State, path: &std::path::Path, out: OutputArgs) -> i32 {
    use crate::registry::{register, RegisterOutcome};
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

fn measured_text(m: &std::collections::BTreeMap<String, crate::receipt::Measured>) -> String {
    use crate::receipt::Measured;
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
    use crate::doctor::{self, DoctorEnv, Verdict};
    use crate::receipt::CheckRecord;
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let macos = platform::os_version();
    let env = DoctorEnv {
        tmp_parent: std::env::temp_dir(),
        macos: macos.clone(),
        toolchain: doctor::resolve_toolchain(state.root()),
    };
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "doctor",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let report = match doctor::run(&*platform.fs, &*platform.approve, &env) {
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
    if let Err(e) = state.locked(|tx| tx.write(doctor::CAPABILITIES_FILE, &caps)) {
        return emit_failure(state, out, "doctor", &e.to_string(), 1);
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
        let ids: Vec<String> = checks.iter().map(|c| format!("({})", c.letter())).collect();
        let line = format!(
            "{} is disabled: doctor check {} did not pass",
            cap.describe(),
            ids.join(", ")
        );
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

fn status(state: &State, args: &[std::path::PathBuf], no_sizes: bool, out: OutputArgs) -> i32 {
    use crate::classify::Class;
    use crate::discover::{self, Snapshot, SNAPSHOT_FILE};
    use crate::doctor::{self, Capabilities, Capability};
    let loaded = (|| -> Result<_, crate::state::StateError> {
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
    let release = doctor::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = doctor::require(&caps, Capability::FingerprintReading, release.as_deref())
    {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
    let mut code = 0;
    let mut selected = Vec::new();
    if args.is_empty() {
        selected = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => {
                if !selected.iter().any(|s| s.main == r.main) {
                    selected.push(r.clone());
                }
            }
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
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "status",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let all_repos = selected.len() == reg.repos.len()
        && selected
            .iter()
            .all(|s| reg.repos.iter().any(|r| r.main == s.main));
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
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
            match crate::size::allocated_bytes(&t.path, crate::size::MAX_DEPTH) {
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
    report_targets(&mut b, &mut human, &scan);
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

fn report_targets(b: &mut ReceiptBuilder, human: &mut String, scan: &crate::discover::TargetScan) {
    use crate::cargo_target::Owner;
    use crate::fingerprint::Format;
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
                    human.push_str(&format!(
                        "    {key}  {} units, edges {} resolved / {} unresolved / {} ambiguous\n",
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
