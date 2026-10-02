//! `xtask check-imports <exe> [--allow a.dll,b.dll,...]`
//!
//! Controllo di CI per S8 e per l'hardening di §6.3 della spec:
//! - gli import statici devono stare nell'allowlist (le DLL opzionali si
//!   caricano a runtime, mai per import);
//! - la Delay Import Directory deve essere vuota (niente `/DELAYLOAD`);
//! - `DependentLoadFlags` della Load Config Directory deve valere 0x0800
//!   (`LOAD_LIBRARY_SEARCH_SYSTEM32`, impostato con `/DEPENDENTLOADFLAG:0x800`).
#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::args::{Args, CliError, Verdict};
use crate::pe::{Pe, PeError};

/// Valore atteso di `DependentLoadFlags`: `LOAD_LIBRARY_SEARCH_SYSTEM32`.
pub const REQUIRED_DEPENDENT_LOAD_FLAGS: u16 = 0x0800;

/// Allowlist predefinita (confronto senza distinzione tra maiuscole e minuscole).
/// Il carattere `*` vale "qualsiasi sequenza di caratteri": così `api-ms-win-crt-*.dll`
/// copre tutta la UCRT di sistema.
pub const DEFAULT_ALLOW: &[&str] = &[
    "kernel32.dll",
    "user32.dll",
    "gdi32.dll",
    "shell32.dll",
    "ntdll.dll",
    // ImmDisableIME: imm32 è comunque caricata da user32 in ogni processo con finestre.
    "imm32.dll",
    "api-ms-win-core-registry-l1-1-0.dll",
    "api-ms-win-core-synch-l1-2-0.dll",
    "api-ms-win-crt-*.dll",
];

/// Una DLL importata e se l'allowlist la ammette.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: String,
    pub allowed: bool,
}

/// Risultato dell'analisi di un PE.
#[derive(Debug)]
pub struct Analysis {
    /// Architettura letta dal PE (`x64`, `ARM64`, ...).
    pub machine: String,
    pub imports: Vec<ImportEntry>,
    /// Nomi delle DLL in delay-load (deve essere vuoto) oppure l'errore di lettura.
    pub delay: Result<Vec<String>, PeError>,
    /// `DependentLoadFlags` oppure il motivo per cui non si legge.
    pub load_flags: Result<u16, PeError>,
}

impl Analysis {
    /// Elenco dei problemi trovati, uno per riga di rapporto. Vuoto = controllo superato.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in self.imports.iter().filter(|i| !i.allowed) {
            out.push(format!("DLL non ammessa: {}", i.name));
        }
        match &self.delay {
            Ok(names) if !names.is_empty() => {
                out.push(format!("delay-load non vuoto: {}", names.join(", ")));
            }
            Ok(_) => {}
            Err(e) => out.push(format!("delay-load non leggibile: {e}")),
        }
        match &self.load_flags {
            Ok(REQUIRED_DEPENDENT_LOAD_FLAGS) => {}
            Ok(v) => out.push(format!("DependentLoadFlags = 0x{v:04X}, atteso 0x{REQUIRED_DEPENDENT_LOAD_FLAGS:04X}")),
            Err(e) => out.push(format!("DependentLoadFlags non leggibile: {e}")),
        }
        out
    }

    pub fn passed(&self) -> bool {
        self.problems().is_empty()
    }
}

/// Confronto con jolly `*`, su testo già in minuscolo.
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // Ultimo `*` incontrato e posizione del testo da cui riprovare.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Vero se `dll` (senza distinzione di maiuscole) corrisponde a una voce dell'allowlist.
pub fn is_allowed(dll: &str, allow: &[String]) -> bool {
    let name = dll.to_ascii_lowercase();
    allow.iter().any(|pattern| wildcard_match(pattern.to_ascii_lowercase().as_bytes(), name.as_bytes()))
}

/// Interpreta il valore di `--allow`: nomi separati da virgole, senza voci vuote.
pub fn parse_allow_list(list: &str) -> Result<Vec<String>, CliError> {
    let items: Vec<String> = list.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    if items.is_empty() {
        return Err(CliError::usage("--allow richiede almeno un nome di DLL"));
    }
    Ok(items)
}

