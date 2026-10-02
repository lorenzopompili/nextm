//! Tutte le chiamate a Windows di nextm, racchiuse in involucri sicuri.

pub mod autostart;
pub mod console;
pub mod darkmenu;
pub mod elevation;
pub mod hover;
pub mod icon;
pub mod inspect_win;
pub mod menu;
pub mod power;
pub mod registry;
pub mod shell;
pub mod single;
pub mod store;
pub mod theme;
pub mod tray;
pub mod version;
pub mod window;

use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories};
use windows_sys::Win32::System::StationsAndDesktops::SetUserObjectInformationW;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::WindowsAndMessaging::UOI_TIMERPROC_EXCEPTION_SUPPRESSION;

/// Prime istruzioni del processo.
/// - Le DLL caricate dopo l'avvio si cercano solo in System32 (niente DLL "piantate" accanto all'exe).
/// - Un'eccezione dentro una callback di timer non viene soppressa in silenzio (consiglio della doc).
/// - Abilita SeDebugPrivilege se il processo è in esecuzione come amministratore.
pub fn harden() {
    // SAFETY: chiamate senza puntatori, oppure con un puntatore a una variabile locale valida.
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        let suppress: i32 = 0;
        SetUserObjectInformationW(
            GetCurrentProcess(),
            UOI_TIMERPROC_EXCEPTION_SUPPRESSION as i32,
            (&raw const suppress).cast(),
            size_of::<i32>() as u32,
        );
    }
    elevation::enable_debug_privilege();
}

/// Disattiva l'IME e il Text Services Framework per tutto il processo, prima di creare finestre.
/// nextm non riceve testo: senza questa chiamata Windows carica msctf e i suoi servizi, che
/// aggiungono thread e risvegli (misurato: da 11,8 a 6,6 cambi di contesto al secondo).
pub fn disable_ime() {
    // SAFETY: nessun parametro puntatore; u32::MAX = tutti i thread del processo.
    unsafe { windows_sys::Win32::UI::Input::Ime::ImmDisableIME(u32::MAX) };
}

/// Lingua dell'interfaccia di Windows (LANGID).
pub fn ui_langid() -> u16 {
    // SAFETY: nessun parametro.
    unsafe { GetUserDefaultUILanguage() }
}
