//! Shared domain types: fingerprints, target dirs, liveness results, ledger, caps, receipts, and locks.

#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

pub mod caps;
pub mod classify;
pub mod fingerprint;
pub mod ledger;
pub mod liveness;
pub mod locks;
pub mod receipt;
pub mod size;
pub mod target;
