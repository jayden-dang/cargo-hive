//! Read-only proof for invalidating the consumers of a retained producer.

use super::{CheckoutProof, Refusal, check_cancelled, profile};
use cargo_hive_core::{
    caps::CRATE_DIR_CAP,
    fingerprint::{Format, StrictProfileReader, Unit, UnitKind},
    target, unit_path,
};
use cargo_hive_platform::{
    EntryKind, Fs,
    flock::open_owner_state_file,
    host_fs,
    proc::{Capture, ChildOutputError, Spec, run_bounded_complete},
    signals::cancelled,
};
use cargo_hive_state::slots::{
    AnchorWitness, CargoLayout, Consumer, Coverage, FileIdentity, Locator, PoolProfile, Producer,
    object_id,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::Metadata,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn unproved() -> Refusal {
    Refusal::ReturnUncertain(
        "Could not establish the reviewed retained inputs and Cargo invalidation boundary.",
    )
}

/// The effective write configuration must match the packaged guide exactly.
pub(super) fn git_write_scope(
    path: &Path,
    args: &[&str],
    bound: usize,
) -> Result<Vec<u8>, Refusal> {
    let mut spec = Spec::new("/usr/bin/git");
    spec.env_clear = true;
    spec.env_set = [
        ("PATH", "/usr/bin:/bin"),
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_ALLOW_PROTOCOL", ""),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    spec.args.extend(
        [
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-C",
        ]
        .into_iter()
        .map(Into::into),
    );
    spec.args.push(path.as_os_str().into());
    spec.args.extend(args.iter().map(Into::into));
    match run_bounded_complete(
        &spec,
        Duration::from_secs(10),
        Capture {
            stdout_cap: bound + 1,
            stderr_cap: 1,
        },
        &cancelled,
    ) {
        Ok(output)
            if output.status.success()
                && output.stdout.len() <= bound
                && output.stderr_tail.is_empty() =>
        {
            Ok(output.stdout)
        }
        Err(ChildOutputError::Cancelled) => Err(cargo_hive_state::StateError::Cancelled.into()),
        _ => Err(Refusal::ReturnUncertain(
            "Could not verify the guide's bounded Git scope.",
        )),
    }
}

pub(super) fn verify_write_configuration(checkout: &Path) -> Result<(), Refusal> {
    let refused = || {
        Refusal::ReturnUncertain(
            "The guide cannot safely run Git with conditional configuration or external filters.",
        )
    };
    let settings = git_write_scope(
        checkout,
        &["config", "--no-includes", "--null", "--list"],
        super::GIT_OUTPUT_LIMIT,
    )?;
    let records = settings.strip_suffix(b"\0").ok_or_else(refused)?;
    for item in records.split(|&byte| byte == 0) {
        let text = std::str::from_utf8(item).map_err(|_| refused())?;
        let (key, _) = text.split_once('\n').unwrap_or((text, ""));
        let key = key.to_ascii_lowercase();
        if key.is_empty()
            || key.starts_with("include.")
            || key.starts_with("includeif.")
            || key == "extensions.worktreeconfig"
            || key.starts_with("filter.")
        {
            return Err(refused());
        }
    }
    Ok(())
}

pub(super) fn consumers<'a>(
    profile: &'a PoolProfile,
    producer: &Producer,
) -> BTreeMap<PathBuf, Vec<&'a Consumer>> {
    let mut anchors = BTreeMap::<_, Vec<_>>::new();
    let Producer::Complete { invocations, .. } = producer else {
        return anchors;
    };
    let ids = profile
        .mutable_inputs
        .iter()
        .filter_map(|input| match &input.coverage {
            Coverage::Consumers { ids } => Some(ids),
            Coverage::Excluded { .. } => None,
        })
        .flatten()
        .collect::<BTreeSet<_>>();
    for consumer in &profile.consumers {
        if ids.contains(&consumer.id)
            && invocations
                .iter()
                .any(|completed| completed.evidence.invocation == consumer.invocation)
        {
            anchors
                .entry(consumer.anchor.clone())
                .or_default()
                .push(consumer);
        }
    }
    anchors
}

pub(super) fn verify_changes(
    main: &Path,
    prior: &str,
    requested: &str,
    profile: &PoolProfile,
) -> Result<(), Refusal> {
    if prior == requested {
        return Ok(());
    }
    let paths = git_write_scope(
        main,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-renames",
            prior,
            requested,
            "--",
        ],
        super::GIT_OUTPUT_LIMIT,
    )?;
    if paths.is_empty() {
        return Ok(());
    }
    let records = paths.strip_suffix(b"\0").ok_or_else(unproved)?;
    for name in records.split(|&byte| byte == 0) {
        let name = std::str::from_utf8(name).map_err(|_| unproved())?;
        let path = Path::new(name);
        // Keep the first admitted retained change to an exact two-blob Rust source variant.
        // Other mutable inputs can still be invalidated through their reviewed consumer groups.
        if !profile.mutable_inputs.iter().any(|input| matches!(&input.locator, Locator::Checkout { path: allowed } if allowed == path)) {
            return Err(unproved());
        }
        let (prior_mode, prior_object) = endpoint_object(main, prior, name)?;
        let (requested_mode, requested_object) = endpoint_object(main, requested, name)?;
        if prior_mode != requested_mode {
            return Err(unproved());
        }
        let mut candidates = profile
            .consumers
            .iter()
            .filter(|consumer| consumer.anchor == path);
        let matches = |consumer: &Consumer| {
            consumer.prior_object == prior_object && consumer.requested_object == requested_object
        };
        if !candidates.next().is_some_and(matches) || !candidates.all(matches) {
            return Err(unproved());
        }
    }
    Ok(())
}

