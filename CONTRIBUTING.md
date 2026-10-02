# Contribuire a nextm

Grazie dell'interesse. nextm ha due principi che valgono per ogni modifica: **super semplice e minimale** per chi lo
usa, **efficienza misurata** per chi lo scrive. Una funzione in più deve rispondere a un bisogno reale; un costo in
più (memoria, CPU, risvegli, DLL caricate) deve essere misurato e giustificato.

## Segnalare un problema

Apri una issue e allega l'output di `nextm --diagnose` (versione di Windows, CPU, DPI, tema, modalità delle
impostazioni). Descrivi cosa ti aspettavi e cosa è successo.

## Preparare l'ambiente

- Windows 11.
- Rust: la versione è fissata in `rust-toolchain.toml`, rustup la installa da sola.
- Visual Studio Build Tools con "Sviluppo di applicazioni desktop con C++" e il Windows SDK (serve anche `rc.exe`
  per le risorse dell'eseguibile).

## Comandi

```powershell
cargo build --release -p nextm
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check-imports target/release/nextm.exe
cargo xtask check-size target/release/nextm.exe --budget size-budget-x86_64-pc-windows-msvc.txt
cargo xtask bench target/release/nextm.exe --seconds 60 --warmup 45
```

La CI esegue gli stessi controlli per x64 e ARM64.

## Regole del codice

- Tutto l'`unsafe` sta nei moduli `sys`, con involucri che liberano da soli le risorse (RAII) e un commento
  `// SAFETY:` per ogni blocco.
- Niente `unwrap`, `expect` o `panic!` nel codice dell'applicazione (nei test sono ammessi).
- Niente allocazioni né `format!` nel lavoro che si ripete a ogni tick.
- Commenti in italiano, identificatori in inglese.
- **Non aggiungere la feature `Win32_System_Registry` di windows-sys**: dichiarerebbe le funzioni del registro su
  advapi32 e il linker userebbe quelle al posto dell'API set (vedi `crates/nextm/src/sys/registry.rs`).
- Una DLL opzionale si carica con `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)` solo quando la funzione che
  la usa viene attivata; `cargo xtask check-imports` fallisce se compare un import non ammesso.

## Immagini di riferimento

I test in `crates/nextm-render/tests/golden.rs` confrontano le icone disegnate con i file in `tests/golden/`. Se una
modifica al disegno è voluta, rigenerali e controlla il diff prima del commit:

```powershell
$env:NEXTM_BLESS = "1"; cargo test -p nextm-render --test golden; Remove-Item Env:NEXTM_BLESS
```

## Dimensione dell'eseguibile

`size-budget-<target>.txt` contiene la dimensione di riferimento: la CI fallisce se l'eseguibile la supera di oltre
il 5%. Quando la dimensione scende, aggiorna il riferimento con `--update`. Il valore per ARM64 è provvisorio e va
fissato dopo la prima build in CI.

## Branch

- `main`: solo milestone finite e verificate (test, clippy, `bench` e revisione).
- `develop`: integrazione. Sempre compilabile e con i test verdi; le pull request vanno qui.
- `feat/…`, `fix/…`: lavoro in corso. Partono da `develop` e ci tornano.

## Commit

Messaggi brevi e al presente, in italiano, con un prefisso (`feat:`, `fix:`, `perf:`, `docs:`, `test:`, `build:`,
`ci:`). Ogni modifica visibile va annotata in `CHANGELOG.md` sotto *Non rilasciato*.
