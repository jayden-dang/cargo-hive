//! Thin adapter: arguments → library calls → one receipt.

use crate::platform::{self, MACOS_ONLY_MESSAGE};
use crate::receipt::{PlanRecord, PlanTierBody, Receipt, ReceiptBuilder, Scope};
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
    /// Delete superseded incremental sessions and leftover rustc temp dirs. A session with a lock file is deleted under that lock
    Gc {
        /// Registered repos to clean; none means every target hive knows
        repos: Vec<std::path::PathBuf>,
        /// Report what would be deleted and freed, deleting nothing
        #[arg(long)]
        dry_run: bool,
        /// Store a plan of dead-unit paths and delete nothing
        #[arg(long, conflicts_with = "dry_run")]
        plan: bool,
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
    /// Report copies of one crate, feature splits, and units whose last compile failed
    Explain {
        /// Registered repos to explain; none means every registered repo
        repos: Vec<std::path::PathBuf>,
        /// -n, --top. Default 10. 0 means show all.
        #[arg(short = 'n', long = "top", default_value_t = 10)]
        top: usize,
        #[command(flatten)]
        out: OutputArgs,
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
    if let Command::TryLock { kind, path } = &cli.command {
        return try_lock(kind, path);
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
        Command::Gc {
            repos,
            dry_run,
            plan,
            out,
        } => {
            if plan {
                plan_cmd(&state, &repos, out)
            } else {
                gc_cmd(&state, &repos, dry_run, out)
            }
        }
        Command::Apply { id, out } => apply_cmd(&state, &id, out),
        Command::Seed { checkout, out } => seed_cmd(&state, &checkout, out),
        Command::Explain { repos, top, out } => explain_cmd(&state, &repos, top, out),
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
        cancel: doctor::process_cancel,
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
        match reg.repos.iter().find(|r| r.main == real) {
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
    cfg: &crate::state::Config,
    b: &mut ReceiptBuilder,
) -> Vec<crate::discover::Checkout> {
    let mut checkouts = Vec::new();
    for repo in repos {
        b.scope_mut().repos.push(repo.main.clone());
        let (rows, faults) = crate::discover::checkouts(repo, registered_mains, cfg);
        for fault in faults {
            b.finding("walk.unreadable", Some(fault.path), fault.detail);
        }
        for c in rows {
            if !c.missing {
                b.scope_mut().checkouts.push(c.path.clone());
            }
            checkouts.push(c);
        }
    }
    checkouts
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
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "status",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
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
    let seeds = match crate::seed::record::read(state) {
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
    scan: &crate::discover::TargetScan,
    seeds: Option<&crate::seed::record::SeedsFile>,
) {
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
        if let Some(seeds) = seeds {
            if let Some(record) = seeds.targets.get(&t.path) {
                match crate::seed::record::counts(t, record, crate::gc::DEPS_CAP) {
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

fn put_counts(b: &mut ReceiptBuilder, suffix: &str, c: &crate::gc::Counts) {
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

pub(crate) fn cross_check_notes(
    before: &std::collections::BTreeMap<String, (std::path::PathBuf, u64)>,
    after: &std::collections::BTreeMap<String, (std::path::PathBuf, u64)>,
    touched: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut notes = Vec::new();
    for container in touched {
        if let (Some((mount, was)), Some((_, now))) = (before.get(container), after.get(container))
        {
            let change = i128::from(*now) - i128::from(*was);
            notes.push(format!(
                "cross-check, not a count: available bytes in APFS container {container} (mounted at {}) changed by {change:+} during this run",
                mount.display()
            ));
        }
    }
    notes
}

fn gc_cmd(state: &State, args: &[std::path::PathBuf], dry_run: bool, out: OutputArgs) -> i32 {
    use crate::discover::{self, Snapshot, SNAPSHOT_FILE};
    use crate::doctor::{self, Capabilities, Capability};
    use crate::gc;
    use std::collections::{BTreeMap, BTreeSet};
    let command = if dry_run { "gc --dry-run" } else { "gc" };
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            command,
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
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
    let release = doctor::resolve_toolchain(state.root())
        .ok()
        .map(|t| t.release);
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
    let checkouts = live_checkouts(&selection.repos, &registered_mains, &cfg, &mut b);
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
        for note in cross_check_notes(&before, &after, &touched) {
            b.note(note);
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
    if report.cancelled {
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

fn apply_cmd(state: &State, id: &str, out: OutputArgs) -> i32 {
    use crate::doctor::{self, Capabilities, Capability};
    use crate::plan_tier::apply;
    use crate::plan_tier::build;
    use crate::plan_tier::Ctx;
    use crate::receipt::Outcome;
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "apply",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
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
    let release = doctor::resolve_toolchain(state.root())
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
    };
    let applied = match apply::apply(&ctx, id, &cancelled) {
        Ok(applied) => applied,
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
    use crate::doctor::{self, Capabilities, Capability};
    use crate::plan_tier::build::{self, MakeError};
    use crate::plan_tier::record;
    use crate::plan_tier::Ctx;
    use std::collections::BTreeMap;
    use std::time::SystemTime;
    const ID_ATTEMPTS: u32 = 4;
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "gc --plan",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
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
    let release = doctor::resolve_toolchain(state.root())
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
            if let Some(repo) = reg.repos.iter().find(|r| r.main == real) {
                if !named.iter().any(|got| got.main == repo.main) {
                    named.push(repo.clone());
                }
            }
        }
    }
    let _checkouts = live_checkouts(&named, &registered_mains, &cfg, &mut b);
    if !args.is_empty() {
        for repo in &reg.repos {
            let (rows, _) = crate::discover::checkouts(repo, &registered_mains, &cfg);
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
    use crate::discover::{self, Snapshot, SNAPSHOT_FILE};
    use crate::doctor::{self, Capabilities, Capability};
    use crate::explain::{self, FailedUnits, Limits};
    use crate::fingerprint::Format;
    use crate::platform::Availability;
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
    let release = doctor::resolve_toolchain(state.root())
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
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "explain",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
    let registered_mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let checkouts = live_checkouts(&selection.repos, &registered_mains, &cfg, &mut b);
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

fn seed_cmd(state: &State, checkout: &std::path::Path, out: OutputArgs) -> i32 {
    use crate::classify::Class;
    use crate::doctor::{self, Capabilities, Capability};
    use crate::plan_tier::Ctx;
    use crate::state::window_days;
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
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(
            state,
            out,
            "seed",
            &format!("could not install signal handlers: {e}"),
            1,
        );
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
    let real = match checkout.canonicalize() {
        Ok(path) => path,
        Err(err) => {
            let detail = format!(
                "{path}: {err}; pass a checkout of a registered repository",
                path = checkout.display()
            );
            b.finding("refusal", Some(checkout.to_path_buf()), detail.clone());
            return emit(state, out, b.finish(), &format!("refused: {detail}\n"), 1);
        }
    };
    let registered_mains: Vec<_> = reg.repos.iter().map(|repo| repo.main.clone()).collect();
    let mut matched = None;
    for repo in &reg.repos {
        let (rows, _) = crate::discover::checkouts(repo, &registered_mains, &cfg);
        if let Some(row) = rows
            .into_iter()
            .find(|row| row.path == real && !row.missing)
        {
            matched = Some((repo.clone(), row));
            break;
        }
    }
    let Some((repo, row)) = matched else {
        let detail = format!(
            "{path} is not a checkout of a registered repository; register it with `cargo hive repo add` and run the command again",
            path = real.display()
        );
        b.finding("refusal", Some(real), detail.clone());
        return emit(state, out, b.finish(), &format!("refused: {detail}\n"), 1);
    };
    if row.class == Class::Protected {
        let detail = format!(
            "{path} is protected ({reason}); seed leaves a protected checkout untouched",
            path = real.display(),
            reason = row.reason
        );
        b.finding("refusal", Some(real), detail.clone());
        return emit(state, out, b.finish(), &format!("refused: {detail}\n"), 1);
    }
    let toolchain = match doctor::resolve_toolchain(&real) {
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
            b.finding(kind, Some(real), detail.clone());
            let human = if code == 130 {
                format!("{detail}\n")
            } else {
                format!("refused: {detail}\n")
            };
            return emit(state, out, b.finish(), &human, code);
        }
    };
    for cap in [
        Capability::Seeding,
        Capability::StoreCompression,
        Capability::Deletion,
    ] {
        if let Err(refusal) = doctor::require(&caps, cap, Some(toolchain.release.as_str())) {
            let detail = refusal.to_string();
            b.finding("refusal", Some(real.clone()), detail.clone());
            return emit(state, out, b.finish(), &format!("refused: {detail}\n"), 1);
        }
    }
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
    };
    b.toolchain(Some(toolchain.release.clone()));
    b.scope_mut().checkouts.push(real.clone());
    let result = crate::seed::run(&ctx, &repos[0], &row, &toolchain, &cancelled);
    let code = crate::seed::exit_code(&result);
    crate::seed::record_seed_result(&mut b, &result);
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
