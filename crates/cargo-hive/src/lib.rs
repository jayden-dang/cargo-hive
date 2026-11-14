//! The binary crate, with command wiring in `cli` and bench wiring in `bench`.
#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

pub mod bench;
pub mod cli;
