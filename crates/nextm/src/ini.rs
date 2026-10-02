//! Formato del file delle impostazioni in modalità portable: righe `chiave=valore`.

/// Intestazione scritta in cima al file.
pub const HEADER: &str = "# nextm: impostazioni in modalità portable.\n\
# Finché questo file esiste accanto a nextm.exe, le impostazioni si salvano qui e non nel registro.\n";

/// Coppie `chiave=valore` del testo. Ignora righe vuote e commenti (`#` o `;`);
/// toglie gli spazi attorno a chiave e valore. Le righe senza `=` vengono ignorate.
pub fn parse(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        let (k, v) = line.split_once('=')?;
        Some((k.trim(), v.trim()))
    })
}

/// Il valore numerico di `key`, se presente e valido. Vale l'ultima occorrenza.
pub fn get_u32(text: &str, key: &str) -> Option<u32> {
    parse(text).filter(|(k, _)| k.eq_ignore_ascii_case(key)).last()?.1.parse().ok()
}

/// Testo completo del file per le coppie date, con intestazione.
pub fn format(pairs: &[(&str, u32)]) -> String {
    let mut out = String::from(HEADER);
    for (k, v) in pairs {
        out.push_str(k);
        out.push('=');
        out.push_str(&v.to_string());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pairs_comments_and_spaces() {
        let text = "# commento\n; altro\n\n IntervalMs = 2000 \nPower=5\nsenza_uguale\n";
        let pairs: Vec<_> = parse(text).collect();
        assert_eq!(pairs, vec![("IntervalMs", "2000"), ("Power", "5")]);
    }

    #[test]
    fn get_u32_is_case_insensitive_and_takes_last() {
        let text = "intervalms=1000\nIntervalMs=5000\n";
        assert_eq!(get_u32(text, "IntervalMs"), Some(5000));
    }

    #[test]
    fn get_u32_rejects_invalid_numbers() {
        assert_eq!(get_u32("Power=abc\n", "Power"), None);
        assert_eq!(get_u32("Power=-1\n", "Power"), None);
        assert_eq!(get_u32("", "Power"), None);
    }

    #[test]
    fn format_roundtrips() {
        let text = format(&[("IntervalMs", 2000), ("Power", 7)]);
        assert!(text.starts_with('#'));
        assert_eq!(get_u32(&text, "IntervalMs"), Some(2000));
        assert_eq!(get_u32(&text, "Power"), Some(7));
    }

    #[test]
    fn handles_windows_line_endings() {
        assert_eq!(get_u32("Power=3\r\nIntervalMs=5000\r\n", "IntervalMs"), Some(5000));
    }
}
