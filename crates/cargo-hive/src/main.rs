//! Process entry for the cargo-hive binary.
#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

fn main() {
    std::process::exit(cargo_hive::cli::main_with(std::env::args_os().collect()));
}
