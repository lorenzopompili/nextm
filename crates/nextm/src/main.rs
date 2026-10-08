//! nextm: indicatore di sistema leggerissimo per Windows 11.
#![windows_subsystem = "windows"]

#[macro_use]
mod wide;

mod app;
mod cli;
mod diagnose;
mod ini;
mod settings;
mod strings;
mod sys;
mod tooltip;

fn main() {
    sys::harden();
    if sys::elevation::is_elevated() {
        sys::elevation::enable_debug_privilege();
    }
    // Gli argomenti si gestiscono prima del controllo di istanza singola:
    // `--version`, `--diagnose` e `--quit` funzionano anche con nextm già in esecuzione.
    match cli::parse(std::env::args().skip(1)) {
        cli::Command::Version => {
            let text = format!("nextm {}\n", env!("CARGO_PKG_VERSION"));
            if !sys::console::write_to_parent_console(&text) {
                let body: Vec<u16> = text.encode_utf16().chain(core::iter::once(0)).collect();
                sys::shell::message_box(core::ptr::null_mut(), wide!("nextm"), &body);
            }
        }
        cli::Command::Diagnose => diagnose::run(),
        cli::Command::Quit => {
            sys::single::request_quit();
        }
        cli::Command::Run => {
            sys::disable_ime();
            let Some(instance) = sys::single::SingleInstance::acquire() else {
                if !sys::single::notify_existing() {
                    // Se notify_existing fallisce (nessuna finestra risponde), attendiamo
                    // un istante e riproviamo; se continua a non esserci finestra, recuperiamo
                    // il controllo nel caso di un precedente processo terminato male.
                    std::thread::sleep(core::time::Duration::from_millis(200));
                    if !sys::single::notify_existing()
                        && let Some(instance) = sys::single::acquire_with_retry(core::time::Duration::from_millis(1500))
                    {
                        let code = app::run();
                        drop(instance);
                        std::process::exit(code);
                    }
                }
                return;
            };
            let code = app::run();
            drop(instance);
            std::process::exit(code);
        }
        cli::Command::AdminRestart => {
            sys::disable_ime();
            sys::single::request_quit();
            let instance = sys::single::acquire_with_retry(core::time::Duration::from_millis(5000));
            let code = app::run();
            drop(instance);
            std::process::exit(code);
        }
    }
}
