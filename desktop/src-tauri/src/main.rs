#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|argument| argument == "--prime-integrity-cache") {
        if ucode_portable_lib::prime_integrity_cache().is_err() {
            std::process::exit(1);
        }
        return;
    }
    ucode_portable_lib::run();
}
