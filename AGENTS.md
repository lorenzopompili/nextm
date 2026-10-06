# Istruzioni Operative per Agenti AI (nextm)

Questo documento descrive l'architettura dei repository, le regole di hardening e la procedura standard completa per testare, rilasciare, taggare e sincronizzare **nextm** su GitLab e GitHub.

---

## 1. Architettura dei Repository e Percorsi

Il progetto vive su due repository Git locali con ruoli distinti:

### Repository Primario / GitLab (Sviluppo Principale)
- **Percorso locale:** `C:\Users\loren\Documents\_home\personal\git\rust\nextm`
- **Remote `origin`:** `https://gitlab.com/p3678/nextm.git`
- **Permessi agente:** L'agente AI ha accesso e **può pushare direttamente** su `origin`.
- **Branch attivi:** `develop` (branch di feature e lavoro continuo) e `main` (branch di produzione e rilascio stabile).
- **Regola remoti:** Non aggiungere il remote `github` a questo repository: i push pubblici avvengono tramite il repository clone `public/nextm`.

### Repository Pubblico / GitHub (Release Pubblica)
- **Percorso locale:** `C:\Users\loren\Documents\_home\personal\git\rust\public\nextm`
- **Remote `origin`:** `https://github.com/lorenzopompili/nextm.git`
- **Permessi agente:** L'agente AI **non ha credenziali interattive** su GitHub e non deve eseguire comandi `git push` che attendono autenticazione. L'agente deve copiare i dati, verificare la compilazione/test, committare, allineare i branch `develop` e `main`, creare il tag annotato, e infine fornire all'utente il comando per il push manuale.
- **Particolarità file README:** Nel repository GitHub, il file `README.md` alla radice deve essere la versione in lingua **inglese** (derivata da `README_GITHUB.md` del repository primario).

---

## 2. Hardening e Regole Architetturali Fondamentali

Ogni modifica al codice deve rispettare rigorosamente questi vincoli:
1. **Zero DLL esterne & Dipendenze Native:** Solo 15 DLL di sistema Windows ammesse (`kernel32.dll`, `user32.dll`, `gdi32.dll`, `shell32.dll`, `ntdll.dll`, `imm32.dll`, CRT UCRT).
2. **Caricamento Dinamico per DLL opzionali:** Tutte le DLL accessorie (`dwmapi.dll`, `powrprof.dll`, `iphlpapi.dll`, ecc.) devono essere caricate esclusivamente a runtime con `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)`.
   - Invocare sempre `FreeLibrary` al termine o memorizzare/riutilizzare i puntatori a funzione con `OnceLock`.
3. **Controllo Import:** `cargo run -p xtask -- check-imports target/release/nextm.exe` deve sempre passare con `Esito: OK` (delay-load vuoto, `DEPENDENTLOADFLAG: 0x0800`).
4. **Budget Dimensionale Binario (Size Budget):**
   - Esistono due file di budget: `size-budget-x86_64-pc-windows-msvc.txt` e `size-budget-aarch64-pc-windows-msvc.txt`.
   - **IMPORTANTE:** Quando si aggiungono nuove funzionalità o si aggiorna il budget, aggiornare **sempre entrambi i file** (es. a `1600000` byte), altrimenti la CI di GitHub Actions fallirà sul job di compilazione ARM64!
5. **Lingua Documentazione:**
   - `README.md` (in `rust/nextm`): Italiano.
   - `README_GITHUB.md` (in `rust/nextm`): Inglese.
   - `CONTRIBUTING.md` e `SECURITY.md`: Rigorosamente in **Inglese** (standard open source).
   - `CHANGELOG.md`: Italiano (Keep a Changelog).

---

## 3. Procedura Standard di Rilascio e Sincronizzazione (Release Workflow)

Quando l'utente richiede di rilasciare una nuova versione `vX.Y.Z`:

### Fase A — Aggiornamento Versioni e Documentazione (in `rust/nextm`)
1. **Aggiornare la versione:**
   - In `Cargo.toml`: `version = "X.Y.Z"` sotto `[workspace.package]`.
   - In `installer/nextm.iss`: `#define MyAppVersion "X.Y.Z"`.
2. **Aggiornare il budget se necessario:**
   - Verificare sia `size-budget-x86_64-pc-windows-msvc.txt` che `size-budget-aarch64-pc-windows-msvc.txt`.
3. **Aggiornare la documentazione:**
   - In `CHANGELOG.md`: aggiungere la nuova sezione `## [X.Y.Z] - YYYY-MM-DD` con le modifiche organizzate (`Aggiunto`, `Modificato`, `Corretto`, `Migliorato`).
   - In `README.md` e `README_GITHUB.md`: aggiornare i numeri di versione negli esempi e nei link ai file scaricabili.

### Fase B — Quality Gates e Creazione Pacchetti (in `rust/nextm`)
1. **Fermare eventuali istanze aperte:**
   ```powershell
   Stop-Process -Name nextm -Force -ErrorAction SilentlyContinue
   ```
