//! Bench orchestration: fairness check, sample order, statistics, and timed build shapes.

pub mod check;
pub mod command;
pub mod order;
pub mod stats;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::cargo_target::{run_bounded, Capture, ChildOutputError};
use crate::discover::{self, Checkout, Snapshot};
use crate::doctor::Toolchain;
use crate::gc;
use crate::plan_tier::Ctx;
use crate::receipt::{Finding, Measured, ReceiptBuilder};
use crate::registry::Repo;
use crate::seed::{self, record, Fill};

use self::command::{BuildCmd, EnvMap};
use self::order::Arm;

pub const BUILD_DEADLINE: Duration = Duration::from_secs(60 * 60);

pub struct Plan {
    pub repo: Repo,
    pub cold: Vec<Checkout>,
    pub seeded: Vec<Checkout>,
    pub toolchain: Toolchain,
    pub build: BuildCmd,
    pub env: EnvMap,
}

pub struct Limits {
    pub build_deadline: Duration,
}

pub enum End {
    Finished,
    Failed,
    Cancelled,
}

pub struct Report {
    pub end: End,
    pub counts: BTreeMap<String, u64>,
    pub findings: Vec<Finding>,
    pub unmeasured: Vec<(String, String)>,
}

impl Report {
    pub fn write_into(&self, b: &mut ReceiptBuilder) {
        for (name, n) in &self.counts {
            b.count(name, *n);
        }
        for finding in &self.findings {
            b.finding_measured(
                &finding.kind,
                finding.path.clone(),
                finding.detail.clone(),
                finding.measured.clone(),
            );
        }
        for (field, reason) in &self.unmeasured {
            b.unmeasured(field, reason.clone());
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self.end {
            End::Finished => 0,
            End::Failed => 1,
            End::Cancelled => 130,
        }
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

const DETAIL_STDERR_CAP: usize = 4 * 1024;

fn stderr_for_detail(tail: &[u8]) -> String {
    let start = tail.len().saturating_sub(DETAIL_STDERR_CAP);
    String::from_utf8_lossy(&tail[start..]).into_owned()
}

fn mark_remaining(report: &mut Report, from_position: u64, sample_count: u64, reason: &str) {
    for position in from_position..=sample_count {
        report
            .unmeasured
            .push((format!("sample.{position}"), reason.into()));
    }
}

fn fail_run(report: &mut Report, from_position: u64, sample_count: u64, finding: Finding) {
    report.end = End::Failed;
    report.findings.push(finding);
    mark_remaining(
        report,
        from_position,
        sample_count,
        "not run: an earlier step failed",
    );
}

fn cancel_run(report: &mut Report, from_position: u64, sample_count: u64) {
    report.end = End::Cancelled;
    mark_remaining(report, from_position, sample_count, "cancelled");
}

fn record_prime(
    report: &mut Report,
    started: std::time::Instant,
    entries_added: u64,
    targets_read: u64,
    findings: &[(String, Option<PathBuf>, String)],
) {
    report
        .counts
        .insert("prime_ms".into(), millis(started.elapsed()));
    report
        .counts
        .insert("prime_entries_added".into(), entries_added);
    report
        .counts
        .insert("prime_targets_read".into(), targets_read);
    for (kind, path, detail) in findings {
        report.findings.push(Finding {
            kind: kind.clone(),
            path: path.clone(),
            detail: detail.clone(),
            measured: BTreeMap::new(),
        });
    }
}

fn push_seed_finding(report: &mut Report, checkout: &Checkout, kind: &str, detail: String) {
    report.findings.push(Finding {
        kind: kind.to_string(),
        path: Some(checkout.path.clone()),
        detail,
        measured: BTreeMap::new(),
    });
}

fn attach_seed_counts(
    report: &mut Report,
    measured: &mut BTreeMap<String, Measured>,
    ctx: &Ctx<'_>,
    checkout: &Checkout,
    seed_target: Option<&PathBuf>,
    position: u64,
    cancel: &(dyn Fn() -> bool + Sync),
) {
    let field = format!("sample.{position}.seed_counts");
    let Some(target_path) = seed_target else {
        report
            .unmeasured
            .push((field, "the seed cloned no target".into()));
        return;
    };
    let seeds = match record::read(ctx.state) {
        Ok(file) => file,
        Err(err) => {
            report.unmeasured.push((field, err.to_string()));
            return;
        }
    };
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(
        std::slice::from_ref(checkout),
        ctx.cfg,
        false,
        &mut snapshot,
        cancel,
    );
    let Some(target) = scan.targets.iter().find(|t| &t.path == target_path) else {
        report.unmeasured.push((field, "no seed record".into()));
        return;
    };
    let Some(rec) = seeds.targets.get(&target.path) else {
        report.unmeasured.push((field, "no seed record".into()));
        return;
    };
    match record::counts(target, rec, gc::DEPS_CAP) {
        Ok(Some(c)) => {
            measured.insert("seeded_fresh".into(), Measured::Int(c.fresh));
            measured.insert("seeded_unneeded".into(), Measured::Int(c.unneeded));
            measured.insert("seeded_recompiled".into(), Measured::Int(c.recompiled));
            measured.insert("seeded_missing".into(), Measured::Int(c.missing));
        }
        Ok(None) => {
            report.unmeasured.push((
                field,
                "the target directory is not the one the record was written for".into(),
            ));
        }
        Err(why) => {
            report.unmeasured.push((field, why));
        }
    }
}

pub fn run(
    ctx: &Ctx<'_>,
    plan: &Plan,
    limits: Limits,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Report {
    let mut report = Report {
        end: End::Finished,
        counts: BTreeMap::new(),
        findings: Vec::new(),
        unmeasured: Vec::new(),
    };
    let sample_count = (plan.cold.len() + plan.seeded.len()) as u64;

    let prime_started = std::time::Instant::now();
    match seed::run_fill(ctx, &plan.repo, cancel) {
        Ok(Fill::Continue(filled)) => {
            record_prime(
                &mut report,
                prime_started,
                filled.report().entries_added,
                filled.report().targets_read,
                filled.findings(),
            );
        }
        Ok(Fill::Stopped(outcome)) => {
            record_prime(
                &mut report,
                prime_started,
                outcome
                    .counts
                    .get("fill_entries_added")
                    .copied()
                    .unwrap_or(0),
                outcome
                    .counts
                    .get("fill_targets_read")
                    .copied()
                    .unwrap_or(0),
                &outcome.findings,
            );
            cancel_run(&mut report, 1, sample_count);
            return report;
        }
        Err(err) => {
            fail_run(
                &mut report,
                1,
                sample_count,
                Finding {
                    kind: "bench.failed".into(),
                    path: None,
                    detail: format!("prime: {err}"),
                    measured: BTreeMap::new(),
                },
            );
            return report;
        }
    }

    let store_lists_repo = seed::store::read(ctx.state).ok().is_some_and(|store| {
        store
            .units
            .iter()
            .any(|row| row.repos.iter().any(|p| p == &plan.repo.main))
    });
    if !store_lists_repo {
        fail_run(
            &mut report,
            1,
            sample_count,
            Finding {
                kind: "bench.nothing_to_seed".into(),
                path: None,
                detail: format!(
                    "no stored unit lists {}; build it once with {}, then bench again",
                    plan.repo.main.display(),
                    plan.build.command_line()
                ),
                measured: BTreeMap::new(),
            },
        );
        return report;
    }

    let n = plan.cold.len();
    let mut cold_build_ms = Vec::new();
    let mut seeded_build_ms = Vec::new();
    let mut seeded_seed_ms = Vec::new();
    let mut seeded_total_ms = Vec::new();

    for (idx, (arm, i)) in order::abba(n).into_iter().enumerate() {
        let position = idx as u64 + 1;
        if cancel() {
            cancel_run(&mut report, position, sample_count);
            return report;
        }

        let checkout = match arm {
            Arm::Cold => &plan.cold[i],
            Arm::Seeded => &plan.seeded[i],
        };

        let load_before = ctx.fs.load_average_hundredths();

        let mut fill_ms = None;
        let mut clone_ms = None;
        let mut seed_ms = None;
        let mut fill_targets_read = None;
        let mut seed_target = None;

        if matches!(arm, Arm::Seeded) {
            let fill_started = std::time::Instant::now();
            match seed::run_fill(ctx, &plan.repo, cancel) {
                Ok(Fill::Continue(filled)) => {
                    let filled_ms = millis(fill_started.elapsed());
                    fill_targets_read = Some(filled.report().targets_read);
                    let clone_started = std::time::Instant::now();
                    match seed::run_clone(
                        ctx,
                        &plan.repo,
                        checkout,
                        &plan.toolchain,
                        filled,
                        cancel,
                    ) {
                        Ok(outcome) => {
                            if outcome.cancelled {
                                cancel_run(&mut report, position, sample_count);
                                return report;
                            }
                            let cloned_ms = millis(clone_started.elapsed());
                            fill_ms = Some(filled_ms);
                            clone_ms = Some(cloned_ms);
                            seed_ms = Some(filled_ms + cloned_ms);
                            seed_target = outcome.target.map(|(path, _)| path);
                            for (kind, _, detail) in outcome.findings {
                                push_seed_finding(&mut report, checkout, &kind, detail);
                            }
                        }
                        Err(err) => {
                            if let seed::Error::DiskFull(full) = &err {
                                if let Some(kept) = &full.kept {
                                    for (kind, _, detail) in &kept.findings {
                                        push_seed_finding(
                                            &mut report,
                                            checkout,
                                            kind,
                                            detail.clone(),
                                        );
                                    }
                                }
                            }
                            fail_run(
                                &mut report,
                                position + 1,
                                sample_count,
                                Finding {
                                    kind: "bench.failed".into(),
                                    path: Some(checkout.path.clone()),
                                    detail: format!("seed of {}: {err}", checkout.path.display()),
                                    measured: BTreeMap::new(),
                                },
                            );
                            return report;
                        }
                    }
                }
                Ok(Fill::Stopped(_)) => {
                    cancel_run(&mut report, position, sample_count);
                    return report;
                }
                Err(err) => {
                    fail_run(
                        &mut report,
                        position + 1,
                        sample_count,
                        Finding {
                            kind: "bench.failed".into(),
                            path: Some(checkout.path.clone()),
                            detail: format!("seed of {}: {err}", checkout.path.display()),
                            measured: BTreeMap::new(),
                        },
                    );
                    return report;
                }
            }
        }

        let mut cmd = plan
            .env
            .command(&plan.toolchain, &checkout.path, &plan.build);
        let build_ms = match run_bounded(
            &mut cmd,
            limits.build_deadline,
            Capture {
                stdout_cap: 0,
                stderr_cap: 64 * 1024,
            },
            cancel,
        ) {
            Ok(ran) => {
                if !ran.status.success() {
                    let code = ran.status.code().unwrap_or(-1);
                    let tail = stderr_for_detail(&ran.stderr_tail);
                    fail_run(
                        &mut report,
                        position + 1,
                        sample_count,
                        Finding {
                            kind: "bench.failed".into(),
                            path: Some(checkout.path.clone()),
                            detail: format!(
                                "build of {}: exit code {code}; {tail}",
                                checkout.path.display()
                            ),
                            measured: BTreeMap::new(),
                        },
                    );
                    return report;
                }
                millis(ran.exited_after)
            }
            Err(ChildOutputError::Cancelled) => {
                cancel_run(&mut report, position, sample_count);
                return report;
            }
            Err(ChildOutputError::TimedOut) => {
                fail_run(
                    &mut report,
                    position + 1,
                    sample_count,
                    Finding {
                        kind: "bench.failed".into(),
                        path: Some(checkout.path.clone()),
                        detail: format!(
                            "build of {}: past its deadline of {} ms",
                            checkout.path.display(),
                            millis(limits.build_deadline)
                        ),
                        measured: BTreeMap::new(),
                    },
                );
                return report;
            }
            Err(ChildOutputError::Failed(message)) => {
                fail_run(
                    &mut report,
                    position + 1,
                    sample_count,
                    Finding {
                        kind: "bench.failed".into(),
                        path: Some(checkout.path.clone()),
                        detail: format!("build of {}: {message}", checkout.path.display()),
                        measured: BTreeMap::new(),
                    },
                );
                return report;
            }
        };

        let load_after = ctx.fs.load_average_hundredths();

        let mut measured = BTreeMap::new();
        measured.insert(
            "arm".into(),
            Measured::Text(
                match arm {
                    Arm::Cold => "cold",
                    Arm::Seeded => "seeded",
                }
                .into(),
            ),
        );
        measured.insert("position".into(), Measured::Int(position));
        measured.insert("build_ms".into(), Measured::Int(build_ms));
        match load_before {
            Ok(v) => {
                measured.insert("load_before_x100".into(), Measured::Int(v));
            }
            Err(err) => {
                report
                    .unmeasured
                    .push((format!("sample.{position}.load_before"), err.to_string()));
            }
        }
        match load_after {
            Ok(v) => {
                measured.insert("load_after_x100".into(), Measured::Int(v));
            }
            Err(err) => {
                report
                    .unmeasured
                    .push((format!("sample.{position}.load_after"), err.to_string()));
            }
        }

        match arm {
            Arm::Cold => cold_build_ms.push(build_ms),
            Arm::Seeded => {
                let seed = seed_ms.expect("seeded sample records seed_ms");
                let fill = fill_ms.expect("seeded sample records fill_ms");
                let clone = clone_ms.expect("seeded sample records clone_ms");
                let targets = fill_targets_read.expect("seeded sample records fill_targets_read");
                measured.insert("seed_ms".into(), Measured::Int(seed));
                measured.insert("fill_ms".into(), Measured::Int(fill));
                measured.insert("clone_ms".into(), Measured::Int(clone));
                measured.insert("fill_targets_read".into(), Measured::Int(targets));
                attach_seed_counts(
                    &mut report,
                    &mut measured,
                    ctx,
                    checkout,
                    seed_target.as_ref(),
                    position,
                    cancel,
                );
                seeded_build_ms.push(build_ms);
                seeded_seed_ms.push(seed);
                seeded_total_ms.push(seed + build_ms);
            }
        }

        report.findings.push(Finding {
            kind: "bench.sample".into(),
            path: Some(checkout.path.clone()),
            detail: String::new(),
            measured,
        });
    }

    for (series, values) in [
        ("cold_build_ms", &cold_build_ms),
        ("seeded_build_ms", &seeded_build_ms),
        ("seeded_seed_ms", &seeded_seed_ms),
        ("seeded_total_ms", &seeded_total_ms),
    ] {
        if let Some(summary) = stats::summarize(values) {
            report
                .counts
                .insert(format!("{series}_median"), summary.median);
            report.counts.insert(format!("{series}_p90"), summary.p90);
            report.counts.insert(format!("{series}_min"), summary.min);
            report.counts.insert(format!("{series}_max"), summary.max);
            report
                .counts
                .insert(format!("{series}_spread"), summary.spread);
        }
    }

    report
}
