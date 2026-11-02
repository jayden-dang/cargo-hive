//! Plan-tier removal inside one directory: a path is deleted only after its canonical
//! parent is shown to lie inside the root and its directory entry is the one expected.
//! Removal does not follow a symbolic link. Canonicalizing the parent is what stops a
//! link from pointing that removal outside the root.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::discover::WalkFault;
use crate::platform::Fs;
use crate::registry::Repo;
use crate::state::{Config, State};

pub mod apply;
pub mod build;
pub mod guard;
pub mod locks;
pub mod orphan;
pub mod record;

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
