//! `cargo hive gc`: deletes superseded rustc incremental sessions and leftover rustc temp
//! dirs in Cargo target dirs, each under its owner's lock, and counts the bytes freed per inode.

#[cfg_attr(not(test), allow(dead_code))]
mod ledger;
#[cfg_attr(not(test), allow(dead_code))]
mod plan;

pub use ledger::{Bytes, UNMEASURED_CAP};
pub use plan::{is_temp_entry, MIN_AGE};