/// Analizza i byte di un PE contro l'allowlist.
///
/// Un errore qui significa che il file non si legge come PE32+ o che la tabella
/// degli import è illeggibile; la mancanza della Load Config o del delay-load
/// finisce invece dentro `Analysis`, così il rapporto resta completo.
pub fn analyze(data: &[u8], allow: &[String]) -> Result<Analysis, PeError> {
    let pe = Pe::parse(data)?;
    let imports = pe
        .imports()?
        .into_iter()
        .map(|name| {
            let allowed = is_allowed(&name, allow);
            ImportEntry { name, allowed }
        })
        .collect();
    Ok(Analysis {
        machine: pe.machine_name(),
        imports,
        delay: pe.delay_imports(),
        load_flags: pe.dependent_load_flags(),
    })
}

/// Rapporto in italiano, pronto da stampare.
pub fn render(file_label: &str, file_len: usize, a: &Analysis) -> String {
    let mut s = String::new();
    // Scrivere su una String non può fallire: il risultato di `writeln!` si ignora.
    let _ = writeln!(s, "File: {file_label}");
    let _ = writeln!(s, "Formato: PE32+ {} ({file_len} byte)", a.machine);
    let _ = writeln!(s);
    // Voci della tabella degli import: una stessa DLL può comparire più volte
    // (per esempio kernel32 da raw-dylib e dalla libreria di import classica).
    let _ = writeln!(s, "Import statici ({} voci):", a.imports.len());
    for i in &a.imports {
        if i.allowed {
            let _ = writeln!(s, "  ok  {}", i.name);
        } else {
            let _ = writeln!(s, "  NO  {}   <- non ammessa", i.name);
        }
    }
    match &a.delay {
        Ok(names) if names.is_empty() => {
            let _ = writeln!(s, "Delay-load: vuoto (ok)");
        }
        Ok(names) => {
            let _ = writeln!(s, "Delay-load: NON vuoto ({} DLL): {}", names.len(), names.join(", "));
        }
        Err(e) => {
            let _ = writeln!(s, "Delay-load: ERRORE, {e}");
        }
    }
    match &a.load_flags {
        Ok(REQUIRED_DEPENDENT_LOAD_FLAGS) => {
            let _ = writeln!(s, "DependentLoadFlags: 0x{REQUIRED_DEPENDENT_LOAD_FLAGS:04X} (ok)");
        }
        Ok(v) => {
            let _ =
                writeln!(s, "DependentLoadFlags: 0x{v:04X}, atteso 0x{REQUIRED_DEPENDENT_LOAD_FLAGS:04X} (FALLITO)");
        }
        Err(e) => {
            let _ = writeln!(s, "DependentLoadFlags: ERRORE, {e}");
        }
    }
    let _ = writeln!(s);
    let problems = a.problems();
    if problems.is_empty() {
        let _ = writeln!(s, "Esito: OK");
    } else {
        let count = match problems.len() {
            1 => "1 problema".to_string(),
            n => format!("{n} problemi"),
        };
        let _ = writeln!(s, "Esito: FALLITO ({count})");
        for p in &problems {
            let _ = writeln!(s, "  - {p}");
        }
    }
    s
}

