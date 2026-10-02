//! La finestra nascosta che possiede le icone e riceve i messaggi di sistema.

use core::mem::zeroed;
use core::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetMessageW, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, MSG, RegisterClassExW,
    RegisterWindowMessageW, TranslateMessage, WM_APP, WNDCLASSEXW, WNDPROC, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// Nome della classe della finestra (usato anche per trovare un'istanza già attiva).
pub const CLASS_NAME: &[u16] = wide!("nextm-main");
/// Messaggio con cui la shell notifica gli eventi delle icone.
pub const WM_TRAY: u32 = WM_APP + 1;

/// Crea la finestra top-level nascosta (mai mostrata). Non è una message-only window:
/// deve ricevere i messaggi broadcast come `TaskbarCreated` e `WM_SETTINGCHANGE`.
#[allow(clippy::manual_dangling_ptr)]
const fn make_int_resource(id: u16) -> *const u16 {
    id as usize as *const u16
}

pub fn create_hidden(wndproc: WNDPROC) -> Option<HWND> {
    // SAFETY: strutture inizializzate a zero e poi compilate; stringhe terminate da zero.
    unsafe {
        let instance = GetModuleHandleW(core::ptr::null());
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = wndproc;
        wc.hInstance = instance;
        let hicon_big = LoadImageW(instance, make_int_resource(1), IMAGE_ICON, 32, 32, LR_DEFAULTCOLOR);
        let hicon_sm = LoadImageW(instance, make_int_resource(1), IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR);
        if !hicon_big.is_null() {
            wc.hIcon = hicon_big as _;
        }
        if !hicon_sm.is_null() {
            wc.hIconSm = hicon_sm as _;
        }
        wc.lpszClassName = CLASS_NAME.as_ptr();
        if RegisterClassExW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return None;
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS_NAME.as_ptr(),
            wide!("nextm").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            core::ptr::null(),
        );
        (!hwnd.is_null()).then_some(hwnd)
    }
}

/// Registra (o recupera) un messaggio di sistema per nome.
pub fn register_message(name: &[u16]) -> u32 {
    // SAFETY: nome terminato da zero.
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
}

/// Consente a un messaggio di passare il filtro UIPI (User Interface Privilege Isolation)
/// quando il processo corrente è in esecuzione con privilegi elevati (Amministratore).
pub fn allow_message_through_uipi(hwnd: HWND, message: u32) {
    const MSGFLT_ALLOW: u32 = 1;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::ChangeWindowMessageFilterEx(
            hwnd,
            message,
            MSGFLT_ALLOW,
            core::ptr::null_mut(),
        );
    }
}

/// Ciclo dei messaggi del thread; termina con `WM_QUIT` e ne restituisce il codice.
pub fn run_message_loop() -> i32 {
    // SAFETY: `msg` è una struttura locale valida.
    unsafe {
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        msg.wParam as i32
    }
}
