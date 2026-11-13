//! GC, plan-tier removal, explain, and the liveness algorithm that marks units live or dead.

#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

pub mod explain;
pub mod gc;
pub mod liveness;
pub mod plan_tier;
