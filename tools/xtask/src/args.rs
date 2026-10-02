//! Analisi minimale della riga di comando, senza dipendenze esterne.
#![forbid(unsafe_code)]

use std::ffi::OsString;

/// Esito di un sottocomando portato a termine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Controllo superato: codice di uscita 0.
    Pass,
    /// Controllo fallito: codice di uscita 1.
    Fail,
}

/// Errore che interrompe un sottocomando prima che abbia un esito.
#[derive(Debug)]
pub enum CliError {
    /// Uso scorretto della riga di comando: codice di uscita 2.
    Usage(String),
    /// Errore durante l'esecuzione (file illeggibile, PE non valido, processo
    /// che non parte): codice di uscita 1.
    Runtime(String),
}

impl CliError {
    pub fn usage(msg: impl Into<String>) -> Self {
        CliError::Usage(msg.into())
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        CliError::Runtime(msg.into())
    }
}

/// Argomenti di un sottocomando (senza il nome del sottocomando stesso).
///
/// Si "consumano" le opzioni note una alla volta; alla fine `single_positional`
/// verifica che resti soltanto l'argomento posizionale atteso, quindi un'opzione
/// sconosciuta o ripetuta diventa un errore d'uso.
pub struct Args {
    rest: Vec<OsString>,
}

impl Args {
    pub fn new(rest: Vec<OsString>) -> Self {
        Args { rest }
    }

    /// Toglie l'opzione senza valore `name` (per esempio `--update`) e dice se c'era.
    pub fn take_flag(&mut self, name: &str) -> bool {
        match self.rest.iter().position(|a| a.to_str() == Some(name)) {
            Some(i) => {
                self.rest.remove(i);
                true
            }
            None => false,
        }
    }

    /// Toglie l'opzione con valore `name`, nella forma `--nome valore` oppure
    /// `--nome=valore`. Restituisce `None` se l'opzione non c'è.
    pub fn take_value(&mut self, name: &str) -> Result<Option<OsString>, CliError> {
        let prefix = format!("{name}=");
        let pos = self.rest.iter().position(|a| a.to_str().is_some_and(|s| s == name || s.starts_with(&prefix)));
        let Some(i) = pos else {
            return Ok(None);
        };
        let arg = self.rest.remove(i);
        if let Some(value) = arg.to_str().and_then(|s| s.strip_prefix(&prefix)) {
            return Ok(Some(OsString::from(value)));
        }
        // Forma con valore separato: il valore non può essere un'altra opzione.
        match self.rest.get(i) {
            Some(next) if !next.to_str().is_some_and(|s| s.starts_with("--")) => Ok(Some(self.rest.remove(i))),
            _ => Err(CliError::usage(format!("manca il valore dopo {name}"))),
        }
    }

    /// Come `take_value`, ma il valore deve essere testo Unicode.
    pub fn take_string(&mut self, name: &str) -> Result<Option<String>, CliError> {
        match self.take_value(name)? {
            None => Ok(None),
            Some(v) => {
                v.into_string().map(Some).map_err(|_| CliError::usage(format!("il valore di {name} non è Unicode")))
            }
        }
    }

    /// Come `take_string`, ma il valore deve essere un intero non negativo.
    pub fn take_u64(&mut self, name: &str) -> Result<Option<u64>, CliError> {
        match self.take_string(name)? {
            None => Ok(None),
            Some(s) => s.trim().parse::<u64>().map(Some).map_err(|_| {
                CliError::usage(format!("il valore di {name} deve essere un intero non negativo, trovato «{s}»"))
            }),
        }
    }

    /// Consuma gli argomenti e restituisce l'unico posizionale atteso (`what` ne
    /// dà il nome nei messaggi). Ogni altro argomento è un errore d'uso.
    pub fn single_positional(self, what: &str) -> Result<OsString, CliError> {
        if let Some(opt) = self.rest.iter().filter_map(|a| a.to_str()).find(|s| s.starts_with('-') && s.len() > 1) {
            return Err(CliError::usage(format!("opzione sconosciuta o ripetuta: {opt}")));
        }
        let mut it = self.rest.into_iter();
        match (it.next(), it.next()) {
            (Some(one), None) => Ok(one),
            (None, _) => Err(CliError::usage(format!("manca l'argomento {what}"))),
            (Some(_), Some(extra)) => Err(CliError::usage(format!("argomento in più: {}", extra.to_string_lossy()))),
        }
    }

    /// Verifica che non ci siano argomenti residui.
    pub fn ensure_empty(self) -> Result<(), CliError> {
        if let Some(extra) = self.rest.first() {
            return Err(CliError::usage(format!("argomento inatteso: {}", extra.to_string_lossy())));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        Args::new(v.iter().map(OsString::from).collect())
    }

    #[test]
    fn takes_flag_and_values_in_both_forms() {
        let mut a = args(&["x.exe", "--update", "--budget", "b.txt", "--seconds=5"]);
        assert!(a.take_flag("--update"));
        assert!(!a.take_flag("--update"));
        assert_eq!(a.take_value("--budget").unwrap(), Some(OsString::from("b.txt")));
        assert_eq!(a.take_u64("--seconds").unwrap(), Some(5));
        assert_eq!(a.single_positional("<exe>").unwrap(), OsString::from("x.exe"));
    }

    #[test]
    fn missing_option_is_none() {
        let mut a = args(&["x.exe"]);
        assert_eq!(a.take_value("--budget").unwrap(), None);
        assert_eq!(a.take_u64("--seconds").unwrap(), None);
    }

    #[test]
    fn value_missing_or_looks_like_option() {
        assert!(matches!(args(&["x.exe", "--budget"]).take_value("--budget"), Err(CliError::Usage(_))));
        assert!(matches!(args(&["x.exe", "--budget", "--update"]).take_value("--budget"), Err(CliError::Usage(_))));
    }

    #[test]
    fn bad_number_is_usage_error() {
        assert!(matches!(args(&["--seconds", "abc"]).take_u64("--seconds"), Err(CliError::Usage(_))));
        assert!(matches!(args(&["--seconds", "-3"]).take_u64("--seconds"), Err(CliError::Usage(_))));
    }

    #[test]
    fn positional_errors() {
        assert!(matches!(args(&[]).single_positional("<exe>"), Err(CliError::Usage(_))));
        assert!(matches!(args(&["a", "b"]).single_positional("<exe>"), Err(CliError::Usage(_))));
        assert!(matches!(args(&["a", "--boh"]).single_positional("<exe>"), Err(CliError::Usage(_))));
        // Un'opzione ripetuta resta nell'elenco e viene rifiutata.
        let mut a = args(&["a", "--x", "1", "--x", "2"]);
        a.take_value("--x").unwrap();
        assert!(matches!(a.single_positional("<exe>"), Err(CliError::Usage(_))));
    }

    #[test]
    fn ensure_empty_validates_remaining_args() {
        assert!(args(&[]).ensure_empty().is_ok());
        assert!(matches!(args(&["extra"]).ensure_empty(), Err(CliError::Usage(_))));
    }
}
