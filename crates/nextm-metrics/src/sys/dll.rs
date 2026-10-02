//! Caricamento su richiesta delle DLL opzionali, sempre e solo da System32.
//!
//! Una metrica che usa una DLL non di base (iphlpapi per la rete, advapi32 per PerfLib) la
//! carica quando viene attivata e la scarica quando viene spenta: una metrica mai attivata non
//! costa nulla. Niente import statici (il controllo `cargo xtask check-imports` li vieta) e niente
//! `/DELAYLOAD` (errori non intercettabili e ricerca nella cartella dell'exe).

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

/// Puntatore generico a una funzione esportata, da convertire con `core::mem::transmute`
/// nella firma esatta della funzione.
pub type RawProc = unsafe extern "system" fn() -> isize;

/// Una DLL di sistema caricata da System32, scaricata quando esce di scena.
pub struct Library(HMODULE);

impl Library {
    /// Carica `name` (solo il nome del file, es. `"iphlpapi.dll"`) dalla cartella di sistema.
    pub fn load(name: &str) -> Option<Library> {
        let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
        // SAFETY: nome terminato da zero; il flag limita la ricerca a System32.
        let h = unsafe { LoadLibraryExW(wide.as_ptr(), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        (!h.is_null()).then_some(Library(h))
    }

    /// Indirizzo della funzione `name` (ASCII terminato da zero, es. `b"GetIfEntry2\0"`).
    pub fn proc(&self, name: &[u8]) -> Option<RawProc> {
        if name.last() != Some(&0) {
            return None;
        }
        // SAFETY: modulo valido finché `self` vive; nome terminato da zero.
        unsafe { GetProcAddress(self.0, name.as_ptr()) }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: modulo caricato da noi, rilasciato una sola volta.
        unsafe { FreeLibrary(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_system_dll_and_finds_export() {
        let k = Library::load("kernel32.dll").expect("kernel32");
        assert!(k.proc(b"GetTickCount64\0").is_some());
        assert!(k.proc(b"NonEsiste\0").is_none());
    }

    #[test]
    fn rejects_names_without_terminator() {
        let k = Library::load("kernel32.dll").expect("kernel32");
        assert!(k.proc(b"GetTickCount64").is_none());
    }

    #[test]
    fn missing_dll_is_none() {
        assert!(Library::load("nextm-non-esiste.dll").is_none());
    }
}
