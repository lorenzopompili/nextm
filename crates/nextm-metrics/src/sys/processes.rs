//! Scansione e controllo dei processi attivi di Windows via NtQuerySystemInformation.
//!
//! Raccoglie in un'unica chiamata al kernel ad altissima efficienza PID, nome dell'eseguibile,
//! numero di thread, memoria Working Set, memoria privata (commit) e tempo totale CPU.
//! Include la funzione `kill_process` per la terminazione pulita dei processi.

use core::mem::size_of;
use core::ptr::null_mut;

use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows_sys::Win32::Foundation::{CloseHandle, STATUS_INFO_LENGTH_MISMATCH};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows_sys::Win32::System::WindowsProgramming::{SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION};

use crate::inspect::ProcessRecord;

const INITIAL_BUFFER_SIZE: usize = 1024 * 1024; // 1 MiB
const MAX_SCAN_BUFFER: usize = 64 * 1024 * 1024; // 64 MiB
const MAX_SCAN_TRIES: usize = 8;

/// Scanner dei processi di sistema ad alte prestazioni.
pub struct ProcessScanner {
    ptr: *mut u8,
    capacity: usize,
}

// SAFETY: il buffer VirtualAlloc appartiene esclusivamente a questa istanza.
unsafe impl Send for ProcessScanner {}

impl Default for ProcessScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessScanner {
    /// Inizializza lo scanner. Nessuna memoria viene allocata fino alla prima chiamata a `scan()`.
    pub fn new() -> ProcessScanner {
        ProcessScanner { ptr: null_mut(), capacity: 0 }
    }

    /// Esegue la scansione di tutti i processi attivi di sistema.
    pub fn scan(&mut self, out: &mut Vec<ProcessRecord>) -> bool {
        let len = match self.query_system_processes() {
            Some(l) => l,
            None => return false,
        };

        out.clear();

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

            // Determina il nome del processo
            let name = if pid == 0 {
                String::from("System Idle Process")
            } else if pid == 4 {
                String::from("System")
            } else {
                let byte_len = proc.ImageName.Length as usize;
                let str_buf = proc.ImageName.Buffer as usize;
                if byte_len > 0
                    && str_buf >= buf_start
                    && str_buf.checked_add(byte_len).is_some_and(|end| end <= buf_end)
                {
                    let chars = byte_len / 2;
                    let slice = unsafe { core::slice::from_raw_parts(proc.ImageName.Buffer, chars) };
                    String::from_utf16_lossy(slice)
                } else {
                    String::from("<unknown>")
                }
            };

            // Somma il tempo CPU di tutti i thread del processo
            let threads_count = proc.NumberOfThreads as usize;
            let threads_start = offset + size_of::<SYSTEM_PROCESS_INFORMATION>();
            let threads_end =
                threads_start.checked_add(threads_count.saturating_mul(size_of::<SYSTEM_THREAD_INFORMATION>()));

            let mut cpu_time_100ns: u64 = 0;
            if let Some(end) = threads_end {
                let limit =
                    if proc.NextEntryOffset == 0 { len } else { (offset + proc.NextEntryOffset as usize).min(len) };
                let valid_end = end.min(limit);
                let actual_count = valid_end.saturating_sub(threads_start) / size_of::<SYSTEM_THREAD_INFORMATION>();

                for i in 0..actual_count {
                    let t_offset = threads_start + i * size_of::<SYSTEM_THREAD_INFORMATION>();
                    // SAFETY: t_offset è all'interno della porzione valida dei thread.
                    let t = unsafe { &*self.ptr.add(t_offset).cast::<SYSTEM_THREAD_INFORMATION>() };

                    let kernel = t.Reserved1[0] as u64; // KernelTime
                    let user = t.Reserved1[1] as u64; // UserTime
                    cpu_time_100ns = cpu_time_100ns.wrapping_add(kernel.wrapping_add(user));
                }
            }

            out.push(ProcessRecord {
                pid,
                name,
                threads: proc.NumberOfThreads,
                working_set_bytes: proc.WorkingSetSize as u64,
                private_bytes: proc.PrivatePageCount as u64,
                cpu_time_100ns,
                io_read_bytes: proc.Reserved7[3] as u64,
                io_write_bytes: proc.Reserved7[4] as u64,
            });

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

    /// Rilascia la memoria allocata con `VirtualAlloc`.
    pub fn release(&mut self) {
        if !self.ptr.is_null() {
            unsafe { VirtualFree(self.ptr.cast(), 0, MEM_RELEASE) };
            self.ptr = null_mut();
            self.capacity = 0;
        }
    }

    /// Termina forzatamente il processo con il PID specificato.
    pub fn kill_process(pid: u32) -> bool {
        if pid == 0 || pid == 4 {
            return false;
        }

        // SAFETY: chiamata Win32 a OpenProcess con privilegi di terminazione.
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return false;
        }

        let ret = unsafe { TerminateProcess(handle, 1) };
        unsafe { CloseHandle(handle) };
        ret != 0
    }

