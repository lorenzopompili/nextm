//! `xtask check-size <exe> --budget <file> [--update]`
//!
//! Tetto "a cricchetto" sulla dimensione dell'eseguibile (criterio S4).
//! Il file budget contiene una sola riga: la dimensione di riferimento in byte.
//! - senza `--update` fallisce se la dimensione supera il riferimento + 5%
//!   (aritmetica intera: `riferimento * 105 / 100`) e suggerisce di aggiornare il
//!   riferimento quando l'exe è diventato più piccolo;
//! - con `--update` scrive la dimensione attuale nel file.
#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::path::Path;

use crate::args::{Args, CliError, Verdict};

/// Tolleranza del cricchetto, in percentuale del riferimento.
const TOLERANCE_PERCENT: u128 = 5;

/// Decisione del cricchetto per una coppia (dimensione attuale, riferimento).
#[derive(Debug, PartialEq, Eq)]
pub enum SizeVerdict {
    /// Dentro il tetto e non sotto il riferimento.
    Ok,
    /// Dentro il tetto e sotto il riferimento: conviene abbassarlo con `--update`.
    OkBelowReference,
    /// Oltre il tetto (`limit` è il massimo consentito, in byte).
    TooBig { limit: u64 },
}

/// Tetto consentito: `riferimento * 105 / 100` in interi, senza overflow.
pub fn ceiling(reference: u64) -> u64 {
    let limit = u128::from(reference) * (100 + TOLERANCE_PERCENT) / 100;
    u64::try_from(limit).unwrap_or(u64::MAX)
}

/// Logica pura del cricchetto.
pub fn decide(current: u64, reference: u64) -> SizeVerdict {
    let limit = ceiling(reference);
    if current > limit {
        SizeVerdict::TooBig { limit }
    } else if current < reference {
        SizeVerdict::OkBelowReference
    } else {
        SizeVerdict::Ok
    }
}

/// Legge il contenuto del file budget: un solo intero, con spazi o a-capo intorno.
pub fn parse_budget(text: &str) -> Result<u64, String> {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.is_empty() {
        return Err("il file è vuoto".to_string());
    }
    t.parse::<u64>().map_err(|_| format!("atteso un solo intero (byte), trovato «{t}»"))
}

fn kib(bytes: u64) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

pub fn run(mut args: Args) -> Result<Verdict, CliError> {
    let update = args.take_flag("--update");
    let budget = args.take_value("--budget")?.ok_or_else(|| CliError::usage("manca --budget <file>"))?;
    let exe = args.single_positional("<exe>")?;
    let (exe, budget): (&Path, &Path) = (exe.as_ref(), budget.as_ref());

    let current =
        fs::metadata(exe).map_err(|e| CliError::runtime(format!("non riesco a leggere {}: {e}", exe.display())))?.len();
    println!("Dimensione di {}: {current} byte ({})", exe.display(), kib(current));

    let previous = match fs::read_to_string(budget) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(CliError::runtime(format!("non riesco a leggere il file budget {}: {e}", budget.display())));
        }
    };

    if update {
        return update_budget(budget, current, previous.as_deref());
    }

    let Some(text) = previous else {
        return Err(CliError::usage(format!(
            "il file budget {0} non esiste.\n\
             Crealo con la dimensione attuale:\n  xtask check-size {1} --budget {0} --update",
            budget.display(),
            exe.display()
        )));
    };
    let reference = parse_budget(&text)
        .map_err(|why| CliError::usage(format!("file budget non valido ({}): {why}", budget.display())))?;

    match decide(current, reference) {
        SizeVerdict::Ok => {
            println!(
                "Riferimento: {reference} byte, tetto {} byte (+{TOLERANCE_PERCENT}%). Esito: OK",
                ceiling(reference)
            );
            Ok(Verdict::Pass)
        }
        SizeVerdict::OkBelowReference => {
            println!("Riferimento: {reference} byte. Esito: OK (l'exe è più piccolo di {} byte)", reference - current);
            println!(
                "Suggerimento: abbassa il riferimento con\n  xtask check-size {} --budget {} --update",
                exe.display(),
                budget.display()
            );
            Ok(Verdict::Pass)
        }
        SizeVerdict::TooBig { limit } => {
            println!("Riferimento: {reference} byte, tetto {limit} byte (+{TOLERANCE_PERCENT}%). Esito: FALLITO");
            println!(
                "L'exe supera il tetto di {} byte ({} sopra il riferimento). Se l'aumento è voluto \
                 va discusso prima di aggiornare il riferimento con --update.",
                current - limit,
                current - reference
            );
            Ok(Verdict::Fail)
        }
    }
}

