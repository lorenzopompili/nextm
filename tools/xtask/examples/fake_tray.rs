//! Finto nextm per provare `xtask bench` senza avere l'app vera.
//!
//! Imita lo scheletro di nextm: sottosistema "windows" (nessuna console), una
//! finestra top-level nascosta di classe `nextm-main` (`WS_POPUP`, mai mostrata),
//! un timer da 1 s come quello del campionamento, chiusura pulita su `WM_CLOSE`
//! (`DestroyWindow`, poi `PostQuitMessage` su `WM_DESTROY`). Non disegna nulla e
//! non mette icone nella tray.
//!
//! Prova: cargo build --release --example fake_tray
//!        cargo run --release -- bench target/release/examples/fake_tray.exe --seconds 5 --warmup 1
#![windows_subsystem = "windows"]

use std::process::ExitCode;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostQuitMessage,
    RegisterClassW, SetTimer, TranslateMessage, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

const CLASS_NAME: &str = "nextm-main";
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 1000;

/// Stringa UTF-16 terminata da NUL.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        // Qui nextm campionerebbe le metriche e aggiornerebbe le icone.
        WM_TIMER => 0,
        WM_CLOSE => {
            // SICUREZZA: `hwnd` è la finestra che sta ricevendo il messaggio.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            // SICUREZZA: nessun puntatore in gioco; termina il ciclo dei messaggi.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SICUREZZA: si inoltrano invariati i parametri ricevuti dal sistema.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn main() -> ExitCode {
    let class_name = wide(CLASS_NAME);
    // SICUREZZA: con NULL restituisce il modulo del processo corrente.
    let instance = unsafe { GetModuleHandleW(null()) };

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SICUREZZA: `class` e la stringa del nome restano vivi per tutta la chiamata.
    if unsafe { RegisterClassW(&class) } == 0 {
        return ExitCode::FAILURE;
    }

    // SICUREZZA: la classe è registrata; nome e classe sono stringhe terminate da NUL.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    if hwnd.is_null() {
        return ExitCode::FAILURE;
    }

    // SICUREZZA: `hwnd` è la finestra appena creata da questo thread.
    if unsafe { SetTimer(hwnd, TIMER_ID, TIMER_MS, None) } == 0 {
        return ExitCode::FAILURE;
    }

    // Ciclo dei messaggi: GetMessageW restituisce 0 su WM_QUIT e -1 in caso di errore.
    // SICUREZZA: `message` è una MSG valida da riempire.
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        // SICUREZZA: `message` è stata appena riempita da GetMessageW.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    ExitCode::SUCCESS
}
