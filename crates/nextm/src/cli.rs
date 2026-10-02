//! Riga di comando: `--version`, `--diagnose`, `--quit`; senza argomenti parte l'indicatore.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Run,
    Version,
    Diagnose,
    Quit,
    AdminRestart,
}

/// Interpreta gli argomenti (senza il nome del programma). Gli argomenti sconosciuti
/// vengono ignorati, così un collegamento vecchio non impedisce l'avvio.
pub fn parse<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for a in args {
        match a.as_ref() {
            "--version" | "-V" | "/version" => return Command::Version,
            "--diagnose" | "/diagnose" => return Command::Diagnose,
            "--quit" | "/quit" => return Command::Quit,
            "--admin-restart" => return Command::AdminRestart,
            _ => {}
        }
    }
    Command::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_runs() {
        assert_eq!(parse(Vec::<String>::new()), Command::Run);
    }

    #[test]
    fn recognizes_flags() {
        assert_eq!(parse(["--version"]), Command::Version);
        assert_eq!(parse(["-V"]), Command::Version);
        assert_eq!(parse(["--diagnose"]), Command::Diagnose);
        assert_eq!(parse(["--quit"]), Command::Quit);
        assert_eq!(parse(["--admin-restart"]), Command::AdminRestart);
    }

    #[test]
    fn unknown_args_are_ignored() {
        assert_eq!(parse(["--boh", "x"]), Command::Run);
        assert_eq!(parse(["--boh", "--diagnose"]), Command::Diagnose);
    }
}
