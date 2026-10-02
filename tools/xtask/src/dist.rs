//! Generazione dei pacchetti di distribuzione (Installer Inno Setup, ZIP portatile, Standalone EXE e Checksums SHA256).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::{Args, CliError, Verdict};

fn find_iscc() -> Option<PathBuf> {
    // 1. Cerca nel PATH
    if let Ok(output) = Command::new("where.exe").arg("iscc").output()
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout);
        if let Some(first) = s.lines().next() {
            let p = PathBuf::from(first.trim());
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 2. Percorsi standard Inno Setup
    let candidates = [
        std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join(r"Programs\Inno Setup 6\ISCC.exe")).ok(),
        std::env::var("ProgramFiles(x86)").map(|p| PathBuf::from(p).join(r"Inno Setup 6\ISCC.exe")).ok(),
        std::env::var("ProgramFiles").map(|p| PathBuf::from(p).join(r"Inno Setup 6\ISCC.exe")).ok(),
        std::env::var("ProgramFiles").map(|p| PathBuf::from(p).join(r"Inno Setup 7\ISCC.exe")).ok(),
        std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join(r"Programs\Inno Setup 7\ISCC.exe")).ok(),
    ];

    candidates.into_iter().flatten().find(|c| c.is_file())
}

fn compute_sha256(path: &Path) -> Option<String> {
    let output = Command::new("certutil")
        .args(["-hashfile", &path.to_string_lossy(), "SHA256"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(trimmed.to_lowercase());
        }
    }
    None
}

pub fn run(args: Args) -> Result<Verdict, CliError> {
    args.ensure_empty()?;

    println!("Compilazione / verifica di nextm in modalità release...");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "nextm"])
        .status()
        .map_err(|e| CliError::Runtime(format!("impossibile eseguire cargo build: {e}")))?;
    if !status.success() {
        return Err(CliError::Runtime("cargo build --release fallito".to_string()));
    }
    let release_exe = Path::new("target/release/nextm.exe");
    if !release_exe.is_file() {
        return Err(CliError::Runtime("target/release/nextm.exe non trovato".to_string()));
    }

    let dist_dir = Path::new("target/dist");
    fs::create_dir_all(dist_dir)
        .map_err(|e| CliError::Runtime(format!("impossibile creare la cartella target/dist: {e}")))?;

    // 1. Compilazione Installer Inno Setup
    if let Some(iscc) = find_iscc() {
        println!("Compilatore Inno Setup trovato: {}", iscc.display());
        let iss_path = Path::new("installer/nextm.iss");
        if iss_path.exists() {
            println!("Compilazione installer con {}...", iss_path.display());
            let status = Command::new(&iscc)
                .arg(iss_path)
                .status()
                .map_err(|e| CliError::Runtime(format!("impossibile avviare ISCC: {e}")))?;
            if !status.success() {
                return Err(CliError::Runtime(format!(
                    "compilazione installer fallita con codice {:?}",
                    status.code()
                )));
            }
            println!("✓ Installer Inno Setup generato: target/dist/nextm-setup-v0.1.0.exe");
        }
    } else {
        println!("ATTENZIONE: ISCC.exe non trovato. Installer non generato (installa Inno Setup 6 per generarlo).");
    }

    // 2. Standalone Portable Executable
    let standalone_exe = dist_dir.join("nextm-v0.1.0-windows-x64.exe");
    fs::copy(release_exe, &standalone_exe)
        .map_err(|e| CliError::Runtime(format!("impossibile copiare standalone exe: {e}")))?;
    println!("✓ Standalone EXE generato: {}", standalone_exe.display());

    // 3. Pacchetto Portatile ZIP
    let zip_name = "nextm-v0.1.0-windows-x64-portable.zip";
    let zip_path = dist_dir.join(zip_name);
    let tmp_portable = dist_dir.join("_portable_tmp");
    let _ = fs::remove_dir_all(&tmp_portable);
    fs::create_dir_all(&tmp_portable)
        .map_err(|e| CliError::Runtime(format!("impossibile creare directory temporanea: {e}")))?;

    fs::copy(release_exe, tmp_portable.join("nextm.exe"))
        .map_err(|e| CliError::Runtime(format!("errore copia nextm.exe: {e}")))?;
    fs::write(
        tmp_portable.join("nextm.ini"),
        "# nextm - configurazione portatile locale\n# Rimuovere o modificare le chiavi a piacimento\n",
    )
    .map_err(|e| CliError::Runtime(format!("errore creazione nextm.ini: {e}")))?;
    if Path::new("README.md").exists() {
        let _ = fs::copy("README.md", tmp_portable.join("README.md"));
    }
    if Path::new("LICENSE-MIT").exists() {
        let _ = fs::copy("LICENSE-MIT", tmp_portable.join("LICENSE-MIT"));
    }
    if Path::new("LICENSE-APACHE").exists() {
        let _ = fs::copy("LICENSE-APACHE", tmp_portable.join("LICENSE-APACHE"));
    }

    let _ = fs::remove_file(&zip_path);

    // Usa PowerShell Compress-Archive per produrre uno ZIP pulito e nativo compatibile con Esplora File di Windows
    let ps_cmd = format!(
        "Compress-Archive -Path '{}\\*' -DestinationPath '{}' -Force",
        tmp_portable.to_string_lossy(),
        zip_path.to_string_lossy()
    );
    let zip_ok = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps_cmd])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = fs::remove_dir_all(&tmp_portable);

    if zip_ok && zip_path.is_file() {
        println!("✓ Pacchetto portatile ZIP generato: {}", zip_path.display());
    } else {
        println!("AVVISO: Creazione ZIP non riuscita.");
    }

    // 4. Calcolo Checksums SHA-256
    let mut checksum_lines = Vec::new();
    let release_files = [
        "nextm-setup-v0.1.0.exe",
        "nextm-v0.1.0-windows-x64.exe",
        "nextm-v0.1.0-windows-x64-portable.zip",
    ];

    println!("\nChecksums SHA-256 dei file di rilascio:");
    for name in release_files {
        let p = dist_dir.join(name);
        if !p.is_file() {
            continue;
        }
        if let Some(hash) = compute_sha256(&p) {
            println!("  {hash}  {name}");
            checksum_lines.push(format!("{hash}  {name}"));
        }
    }

    if !checksum_lines.is_empty() {
        let sha_file = dist_dir.join("SHA256SUMS.txt");
        let content = checksum_lines.join("\n") + "\n";
        let _ = fs::write(&sha_file, content);
        println!("✓ File checksums generato: {}", sha_file.display());
    }

    println!("\nTutti i pacchetti per il rilascio sono pronti in target/dist/");
    Ok(Verdict::Pass)
}
