//! Avvio con Windows tramite la chiave `Run` dell'utente, rispettando `StartupApproved`
//! (la disattivazione fatta dall'utente in Impostazioni o in Gestione attività).

use crate::sys::registry;

const RUN: &[u16] = wide!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APPROVED: &[u16] = wide!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
const NAME: &[u16] = wide!("nextm");

/// `true` se Windows avvierà questo eseguibile all'accesso.
pub fn is_enabled(exe: &str) -> bool {
    let mut buf = [0u16; 1024];
    let Some(n) = registry::get_string(RUN, NAME, &mut buf) else { return false };
    if !run_value_matches(&String::from_utf16_lossy(&buf[..n]), exe) {
        return false;
    }
    let mut approved = [0u8; 16];
    match registry::get_binary(APPROVED, NAME, &mut approved) {
        Some(len) => !approved_says_disabled(&approved[..len]),
        None => true,
    }
}

/// Attiva l'avvio con Windows per `exe`. Se l'utente l'aveva disattivato nelle Impostazioni,
/// la scelta esplicita fatta ora nel menu di nextm la sostituisce.
pub fn enable(exe: &str) -> bool {
    let value: Vec<u16> = format!("\"{exe}\"").encode_utf16().chain(core::iter::once(0)).collect();
    registry::set_string(RUN, NAME, &value) && registry::delete_value(APPROVED, NAME)
}

/// Disattiva l'avvio con Windows.
pub fn disable() -> bool {
    registry::delete_value(RUN, NAME) && registry::delete_value(APPROVED, NAME)
}

/// `StartupApproved`: il primo byte dispari indica "disattivato dall'utente".
pub fn approved_says_disabled(data: &[u8]) -> bool {
    data.first().is_some_and(|b| b & 1 == 1)
}

/// Confronta il valore della chiave `Run` con il percorso dell'exe, ignorando virgolette,
/// spazi esterni e maiuscole/minuscole ASCII. Il valore lo scrive nextm stesso con il
/// percorso esatto; il confronto ASCII evita di includere le tabelle Unicode nell'exe.
pub fn run_value_matches(value: &str, exe: &str) -> bool {
    let v = value.trim().trim_matches('"');
    v.eq_ignore_ascii_case(exe.trim().trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_flags() {
        assert!(!approved_says_disabled(&[0x02, 0, 0, 0]));
        assert!(!approved_says_disabled(&[0x06, 0, 0, 0]));
        assert!(approved_says_disabled(&[0x03, 0, 0, 0]));
        assert!(!approved_says_disabled(&[]));
    }

    #[test]
    fn run_value_comparison() {
        let exe = r"C:\Users\Io\AppData\Local\Programs\nextm\nextm.exe";
        assert!(run_value_matches(r#""C:\users\io\appdata\local\programs\nextm\NEXTM.EXE""#, exe));
        assert!(run_value_matches(&format!("  \"{exe}\" "), exe));
        assert!(!run_value_matches(r#""C:\altro\nextm.exe""#, exe));
    }
}
