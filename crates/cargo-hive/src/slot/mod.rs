//! The slot CLI scopes operations to a registered main and its private pool namespace.

mod result;

use crate::cli::SlotCommand;
use cargo_hive_cargo::{
    env,
    registry::{self, Registry},
};
use cargo_hive_platform::proc::{self, Capture, Spec};
use cargo_hive_platform::signals::cancelled;
use cargo_hive_state::slots::{
    self, CHECKOUT_NAMESPACE, FileIdentity, GitIdentity, LOCK_NAMESPACE, MAX_POOLS, Phase, Pool,
    PoolProfile, Slot, SlotsFile,
};
use cargo_hive_state::{State, StateError, Tx};
use result::{FRESH_FALLBACK, SlotResult, SlotStatus};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HELD_ACTION: &str = "Preserve the slot files and ask the repository operator to inspect identity, quiescence and unfinished work. Create a separate fresh secondary checkout and run `cargo hive seed <checkout>` there.";
const LOST_PROVENANCE: &str = "This slot does not regain warm capacity for these retained outputs in this version. Create a separate fresh secondary checkout and run `cargo hive seed <checkout>` there.";
const GIT_OUTPUT_LIMIT: usize = 64 * 1024;
const MAX_WORKTREES: usize = 256;
const MAX_ROOT_ENTRIES: usize = 4096;
const MAX_ROOT_DEPTH: usize = 32;

