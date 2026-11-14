//! Directory listing bounds shared by seed, gc, explain, and locks.

/// Most entries listed from one `incremental/` level, crate dir or scanned directory.
pub const CRATE_DIR_CAP: usize = 100_000;
/// Most entries listed from one `deps/` directory.
pub const DEPS_CAP: usize = 1_000_000;
