//! Bench orchestration: fairness check, sample order, statistics, and timed build shapes.

pub mod check;
pub mod command;
pub mod order;
pub mod stats;

use std::collections::BTreeMap;
use std::time::Duration;

use crate::cargo_target::{run_bounded, Capture, ChildOutputError};
use crate::discover::Checkout;
use crate::doctor::Toolchain;
use crate::plan_tier::Ctx;
use crate::receipt::{Finding, Measured, ReceiptBuilder};
use crate::registry::Repo;
use crate::seed::{self, Fill};

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

fn push_seed_finding(report: &mut Report, checkout: &Checkout, kind: &str, detail: String) {
    report.findings.push(Finding {
        kind: kind.to_string(),
        path: Some(checkout.path.clone()),
        detail,
        measured: BTreeMap::new(),
    });
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

    let prime_started = std::time::Instant::now();
    match seed::run_fill(ctx, &plan.repo, cancel) {
        Ok(Fill::Continue(filled)) => {
            report
                .counts
                .insert("prime_ms".into(), millis(prime_started.elapsed()));
            report
                .counts
                .insert("prime_entries_added".into(), filled.report().entries_added);
            report
                .counts
                .insert("prime_targets_read".into(), filled.report().targets_read);
            for (kind, path, detail) in filled.findings() {
                report.findings.push(Finding {
                    kind: kind.clone(),
                    path: path.clone(),
                    detail: detail.clone(),
                    measured: BTreeMap::new(),
                });
            }
        }
        Ok(Fill::Stopped(_)) => {
            report.end = End::Cancelled;
            return report;
        }
        Err(err) => {
            report.end = End::Failed;
            report.findings.push(Finding {
                kind: "bench.failed".into(),
                path: None,
                detail: format!("prime: {err}"),
                measured: BTreeMap::new(),
            });
            return report;
        }
    }

    let n = plan.cold.len();
    let mut cold_build_ms = Vec::new();
    let mut seeded_build_ms = Vec::new();
    let mut seeded_seed_ms = Vec::new();
    let mut seeded_total_ms = Vec::new();

    for (idx, (arm, i)) in order::abba(n).into_iter().enumerate() {
        let position = idx as u64 + 1;
        if cancel() {
            report.end = End::Cancelled;
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
                                report.end = End::Cancelled;
                                return report;
                            }
                            let cloned_ms = millis(clone_started.elapsed());
                            fill_ms = Some(filled_ms);
                            clone_ms = Some(cloned_ms);
                            seed_ms = Some(filled_ms + cloned_ms);
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
                            report.end = End::Failed;
                            report.findings.push(Finding {
                                kind: "bench.failed".into(),
                                path: Some(checkout.path.clone()),
                                detail: format!("seed of {}: {err}", checkout.path.display()),
                                measured: BTreeMap::new(),
                            });
                            return report;
                        }
                    }
                }
                Ok(Fill::Stopped(_)) => {
                    report.end = End::Cancelled;
                    return report;
                }
                Err(err) => {
                    report.end = End::Failed;
                    report.findings.push(Finding {
                        kind: "bench.failed".into(),
                        path: Some(checkout.path.clone()),
                        detail: format!("seed of {}: {err}", checkout.path.display()),
                        measured: BTreeMap::new(),
                    });
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
                    let tail = String::from_utf8_lossy(&ran.stderr_tail);
                    report.end = End::Failed;
                    report.findings.push(Finding {
                        kind: "bench.failed".into(),
                        path: Some(checkout.path.clone()),
                        detail: format!(
                            "build of {}: exit code {code}; {tail}",
                            checkout.path.display()
                        ),
                        measured: BTreeMap::new(),
                    });
                    return report;
                }
                millis(ran.exited_after)
            }
            Err(ChildOutputError::Cancelled) => {
                report.end = End::Cancelled;
                return report;
            }
            Err(ChildOutputError::TimedOut) => {
                report.end = End::Failed;
                report.findings.push(Finding {
                    kind: "bench.failed".into(),
                    path: Some(checkout.path.clone()),
                    detail: format!(
                        "build of {}: past its deadline of {} ms",
                        checkout.path.display(),
                        millis(limits.build_deadline)
                    ),
                    measured: BTreeMap::new(),
                });
                return report;
            }
            Err(ChildOutputError::Failed(message)) => {
                report.end = End::Failed;
                report.findings.push(Finding {
                    kind: "bench.failed".into(),
                    path: Some(checkout.path.clone()),
                    detail: format!("build of {}: {message}", checkout.path.display()),
                    measured: BTreeMap::new(),
                });
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
