//! `cargo hive gc`: deletes superseded rustc incremental sessions and leftover rustc temp
//! dirs in Cargo target dirs, each under its owner's lock, and counts the bytes freed per inode.

mod ledger;
mod plan;
mod run;

pub use ledger::{Bytes, UNMEASURED_CAP};
pub use plan::{is_temp_entry, MIN_AGE};
pub use run::{run, Counts, GcReport, RunOpts, Scope, Skip, CRATE_DIR_CAP, DEPS_CAP};