pub fn run(mut args: Args) -> Result<Verdict, CliError> {
    let allow = match args.take_string("--allow")? {
        Some(list) => parse_allow_list(&list)?,
        None => DEFAULT_ALLOW.iter().map(|s| (*s).to_string()).collect(),
    };
    let exe = args.single_positional("<exe>")?;
    let exe: &Path = exe.as_ref();

    let data = fs::read(exe).map_err(|e| CliError::runtime(format!("non riesco a leggere {}: {e}", exe.display())))?;
    let analysis = analyze(&data, &allow).map_err(|e| CliError::runtime(format!("{}: {e}", exe.display())))?;

    print!("{}", render(&exe.display().to_string(), data.len(), &analysis));
    Ok(if analysis.passed() { Verdict::Pass } else { Verdict::Fail })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::testutil::{LoadConfig, Spec, build_pe};

    fn default_allow() -> Vec<String> {
        DEFAULT_ALLOW.iter().map(|s| (*s).to_string()).collect()
    }

    fn check(imports: &[&str], delay: Option<&[&str]>, load_config: LoadConfig) -> Analysis {
        let data = build_pe(&Spec { imports, delay, load_config });
        analyze(&data, &default_allow()).unwrap()
    }

    #[test]
    fn wildcard_matching() {
        assert!(wildcard_match(b"api-ms-win-crt-*.dll", b"api-ms-win-crt-runtime-l1-1-0.dll"));
        assert!(wildcard_match(b"api-ms-win-crt-*.dll", b"api-ms-win-crt-.dll"));
        assert!(!wildcard_match(b"api-ms-win-crt-*.dll", b"api-ms-win-core-file-l1-1-0.dll"));
        assert!(!wildcard_match(b"api-ms-win-crt-*.dll", b"api-ms-win-crt-runtime.dll.exe"));
        assert!(wildcard_match(b"*", b""));
        assert!(wildcard_match(b"a*b*c", b"aXXbYYc"));
        assert!(wildcard_match(b"a*b*c", b"abbbc"));
        assert!(!wildcard_match(b"a*b*c", b"aXXbYY"));
        assert!(wildcard_match(b"kernel32.dll", b"kernel32.dll"));
        assert!(!wildcard_match(b"kernel32.dll", b"kernel32.dl"));
    }

    #[test]
    fn allowlist_is_case_insensitive_and_handles_ucrt() {
        let allow = default_allow();
        assert!(is_allowed("KERNEL32.dll", &allow));
        assert!(is_allowed("USER32.DLL", &allow));
        assert!(is_allowed("api-ms-win-crt-runtime-l1-1-0.dll", &allow));
        assert!(is_allowed("API-MS-WIN-CRT-STDIO-L1-1-0.DLL", &allow));
        assert!(is_allowed("api-ms-win-core-registry-l1-1-0.dll", &allow));
        assert!(is_allowed("api-ms-win-core-synch-l1-2-0.dll", &allow));
    }

    #[test]
    fn default_allowlist_rejects_everything_else() {
        let allow = default_allow();
        for dll in [
            "VCRUNTIME140.dll",
            "advapi32.dll",
            "iphlpapi.dll",
            "pdh.dll",
            "dxcore.dll",
            "ole32.dll",
            "oleaut32.dll",
            "wtsapi32.dll",
            "comctl32.dll",
            "dwmapi.dll",
            "uxtheme.dll",
            "powrprof.dll",
            "sechost.dll",
            "userenv.dll",
            "ws2_32.dll",
            "dbghelp.dll",
            "api-ms-win-core-file-l1-1-0.dll",
        ] {
            assert!(!is_allowed(dll, &allow), "{dll} non dovrebbe essere ammessa");
        }
    }

    #[test]
    fn parses_allow_list() {
        assert_eq!(parse_allow_list("A.dll, b.DLL,,c*.dll").unwrap(), ["a.dll", "b.dll", "c*.dll"]);
        assert!(matches!(parse_allow_list(" , "), Err(CliError::Usage(_))));
    }

    #[test]
    fn clean_binary_passes() {
        let a = check(
            &["KERNEL32.dll", "USER32.dll", "api-ms-win-crt-runtime-l1-1-0.dll"],
            Some(&[]),
            LoadConfig::Flags(0x0800),
        );
        assert!(a.passed(), "problemi: {:?}", a.problems());
        assert!(a.imports.iter().all(|i| i.allowed));
    }

    #[test]
    fn binary_without_delay_directory_passes() {
        let a = check(&["kernel32.dll"], None, LoadConfig::Flags(0x0800));
        assert!(a.passed(), "problemi: {:?}", a.problems());
    }

    #[test]
    fn forbidden_dll_fails() {
        let a = check(&["kernel32.dll", "VCRUNTIME140.dll"], None, LoadConfig::Flags(0x0800));
        assert!(!a.passed());
        assert_eq!(a.problems(), ["DLL non ammessa: VCRUNTIME140.dll"]);
    }

    #[test]
    fn non_empty_delay_load_fails() {
        let a = check(&["kernel32.dll"], Some(&["dxcore.dll"]), LoadConfig::Flags(0x0800));
        assert_eq!(a.problems(), ["delay-load non vuoto: dxcore.dll"]);
    }

    #[test]
    fn wrong_dependent_load_flags_fails() {
        let a = check(&["kernel32.dll"], None, LoadConfig::Flags(0));
        assert_eq!(a.problems(), ["DependentLoadFlags = 0x0000, atteso 0x0800"]);
        // Il valore deve essere esattamente 0x0800, non un insieme di flag che lo contiene.
        let b = check(&["kernel32.dll"], None, LoadConfig::Flags(0x0C00));
        assert!(!b.passed());
    }

    #[test]
    fn missing_or_short_load_config_is_an_error_not_a_pass() {
        let missing = check(&["kernel32.dll"], None, LoadConfig::Missing);
        assert!(!missing.passed());
        assert!(matches!(missing.load_flags, Err(PeError::NoLoadConfig)));
        let short = check(&["kernel32.dll"], None, LoadConfig::Short);
        assert!(!short.passed());
        assert!(matches!(short.load_flags, Err(PeError::LoadConfigTooShort(_))));
    }

    #[test]
    fn custom_allow_list_replaces_the_default() {
        let data = build_pe(&Spec {
            imports: &["kernel32.dll", "iphlpapi.dll"],
            delay: None,
            load_config: LoadConfig::Flags(0x0800),
        });
        let allow = parse_allow_list("kernel32.dll,IPHLPAPI.dll").unwrap();
        assert!(analyze(&data, &allow).unwrap().passed());
        // Con un elenco che non contiene kernel32 è proprio kernel32 a essere rifiutata.
        let only_iphlpapi = parse_allow_list("iphlpapi.dll").unwrap();
        let a = analyze(&data, &only_iphlpapi).unwrap();
        assert_eq!(a.problems(), ["DLL non ammessa: kernel32.dll"]);
    }

    #[test]
    fn report_mentions_every_finding() {
        let a = check(&["kernel32.dll", "VCRUNTIME140.dll"], Some(&["dxcore.dll"]), LoadConfig::Flags(0));
        let text = render("prova.exe", 1234, &a);
        assert!(text.contains("File: prova.exe"));
        assert!(text.contains("PE32+ x64 (1234 byte)"));
        assert!(text.contains("  ok  kernel32.dll"));
        assert!(text.contains("  NO  VCRUNTIME140.dll"));
        assert!(text.contains("Delay-load: NON vuoto (1 DLL): dxcore.dll"));
        assert!(text.contains("DependentLoadFlags: 0x0000, atteso 0x0800 (FALLITO)"));
        assert!(text.contains("Esito: FALLITO (3 problemi)"));

        let good = check(&["kernel32.dll"], Some(&[]), LoadConfig::Flags(0x0800));
        let text = render("ok.exe", 10, &good);
        assert!(text.contains("Delay-load: vuoto (ok)"));
        assert!(text.contains("DependentLoadFlags: 0x0800 (ok)"));
        assert!(text.contains("Esito: OK"));

        // Un solo problema: singolare.
        let one = check(&["kernel32.dll"], None, LoadConfig::Flags(0));
        assert!(render("uno.exe", 10, &one).contains("Esito: FALLITO (1 problema)"));
        // Load Config assente: il rapporto lo dice e l'esito è negativo.
        let none = check(&["kernel32.dll"], None, LoadConfig::Missing);
        let text = render("senza.exe", 10, &none);
        assert!(text.contains("DependentLoadFlags: ERRORE, Load Config Directory assente"));
        assert!(text.contains("Esito: FALLITO"));
    }

    #[test]
    fn own_test_binary_lists_kernel32_as_allowed() {
        let exe = std::env::current_exe().unwrap();
        let data = fs::read(exe).unwrap();
        let a = analyze(&data, &default_allow()).unwrap();
        assert!(a.imports.iter().any(|i| i.name.eq_ignore_ascii_case("kernel32.dll") && i.allowed));
    }
}
