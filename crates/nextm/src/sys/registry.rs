//! Registro di sistema tramite l'API set `api-ms-win-core-registry-l1-1-0` (niente advapi32).
//!
//! Tipi e costanti sono definiti qui a mano: la feature `Win32_System_Registry` di windows-sys
//! dichiarerebbe le stesse funzioni su advapi32.dll e il linker userebbe quelle.

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::core::PCWSTR;

pub type Hkey = *mut c_void;

/// `HKEY_CURRENT_USER` (0x80000001 esteso con segno, come nell'header C).
pub const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize as Hkey;

const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_BINARY: u32 = 0x0000_0008;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_OPTION_NON_VOLATILE: u32 = 0;

windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegGetValueW(hkey: Hkey, lpsubkey: PCWSTR, lpvalue: PCWSTR, dwflags: u32, pdwtype: *mut u32, pvdata: *mut c_void, pcbdata: *mut u32) -> WIN32_ERROR);
windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegCreateKeyExW(hkey: Hkey, lpsubkey: PCWSTR, reserved: u32, lpclass: PCWSTR, dwoptions: u32, samdesired: u32, lpsecurityattributes: *const c_void, phkresult: *mut Hkey, lpdwdisposition: *mut u32) -> WIN32_ERROR);
windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegOpenKeyExW(hkey: Hkey, lpsubkey: PCWSTR, uloptions: u32, samdesired: u32, phkresult: *mut Hkey) -> WIN32_ERROR);
windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegSetValueExW(hkey: Hkey, lpvaluename: PCWSTR, reserved: u32, dwtype: u32, lpdata: *const u8, cbdata: u32) -> WIN32_ERROR);
windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegDeleteValueW(hkey: Hkey, lpvaluename: PCWSTR) -> WIN32_ERROR);
windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegCloseKey(hkey: Hkey) -> WIN32_ERROR);

/// Chiave aperta in scrittura, chiusa automaticamente.
struct Key(Hkey);

impl Key {
    /// Apre (creandola se serve) `HKCU\sub` in scrittura.
    fn create(sub: &[u16]) -> Option<Key> {
        let mut hk: Hkey = null_mut();
        // SAFETY: `sub` è terminata da zero; `hk` è un puntatore valido.
        let r = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut hk,
                null_mut(),
            )
        };
        (r == ERROR_SUCCESS).then_some(Key(hk))
    }

    /// Apre `HKCU\sub` in scrittura se esiste.
    fn open(sub: &[u16]) -> Option<Key> {
        let mut hk: Hkey = null_mut();
        // SAFETY: come sopra.
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut hk) };
        (r == ERROR_SUCCESS).then_some(Key(hk))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: la chiave è stata aperta da noi e si chiude una sola volta.
        unsafe { RegCloseKey(self.0) };
    }
}

/// Legge un `REG_DWORD` da `HKCU\sub\name`. Stringhe con zero finale.
pub fn get_dword(sub: &[u16], name: &[u16]) -> Option<u32> {
    let mut v: u32 = 0;
    let mut cb: u32 = 4;
    // SAFETY: stringhe terminate da zero; `v` e `cb` validi, `cb` = dimensione di `v`.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut v).cast(),
            &mut cb,
        )
    };
    (r == ERROR_SUCCESS).then_some(v)
}

/// Legge un `REG_SZ` in `out` (con zero finale); restituisce il numero di unità senza lo zero.
pub fn get_string(sub: &[u16], name: &[u16], out: &mut [u16]) -> Option<usize> {
    let mut cb: u32 = (out.len() * 2) as u32;
    // SAFETY: `out` ha `cb` byte scrivibili.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            out.as_mut_ptr().cast(),
            &mut cb,
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    Some((cb as usize / 2).saturating_sub(1))
}

/// Legge un `REG_BINARY` in `out`; restituisce il numero di byte letti.
pub fn get_binary(sub: &[u16], name: &[u16], out: &mut [u8]) -> Option<usize> {
    let mut cb: u32 = out.len() as u32;
    // SAFETY: `out` ha `cb` byte scrivibili.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_BINARY,
            null_mut(),
            out.as_mut_ptr().cast(),
            &mut cb,
        )
    };
    (r == ERROR_SUCCESS).then_some(cb as usize)
}

/// Scrive un `REG_DWORD` in `HKCU\sub\name` (creando la chiave se serve).
pub fn set_dword(sub: &[u16], name: &[u16], value: u32) -> bool {
    let Some(key) = Key::create(sub) else { return false };
    // SAFETY: 4 byte validi letti da `value`.
    unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_DWORD, (&raw const value).cast(), 4) == ERROR_SUCCESS }
}

/// Scrive un `REG_SZ`; `value` deve terminare con zero.
pub fn set_string(sub: &[u16], name: &[u16], value: &[u16]) -> bool {
    let Some(key) = Key::create(sub) else { return false };
    // SAFETY: `value` ha `value.len() * 2` byte leggibili, zero finale compreso.
    unsafe {
        RegSetValueExW(key.0, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
            == ERROR_SUCCESS
    }
}

/// Cancella un valore; `true` anche se non esisteva.
pub fn delete_value(sub: &[u16], name: &[u16]) -> bool {
    let Some(key) = Key::open(sub) else { return true };
    // SAFETY: chiave aperta, nome terminato da zero.
    let r = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
    r == ERROR_SUCCESS || r == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUB: &[u16] = wide!("Software\\nextm-test-registry");

    #[test]
    fn dword_string_roundtrip_and_delete() {
        assert!(set_dword(SUB, wide!("D"), 1234));
        assert_eq!(get_dword(SUB, wide!("D")), Some(1234));
        assert!(set_string(SUB, wide!("S"), wide!("ciao")));
        let mut buf = [0u16; 16];
        let n = get_string(SUB, wide!("S"), &mut buf).unwrap();
        assert_eq!(String::from_utf16_lossy(&buf[..n]), "ciao");
        assert!(delete_value(SUB, wide!("D")));
        assert!(delete_value(SUB, wide!("S")));
        assert_eq!(get_dword(SUB, wide!("D")), None);
        assert!(delete_value(SUB, wide!("NonEsiste")));
    }

    #[test]
    fn reads_the_taskbar_theme_value() {
        let v = get_dword(
            wide!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            wide!("SystemUsesLightTheme"),
        );
        assert!(matches!(v, None | Some(0) | Some(1)));
    }
}
