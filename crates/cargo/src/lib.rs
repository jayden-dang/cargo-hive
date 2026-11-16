//! Cargo and git child processes, registry, discovery, toolchain, and shared ctx.

#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

pub mod ctx;
pub mod discover;
pub mod env;
pub mod git;
pub mod metadata;
mod object_store;
pub mod registry;
pub mod toolchain;
