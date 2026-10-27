//! Registered repositories. Task 3 adds `register`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const FILE: &str = "registry.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub repos: Vec<Repo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub main: PathBuf,
    pub root_commits: BTreeSet<String>,
}
