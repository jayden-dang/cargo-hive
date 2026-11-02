//! The seed command's library entry: the live registry units of a target, the
//! compressed store those units are published into, and the fill that publishes them.

pub mod fill;
pub mod source;
pub mod store;

use std::fmt;

use crate::state::StateError;

#[derive(Debug)]
pub enum Error {
    State(StateError),
    DiskFull(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::State(err) => write!(f, "{err}"),
            Error::DiskFull(why) => f.write_str(&disk_full(why)),
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
            Error::DiskFull(_) => None,
        }
    }
}
