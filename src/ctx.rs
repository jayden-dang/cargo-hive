//! Shared checkout context and walk helpers used by plan-tier and seed.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::discover::WalkFault;
use crate::platform::Fs;
use crate::registry::Repo;
use crate::state::{Config, State};

pub struct Ctx<'a> {
    pub state: &'a State,
    pub fs: &'a dyn Fs,
    pub cfg: &'a Config,
    pub now: SystemTime,
    pub account_bytes: bool,
    pub repos: &'a [Repo],
    pub mains: &'a [PathBuf],
}

pub(crate) fn checkout_walk(
    ctx: &Ctx<'_>,
) -> (
    Vec<crate::discover::Checkout>,
    Vec<crate::discover::WalkFault>,
) {
    let mut checkouts = Vec::new();
    let mut faults = Vec::new();
    for repo in ctx.repos {
        let (rows, found) = crate::discover::checkouts(repo, ctx.mains, ctx.cfg);
        faults.extend(found);
        checkouts.extend(rows);
    }
    (checkouts, faults)
}

/// Splits checkout-walk faults into those that can hide an owner and those that cannot.
/// A directory under a secondary root that this user cannot enter holds no checkout that
/// any build of this user runs in, so it is only reported. Any other fault can hide one.
pub(crate) fn split_checkout_faults(faults: Vec<WalkFault>) -> (Vec<WalkFault>, Vec<WalkFault>) {
    faults.into_iter().partition(|fault| !fault.cannot_enter)
}

/// A configured cache root that is not on disk is how a home says "do not scan a
/// cache". That absence does not hide an owner. Any other scan fault can.
pub(crate) fn faults_that_can_hide_an_owner(
    faults: Vec<WalkFault>,
    cache_roots: &[PathBuf],
) -> Vec<WalkFault> {
    faults
        .into_iter()
        .filter(|fault| !absent_cache_root(fault, cache_roots))
        .collect()
}

fn absent_cache_root(fault: &WalkFault, cache_roots: &[PathBuf]) -> bool {
    cache_roots.iter().any(|root| root == &fault.path) && fault.detail.contains("os error 2")
}
