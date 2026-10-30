//! `cargo hive gc`: deletes superseded rustc incremental sessions and leftover rustc temp
//! dirs in Cargo target dirs. A session with a lock file is deleted under that lock; a
//! session with no lock file is deleted without one. Temp dirs are deleted under the
//! profile's `.cargo-build-lock`. Freed bytes are counted per inode and reported as totals.

mod ledger;
mod plan;
mod run;

pub use crate::discover::Scope;
pub use ledger::{Bytes, UNMEASURED_CAP};
pub use plan::{is_temp_entry, MIN_AGE};
pub use run::{measure, run, Counts, Footprint, GcReport, RunOpts, Skip, CRATE_DIR_CAP, DEPS_CAP};
