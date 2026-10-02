//! Output testuale per `--version` e `--diagnose` (nextm è un'app a finestre, senza console propria).

use core::ptr::null;

use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE, WriteConsoleW,
};

/// Scrive `text` nella console da cui è stato lanciato nextm; `false` se non ce n'è una.
pub fn write_to_parent_console(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: buffer valido della lunghezza indicata.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return false;
        }
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut written = 0u32;
        WriteConsoleW(out, wide.as_ptr(), wide.len() as u32, &mut written, null()) != 0
    }
}
