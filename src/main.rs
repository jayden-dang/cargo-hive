fn main() {
    std::process::exit(cargo_hive::cli::main_with(std::env::args_os().collect()));
}
