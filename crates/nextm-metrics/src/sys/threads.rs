//! Scansione dei thread di tutti i processi per il core saturo (M2).
//!
//! Usa `NtQuerySystemInformation(SystemProcessInformation)` con un buffer allocato tramite
//! `VirtualAlloc`. Quando la metrica o la scansione viene rilasciata (`release`), la memoria
//! virtuale viene liberata completamente, rispettando il principio "paghi solo ciò che attivi".

use core::ffi::c_void;
use core::mem::{offset_of, size_of};
use core::ptr::null_mut;

use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows_sys::Win32::Foundation::STATUS_INFO_LENGTH_MISMATCH;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::WindowsProgramming::{SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION};

use crate::saturation::{ThreadKey, ThreadRead};

const _: () = {
    assert!(size_of::<SYSTEM_PROCESS_INFORMATION>() == 0x100);
    assert!(offset_of!(SYSTEM_PROCESS_INFORMATION, UniqueProcessId) == 0x50);
    assert!(offset_of!(SYSTEM_PROCESS_INFORMATION, ImageName) == 0x38);
    assert!(size_of::<SYSTEM_THREAD_INFORMATION>() == 0x50);
    assert!(offset_of!(SYSTEM_THREAD_INFORMATION, Reserved1) == 0x00);
    assert!(offset_of!(SYSTEM_THREAD_INFORMATION, ClientId) == 0x28);
};

const INITIAL_BUFFER_SIZE: usize = 1024 * 1024; // 1 MiB
const MAX_SCAN_BUFFER: usize = 64 * 1024 * 1024; // 64 MiB
const MAX_SCAN_TRIES: usize = 8;

/// Scansiona i thread di tutti i processi attivi di Windows e mantiene la cache dei nomi dei processi.
pub struct ThreadScanner {
    ptr: *mut u8,
    capacity: usize,
    names: Vec<(u32, String)>,
}

// SAFETY: il puntatore punta a un blocco VirtualAlloc di nostra proprietà, non condiviso tra thread.
unsafe impl Send for ThreadScanner {}

impl ThreadScanner {
    /// Crea un nuovo scanner. Nessuna memoria viene allocata finché non viene chiamato `scan()`.
    pub fn new() -> ThreadScanner {
        ThreadScanner { ptr: null_mut(), capacity: 0, names: Vec::new() }
    }

    /// Esegue la scansione dei thread di tutti i processi e popola `out` con i `ThreadRead`.
    /// I thread con PID 0 (System Idle) vengono esclusi.
    pub fn scan(&mut self, out: &mut Vec<ThreadRead>) -> bool {
        let len = match self.query_system_processes() {
            Some(l) => l,
            None => return false,
        };

        out.clear();
        self.names.clear();

        let buf_start = self.ptr as usize;
        let buf_end = buf_start + len;

        let mut offset = 0usize;
        loop {
            if offset + size_of::<SYSTEM_PROCESS_INFORMATION>() > len {
                break;
            }

            // SAFETY: l'offset rientra nei limiti del buffer scritto dal kernel.
            let proc = unsafe { &*self.ptr.add(offset).cast::<SYSTEM_PROCESS_INFORMATION>() };
            let pid = proc.UniqueProcessId as usize as u32;

            if pid != 0 {
                // Estrae il nome dell'immagine, se presente e valido nel buffer
                let byte_len = proc.ImageName.Length as usize;
                let str_buf = proc.ImageName.Buffer as usize;
                if byte_len > 0
                    && str_buf >= buf_start
                    && str_buf.checked_add(byte_len).is_some_and(|end| end <= buf_end)
                {
                    let chars = byte_len / 2;
                    let slice = unsafe { core::slice::from_raw_parts(proc.ImageName.Buffer, chars) };
                    let name = String::from_utf16_lossy(slice);
                    self.names.push((pid, name));
                }

                let threads_count = proc.NumberOfThreads as usize;
                let threads_start = offset + size_of::<SYSTEM_PROCESS_INFORMATION>();
                let threads_end =
                    threads_start.checked_add(threads_count.saturating_mul(size_of::<SYSTEM_THREAD_INFORMATION>()));

                if let Some(end) = threads_end {
                    let limit =
                        if proc.NextEntryOffset == 0 { len } else { (offset + proc.NextEntryOffset as usize).min(len) };
                    let valid_end = end.min(limit);
                    let actual_count = valid_end.saturating_sub(threads_start) / size_of::<SYSTEM_THREAD_INFORMATION>();

                    for i in 0..actual_count {
                        let t_offset = threads_start + i * size_of::<SYSTEM_THREAD_INFORMATION>();
                        // SAFETY: t_offset è all'interno della porzione valida dei thread.
                        let t = unsafe { &*self.ptr.add(t_offset).cast::<SYSTEM_THREAD_INFORMATION>() };

                        let tid = t.ClientId.UniqueThread as usize as u32;
                        let create = t.Reserved1[2] as u64; // CreateTime
                        let kernel = t.Reserved1[0] as u64; // KernelTime
                        let user = t.Reserved1[1] as u64; // UserTime
                        let cpu = kernel.wrapping_add(user);

                        out.push(ThreadRead { key: ThreadKey { pid, tid, create }, cpu });
                    }
                }
            }

            if proc.NextEntryOffset == 0 {
                break;
            }
            offset = match offset.checked_add(proc.NextEntryOffset as usize) {
                Some(next) => next,
                None => break,
            };
        }

        true
    }