pub(super) fn dispatch(command: SlotCommand) -> i32 {
    let (name, repo, json, init) = match command {
        SlotCommand::Init {
            repo,
            root,
            profile,
            out,
        } => ("slot init", repo, out.json, Some((root, profile))),
        SlotCommand::Status { repo, out } => ("slot status", repo, out.json, None),
        SlotCommand::Recover(op) => {
            return unavailable("slot recover", Some(op.scope.repo), op.out.json);
        }
        SlotCommand::Key { out, .. } => return unavailable("slot key", None, out.json),
        SlotCommand::Reserve { scope, out, .. } => {
            return unavailable("slot reserve", Some(scope.repo), out.json);
        }
        SlotCommand::Begin(op) => {
            return unavailable(
                "slot begin",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
        SlotCommand::StepCheck { scope, out, .. } => {
            return unavailable("slot step-check", Some(scope.repo), out.json);
        }
        SlotCommand::Producer(op) => {
            return unavailable(
                "slot producer",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
        SlotCommand::Ready { request, .. } => {
            return unavailable(
                "slot ready",
                Some(request.operation.scope.repo),
                request.operation.out.json,
            );
        }
        SlotCommand::Return(op) => {
            return unavailable("slot return", Some(op.scope.repo), op.out.json);
        }
        SlotCommand::Outcome(op) => {
            return unavailable(
                "slot outcome",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
        SlotCommand::Ack(op) => {
            return unavailable(
                "slot ack",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
    };
    match State::open_slot_state() {
        Ok(state) => run(&state, name, repo, json, init),
        Err(err) => refusal(name, repo, err.into()).emit(json),
    }
}

fn unavailable(command: &'static str, repo: Option<PathBuf>, json: bool) -> i32 {
    let mut result = SlotResult::new(command, "unavailable", repo, 1);
    result.detail = Some("This slot operation is unavailable in this version.".into());
    result.next_action = Some(FRESH_FALLBACK);
    result.emit(json)
}

fn run(
    state: &State,
    name: &'static str,
    repo: PathBuf,
    json: bool,
    init: Option<(PathBuf, PathBuf)>,
) -> i32 {
    let result = match state.locked_or_cancelled(cancelled, |tx| {
        Ok(inspect_or_init(tx, state, name, &repo, init))
    }) {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => refusal(name, repo, err),
        Err(err) => refusal(name, repo, err.into()),
    };
    match serde_json::to_string(&result.audit_receipt()) {
        Ok(line) => {
            if let Err(err) = state
                .locked_or_cancelled(cancelled, |tx| tx.append_slot_audit_line(&line).map(|_| ()))
            {
                eprintln!("cargo-hive: could not log the slot receipt: {err}");
            }
        }
        Err(_) => eprintln!("cargo-hive: could not encode the slot receipt"),
    }
    result.emit(json)
}

enum Refusal {
    State(StateError),
    Scope(String),
    Orphan(PathBuf, PathBuf),
}

impl From<StateError> for Refusal {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

fn refusal(command: &'static str, repo: PathBuf, err: Refusal) -> SlotResult {
    let (kind, code, detail, next) = match err {
        Refusal::State(StateError::Cancelled) => (
            "cancelled",
            130,
            StateError::Cancelled.to_string(),
            "Retry when ready.",
        ),
        Refusal::State(err) => ("refused", 1, err.to_string(), FRESH_FALLBACK),
        Refusal::Scope(detail) => ("refused", 1, detail, FRESH_FALLBACK),
        Refusal::Orphan(record, evidence) => (
            "held",
            1,
            format!(
                "{} cannot establish the last authoritative pool state for {}. Preserve the existing files. Ask the repository operator to restore only a proven current epoch, or explicitly quarantine the old namespace before initializing at a non-overlapping root.",
                record.display(),
                evidence.display()
            ),
            FRESH_FALLBACK,
        ),
    };
    let mut result = SlotResult::new(command, kind, Some(repo), code);
    result.detail = Some(detail);
    result.next_action = Some(next);
    result
}

fn inspect_or_init(
    tx: &Tx<'_>,
    state: &State,
    command: &'static str,
    main: &Path,
    init: Option<(PathBuf, PathBuf)>,
) -> Result<SlotResult, Refusal> {
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }
    let saved = tx.read_slots()?;
    let pools = saved.as_ref().map_or(&[][..], |file| file.pools.as_slice());
    let registry: Registry = tx.read_slot_registry()?;
    let config = state.slot_config()?;
    let existing = pools.iter().find(|pool| pool.main == main);
    let Some((root, profile)) = init else {
        if let Some(pool) = existing {
            let scope_verified = registered_main(main, &registry)
                .is_ok_and(|identity| pool.common_git_id == identity)
                && accepted_root(&pool.root, &config.secondary_roots)
                && slots::owner_directory(&pool.root).is_ok()
                && pool_directories_present(state.root(), pool);
            return Ok(project_pool(
                command,
                "status",
                state.root(),
                pool,
                scope_verified,
            ));
        }
        registered_main(main, &registry)?;
        check_namespaces(state.root(), &config.secondary_roots, pools)?;
        let mut result = SlotResult::new(command, "unconfigured", Some(main.to_path_buf()), 0);
        result.next_action = Some(
            "Use `cargo hive slot init` with a durable secondary root and a reviewed profile.",
        );
        return Ok(result);
    };

    let identity = registered_main(main, &registry)?;
    if existing.is_some()
        || pools
            .iter()
            .any(|pool| pool.common_git_id.file_id() == identity.file_id())
    {
        return Err(Refusal::Scope(format!(
            "{} already records a pool for this repository. Use `cargo hive slot status` to inspect its capacity.",
            state.root().join(slots::FILE).display()
        )));
    }
    if pools.len() >= MAX_POOLS {
        return Err(Refusal::Scope(
            "The saved pool count is at its limit.".into(),
        ));
    }
    slots::owner_directory(&root).map_err(|err| path_refusal(&root, err))?;
    let mut known_roots = config.secondary_roots.clone();
    if !known_roots.contains(&root) {
        known_roots.push(root.clone());
    }
    check_namespaces(state.root(), &known_roots, pools)?;
    verify_root(&root, state.root(), &config.secondary_roots, &registry)?;
    let profile = PoolProfile::read(&profile).map_err(|err| match err {
        StateError::Malformed { detail, .. } => Refusal::Scope(format!(
            "The reviewed profile {} is invalid ({detail}). Supply a compatible schema 1 profile.",
            profile.display()
        )),
        other => other.into(),
    })?;
    if profile.common_git_id != identity {
        return Err(Refusal::Scope(
            "The reviewed profile names a different registered Git identity.".into(),
        ));
    }
    let revision = match saved.as_ref() {
        None => 1,
        Some(file) => file.revision.checked_add(1).ok_or_else(|| {
            Refusal::Scope(format!(
                "{} has exhausted its revision counter.",
                state.root().join(slots::FILE).display()
            ))
        })?,
    };
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }

    let pool = provision_pool(state.root(), main, root, identity, profile)?;
    let result = project_pool(command, "initialized", state.root(), &pool, true);
    let previous = saved.as_ref().map(|file| file.revision);
    let mut pools = saved.map_or_else(Vec::new, |file| file.pools);
    pools.push(pool);
    tx.commit_slots(previous, &SlotsFile::new(revision, pools))?;
    Ok(result)
}

fn registered_main(main: &Path, registry: &Registry) -> Result<GitIdentity, Refusal> {
    let repo = registry
        .repos
        .iter()
        .find(|repo| repo.main.as_os_str() == main.as_os_str())
        .ok_or_else(|| {
            Refusal::Scope(
                "Name an exact registered main checkout from `cargo hive repo list`.".into(),
            )
        })?;
    current_git_identity(repo)
}

fn current_git_identity(repo: &registry::Repo) -> Result<GitIdentity, Refusal> {
    let main = &repo.main;
    slots::owner_directory(main).map_err(|err| path_refusal(main, err))?;
    let output = git_scope(
        main,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--show-superproject-working-tree",
        ],
    )?;
    let decoded = std::str::from_utf8(&output).map_err(|_| git_scope_refusal(main))?;
    let fields: Vec<_> = decoded.lines().collect();
    let [top, admin, common] = fields.as_slice() else {
        return Err(git_scope_refusal(main));
    };
    if main.as_os_str() != *top || admin != common {
        return Err(git_scope_refusal(main));
    }
    let common = PathBuf::from(common);
    let metadata = slots::owner_directory(&common).map_err(|err| path_refusal(&common, err))?;
    let roots = git_scope(main, &["rev-list", "--max-parents=0", "HEAD"])?;
    let roots = std::str::from_utf8(&roots).map_err(|_| git_scope_refusal(main))?;
    let roots: BTreeSet<_> = roots.lines().collect();
    if roots.is_empty()
        || !roots
            .into_iter()
            .eq(repo.root_commits.iter().map(String::as_str))
    {
        return Err(git_scope_refusal(main));
    }
    Ok(GitIdentity::of(common, &metadata))
}

fn git_scope(main: &Path, args: &[&str]) -> Result<Vec<u8>, Refusal> {
    let mut spec = Spec::new("git");
    spec.args.extend([
        "--no-optional-locks".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-C".into(),
    ]);
    spec.args.push(main.as_os_str().to_os_string());
    spec.args.extend(args.iter().map(|arg| (*arg).into()));
    let parent: Vec<_> = std::env::vars_os().collect();
    env::git(&mut spec, &parent);
    match proc::run_bounded(
        &spec,
        Duration::from_secs(10),
        Capture {
            stdout_cap: GIT_OUTPUT_LIMIT + 1,
            stderr_cap: 1,
        },
        cancelled,
    ) {
        Ok(ran)
            if ran.status.success()
                && ran.stderr_tail.is_empty()
                && ran.stdout.len() <= GIT_OUTPUT_LIMIT =>
        {
            Ok(ran.stdout)
        }
        Err(proc::ChildOutputError::Cancelled) => Err(StateError::Cancelled.into()),
        _ => Err(git_scope_refusal(main)),
    }
}

fn git_scope_refusal(main: &Path) -> Refusal {
    Refusal::Scope(format!(
        "Could not establish the complete registered Git scope for {}.",
        main.display()
    ))
}

fn accepted_root(root: &Path, configured: &[PathBuf]) -> bool {
    configured
        .iter()
        .any(|candidate| candidate.is_absolute() && root.starts_with(candidate))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn verify_root(
    root: &Path,
    state: &Path,
    configured: &[PathBuf],
    registry: &Registry,
) -> Result<(), Refusal> {
    slots::owner_directory(root).map_err(|err| path_refusal(root, err))?;
    if !accepted_root(root, configured) || overlaps(root, state) {
        return Err(Refusal::Scope(format!(
            "{} must be an independent owner-controlled path in a configured secondary root.",
            root.display()
        )));
    }
    for ancestor in root.ancestors() {
        if exists(&ancestor.join(".git")).map_err(|err| path_refusal(ancestor, err))? {
            return Err(Refusal::Scope(format!(
                "{} is within an existing Git checkout.",
                root.display()
            )));
        }
    }
    let mut remaining = MAX_ROOT_ENTRIES;
    check_nested_checkouts(root, 0, &mut remaining)?;
    if registry.repos.len() > MAX_POOLS {
        return Err(Refusal::Scope(
            "Too many registered repositories to check the complete owner scope.".into(),
        ));
    }
    for repo in &registry.repos {
        let common = current_git_identity(repo)?;
        if overlaps(root, &repo.main) || overlaps(root, &common.path) {
            return Err(overlap_refusal(root));
        }
        let inventory = git_scope(&repo.main, &["worktree", "list", "--porcelain", "-z"])?;
        verify_worktree_inventory(root, &repo.main, &inventory)?;
    }
    Ok(())
}

fn check_nested_checkouts(path: &Path, depth: usize, remaining: &mut usize) -> Result<(), Refusal> {
    if depth > MAX_ROOT_DEPTH {
        return Err(Refusal::Scope(
            "Could not check the complete secondary root within the directory depth limit.".into(),
        ));
    }
    for entry in fs::read_dir(path).map_err(|err| path_refusal(path, err))? {
        let entry = entry.map_err(|err| path_refusal(path, err))?;
        *remaining = remaining.checked_sub(1).ok_or_else(|| {
            Refusal::Scope(
                "Could not check the complete secondary root within the directory entry limit."
                    .into(),
            )
        })?;
        if entry.file_name() == ".git" {
            return Err(Refusal::Scope(format!(
                "{} contains an existing Git checkout.",
                path.display()
            )));
        }
        let kind = entry
            .file_type()
            .map_err(|err| path_refusal(&entry.path(), err))?;
        if kind.is_dir() {
            check_nested_checkouts(&entry.path(), depth + 1, remaining)?;
        }
    }
    Ok(())
}

fn overlap_refusal(root: &Path) -> Refusal {
    Refusal::Scope(format!(
        "{} overlaps a registered main or owner worktree.",
        root.display()
    ))
}

fn verify_worktree_inventory(root: &Path, main: &Path, inventory: &[u8]) -> Result<(), Refusal> {
    let decoded = std::str::from_utf8(inventory).map_err(|_| git_scope_refusal(main))?;
    let records = decoded
        .strip_suffix("\0\0")
        .ok_or_else(|| git_scope_refusal(main))?;
    let mut found_main = false;
    for (index, record) in records.split("\0\0").enumerate() {
        if index >= MAX_WORKTREES {
            return Err(git_scope_refusal(main));
        }
        let mut fields = record.split('\0');
        let path = fields
            .next()
            .and_then(|first| first.strip_prefix("worktree "))
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| git_scope_refusal(main))?;
        if fields.any(|field| {
            let name = field.split_once(' ').map_or(field, |(name, _)| name);
            !matches!(
                name,
                "HEAD" | "branch" | "bare" | "detached" | "locked" | "prunable"
            )
        }) {
            return Err(git_scope_refusal(main));
        }
        found_main |= path == main;
        if overlaps(root, path) {
            return Err(overlap_refusal(root));
        }
        match path.canonicalize() {
            Ok(canonical) if overlaps(root, &canonical) => return Err(overlap_refusal(root)),
            Ok(_) => (),
            Err(err) if err.kind() == io::ErrorKind::NotFound => (),
            Err(_) => return Err(git_scope_refusal(main)),
        }
    }
    if !found_main {
        return Err(git_scope_refusal(main));
    }
    Ok(())
}

fn path_refusal(path: &Path, err: io::Error) -> Refusal {
    Refusal::Scope(format!("Could not verify {}: {err}", path.display()))
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn check_namespaces(state: &Path, roots: &[PathBuf], pools: &[Pool]) -> Result<(), Refusal> {
    let orphan = |path: &Path| Refusal::Orphan(state.join(slots::FILE), path.to_path_buf());
    let checked_namespace = |path: &Path, root: Option<&Path>| {
        if !exists(path).map_err(|_| orphan(path))? {
            return Ok(());
        }
        let mut known = pools
            .iter()
            .filter(|pool| root.is_none_or(|root| pool.root == root))
            .peekable();
        if known.peek().is_none() {
            return Err(orphan(path));
        }
        check_private_dir(path).map_err(|_| orphan(path))?;
        for (index, entry) in fs::read_dir(path).map_err(|_| orphan(path))?.enumerate() {
            let entry = entry.map_err(|_| orphan(path))?;
            if index >= MAX_POOLS
                || !known
                    .clone()
                    .any(|pool| entry.file_name() == pool.pool_id.as_str())
                || check_private_dir(&entry.path()).is_err()
            {
                return Err(orphan(&entry.path()));
            }
        }
        Ok(())
    };
    checked_namespace(&state.join(LOCK_NAMESPACE), None)?;
    if roots.len() > MAX_POOLS {
        return Err(Refusal::Scope(
            "Too many configured secondary roots to check for orphaned pool paths.".into(),
        ));
    }
    for root in roots {
        if !root.is_absolute() {
            return Err(orphan(root));
        }
        checked_namespace(&root.join(CHECKOUT_NAMESPACE), Some(root))?;
    }
    Ok(())
}

fn check_private_dir(path: &Path) -> io::Result<()> {
    if slots::owner_directory(path)?.mode() & 0o7777 == 0o700 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pool directory must have mode 0700",
        ))
    }
}

fn ensure_private_dir(path: &Path) -> Result<(), Refusal> {
    if exists(path).map_err(|err| path_refusal(path, err))? {
        check_private_dir(path).map_err(|err| path_refusal(path, err))
    } else {
        slots::make_private_directory(path).map_err(|err| path_refusal(path, err))
    }
}

fn provision_pool(
    state: &Path,
    main: &Path,
    root: PathBuf,
    common_git_id: GitIdentity,
    profile: PoolProfile,
) -> Result<Pool, Refusal> {
    let pool_id = slots::random_pool_id().map_err(|err| path_refusal(state, err))?;
    let locks = state.join(LOCK_NAMESPACE);
    let checkout_root = root.join(CHECKOUT_NAMESPACE);
    let lock_dir = locks.join(&pool_id);
    let checkout_dir = checkout_root.join(&pool_id);
    for path in [&lock_dir, &checkout_dir] {
        if exists(path).map_err(|err| path_refusal(path, err))? {
            return Err(Refusal::Orphan(state.join(slots::FILE), path.to_path_buf()));
        }
    }
    ensure_private_dir(&locks)?;
    slots::make_private_directory(&lock_dir).map_err(|err| path_refusal(&lock_dir, err))?;
    ensure_private_dir(&checkout_root)?;
    slots::make_private_directory(&checkout_dir).map_err(|err| path_refusal(&checkout_dir, err))?;

    let unused = |number| {
        let path = lock_dir.join(format!("slot-{number}.lock"));
        slots::new_slot_lock(&path)
            .map(|lock| Slot::unused(number, lock))
            .map_err(|err| path_refusal(&path, err))
    };
    Ok(Pool {
        pool_id,
        main: main.to_path_buf(),
        common_git_id,
        root,
        profile,
        slots: [unused(1)?, unused(2)?],
    })
}

fn pool_directories_present(state: &Path, pool: &Pool) -> bool {
    let locks = state.join(LOCK_NAMESPACE);
    let checkouts = pool.root.join(CHECKOUT_NAMESPACE);
    [
        locks.join(&pool.pool_id),
        locks,
        checkouts.join(&pool.pool_id),
        checkouts,
    ]
    .iter()
    .all(|path| check_private_dir(path).is_ok())
}

fn project_pool(
    command: &'static str,
    kind: &'static str,
    state: &Path,
    pool: &Pool,
    scope_verified: bool,
) -> SlotResult {
    let mut result = SlotResult::new(command, kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.slots = pool
        .slots
        .iter()
        .map(|slot| {
            let path = pool.checkout_path(slot);
            let held = !scope_verified
                || slots::checked_slot_lock(&pool.lock_path(state, slot)).ok()
                    != Some(slot.lock_id)
                || match &slot.checkout_id {
                    None => !matches!(exists(&path), Ok(false)),
                    Some(expected) => {
                        slots::owner_directory(&path)
                            .ok()
                            .as_ref()
                            .map(FileIdentity::of)
                            != Some(expected.directory)
                            || slots::owner_directory(&expected.admin.path)
                                .ok()
                                .as_ref()
                                .map(FileIdentity::of)
                                != Some(expected.admin.file_id())
                    }
                };
            let (phase, action) = if slot.producer.lost() {
                ("held", Some(LOST_PROVENANCE))
            } else if held || matches!(slot.phase, Phase::Held { .. }) {
                ("held", Some(HELD_ACTION))
            } else {
                (
                    slot.phase.name(),
                    match slot.phase {
                        Phase::Vacant => None,
                        Phase::Leased { .. } => {
                            Some("The current holder must resolve work before returning the slot.")
                        }
                        _ => Some(HELD_ACTION),
                    },
                )
            };
            SlotStatus {
                number: slot.number,
                path,
                phase,
                generation: slot.generation,
                next_action: action,
            }
        })
        .collect();
    if result.slots.iter().all(|slot| slot.phase != "vacant") {
        result.next_action = Some(FRESH_FALLBACK);
    }
    result
}
