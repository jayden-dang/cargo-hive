//! The seed command's library entry: the live registry units of a target, the
//! compressed store those units are published into, the fill that publishes them,
//! the choice of units for one repo, and the placement of those units into a
//! target that does not exist yet.

pub mod choose;
pub mod fill;
pub mod place;
pub mod source;
pub mod store;

use std::fmt;

use crate::state::StateError;

#[derive(Debug)]
pub enum Error {
    State(StateError),
    DiskFull(String),
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::State(err) => write!(f, "{err}"),
            Error::DiskFull(why) => f.write_str(&disk_full(why)),
            Error::Failed(why) => f.write_str(why),
        }
    }
}

pub(crate) fn disk_full(why: &str) -> String {
    format!("the disk is full ({why}); free space and run the command again")
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::State(err) => Some(err),
            Error::DiskFull(_) | Error::Failed(_) => None,
        }
    }
}