fn endpoint_object(main: &Path, endpoint: &str, path: &str) -> Result<(u32, String), Refusal> {
    let raw = git_write_scope(
        main,
        &["--literal-pathspecs", "ls-tree", "-z", endpoint, "--", path],
        super::GIT_OUTPUT_LIMIT,
    )?;
    let row = raw
        .strip_suffix(b"\0")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or_else(unproved)?;
    let (fields, name) = row.split_once('\t').ok_or_else(unproved)?;
    let mut fields = fields.split(' ');
    match (fields.next(), fields.next(), fields.next(), fields.next()) {
        (Some(mode @ ("100644" | "100755")), Some("blob"), Some(oid), None)
            if name == path && object_id(oid) =>
        {
            let mode = u32::from_str_radix(mode, 8).map_err(|_| unproved())?;
            Ok((mode, oid.to_owned()))
        }
        _ => Err(unproved()),
    }
}

fn now() -> Result<(i64, i64), Refusal> {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| unproved())?;
    Ok((
        i64::try_from(time.as_secs()).map_err(|_| unproved())?,
        i64::from(time.subsec_nanos()),
    ))
}

fn mtime(metadata: &Metadata) -> (i64, i64) {
    (metadata.mtime(), metadata.mtime_nsec())
}

fn role(kind: UnitKind, role: &str) -> bool {
    kind.as_str() == role || (kind == UnitKind::Lib && role == "library")
}

