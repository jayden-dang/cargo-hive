//! Refuse an unfair set of checkouts before anything builds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::locate_checkout;
use crate::discover::Checkout;
use crate::doctor::Capabilities;
use crate::git::Git;
use crate::registry::{Registry, Repo};
use crate::toolchain::Toolchain;
use cargo_hive_state::Config;

pub struct Checked {
    pub repo: Repo,
    pub cold: Vec<Checkout>,
    pub seeded: Vec<Checkout>,
    pub toolchain: Toolchain,
}

pub struct Unfair {
    pub path: Option<PathBuf>,
    pub reason: String,
}

pub enum CheckStop {
    Unfair(Vec<Unfair>),
    Cancelled,
}

pub fn check(
    reg: &Registry,
    cfg: &Config,
    caps: &Capabilities,
    cold: &[PathBuf],
    seeded: &[PathBuf],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Checked, CheckStop> {
    let mut unfair = Vec::new();

    if cold.len() < 2 || seeded.len() < 2 {
        unfair.push(Unfair {
            path: None,
            reason: "each arm needs at least 2 checkouts".into(),
        });
    }
    if cold.len() != seeded.len() {
        unfair.push(Unfair {
            path: None,
            reason: "the two arms must have equal counts".into(),
        });
    }

    let mut counts: BTreeMap<&Path, usize> = BTreeMap::new();
    for path in cold.iter().chain(seeded.iter()) {
        *counts.entry(path.as_path()).or_insert(0) += 1;
    }
    for (path, n) in &counts {
        if *n > 1 {
            unfair.push(Unfair {
                path: Some(path.to_path_buf()),
                reason: format!("{} is given more than once", path.display()),
            });
        }
    }

    let mut cold_ok: Vec<(Repo, Checkout, Toolchain)> = Vec::new();
    let mut seeded_ok: Vec<(Repo, Checkout, Toolchain)> = Vec::new();
    locate_arm(reg, cfg, caps, cold, &mut cold_ok, &mut unfair)?;
    locate_arm(reg, cfg, caps, seeded, &mut seeded_ok, &mut unfair)?;

    let located: Vec<&(Repo, Checkout, Toolchain)> =
        cold_ok.iter().chain(seeded_ok.iter()).collect();
    if let Some((first_repo, first_checkout, first_tc)) = located.first() {
        let expected_main = &first_repo.main;
        let expected_head = match Git::at(&first_checkout.path).head() {
            Ok(h) => Some(h),
            Err(err) => {
                unfair.push(Unfair {
                    path: Some(first_checkout.path.clone()),
                    reason: err.to_string(),
                });
                None
            }
        };
        let expected_version = &first_tc.version_text;

        for (repo, checkout, tc) in located.iter().skip(1) {
            if &repo.main != expected_main {
                unfair.push(Unfair {
                    path: Some(checkout.path.clone()),
                    reason: format!(
                        "{} belongs to {}, not {}",
                        checkout.path.display(),
                        repo.main.display(),
                        expected_main.display()
                    ),
                });
            }
            if let Some(ref want) = expected_head {
                match Git::at(&checkout.path).head() {
                    Ok(got) if got != *want => {
                        unfair.push(Unfair {
                            path: Some(checkout.path.clone()),
                            reason: format!(
                                "{} is at HEAD {}, not {}",
                                checkout.path.display(),
                                got,
                                want
                            ),
                        });
                    }
                    Ok(_) => {}
                    Err(err) => unfair.push(Unfair {
                        path: Some(checkout.path.clone()),
                        reason: err.to_string(),
                    }),
                }
            }
            if tc.version_text != *expected_version {
                let first_line = tc.version_text.lines().next().unwrap_or("");
                unfair.push(Unfair {
                    path: Some(checkout.path.clone()),
                    reason: format!(
                        "{} resolves a different toolchain: {first_line}",
                        checkout.path.display()
                    ),
                });
            }
        }

        for (_repo, checkout, _tc) in &located {
            match Git::at(&checkout.path).has_tracked_changes() {
                Ok(true) => unfair.push(Unfair {
                    path: Some(checkout.path.clone()),
                    reason: format!(
                        "{} has uncommitted changes to tracked files",
                        checkout.path.display()
                    ),
                }),
                Ok(false) => {}
                Err(err) => unfair.push(Unfair {
                    path: Some(checkout.path.clone()),
                    reason: err.to_string(),
                }),
            }
        }
    }

    let mut seen_targets: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for (_repo, checkout, _tc) in cold_ok.iter().chain(seeded_ok.iter()) {
        let mut cancelled = || cancel();
        match crate::metadata::resolve(&checkout.path, &mut cancelled) {
            Ok(resolved) => {
                if resolved.target_directory.exists() {
                    unfair.push(Unfair {
                        path: Some(checkout.path.clone()),
                        reason: format!(
                            "the target directory {} already exists",
                            resolved.target_directory.display()
                        ),
                    });
                }
                if resolved.build_directory != resolved.target_directory {
                    unfair.push(Unfair {
                        path: Some(checkout.path.clone()),
                        reason: format!(
                            "the build directory {} differs from the target directory {}",
                            resolved.build_directory.display(),
                            resolved.target_directory.display()
                        ),
                    });
                }
                if !resolved.workspace_root.join("Cargo.lock").is_file() {
                    unfair.push(Unfair {
                        path: Some(checkout.path.clone()),
                        reason: format!(
                            "{} has no Cargo.lock; bench builds --locked",
                            resolved.workspace_root.display()
                        ),
                    });
                }
                if let Some(other) = seen_targets.get(&resolved.target_directory) {
                    unfair.push(Unfair {
                        path: Some(checkout.path.clone()),
                        reason: format!(
                            "{} shares the target directory {} with {}",
                            checkout.path.display(),
                            resolved.target_directory.display(),
                            other.display()
                        ),
                    });
                } else {
                    seen_targets.insert(resolved.target_directory.clone(), checkout.path.clone());
                }
            }
            Err(err) if err == "cargo metadata was cancelled" => {
                return Err(CheckStop::Cancelled);
            }
            Err(err) => unfair.push(Unfair {
                path: Some(checkout.path.clone()),
                reason: err,
            }),
        }
    }

    if !unfair.is_empty() {
        return Err(CheckStop::Unfair(unfair));
    }

    let (repo, _, toolchain) = cold_ok
        .first()
        .cloned()
        .expect("arm-size rules require at least one located checkout on success");
    Ok(Checked {
        repo,
        cold: cold_ok.into_iter().map(|(_, c, _)| c).collect(),
        seeded: seeded_ok.into_iter().map(|(_, c, _)| c).collect(),
        toolchain,
    })
}

fn locate_arm(
    reg: &Registry,
    cfg: &Config,
    caps: &Capabilities,
    paths: &[PathBuf],
    dest: &mut Vec<(Repo, Checkout, Toolchain)>,
    unfair: &mut Vec<Unfair>,
) -> Result<(), CheckStop> {
    for path in paths {
        match locate_checkout(reg, cfg, caps, path) {
            Ok(row) => dest.push(row),
            Err(r) if r.code == 130 => return Err(CheckStop::Cancelled),
            Err(r) => unfair.push(Unfair {
                path: r.path,
                reason: r.detail,
            }),
        }
    }
    Ok(())
}
