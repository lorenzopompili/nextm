//! Menu scuri con il tema scuro, tramite due funzioni NON documentate di uxtheme.dll
//! (ordinali 135 `SetPreferredAppMode` e 136 `FlushMenuThemes`, usate da molte app Win32).
//! Se mancano, i menu restano chiari: nessun errore.

use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

/// Imposta il tema dei menu del processo. uxtheme resta caricata: i menu con tema la usano comunque.
pub fn set_menu_theme(dark: bool) {
    // SAFETY: DLL di sistema caricata da System32; i puntatori restituiti hanno le firme
    // usate da tutte le app che adottano queste funzioni.
    unsafe {
        let ux = LoadLibraryExW(wide!("uxtheme.dll").as_ptr(), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if ux.is_null() {
            return;
        }
        let (Some(set_mode), Some(flush)) =
            (GetProcAddress(ux, 135usize as *const u8), GetProcAddress(ux, 136usize as *const u8))
        else {
            FreeLibrary(ux);
            return;
        };
        let set_mode: SetPreferredAppMode = core::mem::transmute(set_mode);
        let flush: FlushMenuThemes = core::mem::transmute(flush);
        set_mode(if dark { FORCE_DARK } else { FORCE_LIGHT });
        flush();
        FreeLibrary(ux);
    }
}