    /// Sospende temporaneamente l'esecuzione di tutti i thread del processo specificato.
    #[allow(clippy::manual_c_str_literals)]
    pub fn suspend_process(pid: u32) -> bool {
        if pid == 0 || pid == 4 {
            return false;
        }

        const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
        let handle = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid) };
        if handle.is_null() {
            return false;
        }

        let ntdll = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(b"ntdll.dll\0".as_ptr()) };
        let mut ok = false;
        if !ntdll.is_null() {
            let proc = unsafe {
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(ntdll, b"NtSuspendProcess\0".as_ptr())
            };
            if let Some(nt_suspend) = proc {
                type FnNtSuspend = unsafe extern "system" fn(windows_sys::Win32::Foundation::HANDLE) -> i32;
                let fn_call: FnNtSuspend = unsafe { core::mem::transmute(nt_suspend) };
                let status = unsafe { fn_call(handle) };
                ok = status >= 0;
            }
        }
        unsafe { CloseHandle(handle) };
        ok
    }

    /// Riprende l'esecuzione dei thread del processo sospeso.
    #[allow(clippy::manual_c_str_literals)]
    pub fn resume_process(pid: u32) -> bool {
        if pid == 0 || pid == 4 {
            return false;
        }

        const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
        let handle = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid) };
        if handle.is_null() {
            return false;
        }

        let ntdll = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(b"ntdll.dll\0".as_ptr()) };
        let mut ok = false;
        if !ntdll.is_null() {
            let proc = unsafe {
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(ntdll, b"NtResumeProcess\0".as_ptr())
            };
            if let Some(nt_resume) = proc {
                type FnNtResume = unsafe extern "system" fn(windows_sys::Win32::Foundation::HANDLE) -> i32;
                let fn_call: FnNtResume = unsafe { core::mem::transmute(nt_resume) };
                let status = unsafe { fn_call(handle) };
                ok = status >= 0;
            }
        }
        unsafe { CloseHandle(handle) };
        ok
    }

    /// Attiva o disattiva la Modalità Efficienza (EcoQoS e priorità minima) sul processo.
    #[allow(clippy::manual_c_str_literals)]
    pub fn set_process_ecoqos(pid: u32, enable: bool) -> bool {
        if pid == 0 || pid == 4 {
            return false;
        }

        const PROCESS_SET_INFORMATION: u32 = 0x0200;
        let handle = unsafe { OpenProcess(PROCESS_SET_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }

        #[repr(C)]
        struct ProcessPowerThrottlingState {
            version: u32,
            control_mask: u32,
            state_mask: u32,
        }
        let state = ProcessPowerThrottlingState {
            version: 1,
            control_mask: 1, // PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            state_mask: if enable { 1 } else { 0 },
        };

        let kernel32 =
            unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(b"kernel32.dll\0".as_ptr()) };
        let mut throttled = false;
        if !kernel32.is_null() {
            let proc = unsafe {
                windows_sys::Win32::System::LibraryLoader::GetProcAddress(kernel32, b"SetProcessInformation\0".as_ptr())
            };
            if let Some(set_info) = proc {
                type FnSetInfo = unsafe extern "system" fn(
                    windows_sys::Win32::Foundation::HANDLE,
                    u32,
                    *const core::ffi::c_void,
                    u32,
                ) -> i32;
                let fn_call: FnSetInfo = unsafe { core::mem::transmute(set_info) };
                let ret = unsafe {
                    fn_call(
                        handle,
                        4, // ProcessPowerThrottling
                        &state as *const _ as _,
                        core::mem::size_of::<ProcessPowerThrottlingState>() as u32,
                    )
                };
                throttled = ret != 0;
            }
        }

        const IDLE_PRIORITY_CLASS: u32 = 0x00000040;
        const NORMAL_PRIORITY_CLASS: u32 = 0x00000020;
        let prio = if enable { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
        let prio_ret = unsafe { windows_sys::Win32::System::Threading::SetPriorityClass(handle, prio) };

        unsafe { CloseHandle(handle) };
        throttled || prio_ret != 0
    }

    fn query_system_processes(&mut self) -> Option<usize> {
        if self.ptr.is_null() {
            let p = unsafe { VirtualAlloc(null_mut(), INITIAL_BUFFER_SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            if p.is_null() {
                return None;
            }
            self.ptr = p.cast();
            self.capacity = INITIAL_BUFFER_SIZE;
        }

        let mut tries = 0;
        loop {
            let mut returned_len: u32 = 0;
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.ptr.cast(),
                    self.capacity as u32,
                    &mut returned_len,
                )
            };

            if status == 0 {
                let len = if returned_len > 0 { (returned_len as usize).min(self.capacity) } else { self.capacity };
                return Some(len);
            }

            if status != STATUS_INFO_LENGTH_MISMATCH || tries >= MAX_SCAN_TRIES {
                return None;
            }

            tries += 1;
            let mut new_cap = self.capacity.saturating_mul(2);
            if (returned_len as usize) > new_cap {
                new_cap = (returned_len as usize).saturating_add(64 * 1024);
            }
            if new_cap > MAX_SCAN_BUFFER {
                return None;
            }

            unsafe { VirtualFree(self.ptr.cast(), 0, MEM_RELEASE) };
            let p = unsafe { VirtualAlloc(null_mut(), new_cap, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            if p.is_null() {
                self.ptr = null_mut();
                self.capacity = 0;
                return None;
            }
            self.ptr = p.cast();
            self.capacity = new_cap;
        }
    }
}

impl Drop for ProcessScanner {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_live_system_processes() {
        let mut scanner = ProcessScanner::new();
        let mut procs = Vec::new();
        let ok = scanner.scan(&mut procs);
        assert!(ok);
        assert!(!procs.is_empty(), "ci devono essere processi attivi");

        // System Idle Process (PID 0) e System (PID 4)
        assert!(procs.iter().any(|p| p.pid == 0));
        assert!(procs.iter().any(|p| p.pid == 4));

        // Processo corrente
        let my_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
        let me = procs.iter().find(|p| p.pid == my_pid);
        assert!(me.is_some(), "il processo di test deve essere presente nell'elenco");
        let me = me.expect("me");
        assert!(me.working_set_bytes > 0);
        assert!(me.threads >= 1);
        assert!(procs.iter().any(|p| p.io_read_bytes > 0 || p.io_write_bytes > 0));
    }
}
