//! Rilevamento delle finestre bloccate ("Non risponde") e terminazione rapida (Quick Kill).

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible,
};
use windows_sys::core::BOOL;

/// Informazioni su una finestra applicativa che non risponde più al sistema operativo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HungWindow {
    pub pid: u32,
    pub title: String,
}

/// Esegue la scansione di tutte le finestre visibili per identificare quelle bloccate (`IsHungAppWindow`).
pub fn scan_hung_windows() -> Vec<HungWindow> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd) != 0 && IsHungAppWindow(hwnd) != 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid != 0 {
                    let mut title_buf = [0u16; 256];
                    let len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256);
                    let title = if len > 0 {
                        String::from_utf16_lossy(&title_buf[..len as usize])
                    } else {
                        String::from("Applicazione")
                    };
                    let list = &mut *(lparam as *mut Vec<HungWindow>);
                    if !list.iter().any(|h| h.pid == pid) {
                        list.push(HungWindow { pid, title });
                    }
                }
            }
            1
        }
    }

    let mut result = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc), (&raw mut result) as isize);
    }
    result
}

/// Termina immediatamente un processo bloccato dato il suo PID (Quick Kill).
pub fn quick_terminate_process(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let success = TerminateProcess(handle, 1) != 0;
        CloseHandle(handle);
        success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_hung_windows_does_not_crash() {
        let hung = scan_hung_windows();
        eprintln!("Finestre bloccate trovate: {:?}", hung);
    }
}
