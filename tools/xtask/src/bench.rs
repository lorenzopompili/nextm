//! `xtask bench <exe> [--seconds N=60] [--warmup N=10] [--window-class nextm-main]`
//!
//! Avvia l'exe, misura il tempo di avvio, lascia passare il warmup e poi misura
//! per N secondi CPU, cicli, cambi di contesto e memoria; alla fine legge
//! thread, handle, oggetti GDI/USER e moduli caricati, chiude il processo con
//! `WM_CLOSE` e stampa una tabella markdown (criteri S5, S6, S7 e S8).
//!
//! La tabella va su stdout; l'avanzamento e le avvertenze vanno su stderr, così
//! `xtask bench ... > tabella.md` produce un file pulito.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HANDLE, HWND};

use crate::args::{Args, CliError, Verdict};
use crate::sys::{self, Scanner, ThreadSample};

const DEFAULT_SECONDS: u64 = 60;
const DEFAULT_WARMUP: u64 = 10;
const DEFAULT_CLASS: &str = "nextm-main";
/// Limite per `--seconds` e `--warmup` (24 ore): evita overflow sugli istanti.
const MAX_SECONDS: u64 = 24 * 3600;

/// Massimo tempo di attesa per la comparsa della finestra.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Intervallo del polling di `FindWindow`.
const STARTUP_POLL: Duration = Duration::from_millis(1);
/// Attesa massima dopo `WM_CLOSE`, prima di ricorrere a `TerminateProcess`.
const CLOSE_TIMEOUT_MS: u32 = 5000;

struct Options {
    exe: PathBuf,
    seconds: u64,
    warmup: u64,
    window_class: String,
}

/// Il processo figlio viene terminato se il bench si interrompe con un errore:
/// niente processi orfani in giro.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

// --- Calcoli puri (con test) ------------------------------------------------------

/// CPU del processo: (% di un core, µs di CPU per secondo di orologio).
fn cpu_rates(delta_100ns: u64, span: Duration) -> (f64, f64) {
    let secs = span.as_secs_f64();
    if secs <= 0.0 {
        return (0.0, 0.0);
    }
    let us_per_s = delta_100ns as f64 / 10.0 / secs;
    // 1 core = 1.000.000 µs di CPU al secondo.
    (us_per_s / 10_000.0, us_per_s)
}

/// Cambi di contesto avvenuti tra due istantanee, sommati sui thread.
///
/// I thread si abbinano per (TID, istante di creazione). Un thread nato tra le due
/// istantanee conta per intero; uno terminato nel frattempo non compare più e i
/// suoi cambi di contesto non si possono conoscere (mai un risultato negativo).
fn context_switch_delta(before: &[ThreadSample], after: &[ThreadSample]) -> u64 {
    let base: HashMap<(u64, i64), u32> = before.iter().map(|t| ((t.tid, t.create_time), t.context_switches)).collect();
    after
        .iter()
        .map(|t| match base.get(&(t.tid, t.create_time)) {
            Some(&start) => u64::from(t.context_switches.wrapping_sub(start)),
            None => u64::from(t.context_switches),
        })
        .sum()
}

/// Media e massimo di una serie di campioni.
#[derive(Debug, Default, Clone, Copy)]
struct Acc {
    count: u64,
    sum: u128,
    max: u64,
}

impl Acc {
    fn add(&mut self, value: u64) {
        self.count += 1;
        self.sum += u128::from(value);
        self.max = self.max.max(value);
    }

    fn average(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.sum as f64 / self.count as f64 }
    }
}

fn fmt_kib(bytes: f64) -> String {
    format!("{:.1} KiB", bytes / 1024.0)
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// Numero con suffisso K, M o G (base 1000).
fn fmt_scaled(v: f64) -> String {
    if v >= 1e9 {
        format!("{:.2} G", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.2} M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1} K", v / 1e3)
    } else {
        format!("{v:.1}")
    }
}

fn exit_text(status: ExitStatus) -> String {
    match status.code() {
        Some(c) if (0..=255).contains(&c) => format!("codice {c}"),
        Some(c) => format!("codice 0x{:08X}", c as u32),
        None => "terminato in modo anomalo".to_string(),
    }
}