fn unit_names_anchor(
    fs: &dyn Fs,
    directory: &Path,
    unit: &Unit,
    checkout: &Path,
    anchor: &Path,
) -> Result<bool, Refusal> {
    let crate_name = unit
        .stem
        .strip_prefix(unit.kind.as_str())
        .and_then(|name| name.strip_prefix('-'))
        .ok_or_else(unproved)?;
    if crate_name.is_empty()
        || !crate_name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
    {
        return Err(unproved());
    }
    let output = unit_path::outputs(directory, &unit.loc);
    let hashed = output.join(format!("{crate_name}-{}.d", unit.loc.hex()));
    let dep_info = if unit.loc.layout == unit_path::UnitLayout::Flat {
        hashed
    } else {
        let regular = |path: &Path| match fs.entry_stat(path) {
            Ok(stat) if stat.kind == EntryKind::File => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            _ => Err(unproved()),
        };
        let plain = output.join(format!("{crate_name}.d"));
        match (regular(&hashed)?, regular(&plain)?) {
            (true, false) => hashed,
            (false, true) => plain,
            _ => return Err(unproved()),
        }
    };
    let bytes = fs
        .read_regular_bounded(&dep_info, 1024 * 1024)
        .map_err(|_| unproved())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| unproved())?;
    let (_, inputs) = text
        .lines()
        .next()
        .and_then(|line| line.split_once(": "))
        .ok_or_else(unproved)?;
    let absolute = checkout.join(anchor);
    for path in [anchor, absolute.as_path()] {
        let expected = path.to_str().ok_or_else(unproved)?.replace(' ', "\\ ");
        if inputs
            .strip_prefix(&expected)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn floors(
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    producer: &Producer,
) -> Result<BTreeMap<PathBuf, (i64, i64)>, Refusal> {
    let Producer::Complete { roots, .. } = producer else {
        return Ok(BTreeMap::new());
    };
    let anchors = consumers(profile, producer);
    if anchors.is_empty() {
        return Ok(BTreeMap::new());
    }
    let fs = host_fs();
    let profiles =
        target::identify_strict(fs.as_ref(), &roots.build.path, CRATE_DIR_CAP, &cancelled)
            .map_err(|_| unproved())?;
    let reader = StrictProfileReader::new(fs.as_ref(), &cancelled);
    let mut witnessed = BTreeMap::new();
    let mut by_consumer = BTreeMap::new();
    let mut by_package = BTreeMap::<_, Vec<_>>::new();
    for consumer in anchors.values().flatten() {
        by_package
            .entry(consumer.package.as_str())
            .or_default()
            .push(*consumer);
    }
    for (directory, _) in profiles {
        check_cancelled()?;
        let report = reader.read_profile(&directory);
        if report.format != Format::Supported
            || report.partial_unit_dirs != 0
            || report.not_current != 0
        {
            return Err(unproved());
        }
        let graph = report.graph.ok_or_else(unproved)?;
        for unit in graph.units {
            let Some(consumers) = by_package.get(unit.loc.package()) else {
                continue;
            };
            for consumer in consumers
                .iter()
                .filter(|consumer| role(unit.kind, &consumer.role))
            {
                if !unit_names_anchor(
                    fs.as_ref(),
                    &directory,
                    &unit,
                    &checkout.path,
                    &consumer.anchor,
                )? {
                    continue;
                }
                let invocation = profile
                    .invocations
                    .iter()
                    .find(|invocation| invocation.id == consumer.invocation)
                    .ok_or_else(unproved)?;
                let layout = match invocation.layout {
                    CargoLayout::Flat => unit_path::UnitLayout::Flat,
                    CargoLayout::UnitDir => unit_path::UnitLayout::UnitDir,
                };
                if unit.loc.layout != layout {
                    return Err(unproved());
                }
                let stamp_path = unit_path::stamp(&directory, &unit.loc, unit.kind);
                let file = open_owner_state_file(&stamp_path).map_err(|_| unproved())?;
                let stamp = mtime(&file.metadata().map_err(|_| unproved())?);
                if !stamp_path.starts_with(&roots.build.path) || stamp.0 <= 0 || stamp > now()? {
                    return Err(unproved());
                }
                by_consumer
                    .entry(&consumer.id)
                    .and_modify(|seen: &mut (i64, i64)| *seen = (*seen).max(stamp))
                    .or_insert(stamp);
            }
        }
    }
    for (path, consumers) in anchors {
        if checkout.path.join(&path).starts_with(&roots.build.path) {
            return Err(unproved());
        }
        let mut floor = (0, 0);
        for consumer in consumers {
            floor = floor.max(*by_consumer.get(&consumer.id).ok_or_else(unproved)?);
        }
        witnessed.insert(path, floor);
    }
    Ok(witnessed)
}

fn anchor(checkout: &CheckoutProof, path: &Path) -> Result<(Metadata, String), Refusal> {
    let absolute = checkout.path.join(path);
    let file = open_owner_state_file(&absolute).map_err(|_| unproved())?;
    let meta = file.metadata().map_err(|_| unproved())?;
    if absolute.canonicalize().as_ref().ok() != Some(&absolute) {
        return Err(unproved());
    }
    let object = profile::tracked_object(checkout, path)?;
    let bytes = profile::read_input(&absolute)?.ok_or_else(unproved)?;
    if bytes != profile::blob(checkout, &object)? {
        return Err(unproved());
    }
    let actual = std::fs::symlink_metadata(absolute).map_err(|_| unproved())?;
    if super::directory_stamp(&meta) != super::directory_stamp(&actual) {
        return Err(unproved());
    }
    Ok((meta, object))
}

pub(super) fn witness(
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    producer: &Producer,
) -> Result<Vec<AnchorWitness>, Refusal> {
    floors(checkout, profile, producer)?
        .into_iter()
        .map(|(path, prior_unit)| {
            let (meta, object) = anchor(checkout, &path)?;
            let current = now()?;
            if mtime(&meta) > current {
                return Err(unproved());
            }
            Ok(AnchorWitness {
                path,
                identity: FileIdentity::of(&meta),
                object,
                mode: meta.mode(),
                prior_unit,
                floor: current.max(prior_unit),
                touched: None,
            })
        })
        .collect()
}

pub(super) fn observe_touch(
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    producer: &Producer,
    witnesses: &[AnchorWitness],
) -> Result<Vec<AnchorWitness>, Refusal> {
    let floors = floors(checkout, profile, producer)?;
    if floors.len() != witnesses.len()
        || witnesses
            .iter()
            .any(|proof| floors.get(&proof.path) != Some(&proof.prior_unit))
    {
        return Err(unproved());
    }
    witnesses
        .iter()
        .map(|proof| {
            let timestamp = verify_one(checkout, proof)?;
            let mut completed = proof.clone();
            completed.touched = Some(timestamp);
            Ok(completed)
        })
        .collect()
}

pub(super) fn verify_touch(
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    producer: &Producer,
    witnesses: &[AnchorWitness],
) -> Result<(), Refusal> {
    let expected = consumers(profile, producer);
    if expected.len() != witnesses.len() {
        return Err(unproved());
    }
    for proof in witnesses {
        if !expected.contains_key(&proof.path)
            || proof.touched != Some(verify_one(checkout, proof)?)
        {
            return Err(unproved());
        }
    }
    Ok(())
}

fn verify_one(checkout: &CheckoutProof, proof: &AnchorWitness) -> Result<(i64, i64), Refusal> {
    let (meta, object) = anchor(checkout, &proof.path)?;
    let stamp = mtime(&meta);
    if proof.identity != FileIdentity::of(&meta)
        || proof.object != object
        || proof.mode != meta.mode()
        || stamp <= proof.floor
        || stamp > now()?
    {
        return Err(unproved());
    }
    Ok(stamp)
}