/// Scrive la dimensione attuale nel file budget e racconta cosa è cambiato.
fn update_budget(budget: &Path, current: u64, previous: Option<&str>) -> Result<Verdict, CliError> {
    fs::write(budget, format!("{current}\n"))
        .map_err(|e| CliError::runtime(format!("non riesco a scrivere {}: {e}", budget.display())))?;
    match previous.map(parse_budget) {
        Some(Ok(old)) if old == current => {
            println!("Riferimento invariato: {current} byte ({})", budget.display());
        }
        Some(Ok(old)) => {
            println!("Riferimento aggiornato: {old} -> {current} byte ({})", budget.display());
            if current > old {
                println!(
                    "Attenzione: il riferimento è salito di {} byte. Il cricchetto dovrebbe solo scendere.",
                    current - old
                );
            }
        }
        Some(Err(_)) => {
            println!(
                "Riferimento riscritto: {current} byte ({}); il valore precedente non era valido",
                budget.display()
            );
        }
        None => println!("Riferimento creato: {current} byte ({})", budget.display()),
    }
    Ok(Verdict::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn ceiling_is_reference_plus_five_percent_in_integers() {
        assert_eq!(ceiling(200_000), 210_000);
        assert_eq!(ceiling(101), 106); // 106,05 arrotondato per difetto
        assert_eq!(ceiling(0), 0);
        assert_eq!(ceiling(u64::MAX), u64::MAX); // niente overflow
    }

    #[test]
    fn ratchet_decisions() {
        // Uguale al riferimento.
        assert_eq!(decide(200_000, 200_000), SizeVerdict::Ok);
        // Nella tolleranza, sopra il riferimento.
        assert_eq!(decide(205_000, 200_000), SizeVerdict::Ok);
        // Esattamente sul tetto: passa. Un byte oltre: fallisce.
        assert_eq!(decide(210_000, 200_000), SizeVerdict::Ok);
        assert_eq!(decide(210_001, 200_000), SizeVerdict::TooBig { limit: 210_000 });
        // Sotto il riferimento: passa e suggerisce l'aggiornamento.
        assert_eq!(decide(199_999, 200_000), SizeVerdict::OkBelowReference);
        assert_eq!(decide(0, 200_000), SizeVerdict::OkBelowReference);
        // Riferimento a zero: qualunque byte in più supera il tetto.
        assert_eq!(decide(0, 0), SizeVerdict::Ok);
        assert_eq!(decide(1, 0), SizeVerdict::TooBig { limit: 0 });
    }

    #[test]
    fn budget_file_parsing() {
        assert_eq!(parse_budget("138752\n"), Ok(138_752));
        assert_eq!(parse_budget("  42  \r\n"), Ok(42));
        assert_eq!(parse_budget("\u{feff}7"), Ok(7));
        assert!(parse_budget("").is_err());
        assert!(parse_budget("   \n").is_err());
        assert!(parse_budget("12 34").is_err());
        assert!(parse_budget("1\n2\n").is_err());
        assert!(parse_budget("-5").is_err());
        assert!(parse_budget("abc").is_err());
    }

    /// Cartella temporanea unica per test, ripulita alla fine.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("xtask-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run_with(exe: &Path, budget: &Path, update: bool) -> Result<Verdict, CliError> {
        let mut v: Vec<OsString> = vec![exe.into(), "--budget".into(), budget.into()];
        if update {
            v.push("--update".into());
        }
        run(Args::new(v))
    }

    #[test]
    fn end_to_end_with_files() {
        let tmp = TempDir::new("size");
        let exe = tmp.0.join("app.exe");
        let budget = tmp.0.join("budget.txt");
        fs::write(&exe, vec![0u8; 1000]).unwrap();

        // Senza file e senza --update: errore d'uso con istruzioni.
        assert!(matches!(run_with(&exe, &budget, false), Err(CliError::Usage(m)) if m.contains("--update")));
        // --update crea il file con la dimensione attuale.
        assert_eq!(run_with(&exe, &budget, true).unwrap(), Verdict::Pass);
        assert_eq!(fs::read_to_string(&budget).unwrap(), "1000\n");
        // Ora il controllo passa.
        assert_eq!(run_with(&exe, &budget, false).unwrap(), Verdict::Pass);
        // Dentro il 5% passa ancora (1050 = tetto), oltre fallisce.
        fs::write(&exe, vec![0u8; 1050]).unwrap();
        assert_eq!(run_with(&exe, &budget, false).unwrap(), Verdict::Pass);
        fs::write(&exe, vec![0u8; 1051]).unwrap();
        assert_eq!(run_with(&exe, &budget, false).unwrap(), Verdict::Fail);
        // Il file non viene toccato dal controllo; --update lo alza comunque.
        assert_eq!(fs::read_to_string(&budget).unwrap(), "1000\n");
        assert_eq!(run_with(&exe, &budget, true).unwrap(), Verdict::Pass);
        assert_eq!(fs::read_to_string(&budget).unwrap(), "1051\n");
        // File budget con contenuto sbagliato: errore d'uso.
        fs::write(&budget, "non un numero").unwrap();
        assert!(matches!(run_with(&exe, &budget, false), Err(CliError::Usage(_))));
    }

    #[test]
    fn missing_exe_is_a_runtime_error() {
        let tmp = TempDir::new("noexe");
        let r = run_with(&tmp.0.join("non-esiste.exe"), &tmp.0.join("b.txt"), false);
        assert!(matches!(r, Err(CliError::Runtime(_))));
    }

    #[test]
    fn budget_option_is_required() {
        let r = run(Args::new(vec![OsString::from("x.exe")]));
        assert!(matches!(r, Err(CliError::Usage(_))));
    }
}