    /// Riempie il buffer tramite `NtQuerySystemInformation`, riallocando con `VirtualAlloc` se necessario.
    fn query_system_processes(&mut self) -> Option<usize> {
        if self.ptr.is_null() {
            let p = unsafe { VirtualAlloc(null_mut(), INITIAL_BUFFER_SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            if p.is_null() {
                return None;
            }
            self.ptr = p.cast();
            self.capacity = INITIAL_BUFFER_SIZE;
        }

        for _ in 0..MAX_SCAN_TRIES {
            let mut needed = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.ptr.cast::<c_void>(),
                    self.capacity as u32,
                    &mut needed,
                )
            };

            if status == STATUS_INFO_LENGTH_MISMATCH {
                let needed = needed as usize;
                let wanted = needed.saturating_add(needed / 8).saturating_add(64 * 1024).max(self.capacity + 64 * 1024);

                if wanted > MAX_SCAN_BUFFER {
                    return None;
                }

                unsafe {
                    VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
                    let p = VirtualAlloc(null_mut(), wanted, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                    if p.is_null() {
                        self.ptr = null_mut();
                        self.capacity = 0;
                        return None;
                    }
                    self.ptr = p.cast();
                    self.capacity = wanted;
                }
                continue;
            }

            if status < 0 {
                return None;
            }

            return Some((needed as usize).min(self.capacity));
        }

        None
    }

    /// Restituisce il nome dell'immagine del processo con identificatore `pid` (se trovato nell'ultima scansione).
    pub fn process_name(&self, pid: u32, out: &mut String) -> bool {
        if let Some((_, name)) = self.names.iter().find(|(p, _)| *p == pid) {
            out.clear();
            out.push_str(name);
            true
        } else {
            false
        }
    }

    /// Rilascia la memoria virtuale del buffer al sistema operativo (VirtualFree MEM_RELEASE).
    pub fn release(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
            }
            self.ptr = null_mut();
            self.capacity = 0;
        }
        self.names.clear();
        self.names.shrink_to_fit();
    }
}

impl Default for ThreadScanner {
    fn default() -> ThreadScanner {
        ThreadScanner::new()
    }
}

impl Drop for ThreadScanner {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use crate::saturation::{SatState, SaturationDetector};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};

    #[test]
    fn scans_current_system_threads() {
        let mut scanner = ThreadScanner::new();
        let mut threads = Vec::new();
        assert!(scanner.scan(&mut threads));
        assert!(!threads.is_empty());

        let my_pid = unsafe { GetCurrentProcessId() };
        let my_tid = unsafe { GetCurrentThreadId() };

        // Deve trovare il thread corrente
        let found = threads.iter().any(|t| t.key.pid == my_pid && t.key.tid == my_tid);
        assert!(found, "thread corrente PID {my_pid} TID {my_tid} non trovato nella scansione");

        // Deve trovare il nome del processo
        let mut name = String::new();
        assert!(scanner.process_name(my_pid, &mut name));
        assert!(!name.is_empty(), "nome processo vuoto");
    }

    #[test]
    fn release_frees_buffer_and_can_rescan() {
        let mut scanner = ThreadScanner::new();
        let mut threads = Vec::new();
        assert!(scanner.scan(&mut threads));
        assert!(!threads.is_empty());
        assert!(!scanner.ptr.is_null());

        scanner.release();
        assert!(scanner.ptr.is_null());
        assert_eq!(scanner.capacity, 0);

        // Può riscansionare dopo il release
        threads.clear();
        assert!(scanner.scan(&mut threads));
        assert!(!threads.is_empty());
        assert!(!scanner.ptr.is_null());
    }

    #[test]
    fn busy_loop_thread_reaches_saturated_state() {
        let logical = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        }
        .max(1);
        let mut detector = SaturationDetector::new(logical);
        let mut scanner = ThreadScanner::new();
        let mut threads = Vec::new();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let worker = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        });

        let mut reached_saturated = false;
        let start = Instant::now();

        // Facciamo campionamenti ogni ~1 s per massimo 8 secondi
        while start.elapsed() < Duration::from_secs(8) {
            let wall_100ns = (start.elapsed().as_nanos() / 100) as u64;
            if scanner.scan(&mut threads) {
                let report = detector.update(wall_100ns, &threads, false);
                if matches!(report.state, SatState::Saturated(count) if count >= 1) {
                    reached_saturated = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(900));
        }

        stop.store(true, Ordering::Relaxed);
        let _ = worker.join();

        assert!(reached_saturated, "il thread al 100% non ha raggiunto lo stato Saturated entro 8 secondi");
    }

    #[test]
    #[ignore = "diagnostica e misura: tempo e memoria di scan()"]
    fn measure_scan_cost() {
        let mut scanner = ThreadScanner::new();
        let mut threads = Vec::with_capacity(8192);

        // Warmup
        assert!(scanner.scan(&mut threads));

        let iterations = 10;
        let mut total_duration = Duration::ZERO;
        let mut total_threads = 0usize;

        for _ in 0..iterations {
            let start = Instant::now();
            assert!(scanner.scan(&mut threads));
            total_duration += start.elapsed();
            total_threads += threads.len();
        }

        let avg_duration = total_duration / iterations;
        let avg_threads = total_threads / (iterations as usize);
        println!(
            "scan(): media su {iterations} iterazioni: {:.2} ms per scansione, {} thread totali, buffer allocato: {} KiB",
            avg_duration.as_secs_f64() * 1000.0,
            avg_threads,
            scanner.capacity / 1024
        );
        assert!(avg_duration < Duration::from_millis(50));
    }
}