2. **Formattazione, Test e Linter:**
   ```powershell
   cargo fmt --all -- --check
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   ```
3. **Compilazione Release e Verifica Binario:**
   ```powershell
   cargo build --release -p nextm
   cargo run -p xtask -- check-imports target/release/nextm.exe
   cargo run -p xtask -- check-size target/release/nextm.exe --budget size-budget-x86_64-pc-windows-msvc.txt
   cargo run -p xtask -- check-size target/release/nextm.exe --budget size-budget-aarch64-pc-windows-msvc.txt
   ```
4. **Generazione Pacchetti di Rilascio:**
   ```powershell
   cargo run -p xtask -- dist
   ```
   Verificare che in `target/dist/` siano stati generati:
   - `nextm-setup-vX.Y.Z.exe`
   - `nextm-vX.Y.Z-windows-x64.exe`
   - `nextm-vX.Y.Z-windows-x64-portable.zip`
   - `SHA256SUMS.txt`

### Fase C — Commit e Push su GitLab (repository `rust/nextm`)
1. **Commit su `develop`:**
   ```powershell
   git add -A
   git commit -m "feat(release): vX.Y.Z - descrizione sintetica in italiano"
   ```
2. **Fast-forward merge su `main`:**
   ```powershell
   git checkout main
   git merge develop --ff-only
   git checkout develop
   ```
3. **Creazione Tag annotato:**
   ```powershell
   git tag -a vX.Y.Z -m "Release vX.Y.Z - note di rilascio"
   ```
4. **Push su GitLab:**
   ```powershell
   git push origin develop main
   git push origin vX.Y.Z
   ```

### Fase D — Sincronizzazione con il Repository GitHub (`public/nextm`)
1. **Copia file sincronizzati:**
   ```powershell
   $src = "C:\Users\loren\Documents\_home\personal\git\rust\nextm"
   $dst = "C:\Users\loren\Documents\_home\personal\git\rust\public\nextm"

   # Cartelle sorgenti
   Copy-Item -Path "$src\crates" -Destination $dst -Recurse -Force
   Copy-Item -Path "$src\tools" -Destination $dst -Recurse -Force
   Copy-Item -Path "$src\installer" -Destination $dst -Recurse -Force
   Copy-Item -Path "$src\logo" -Destination $dst -Recurse -Force
   Copy-Item -Path "$src\.cargo" -Destination $dst -Recurse -Force
   Copy-Item -Path "$src\.github" -Destination $dst -Recurse -Force

   # File radice
   Copy-Item -Path "$src\Cargo.toml" -Destination "$dst\Cargo.toml" -Force
   Copy-Item -Path "$src\Cargo.lock" -Destination "$dst\Cargo.lock" -Force
   Copy-Item -Path "$src\CHANGELOG.md" -Destination "$dst\CHANGELOG.md" -Force
   Copy-Item -Path "$src\clippy.toml" -Destination "$dst\clippy.toml" -Force
   Copy-Item -Path "$src\CONTRIBUTING.md" -Destination "$dst\CONTRIBUTING.md" -Force
   Copy-Item -Path "$src\SECURITY.md" -Destination "$dst\SECURITY.md" -Force
   Copy-Item -Path "$src\size-budget-x86_64-pc-windows-msvc.txt" -Destination "$dst\size-budget-x86_64-pc-windows-msvc.txt" -Force
   Copy-Item -Path "$src\size-budget-aarch64-pc-windows-msvc.txt" -Destination "$dst\size-budget-aarch64-pc-windows-msvc.txt" -Force
   Copy-Item -Path "$src\AGENTS.md" -Destination "$dst\AGENTS.md" -Force

   # README per GitHub (deve essere la versione in inglese)
   Copy-Item -Path "$src\README_GITHUB.md" -Destination "$dst\README.md" -Force

   # Pacchetti in target/dist
   if (!(Test-Path "$dst\target\dist")) { New-Item -ItemType Directory -Path "$dst\target\dist" -Force }
   Copy-Item -Path "$src\target\dist\nextm-setup-vX.Y.Z.exe" -Destination "$dst\target\dist\" -Force
   Copy-Item -Path "$src\target\dist\nextm-vX.Y.Z-windows-x64.exe" -Destination "$dst\target\dist\" -Force
   Copy-Item -Path "$src\target\dist\nextm-vX.Y.Z-windows-x64-portable.zip" -Destination "$dst\target\dist\" -Force
   Copy-Item -Path "$src\target\dist\SHA256SUMS.txt" -Destination "$dst\target\dist\" -Force
   ```
2. **Commit e Tag in `public/nextm`:**
   ```powershell
   # Assicurarsi di essere sul branch develop
   git -C $dst checkout develop
   git -C $dst add -A
   git -C $dst commit -m "feat(release): vX.Y.Z - release summary in english"

   # Allineamento branch main
   git -C $dst checkout main
   git -C $dst merge develop --ff-only
   git -C $dst checkout develop

   # Creazione del tag annotato
   git -C $dst tag -a vX.Y.Z -m "Release vX.Y.Z - release notes"
   ```

### Fase E — Istruzioni per l'Utente
Avvisare sempre l'utente che entrambi i repository sono allineati, GitLab è già aggiornato, e fornire all'utente il comando per completare il push su GitHub dal suo terminale:
```powershell
cd C:\Users\loren\Documents\_home\personal\git\rust\public\nextm
git push origin develop main --tags
```
