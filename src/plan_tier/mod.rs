//! Plan-tier removal inside one directory: a path is deleted only after its canonical
//! parent is shown to lie inside the root and its directory entry is the one expected.
//! Removal does not follow a symbolic link. Canonicalizing the parent is what stops a
//! link from pointing that removal outside the root.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::platform::Fs;
use crate::registry::Repo;
use crate::state::{Config, State};

pub mod apply;
pub mod build;
pub mod guard;
pub mod locks;
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
