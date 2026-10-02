//! Taskbar, DPI, avvio di programmi e finestre di messaggio.
//!
//! Niente ShellExecute: caricherebbe nel processo DLL della shell che poi restano in memoria.
//! Programmi, file e URI si aprono con CreateProcessW, delegando a explorer.exe quando serve.

use core::mem::zeroed;
use core::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ELEVATION_REQUIRED, GetLastError, HWND};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsWindow, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW, SM_CXSMICON,
};

/// La finestra della taskbar principale (`Shell_TrayWnd`), nulla se Explorer non c'è.
pub fn taskbar() -> HWND {
    // SAFETY: nome di classe terminato da zero.
    unsafe { FindWindowW(wide!("Shell_TrayWnd").as_ptr(), null()) }
}

/// `true` se la finestra esiste ancora.
#[allow(dead_code)]
pub fn is_window(hwnd: HWND) -> bool {
    // SAFETY: IsWindow accetta qualunque valore.
    !hwnd.is_null() && unsafe { IsWindow(hwnd) } != 0
}

/// DPI della finestra; 96 se non disponibile.
pub fn dpi_of(hwnd: HWND) -> u32 {
    if hwnd.is_null() {
        return 96;
    }
    // SAFETY: finestra valida o nulla (gestita sopra).
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        d => d,
    }
}

/// Lato in pixel delle icone piccole (quelle della tray) a questo DPI.
pub fn small_icon_size(dpi: u32) -> u32 {
    // SAFETY: indice di metrica valido.
    let s = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if s > 0 { s as u32 } else { (16 * dpi / 96).max(16) }
}

/// Finestra di messaggio informativa, in primo piano. Blocca fino alla chiusura.
pub fn message_box(owner: HWND, title: &[u16], text: &[u16]) {
    // SAFETY: stringhe terminate da zero.
    unsafe { MessageBoxW(owner, text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND) };
}

/// Apre Gestione attività. Per gli amministratori Windows chiede l'elevazione: in quel caso
/// si passa da explorer.exe, che la gestisce senza caricare nulla in nextm.
#[allow(dead_code)]
pub fn open_task_manager() {
    let mut taskmgr = system_dir();
    taskmgr.extend("\\Taskmgr.exe".encode_utf16());
    if let Err(ERROR_ELEVATION_REQUIRED) = spawn(&taskmgr, &quoted(&taskmgr)) {
        open_with_explorer(&taskmgr);
    }
}

/// Apre un file o un URI (es. `ms-settings:taskbar`) con l'azione predefinita, tramite explorer.exe.
/// L'eventuale zero finale di `target` viene ignorato.
pub fn open_with_explorer(target: &[u16]) {
    let target = &target[..target.iter().position(|&c| c == 0).unwrap_or(target.len())];
    let mut explorer = windows_dir();
    explorer.extend("\\explorer.exe".encode_utf16());
    let mut cmd = quoted(&explorer);
    cmd.push(b' ' as u16);
    cmd.extend(quoted(target));
    let _ = spawn(&explorer, &cmd);
}

/// Avvia `app` con la riga di comando `cmdline` (entrambe senza zero finale).
/// Restituisce il codice d'errore di Windows se non parte.
fn spawn(app: &[u16], cmdline: &[u16]) -> Result<(), u32> {
    let app: Vec<u16> = app.iter().copied().chain(core::iter::once(0)).collect();
    // CreateProcessW può modificare la riga di comando: serve un buffer scrivibile.
    let mut cmd: Vec<u16> = cmdline.iter().copied().chain(core::iter::once(0)).collect();
    // SAFETY: strutture azzerate con cb corretto; stringhe terminate da zero; handle chiusi subito.
    unsafe {
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = zeroed();
        if CreateProcessW(app.as_ptr(), cmd.as_mut_ptr(), null(), null(), 0, 0, null(), null(), &si, &mut pi) == 0 {
            return Err(GetLastError());
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    Ok(())
}

/// Mette tra virgolette (sempre: i percorsi possono contenere spazi).
fn quoted(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'"' as u16);
    out.extend_from_slice(s);
    out.push(b'"' as u16);
    out
}

fn system_dir() -> Vec<u16> {
    let mut buf = [0u16; 260];
    // SAFETY: buffer valido della dimensione indicata.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    buf[..n.min(buf.len())].to_vec()
}

fn windows_dir() -> Vec<u16> {
    let mut buf = [0u16; 260];
    // SAFETY: come sopra.
    let n = unsafe { GetWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    buf[..n.min(buf.len())].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr::null_mut;

    #[test]
    fn icon_size_scales_with_dpi() {
        assert_eq!(small_icon_size(96), 16);
        assert!(small_icon_size(144) >= 24);
    }

    #[test]
    fn missing_window_defaults_to_96_dpi() {
        assert_eq!(dpi_of(null_mut()), 96);
        assert!(!is_window(null_mut()));
    }

    #[test]
    fn system_folders_are_found() {
        let sys = String::from_utf16_lossy(&system_dir()).to_ascii_lowercase();
        let win = String::from_utf16_lossy(&windows_dir()).to_ascii_lowercase();
        assert!(sys.ends_with("\\system32"), "{sys}");
        assert!(sys.starts_with(&win), "{sys} / {win}");
    }

    #[test]
    fn quoting() {
        let q = quoted(&"C:\\a b\\c.exe".encode_utf16().collect::<Vec<u16>>());
        assert_eq!(String::from_utf16_lossy(&q), "\"C:\\a b\\c.exe\"");
    }

    #[test]
    fn spawn_reports_missing_programs() {
        let missing: Vec<u16> = "C:\\non\\esiste.exe".encode_utf16().collect();
        assert!(spawn(&missing, &quoted(&missing)).is_err());
    }
}
