//! xtask di nextm: strumenti di sviluppo per la CI e per la vetrina delle prestazioni.
//!
//! Sottocomandi (il primo argomento):
//! - `check-imports`: import statici, delay-load e `DependentLoadFlags` (S8, hardening §6.3);
//! - `check-size`: tetto "a cricchetto" sulla dimensione dell'exe (S4);
//! - `bench`: avvio, CPU, cicli, cambi di contesto, memoria, handle... (S5-S8).
//!
//! Codici di uscita: 0 ok, 1 controllo fallito (o errore durante l'esecuzione),
//! 2 errore d'uso.

mod args;
mod bench;
mod check_imports;
mod check_size;
mod dist;
mod pe;
mod sys;

use std::ffi::OsString;
use std::process::ExitCode;

use args::{Args, CliError, Verdict};

const USAGE: &str = "\
uso: xtask <comando> [opzioni]

comandi:
  check-imports <exe> [--allow a.dll,b.dll,...]
      Controlla gli import statici contro un'allowlist, che il delay-load sia
      vuoto e che DependentLoadFlags valga 0x0800. Con --allow l'elenco
      sostituisce quello predefinito; il carattere * fa da jolly
      (per esempio api-ms-win-crt-*.dll).

  check-size <exe> --budget <file> [--update]
      Cricchetto sulla dimensione dell'exe: fallisce oltre il riferimento
      del file budget + 5%. Con --update scrive la dimensione attuale nel file.

  bench <exe> [--seconds N] [--warmup N] [--window-class NOME]
      Avvia l'exe e ne misura avvio, CPU, cicli, cambi di contesto, memoria,
      thread, handle e oggetti GDI/USER. Stampa una tabella markdown.
      Predefiniti: 60 s di misura, 10 s di warmup, classe nextm-main.

  dist
      Genera il pacchetto installer Windows (setup .exe) tramite Inno Setup
      nella cartella target/dist/.

codici di uscita: 0 ok, 1 controllo fallito o errore, 2 errore d'uso
";

fn dispatch(argv: Vec<OsString>) -> Result<Verdict, CliError> {
    let mut argv = argv.into_iter();
    let Some(command) = argv.next() else {
        return Err(CliError::usage("manca il comando"));
    };
    let rest = Args::new(argv.collect());
    match command.to_str() {
        Some("check-imports") => check_imports::run(rest),
        Some("check-size") => check_size::run(rest),
        Some("bench") => bench::run(rest),
        Some("dist") => dist::run(rest),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            Ok(Verdict::Pass)
        }
        _ => Err(CliError::usage(format!("comando sconosciuto: {}", command.to_string_lossy()))),
    }
}

fn main() -> ExitCode {
    match dispatch(std::env::args_os().skip(1).collect()) {
        Ok(Verdict::Pass) => ExitCode::SUCCESS,
        Ok(Verdict::Fail) => ExitCode::from(1),
        Err(CliError::Runtime(message)) => {
            eprintln!("errore: {message}");
            ExitCode::from(1)
        }
        Err(CliError::Usage(message)) => {
            eprintln!("errore d'uso: {message}\nper l'elenco dei comandi: xtask help");
            ExitCode::from(2)
        }
    }
}
