//! Process entry for the cargo-hive binary.
#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]

fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();
    std::process::exit(
        cargo_hive::recipe_child(&args).unwrap_or_else(|| cargo_hive::cli::main_with(args)),
    );
}
