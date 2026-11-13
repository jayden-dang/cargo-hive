#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

pub mod bench;
pub mod cli;
pub mod ctx;
pub mod discover;
pub mod doctor;
pub mod env;
pub mod explain;
pub mod gc;
pub mod git;
pub mod metadata;
pub mod plan_tier;
pub mod registry;
pub mod seed;
pub mod toolchain;
