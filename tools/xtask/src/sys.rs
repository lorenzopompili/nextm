//! Involucri sicuri sulle chiamate di sistema usate da `bench`.
//! Tutto l'`unsafe` della crate sta in questo file.
//!
//! Nota: NON si usano `QueryProcessCycleTime` né `QueryIdleProcessorCycleTime`,
//! perché generano interrupt su tutte le CPU e falserebbero la misura. I cicli
//! del processo e i cambi di contesto si leggono da `NtQuerySystemInformation`.

use std::mem::{offset_of, size_of};
use std::ptr::{null, null_mut};

use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, HMODULE, HWND, STATUS_INFO_LENGTH_MISMATCH, WAIT_OBJECT_0};
use windows_sys::Win32::System::ProcessStatus::{
    K32EnumProcessModulesEx, K32GetModuleBaseNameW, K32GetProcessMemoryInfo, LIST_MODULES_ALL, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_USEROBJECTS, GetGuiResources, GetProcessHandleCount, GetProcessTimes, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::{SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

// --- Layout di SystemProcessInformation (x64 e ARM64, entrambi a 64 bit) ---------
//
// windows-sys espone le struct "pubbliche" di winternl.h, dove parecchi campi
// sono `Reserved*`. Le posizioni che ci servono sono queste (offset in byte):
//
//   SYSTEM_PROCESS_INFORMATION (0x100 byte, poi l'array dei thread)
//     0x00 NextEntryOffset (u32)     0x04 NumberOfThreads (u32)
//     0x18 CycleTime (u64)           = Reserved1[16..24], perché Reserved1 parte a 0x08
//     0x50 UniqueProcessId           = UniqueProcessId
//   SYSTEM_THREAD_INFORMATION (0x50 byte ciascuno, a partire da 0x100)
//     0x10 CreateTime (i64)          = Reserved1[2]
//     0x28 ClientId.UniqueProcess    0x30 ClientId.UniqueThread
//     0x40 ContextSwitches (u32)     = Reserved3
//
// Le asserzioni seguenti fanno fallire la compilazione se la definizione di
// windows-sys non coincide (per esempio su un target a 32 bit).
const _: () = {
    assert!(size_of::<SYSTEM_PROCESS_INFORMATION>() == 0x100);
    assert!(offset_of!(SYSTEM_PROCESS_INFORMATION, Reserved1) == 0x08);
    assert!(offset_of!(SYSTEM_PROCESS_INFORMATION, UniqueProcessId) == 0x50);
    assert!(size_of::<SYSTEM_THREAD_INFORMATION>() == 0x50);
    assert!(offset_of!(SYSTEM_THREAD_INFORMATION, ClientId) == 0x28);
    assert!(offset_of!(SYSTEM_THREAD_INFORMATION, Reserved3) == 0x40);
};

/// Posizione di CycleTime dentro `Reserved1` (0x18 - 0x08).
const CYCLE_TIME_IN_RESERVED1: usize = 16;
/// Posizione di CreateTime dentro `SYSTEM_THREAD_INFORMATION::Reserved1`
/// (KernelTime, UserTime, CreateTime).
const CREATE_TIME_IN_THREAD_RESERVED1: usize = 2;

/// Dimensione massima accettata per il buffer dell'elenco processi.
const MAX_SCAN_BUFFER: usize = 256 * 1024 * 1024;
/// Tentativi massimi quando il buffer risulta troppo piccolo (l'elenco dei
/// processi può crescere tra una chiamata e l'altra).
const MAX_SCAN_TRIES: usize = 16;

fn last_error(what: &str) -> String {
    format!("{what} non riuscita: {}", std::io::Error::last_os_error())
}

/// Converte una stringa in UTF-16 terminato da NUL.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// --- Tempi, memoria, handle, oggetti GUI -------------------------------------

fn filetime_to_u64(ft: &FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Tempo CPU cumulativo del processo (kernel + user) in unità da 100 ns.
///
/// Attenzione: `GetProcessTimes` ha la risoluzione del tick del timer di sistema
/// (circa 15,6 ms), quindi per carichi minimi vale la misura dei cicli.
pub fn process_cpu_100ns(process: HANDLE) -> Result<u64, String> {
    let (mut creation, mut exit, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SICUREZZA: i quattro puntatori riferiscono variabili locali valide per tutta
    // la chiamata; `process` è l'handle di un processo figlio con accesso pieno.
    let ok = unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(last_error("GetProcessTimes"));
    }
    Ok(filetime_to_u64(&kernel) + filetime_to_u64(&user))
}

/// Contatori di memoria del processo, in byte.
#[derive(Debug, Clone, Copy)]
pub struct MemoryCounters {
    /// Working set privato: la colonna "Memoria" di Gestione attività.
    pub private_working_set: u64,
    /// Byte privati impegnati (commit privato).
    pub private_usage: u64,
    /// Working set totale (privato + condiviso).
    pub working_set: u64,
}

pub fn memory_counters(process: HANDLE) -> Result<MemoryCounters, String> {
    let size = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    let mut c = PROCESS_MEMORY_COUNTERS_EX2 { cb: size, ..Default::default() };
    // SICUREZZA: il parametro è tipizzato come la struct base, ma la funzione
    // riempie la variante estesa quando `cb` è la dimensione di
    // PROCESS_MEMORY_COUNTERS_EX2; `c` vive per tutta la chiamata.
    let ok = unsafe { K32GetProcessMemoryInfo(process, (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), size) };
    if ok == 0 {
        return Err(last_error("K32GetProcessMemoryInfo"));
    }
    Ok(MemoryCounters {
        private_working_set: c.PrivateWorkingSetSize as u64,
        private_usage: c.PrivateUsage as u64,
        working_set: c.WorkingSetSize as u64,
    })
}

/// Numero di handle aperti dal processo.
pub fn handle_count(process: HANDLE) -> Result<u32, String> {
    let mut count = 0u32;
    // SICUREZZA: `count` è una variabile locale valida per la chiamata.
    let ok = unsafe { GetProcessHandleCount(process, &mut count) };
    if ok == 0 {
        return Err(last_error("GetProcessHandleCount"));
    }
    Ok(count)
}

/// Oggetti GDI del processo (0 anche se la chiamata fallisce).
pub fn gdi_objects(process: HANDLE) -> u32 {
    // SICUREZZA: nessun puntatore in gioco; un handle non valido dà solo 0.
    unsafe { GetGuiResources(process, GR_GDIOBJECTS) }
}

/// Oggetti USER del processo (0 anche se la chiamata fallisce).
pub fn user_objects(process: HANDLE) -> u32 {
    // SICUREZZA: come sopra.
    unsafe { GetGuiResources(process, GR_USEROBJECTS) }
}

/// Attende la fine del processo per al massimo `timeout_ms`. Vero se è terminato.
pub fn wait_exit(process: HANDLE, timeout_ms: u32) -> bool {
    // SICUREZZA: nessun puntatore in gioco.
    unsafe { WaitForSingleObject(process, timeout_ms) == WAIT_OBJECT_0 }
}

/// Nomi (senza percorso) dei moduli caricati nel processo, ordinati e in minuscolo.
pub fn loaded_modules(process: HANDLE) -> Result<Vec<String>, String> {
    let mut modules: Vec<HMODULE> = vec![null_mut(); 256];
    for _ in 0..4 {
        let bytes = (modules.len() * size_of::<HMODULE>()) as u32;
        let mut needed = 0u32;
        // SICUREZZA: `modules` ha esattamente `bytes` byte scrivibili.
        let ok =
            unsafe { K32EnumProcessModulesEx(process, modules.as_mut_ptr(), bytes, &mut needed, LIST_MODULES_ALL) };
        if ok == 0 {
            return Err(last_error("K32EnumProcessModulesEx"));
        }
        let count = needed as usize / size_of::<HMODULE>();
        if count <= modules.len() {
            modules.truncate(count);
            break;
        }
        // Nel frattempo sono stati caricati altri moduli: riprova con più spazio.
        modules.resize(count + 16, null_mut());
    }

    let mut names = Vec::with_capacity(modules.len());
    let mut buf = [0u16; 260];
    for module in modules {
        // SICUREZZA: `buf` ha `buf.len()` elementi scrivibili.
        let n = unsafe { K32GetModuleBaseNameW(process, module, buf.as_mut_ptr(), buf.len() as u32) };
        if n > 0 {
            names.push(String::from_utf16_lossy(&buf[..n as usize]).to_ascii_lowercase());
        }
    }
    names.sort();
    Ok(names)
}

// --- Finestre -----------------------------------------------------------------

/// PID del processo che possiede la finestra.
pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SICUREZZA: `pid` è una variabile locale valida; un HWND non valido dà 0.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

/// Cerca una finestra top-level (anche nascosta) della classe `class` (UTF-16
/// terminato da NUL). Con `pid` restituisce solo le finestre di quel processo.
pub fn find_window(class: &[u16], pid: Option<u32>) -> Option<HWND> {
    debug_assert_eq!(class.last(), Some(&0), "la classe deve essere terminata da NUL");
    let mut previous: HWND = null_mut();
    // Tetto di sicurezza: FindWindowExW visita le finestre una per volta.
    for _ in 0..4096 {
        // SICUREZZA: `class` è una stringa UTF-16 terminata da NUL; il titolo è NULL.
        let hwnd = unsafe { FindWindowExW(null_mut(), previous, class.as_ptr(), null()) };
        if hwnd.is_null() {
            return None;
        }
        if pid.is_none_or(|want| window_pid(hwnd) == want) {
            return Some(hwnd);
        }
        previous = hwnd;
    }
    None
}

/// Accoda `WM_CLOSE` alla finestra. Vero se il messaggio è stato accodato.
pub fn post_close(hwnd: HWND) -> bool {
    // SICUREZZA: nessun puntatore in gioco; un HWND non valido fa fallire la chiamata.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 }
}

// --- Istantanea dei processi (NtQuerySystemInformation) ------------------------------

/// Dati di un thread del processo osservato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSample {
    pub tid: u64,
    /// Istante di creazione (unità da 100 ns): con il TID identifica il thread
    /// anche se il TID viene riusato.
    pub create_time: i64,
    /// Contatore cumulativo dei cambi di contesto del thread.
    pub context_switches: u32,
}

/// Istantanea di un processo: cicli CPU cumulativi e thread.
#[derive(Debug, Clone)]
pub struct ProcessScan {
    /// Cicli CPU cumulativi di tutti i thread (compresi quelli già terminati).
    pub cycles: u64,
    pub threads: Vec<ThreadSample>,
}

/// Legge una struct POD dai byte, senza requisiti di allineamento.
fn read_struct<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(size_of::<T>())?;
    let slice = bytes.get(offset..end)?;
    // SICUREZZA: `slice` contiene esattamente size_of::<T>() byte. Lo usiamo solo
    // con le struct di windows-sys fatte di interi e puntatori, per cui ogni
    // schema di bit è valido; la lettura non richiede allineamento.
    Some(unsafe { slice.as_ptr().cast::<T>().read_unaligned() })
}

/// Cerca `pid` nell'elenco `SystemProcessInformation` contenuto in `bytes` ed
/// estrae cicli e thread. Funzione pura: si prova con buffer sintetici.
fn parse_process_list(bytes: &[u8], pid: u32) -> Result<ProcessScan, String> {
    const HEADER: usize = size_of::<SYSTEM_PROCESS_INFORMATION>();
    const THREAD: usize = size_of::<SYSTEM_THREAD_INFORMATION>();
    let truncated = || "elenco dei processi troncato o con layout inatteso".to_string();

    let mut at = 0usize;
    loop {
        let entry: SYSTEM_PROCESS_INFORMATION = read_struct(bytes, at).ok_or_else(truncated)?;
        let next = entry.NextEntryOffset as usize;

        if entry.UniqueProcessId as usize == pid as usize {
            let count = entry.NumberOfThreads as usize;
            let threads_end =
                at.checked_add(HEADER).and_then(|t| t.checked_add(count.checked_mul(THREAD)?)).ok_or_else(truncated)?;
            // L'array dei thread deve stare nella voce (o nel buffer, per l'ultima).
            let limit = if next == 0 { bytes.len() } else { at + next };
            if threads_end > limit || threads_end > bytes.len() {
                return Err(truncated());
            }

            let mut cycle_bytes = [0u8; 8];
            cycle_bytes.copy_from_slice(&entry.Reserved1[CYCLE_TIME_IN_RESERVED1..CYCLE_TIME_IN_RESERVED1 + 8]);
            let cycles = u64::from_le_bytes(cycle_bytes);

            let mut threads = Vec::with_capacity(count);
            for i in 0..count {
                let t: SYSTEM_THREAD_INFORMATION =
                    read_struct(bytes, at + HEADER + i * THREAD).ok_or_else(truncated)?;
                // Controllo di coerenza del layout: ogni thread deve appartenere al PID.
                if t.ClientId.UniqueProcess as usize != pid as usize {
                    return Err(format!(
                        "layout di SYSTEM_THREAD_INFORMATION inatteso: il thread {i} risulta del processo {}",
                        t.ClientId.UniqueProcess as usize
                    ));
                }
                threads.push(ThreadSample {
                    tid: t.ClientId.UniqueThread as usize as u64,
                    create_time: t.Reserved1[CREATE_TIME_IN_THREAD_RESERVED1],
                    context_switches: t.Reserved3,
                });
            }
            return Ok(ProcessScan { cycles, threads });
        }

        if next == 0 {
            return Err(format!("il processo {pid} non risulta nell'elenco dei processi"));
        }
        at = at.checked_add(next).ok_or_else(truncated)?;
    }
}

/// Esegue istantanee di `SystemProcessInformation` riusando lo stesso buffer.
pub struct Scanner {
    /// Buffer in unità da 8 byte, così è allineato come si aspetta il kernel.
    buf: Vec<u64>,
}

impl Scanner {
    pub fn new() -> Self {
        // 1 MiB di partenza: basta per circa 450 processi e 7000 thread.
        Scanner { buf: vec![0; (1 << 20) / 8] }
    }

    /// Istantanea del processo `pid`.
    pub fn scan(&mut self, pid: u32) -> Result<ProcessScan, String> {
        let len = self.query()?;
        // SICUREZZA: `len` non supera la capacità del buffer (verificato in `query`)
        // e il buffer non viene toccato finché la slice è in uso.
        let bytes = unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), len) };
        parse_process_list(bytes, pid)
    }

    /// Riempie il buffer, ingrandendolo finché la chiamata non restituisce più
    /// `STATUS_INFO_LENGTH_MISMATCH`. Restituisce i byte scritti.
    fn query(&mut self) -> Result<usize, String> {
        for _ in 0..MAX_SCAN_TRIES {
            let capacity = self.buf.len() * 8;
            let mut needed = 0u32;
            // SICUREZZA: il buffer ha `capacity` byte scrivibili; `needed` è una
            // variabile locale valida.
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr().cast(),
                    capacity as u32,
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                // Il numero di processi può salire tra due chiamate: margine del 12%
                // più 64 KiB, e comunque sempre più grande di prima.
                let needed = needed as usize;
                let wanted = needed.saturating_add(needed / 8).saturating_add(64 * 1024).max(capacity + 64 * 1024);
                if wanted > MAX_SCAN_BUFFER {
                    return Err(format!("buffer richiesto troppo grande ({wanted} byte)"));
                }
                self.buf.resize(wanted.div_ceil(8), 0);
                continue;
            }
            if status < 0 {
                return Err(format!(
                    "NtQuerySystemInformation(SystemProcessInformation) ha restituito 0x{:08X}",
                    status as u32
                ));
            }
            return Ok((needed as usize).min(capacity));
        }
        Err("NtQuerySystemInformation: buffer ancora troppo piccolo dopo vari tentativi".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

    fn my_pid() -> u32 {
        // SICUREZZA: nessun puntatore in gioco.
        unsafe { GetCurrentProcessId() }
    }

    fn my_tid() -> u32 {
        // SICUREZZA: nessun puntatore in gioco.
        unsafe { GetCurrentThreadId() }
    }

    fn my_process() -> HANDLE {
        // SICUREZZA: restituisce lo pseudo-handle del processo corrente.
        unsafe { GetCurrentProcess() }
    }

    // --- Buffer sintetico con le posizioni scritte a mano (0x18, 0x50, 0x28, ...) ---

    fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
        buf[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// (pid, cicli, thread come (tid, create_time, cambi di contesto)).
    type FakeProcess = (u32, u64, Vec<(u32, i64, u32)>);

    fn fake_list(processes: &[FakeProcess]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for (index, (pid, cycles, threads)) in processes.iter().enumerate() {
            let start = out.len();
            // Voce = intestazione da 0x100 + thread da 0x50 + 24 byte finti di nome.
            let entry_len = 0x100 + threads.len() * 0x50 + 24;
            out.resize(start + entry_len, 0);
            let last = index + 1 == processes.len();
            let next = if last { 0 } else { entry_len as u32 };
            put(&mut out, start, &next.to_le_bytes());
            put(&mut out, start + 0x04, &(threads.len() as u32).to_le_bytes());
            put(&mut out, start + 0x18, &cycles.to_le_bytes());
            put(&mut out, start + 0x50, &u64::from(*pid).to_le_bytes());
            for (i, (tid, create, switches)) in threads.iter().enumerate() {
                let t = start + 0x100 + i * 0x50;
                put(&mut out, t + 0x10, &create.to_le_bytes());
                put(&mut out, t + 0x28, &u64::from(*pid).to_le_bytes());
                put(&mut out, t + 0x30, &u64::from(*tid).to_le_bytes());
                put(&mut out, t + 0x40, &switches.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn parses_synthetic_process_list() {
        let list = fake_list(&[
            (4, 111, vec![(8, 1, 10)]),
            (1234, 987_654_321, vec![(2000, 50, 7), (2004, 60, 9), (2008, 70, 11)]),
            (5678, 5, vec![]),
        ]);
        let scan = parse_process_list(&list, 1234).unwrap();
        assert_eq!(scan.cycles, 987_654_321);
        assert_eq!(
            scan.threads,
            [
                ThreadSample { tid: 2000, create_time: 50, context_switches: 7 },
                ThreadSample { tid: 2004, create_time: 60, context_switches: 9 },
                ThreadSample { tid: 2008, create_time: 70, context_switches: 11 },
            ]
        );
        // Prima e ultima voce dell'elenco.
        assert_eq!(parse_process_list(&list, 4).unwrap().cycles, 111);
        assert!(parse_process_list(&list, 5678).unwrap().threads.is_empty());
        // PID assente.
        assert!(parse_process_list(&list, 99).unwrap_err().contains("non risulta"));
    }

    #[test]
    fn rejects_inconsistent_layouts() {
        // Il ClientId del thread non coincide col PID della voce.
        let mut list = fake_list(&[(1234, 1, vec![(2000, 1, 1)])]);
        put(&mut list, 0x100 + 0x28, &777u64.to_le_bytes());
        assert!(parse_process_list(&list, 1234).unwrap_err().contains("layout"));
        // Troppi thread dichiarati rispetto allo spazio della voce.
        let mut list = fake_list(&[(1234, 1, vec![(2000, 1, 1)]), (1, 1, vec![])]);
        put(&mut list, 0x04, &1000u32.to_le_bytes());
        assert!(parse_process_list(&list, 1234).is_err());
        // Buffer troncato a metà intestazione.
        assert!(parse_process_list(&[0u8; 100], 1234).is_err());
    }

    // --- Prove sul sistema vero ---------------------------------------------------

    #[test]
    fn live_scan_finds_the_current_process() {
        let scan = Scanner::new().scan(my_pid()).unwrap();
        assert!(scan.threads.len() >= 2, "il test gira con almeno 2 thread: {}", scan.threads.len());
        assert!(scan.cycles > 0);
        assert!(scan.threads.iter().any(|t| u64::from(my_tid()) == t.tid));
    }

    #[test]
    fn live_context_switches_of_current_thread_grow_when_sleeping() {
        let mut scanner = Scanner::new();
        let find =
            |scan: &ProcessScan| scan.threads.iter().find(|t| t.tid == u64::from(my_tid())).map(|t| t.context_switches);
        let before = find(&scanner.scan(my_pid()).unwrap()).unwrap();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(2));
        }
        let after = find(&scanner.scan(my_pid()).unwrap()).unwrap();
        // Ogni sleep sospende e poi rimette in esecuzione il thread: almeno un
        // cambio di contesto ciascuno (ne concediamo la metà per prudenza).
        assert!(after.wrapping_sub(before) >= 10, "prima {before}, dopo {after}");
    }

    #[test]
    fn live_cycles_grow_with_work() {
        let mut scanner = Scanner::new();
        let before = scanner.scan(my_pid()).unwrap().cycles;
        // Il lavoro gira in un thread che poi termina: i suoi cicli entrano
        // sicuramente nel totale del processo.
        std::thread::spawn(|| {
            let start = Instant::now();
            let mut x = 0u64;
            while start.elapsed() < Duration::from_millis(50) {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                std::hint::black_box(x);
            }
        })
        .join()
        .unwrap();
        let after = scanner.scan(my_pid()).unwrap().cycles;
        // 50 ms di CPU sono decine di milioni di cicli: un milione è un minimo prudente.
        assert!(after > before + 1_000_000, "prima {before}, dopo {after}");
    }

    #[test]
    fn live_process_counters() {
        let p = my_process();
        let mem = memory_counters(p).unwrap();
        assert!(mem.working_set >= mem.private_working_set);
        assert!(mem.private_working_set > 100 * 1024);
        assert!(handle_count(p).unwrap() > 0);
        assert!(process_cpu_100ns(p).is_ok());
        // Il processo di test non ha finestre: 0 oggetti USER e GDI è lecito, ma la chiamata non deve fallire male.
        let _ = (gdi_objects(p), user_objects(p));
        let modules = loaded_modules(p).unwrap();
        assert!(modules.iter().any(|m| m == "kernel32.dll"), "moduli: {modules:?}");
        assert!(modules.iter().any(|m| m == "ntdll.dll"));
    }

    #[test]
    fn find_window_returns_none_for_unknown_class() {
        let class = wide("xtask-classe-che-non-esiste");
        assert!(find_window(&class, None).is_none());
        assert!(find_window(&class, Some(my_pid())).is_none());
    }
}
