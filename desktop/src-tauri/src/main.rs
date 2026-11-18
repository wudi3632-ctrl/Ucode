#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if let Some(exit_code) = ucode_portable_lib::rescue_elevated::maybe_run_helper(&arguments) {
        std::process::exit(exit_code);
    }
    if arguments
        .iter()
        .any(|argument| argument == "--prime-integrity-cache")
    {
        if ucode_portable_lib::prime_integrity_cache().is_err() {
            std::process::exit(1);
        }
        return;
    }
    ucode_portable_lib::run();
}