/// Tabella markdown `Misura | Valore`, una riga per misura.
fn render_table(rows: &[(String, String)]) -> String {
    let escape = |s: &str| s.replace('|', "\\|");
    let mut out = String::from("| Misura | Valore |\n|---|---|\n");
    for (name, value) in rows {
        let _ = writeln!(out, "| {} | {} |", escape(name), escape(value));
    }
    out
}

// --- Passi del bench ---------------------------------------------------------------

fn ensure_alive(child: &mut Child) -> Result<(), CliError> {
    match child.try_wait() {
        Ok(None) => Ok(()),
        Ok(Some(status)) => Err(CliError::runtime(format!(
            "il processo è terminato prima della fine del bench ({})",
            exit_text(status)
        ))),
        Err(e) => Err(CliError::runtime(format!("stato del processo non leggibile: {e}"))),
    }
}

fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        thread::sleep(deadline - now);
    }
}

/// Aspetta `total` controllando ogni mezzo secondo che il processo sia vivo.
fn idle(child: &mut Child, total: Duration) -> Result<(), CliError> {
    let end = Instant::now() + total;
    while Instant::now() < end {
        sleep_until(end.min(Instant::now() + Duration::from_millis(500)));
        ensure_alive(child)?;
    }
    Ok(())
}

/// Polling ogni 1 ms finché compare una finestra della classe data creata dal
/// processo figlio. Restituisce la finestra e il tempo trascorso da `started`.
fn wait_for_window(
    child: &mut Child,
    class: &[u16],
    class_name: &str,
    started: Instant,
) -> Result<(HWND, Duration), CliError> {
    let pid = child.id();
    loop {
        if let Some(hwnd) = sys::find_window(class, Some(pid)) {
            return Ok((hwnd, started.elapsed()));
        }
        let status =
            child.try_wait().map_err(|e| CliError::runtime(format!("stato del processo non leggibile: {e}")))?;
        if let Some(status) = status {
            return Err(CliError::runtime(format!(
                "il processo è terminato ({}) prima di creare la finestra di classe «{class_name}». \
                 Se è già in esecuzione un'altra istanza dell'app, chiudila e riprova",
                exit_text(status)
            )));
        }
        if started.elapsed() >= STARTUP_TIMEOUT {
            return Err(CliError::runtime(format!(
                "nessuna finestra di classe «{class_name}» entro {} s dall'avvio",
                STARTUP_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(STARTUP_POLL);
    }
}

/// Misure raccolte nella finestra di osservazione.
struct Measurement {
    /// Tempo CPU del processo (kernel + user) consumato, in unità da 100 ns.
    cpu_100ns: u64,
    /// Tempo di orologio su cui rapportare `cpu_100ns`.
    cpu_span: Duration,
    /// Cicli CPU consumati dal processo.
    cycles: u64,
    /// Cambi di contesto di tutti i thread.
    switches: u64,
    /// Tempo di orologio tra le due istantanee di `NtQuerySystemInformation`.
    scan_span: Duration,
    private_working_set: Acc,
    private_usage: Acc,
    working_set: Acc,
    threads: usize,
    handles: Option<u32>,
    gdi: u32,
    user: u32,
    modules: Vec<String>,
}

fn measure(child: &mut Child, process: HANDLE, seconds: u64) -> Result<Measurement, CliError> {
    let pid = child.id();
    let mut scanner = Scanner::new();
    let runtime = CliError::runtime;

    // Istantanea iniziale: prima i tempi CPU, subito dopo l'elenco dei processi.
    let cpu_start = sys::process_cpu_100ns(process).map_err(runtime)?;
    let cpu_start_at = Instant::now();
    let scan_start = scanner.scan(pid).map_err(runtime)?;
    let scan_start_at = Instant::now();

    // Memoria: un campione al secondo, con scadenze assolute (nessuna deriva).
    let (mut pws, mut private, mut ws) = (Acc::default(), Acc::default(), Acc::default());
    for second in 1..=seconds {
        sleep_until(cpu_start_at + Duration::from_secs(second));
        ensure_alive(child)?;
        let m = sys::memory_counters(process).map_err(runtime)?;
        pws.add(m.private_working_set);
        private.add(m.private_usage);
        ws.add(m.working_set);
    }

    // Istantanea finale.
    let cpu_end = sys::process_cpu_100ns(process).map_err(runtime)?;
    let cpu_end_at = Instant::now();
    let scan_end = scanner.scan(pid).map_err(runtime)?;
    let scan_end_at = Instant::now();

    Ok(Measurement {
        cpu_100ns: cpu_end.saturating_sub(cpu_start),
        cpu_span: cpu_end_at - cpu_start_at,
        cycles: scan_end.cycles.saturating_sub(scan_start.cycles),
        switches: context_switch_delta(&scan_start.threads, &scan_end.threads),
        scan_span: scan_end_at - scan_start_at,
        private_working_set: pws,
        private_usage: private,
        working_set: ws,
        threads: scan_end.threads.len(),
        handles: sys::handle_count(process).ok(),
        gdi: sys::gdi_objects(process),
        user: sys::user_objects(process),
        // Elenco dei moduli: informazione accessoria, un errore non ferma il bench.
        modules: sys::loaded_modules(process).unwrap_or_default(),
    })
}

/// Chiude il processo con `WM_CLOSE`; `TerminateProcess` (via `Child::kill`) solo
/// come ultima risorsa. Restituisce la descrizione e se la chiusura è stata pulita.
fn close_process(guard: &mut ChildGuard, process: HANDLE, class: &[u16], found_at_startup: HWND) -> (String, bool) {
    let pid = guard.0.id();
    // La finestra si cerca di nuovo: se è stata ricreata, vale quella attuale.
    let hwnd = sys::find_window(class, Some(pid)).unwrap_or(found_at_startup);
    let closing_started = Instant::now();
    let posted = sys::post_close(hwnd);

    if sys::wait_exit(process, CLOSE_TIMEOUT_MS) {
        let elapsed = fmt_ms(closing_started.elapsed());
        let how = if posted { "WM_CLOSE" } else { "uscito da solo (PostMessageW non riuscita)" };
        return match guard.0.try_wait() {
            Ok(Some(status)) if status.success() => {
                (format!("pulita: {how}, uscito in {elapsed}, {}", exit_text(status)), true)
            }
            Ok(Some(status)) => (format!("NON pulita: {how}, uscito in {elapsed} con {}", exit_text(status)), false),
            _ => (format!("{how}, uscito in {elapsed} (codice non leggibile)"), false),
        };
    }

    // Ultima risorsa. `kill` chiama TerminateProcess.
    let _ = guard.0.kill();
    let _ = guard.0.wait();
    (
        format!(
            "NON pulita: nessuna uscita entro {} s dopo WM_CLOSE, terminato con TerminateProcess",
            CLOSE_TIMEOUT_MS / 1000
        ),
        false,
    )
}

fn build_rows(o: &Options, exe_len: u64, startup: Duration, m: &Measurement, closing: &str) -> Vec<(String, String)> {
    let (cpu_pct, cpu_us) = cpu_rates(m.cpu_100ns, m.cpu_span);
    let scan_secs = m.scan_span.as_secs_f64().max(f64::EPSILON);
    let cycles_per_s = m.cycles as f64 / scan_secs;
    let switches_per_s = m.switches as f64 / scan_secs;

    let mut rows: Vec<(String, String)> = Vec::new();
    let mut row = |name: &str, value: String| rows.push((name.to_string(), value));

    let exe_name = o.exe.file_name().map_or_else(|| o.exe.display().to_string(), |n| n.to_string_lossy().into_owned());
    row("Eseguibile", format!("`{exe_name}`"));
    row("Dimensione eseguibile", format!("{exe_len} byte ({})", fmt_kib(exe_len as f64)));
    row(&format!("Tempo di avvio (fino alla finestra `{}`)", o.window_class), fmt_ms(startup));
    row(
        "Durata della misura",
        format!(
            "{:.1} s dopo {} s di warmup ({} campioni di memoria)",
            m.scan_span.as_secs_f64(),
            o.warmup,
            m.working_set.count
        ),
    );
    row("CPU del processo (GetProcessTimes), % di un core", format!("{cpu_pct:.4} %"));
    row("CPU del processo (GetProcessTimes), µs al secondo", format!("{cpu_us:.1} µs/s"));
    row("Cicli CPU del processo al secondo", format!("{} cicli/s", fmt_scaled(cycles_per_s)));
    row("Cambi di contesto al secondo", format!("{switches_per_s:.2} /s"));
    row("Working set privato, media", fmt_kib(m.private_working_set.average()));
    row("Working set privato, massimo", fmt_kib(m.private_working_set.max as f64));
    row("Memoria privata (commit), media", fmt_kib(m.private_usage.average()));
    row("Memoria privata (commit), massimo", fmt_kib(m.private_usage.max as f64));
    row("Working set totale, media", fmt_kib(m.working_set.average()));
    row("Working set totale, massimo", fmt_kib(m.working_set.max as f64));
    row("Thread", m.threads.to_string());
    row("Handle", m.handles.map_or_else(|| "n/d".to_string(), |h| h.to_string()));
    row("Oggetti GDI", m.gdi.to_string());
    row("Oggetti USER", m.user.to_string());
    let modules =
        if m.modules.is_empty() { "n/d".to_string() } else { format!("{}: {}", m.modules.len(), m.modules.join(", ")) };
    row("Moduli caricati", modules);
    row("Chiusura", closing.to_string());
    rows
}

pub fn run(mut args: Args) -> Result<Verdict, CliError> {
    let seconds = args.take_u64("--seconds")?.unwrap_or(DEFAULT_SECONDS);
    let warmup = args.take_u64("--warmup")?.unwrap_or(DEFAULT_WARMUP);
    let window_class = args.take_string("--window-class")?.unwrap_or_else(|| DEFAULT_CLASS.to_string());
    let exe = PathBuf::from(args.single_positional("<exe>")?);
    if seconds == 0 || seconds > MAX_SECONDS {
        return Err(CliError::usage(format!("--seconds deve essere tra 1 e {MAX_SECONDS}")));
    }
    if warmup > MAX_SECONDS {
        return Err(CliError::usage(format!("--warmup non può superare {MAX_SECONDS}")));
    }
    if window_class.is_empty() || window_class.contains('\0') {
        return Err(CliError::usage("--window-class non può essere vuoto"));
    }
    execute(&Options { exe, seconds, warmup, window_class })
}

fn execute(o: &Options) -> Result<Verdict, CliError> {
    let exe_len = fs::metadata(&o.exe)
        .map_err(|e| CliError::runtime(format!("non riesco a leggere {}: {e}", o.exe.display())))?
        .len();
    let class = sys::wide(&o.window_class);

    // Una seconda istanza di nextm non parte, ed è il caso peggiore: farebbe aprire
    // il pannello alla prima. Meglio rifiutare subito.
    if let Some(existing) = sys::find_window(&class, None) {
        return Err(CliError::runtime(format!(
            "esiste già una finestra di classe «{}» (PID {}): chiudi quell'istanza prima del bench",
            o.window_class,
            sys::window_pid(existing)
        )));
    }

    eprintln!("bench: avvio di {}", o.exe.display());
    let started = Instant::now();
    let child = Command::new(&o.exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CliError::runtime(format!("avvio di {} non riuscito: {e}", o.exe.display())))?;
    let mut guard = ChildGuard(child);
    // L'handle del figlio ha accesso pieno: serve per tempi, memoria e attesa.
    let process: HANDLE = guard.0.as_raw_handle();

    let (hwnd, startup) = wait_for_window(&mut guard.0, &class, &o.window_class, started)?;
    eprintln!("bench: finestra «{}» comparsa dopo {}", o.window_class, fmt_ms(startup));

    eprintln!("bench: warmup di {} s", o.warmup);
    idle(&mut guard.0, Duration::from_secs(o.warmup))?;

    eprintln!("bench: misura per {} s", o.seconds);
    let measurement = measure(&mut guard.0, process, o.seconds)?;

    eprintln!("bench: chiusura");
    let (closing, clean) = close_process(&mut guard, process, &class, hwnd);

    print!("{}", render_table(&build_rows(o, exe_len, startup, &measurement, &closing)));
    eprintln!(
        "bench: nota, GetProcessTimes ha la risoluzione del tick di sistema (circa 15,6 ms): \
         per carichi minimi fanno fede i cicli CPU"
    );
    if !clean {
        eprintln!("bench: la chiusura non è stata pulita, esito FALLITO");
    }
    Ok(if clean { Verdict::Pass } else { Verdict::Fail })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(tid: u64, create_time: i64, context_switches: u32) -> ThreadSample {
        ThreadSample { tid, create_time, context_switches }
    }

    #[test]
    fn cpu_rates_from_100ns_units() {
        // 10 ms di CPU (100.000 unità da 100 ns) in 10 s: 1000 µs/s = 0,1% di un core.
        let (pct, us) = cpu_rates(100_000, Duration::from_secs(10));
        assert!((us - 1000.0).abs() < 1e-9, "{us}");
        assert!((pct - 0.1).abs() < 1e-12, "{pct}");
        // Un core pieno per 2 s.
        let (pct, us) = cpu_rates(20_000_000, Duration::from_secs(2));
        assert!((pct - 100.0).abs() < 1e-9);
        assert!((us - 1_000_000.0).abs() < 1e-6);
        // Intervallo nullo: niente divisioni per zero.
        assert_eq!(cpu_rates(5, Duration::ZERO), (0.0, 0.0));
    }

    #[test]
    fn context_switches_match_threads_by_tid_and_creation_time() {
        // Il thread 1 fa +3, il 2 è invariato, il 3 termina (non compare più e non
        // toglie nulla), il 4 nasce nell'intervallo e conta per intero.
        let before = [thread(1, 100, 10), thread(2, 200, 5), thread(3, 300, 7)];
        let after = [thread(1, 100, 13), thread(2, 200, 5), thread(4, 400, 6)];
        assert_eq!(context_switch_delta(&before, &after), 3 + 6);
    }

    #[test]
    fn reused_tid_with_new_creation_time_is_a_new_thread() {
        let before = [thread(1, 100, 1000)];
        let after = [thread(1, 999, 4)];
        assert_eq!(context_switch_delta(&before, &after), 4);
    }

    #[test]
    fn context_switch_counter_wraparound() {
        let before = [thread(1, 1, u32::MAX - 1)];
        let after = [thread(1, 1, 3)];
        assert_eq!(context_switch_delta(&before, &after), 5);
    }

    #[test]
    fn accumulator_average_and_max() {
        let mut a = Acc::default();
        assert_eq!(a.average(), 0.0);
        for v in [100, 300, 200] {
            a.add(v);
        }
        assert_eq!(a.count, 3);
        assert_eq!(a.max, 300);
        assert!((a.average() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_kib(1536.0), "1.5 KiB");
        assert_eq!(fmt_ms(Duration::from_micros(12_340)), "12.3 ms");
        assert_eq!(fmt_scaled(950.0), "950.0");
        assert_eq!(fmt_scaled(312_460.0), "312.5 K");
        assert_eq!(fmt_scaled(2_469_135.0), "2.47 M");
        assert_eq!(fmt_scaled(3_100_000_000.0), "3.10 G");
    }

    #[test]
    fn table_has_header_one_row_per_measure_and_escapes_pipes() {
        let rows = vec![("Thread".to_string(), "3".to_string()), ("Moduli".to_string(), "a|b".to_string())];
        let table = render_table(&rows);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "| Misura | Valore |");
        assert_eq!(lines[1], "|---|---|");
        assert_eq!(lines[2], "| Thread | 3 |");
        assert_eq!(lines[3], "| Moduli | a\\|b |");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn options_are_validated() {
        let args = |v: &[&str]| Args::new(v.iter().map(std::ffi::OsString::from).collect());
        assert!(matches!(run(args(&["x.exe", "--seconds", "0"])), Err(CliError::Usage(_))));
        assert!(matches!(run(args(&["x.exe", "--seconds", "abc"])), Err(CliError::Usage(_))));
        // Valori enormi: rifiutati prima che possano far traboccare gli istanti.
        assert!(matches!(run(args(&["x.exe", "--seconds", "18446744073709551615"])), Err(CliError::Usage(_))));
        assert!(matches!(run(args(&["x.exe", "--warmup", "18446744073709551615"])), Err(CliError::Usage(_))));
        assert!(matches!(run(args(&["x.exe", "--window-class", ""])), Err(CliError::Usage(_))));
        assert!(matches!(run(args(&["--seconds", "5"])), Err(CliError::Usage(_))));
        // File inesistente: errore a runtime, non d'uso.
        let missing = run(args(&["Z:\\non\\esiste\\app.exe", "--seconds", "1", "--warmup", "0"]));
        assert!(matches!(missing, Err(CliError::Runtime(_))));
    }
}
