//! Scansione e controllo delle applicazioni configurate per l'avvio con Windows.
//!
//! Legge le stesse fonti usate da Gestione attività di Windows 11:
//! - Chiavi di registro `Run` (`HKCU`, `HKLM` 64-bit e `HKLM` WOW64 32-bit) con stato `StartupApproved`
//! - Cartelle `Startup` utente e comune con parser MS-SHLLINK (`.lnk`) e stato `StartupApproved\StartupFolder`
//! - App pacchettizzate Store/MSIX (`SystemAppData` + `AppxManifest.xml`)
//!
//! Zero import statici aggiuntivi: `advapi32.dll`, `version.dll` e `shlwapi.dll` vengono caricati
//! esclusivamente da System32 su richiesta tramite `Library::load` e scaricati al termine.

use core::ffi::c_void;
use core::mem::zeroed;
use core::ptr::{null, null_mut};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

use crate::inspect::{StartupEntry, StartupSource, StartupState, StartupToggleOutcome};
use crate::sys::dll::{Library, RawProc};

windows_link::link!("kernel32.dll" "system" fn ExpandEnvironmentStringsW(lpsrc: *const u16, lpdst: *mut u16, nsize: u32) -> u32);

type Hkey = *mut c_void;

const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize as Hkey;
const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002u32 as i32 as isize as Hkey;

const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_READ: u32 = 0x20019;
const KEY_WOW64_64KEY: u32 = 0x0100;
const KEY_WOW64_32KEY: u32 = 0x0200;

const REG_OPTION_NON_VOLATILE: u32 = 0;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;

const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x0000_0004;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

const ERROR_SUCCESS: u32 = 0;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;

type FnRegOpenKeyExW = unsafe extern "system" fn(Hkey, *const u16, u32, u32, *mut Hkey) -> u32;
type FnRegCreateKeyExW =
    unsafe extern "system" fn(Hkey, *const u16, u32, *const u16, u32, u32, *const c_void, *mut Hkey, *mut u32) -> u32;
type FnRegCloseKey = unsafe extern "system" fn(Hkey) -> u32;
type FnRegEnumValueW =
    unsafe extern "system" fn(Hkey, u32, *mut u16, *mut u32, *mut u32, *mut u32, *mut u8, *mut u32) -> u32;
type FnRegEnumKeyExW =
    unsafe extern "system" fn(Hkey, u32, *mut u16, *mut u32, *mut u32, *mut u16, *mut u32, *mut c_void) -> u32;
type FnRegGetValueW =
    unsafe extern "system" fn(Hkey, *const u16, *const u16, u32, *mut u32, *mut c_void, *mut u32) -> u32;
type FnRegSetValueExW = unsafe extern "system" fn(Hkey, *const u16, u32, u32, *const u8, u32) -> u32;

type FnGetFileVersionInfoSizeW = unsafe extern "system" fn(*const u16, *mut u32) -> u32;
type FnGetFileVersionInfoW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void) -> i32;
type FnVerQueryValueW = unsafe extern "system" fn(*const c_void, *const u16, *mut *const c_void, *mut u32) -> i32;

type FnShLoadIndirectString = unsafe extern "system" fn(*const u16, *mut u16, u32, *mut *mut c_void) -> i32;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Espande eventuali variabili d'ambiente `%VAR%` tramite `ExpandEnvironmentStringsW` (kernel32).
pub fn expand_env_strings(input: &str) -> String {
    if !input.contains('%') {
        return input.to_string();
    }
    let wide_in = to_wide(input);
    let mut buf = [0u16; 1024];
    // SAFETY: `wide_in` è terminata da zero e `buf` ha capacità nota.
    let needed = unsafe { ExpandEnvironmentStringsW(wide_in.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if needed == 0 {
        return input.to_string();
    }
    if (needed as usize) <= buf.len() {
        let len = (needed as usize).saturating_sub(1);
        return String::from_utf16_lossy(&buf[..len]);
    }
    let mut big = vec![0u16; needed as usize];
    // SAFETY: `big` ha la dimensione esatta richiesta.
    let n2 = unsafe { ExpandEnvironmentStringsW(wide_in.as_ptr(), big.as_mut_ptr(), big.len() as u32) };
    if n2 > 0 && (n2 as usize) <= big.len() {
        String::from_utf16_lossy(&big[..(n2 as usize).saturating_sub(1)])
    } else {
        input.to_string()
    }
}

struct OwnedHkey<'a> {
    hkey: Hkey,
    close_fn: FnRegCloseKey,
    _marker: core::marker::PhantomData<&'a ()>,
}

impl Drop for OwnedHkey<'_> {
    fn drop(&mut self) {
        if !self.hkey.is_null() {
            // SAFETY: handle aperto con successo e chiuso una sola volta.
            unsafe { (self.close_fn)(self.hkey) };
        }
    }
}

struct RegLib {
    _dll: Library,
    open: FnRegOpenKeyExW,
    create: FnRegCreateKeyExW,
    close: FnRegCloseKey,
    enum_val: FnRegEnumValueW,
    enum_key: FnRegEnumKeyExW,
    get_val: FnRegGetValueW,
    set_val: FnRegSetValueExW,
}

impl RegLib {
    fn load() -> Option<Self> {
        let dll = Library::load("advapi32.dll")?;
        let open_raw = dll.proc(b"RegOpenKeyExW\0")?;
        let create_raw = dll.proc(b"RegCreateKeyExW\0")?;
        let close_raw = dll.proc(b"RegCloseKey\0")?;
        let enum_val_raw = dll.proc(b"RegEnumValueW\0")?;
        let enum_key_raw = dll.proc(b"RegEnumKeyExW\0")?;
        let get_val_raw = dll.proc(b"RegGetValueW\0")?;
        let set_val_raw = dll.proc(b"RegSetValueExW\0")?;

        // SAFETY: i puntatori provengono da `advapi32.dll` con firme Win32 ufficiali.
        unsafe {
            Some(Self {
                _dll: dll,
                open: core::mem::transmute::<RawProc, FnRegOpenKeyExW>(open_raw),
                create: core::mem::transmute::<RawProc, FnRegCreateKeyExW>(create_raw),
                close: core::mem::transmute::<RawProc, FnRegCloseKey>(close_raw),
                enum_val: core::mem::transmute::<RawProc, FnRegEnumValueW>(enum_val_raw),
                enum_key: core::mem::transmute::<RawProc, FnRegEnumKeyExW>(enum_key_raw),
                get_val: core::mem::transmute::<RawProc, FnRegGetValueW>(get_val_raw),
                set_val: core::mem::transmute::<RawProc, FnRegSetValueExW>(set_val_raw),
            })
        }
    }

    fn open(&self, root: Hkey, sub: &str, access: u32) -> Result<OwnedHkey<'_>, u32> {
        let sub_w = to_wide(sub);
        let mut hk: Hkey = null_mut();
        // SAFETY: `sub_w` terminata da zero, `hk` valido.
        let r = unsafe { (self.open)(root, sub_w.as_ptr(), 0, access, &mut hk) };
        if r == ERROR_SUCCESS {
            Ok(OwnedHkey { hkey: hk, close_fn: self.close, _marker: core::marker::PhantomData })
        } else {
            Err(r)
        }
    }

    fn create(&self, root: Hkey, sub: &str, access: u32) -> Result<OwnedHkey<'_>, u32> {
        let sub_w = to_wide(sub);
        let mut hk: Hkey = null_mut();
        // SAFETY: `sub_w` terminata da zero, `hk` valido.
        let r = unsafe {
            (self.create)(root, sub_w.as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, access, null(), &mut hk, null_mut())
        };
        if r == ERROR_SUCCESS {
            Ok(OwnedHkey { hkey: hk, close_fn: self.close, _marker: core::marker::PhantomData })
        } else {
            Err(r)
        }
    }

    fn enum_subkeys(&self, hk: Hkey) -> Vec<String> {
        let mut out = Vec::new();
        let mut name_buf = vec![0u16; 512];
        let mut idx: u32 = 0;
        loop {
            let mut len = name_buf.len() as u32;
            // SAFETY: `name_buf` ha capacità `len`.
            let r = unsafe {
                (self.enum_key)(
                    hk,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut len,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r == ERROR_MORE_DATA && name_buf.len() < 4096 {
                name_buf.resize(name_buf.len() * 2, 0);
                continue;
            }
            if r == ERROR_SUCCESS {
                out.push(String::from_utf16_lossy(&name_buf[..len as usize]));
            }
            idx += 1;
        }
        out
    }

    fn enum_string_values(&self, hk: Hkey) -> Vec<(String, String, bool)> {
        let mut out = Vec::new();
        let mut name_buf = vec![0u16; 512];
        let mut data_buf = vec![0u8; 4096];
        let mut idx: u32 = 0;
        loop {
            let mut n_len = name_buf.len() as u32;
            let mut d_len = data_buf.len() as u32;
            let mut typ: u32 = 0;
            // SAFETY: buffer validi con lunghezze aggiornate ad ogni iterazione.
            let r = unsafe {
                (self.enum_val)(
                    hk,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut n_len,
                    null_mut(),
                    &mut typ,
                    data_buf.as_mut_ptr(),
                    &mut d_len,
                )
            };
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r == ERROR_MORE_DATA && (name_buf.len() < 8192 || data_buf.len() < 65536) {
                name_buf.resize((n_len as usize + 2).max(name_buf.len() * 2), 0);
                data_buf.resize((d_len as usize + 4).max(data_buf.len() * 2), 0);
                continue;
            }
            if r == ERROR_SUCCESS && n_len > 0 && (typ == REG_SZ || typ == REG_EXPAND_SZ) {
                let name = String::from_utf16_lossy(&name_buf[..n_len as usize]);
                let raw_str = decode_utf16_bytes(&data_buf[..(d_len as usize).min(data_buf.len())]);
                out.push((name, raw_str, typ == REG_EXPAND_SZ));
            }
            idx += 1;
        }
        out
    }

    fn enum_binary_values(&self, hk: Hkey) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut name_buf = vec![0u16; 512];
        let mut data_buf = vec![0u8; 64];
        let mut idx: u32 = 0;
        loop {
            let mut n_len = name_buf.len() as u32;
            let mut d_len = data_buf.len() as u32;
            let mut typ: u32 = 0;
            // SAFETY: buffer validi con lunghezze aggiornate.
            let r = unsafe {
                (self.enum_val)(
                    hk,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut n_len,
                    null_mut(),
                    &mut typ,
                    data_buf.as_mut_ptr(),
                    &mut d_len,
                )
            };
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r == ERROR_MORE_DATA && (name_buf.len() < 8192 || data_buf.len() < 8192) {
                name_buf.resize((n_len as usize + 2).max(name_buf.len() * 2), 0);
                data_buf.resize((d_len as usize + 4).max(data_buf.len() * 2), 0);
                continue;
            }
            if r == ERROR_SUCCESS && n_len > 0 && typ == REG_BINARY {
                let name = String::from_utf16_lossy(&name_buf[..n_len as usize]).to_lowercase();
                let bytes = data_buf[..(d_len as usize).min(data_buf.len())].to_vec();
                out.insert(name, bytes);
            }
            idx += 1;
        }
        out
    }

    fn get_string(&self, root: Hkey, sub: &str, val: &str) -> Option<String> {
        let sub_w = to_wide(sub);
        let val_w = to_wide(val);
        let mut buf = vec![0u16; 512];
        let mut cb: u32 = (buf.len() * 2) as u32;
        // SAFETY: stringhe terminate da zero; buffer scrivibile per `cb` byte.
        let mut r = unsafe {
            (self.get_val)(
                root,
                sub_w.as_ptr(),
                val_w.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut cb,
            )
        };
        if r == ERROR_MORE_DATA && cb > 0 && cb < 65536 {
            buf.resize((cb as usize) / 2 + 2, 0);
            cb = (buf.len() * 2) as u32;
            // SAFETY: buffer ridimensionato alla capacità richiesta.
            r = unsafe {
                (self.get_val)(
                    root,
                    sub_w.as_ptr(),
                    val_w.as_ptr(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut cb,
                )
            };
        }
        if r != ERROR_SUCCESS {
            return None;
        }
        let mut u16_len = (cb as usize / 2).min(buf.len());
        while u16_len > 0 && buf[u16_len - 1] == 0 {
            u16_len -= 1;
        }
        Some(String::from_utf16_lossy(&buf[..u16_len]))
    }

    fn get_dword(&self, root: Hkey, sub: &str, val: &str) -> Option<u32> {
        let sub_w = to_wide(sub);
        let val_w = to_wide(val);
        let mut v: u32 = 0;
        let mut cb: u32 = 4;
        // SAFETY: stringhe terminate da zero, `v` ha 4 byte validi.
        let r = unsafe {
            (self.get_val)(
                root,
                sub_w.as_ptr(),
                val_w.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&raw mut v).cast(),
                &mut cb,
            )
        };
        (r == ERROR_SUCCESS).then_some(v)
    }

    fn set_binary(&self, hk: Hkey, name: &str, data: &[u8]) -> Result<(), u32> {
        let name_w = to_wide(name);
        // SAFETY: chiave aperta in scrittura; `data` ha `data.len()` byte leggibili.
        let r = unsafe { (self.set_val)(hk, name_w.as_ptr(), 0, REG_BINARY, data.as_ptr(), data.len() as u32) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(r) }
    }

    fn set_dword(&self, hk: Hkey, name: &str, value: u32) -> Result<(), u32> {
        let name_w = to_wide(name);
        // SAFETY: chiave aperta in scrittura; `value` ha 4 byte leggibili.
        let r = unsafe { (self.set_val)(hk, name_w.as_ptr(), 0, REG_DWORD, (&raw const value).cast(), 4) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(r) }
    }
}

fn decode_utf16_bytes(bytes: &[u8]) -> String {
    let (chunks, _) = bytes.as_chunks::<2>();
    let mut u16s = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        u16s.push(u16::from_le_bytes(*chunk));
    }
    while u16s.last() == Some(&0) {
        u16s.pop();
    }
    String::from_utf16_lossy(&u16s)
}

#[derive(Clone, Default)]
struct ExeMeta {
    description: Option<String>,
    company: Option<String>,
}

struct VersionReader {
    _dll: Library,
    get_size: FnGetFileVersionInfoSizeW,
    get_info: FnGetFileVersionInfoW,
    ver_query: FnVerQueryValueW,
    cache: BTreeMap<String, ExeMeta>,
}

impl VersionReader {
    fn load() -> Option<Self> {
        let dll = Library::load("version.dll")?;
        let get_size_raw = dll.proc(b"GetFileVersionInfoSizeW\0")?;
        let get_info_raw = dll.proc(b"GetFileVersionInfoW\0")?;
        let ver_query_raw = dll.proc(b"VerQueryValueW\0")?;
        // SAFETY: puntatori validi esportati da `version.dll`.
        unsafe {
            Some(Self {
                _dll: dll,
                get_size: core::mem::transmute::<RawProc, FnGetFileVersionInfoSizeW>(get_size_raw),
                get_info: core::mem::transmute::<RawProc, FnGetFileVersionInfoW>(get_info_raw),
                ver_query: core::mem::transmute::<RawProc, FnVerQueryValueW>(ver_query_raw),
                cache: BTreeMap::new(),
            })
        }
    }

    fn read(&mut self, path: &str) -> ExeMeta {
        let key = path.to_ascii_lowercase();
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        let meta = self.query_uncached(path);
        self.cache.insert(key, meta.clone());
        meta
    }

    fn query_uncached(&self, path: &str) -> ExeMeta {
        let path_w = to_wide(path);
        let mut handle: u32 = 0;
        // SAFETY: `path_w` terminata da zero.
        let size = unsafe { (self.get_size)(path_w.as_ptr(), &mut handle) };
        if size == 0 || size > 4 * 1024 * 1024 {
            return ExeMeta::default();
        }
        let mut block = vec![0u8; size as usize];
        // SAFETY: `block` ha almeno `size` byte scrivibili.
        if unsafe { (self.get_info)(path_w.as_ptr(), 0, size, block.as_mut_ptr().cast()) } == 0 {
            return ExeMeta::default();
        }

        let mut langs: Vec<(u16, u16)> = Vec::with_capacity(6);
        let trans_key = to_wide("\\VarFileInfo\\Translation");
        let mut trans_ptr: *const c_void = null();
        let mut trans_bytes: u32 = 0;
        // SAFETY: `block` contiene il blocco versione validato sopra.
        if unsafe { (self.ver_query)(block.as_ptr().cast(), trans_key.as_ptr(), &mut trans_ptr, &mut trans_bytes) } != 0
            && !trans_ptr.is_null()
            && trans_bytes >= 4
        {
            let pair_count = (trans_bytes as usize) / 4;
            // SAFETY: `trans_ptr` punta dentro `block` per almeno `pair_count * 2` u16.
            let slice = unsafe { core::slice::from_raw_parts(trans_ptr.cast::<u16>(), pair_count * 2) };
            for pair in slice.as_chunks::<2>().0 {
                langs.push((pair[0], pair[1]));
            }
        }
        for fallback in [(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0410, 0x04B0), (0x0000, 0x04B0)] {
            if !langs.contains(&fallback) {
                langs.push(fallback);
            }
        }

        let mut description = None;
        let mut company = None;
        for &(lang, cp) in &langs {
            if description.is_none() {
                description = self
                    .query_field(&block, lang, cp, "FileDescription")
                    .filter(|s| !s.contains("TODO:"))
                    .or_else(|| self.query_field(&block, lang, cp, "ProductName"));
            }
            if company.is_none() {
                company = self.query_field(&block, lang, cp, "CompanyName");
            }
            if description.is_some() && company.is_some() {
                break;
            }
        }

        ExeMeta { description, company }
    }

    fn query_field(&self, block: &[u8], lang: u16, cp: u16, field: &str) -> Option<String> {
        let sub = format!("\\StringFileInfo\\{lang:04X}{cp:04X}\\{field}");
        let sub_w = to_wide(&sub);
        let mut val_ptr: *const c_void = null();
        let mut val_len: u32 = 0;
        // SAFETY: `block` è un buffer `VS_VERSION_INFO` valido; `sub_w` terminato da zero.
        let ok = unsafe { (self.ver_query)(block.as_ptr().cast(), sub_w.as_ptr(), &mut val_ptr, &mut val_len) };
        if ok == 0 || val_ptr.is_null() || val_len == 0 {
            return None;
        }
        // SAFETY: `val_ptr` punta ad una stringa wide di lunghezza `val_len` dentro `block`.
        let slice = unsafe { core::slice::from_raw_parts(val_ptr.cast::<u16>(), val_len as usize) };
        let mut trimmed = slice.len();
        while trimmed > 0 && slice[trimmed - 1] == 0 {
            trimmed -= 1;
        }
        let s = String::from_utf16_lossy(&slice[..trimmed]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

struct ResourceResolver {
    _dll: Library,
    load_indirect: FnShLoadIndirectString,
}

impl ResourceResolver {
    fn load() -> Option<Self> {
        let dll = Library::load("shlwapi.dll")?;
        let proc = dll.proc(b"SHLoadIndirectString\0")?;
        // SAFETY: puntatore valido esportato da `shlwapi.dll`.
        let load_indirect = unsafe { core::mem::transmute::<RawProc, FnShLoadIndirectString>(proc) };
        Some(Self { _dll: dll, load_indirect })
    }

    fn resolve(&self, pkg_full: &str, pkg_family: &str, raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if !trimmed.contains("ms-resource:") {
            return Some(trimmed.to_string()).filter(|s| !s.is_empty());
        }
        let base_pkg = pkg_family.split('_').next().unwrap_or(pkg_family);

        let mut candidates: Vec<String> = Vec::with_capacity(3);
        if trimmed.starts_with("@{") {
            candidates.push(trimmed.to_string());
        } else if trimmed.starts_with("ms-resource://") {
            candidates.push(format!("@{{{pkg_full}?{trimmed}}}"));
        } else if let Some(rest) = trimmed.strip_prefix("ms-resource:") {
            let key = rest.trim_start_matches('/');
            candidates.push(format!("@{{{pkg_full}?ms-resource://{base_pkg}/Resources/{key}}}"));
            candidates.push(format!("@{{{pkg_full}?ms-resource://{base_pkg}/{key}}}"));
        }

        let mut out = [0u16; 256];
        for cand in candidates {
            let in_w = to_wide(&cand);
            // SAFETY: `in_w` terminato da zero; `out` ha capacità 256 u16.
            let hr = unsafe { (self.load_indirect)(in_w.as_ptr(), out.as_mut_ptr(), out.len() as u32, null_mut()) };
            if hr == 0 {
                let end = out.iter().position(|&c| c == 0).unwrap_or(out.len());
                let s = String::from_utf16_lossy(&out[..end]).trim().to_string();
                if !s.is_empty() && !s.starts_with("ms-resource:") {
                    return Some(s);
                }
            }
        }
        None
    }
}

/// Risolve il percorso dell'eseguibile a partire da una riga di comando di avvio Windows.
pub fn extract_exe_path(expanded_cmd: &str) -> Option<String> {
    let s = expanded_cmd.trim();
    if s.is_empty() || s == "\"\"" || s == "''" {
        return None;
    }

    if let Some(after_quote) = s.strip_prefix('"') {
        let end = after_quote.find('"')?;
        let candidate = after_quote[..end].trim();
        return (!candidate.is_empty()).then(|| candidate.to_string());
    }

    if Path::new(s).is_file() {
        return Some(s.to_string());
    }

    // Cerca estensioni standard seguite da spazio o fine stringa (gestisce percorsi non quotati con spazi).
    let lower = s.to_ascii_lowercase();
    for ext in [".exe", ".bat", ".cmd", ".com", ".vbs", ".msc", ".lnk"] {
        let mut search_from = 0;
        while let Some(rel_idx) = lower[search_from..].find(ext) {
            let end = search_from + rel_idx + ext.len();
            let boundary_ok = end == s.len() || s.as_bytes().get(end).is_some_and(|b| b.is_ascii_whitespace());
            if boundary_ok {
                let cand = s[..end].trim();
                if Path::new(cand).is_file() || search_from == 0 {
                    return Some(cand.to_string());
                }
            }
            search_from = end;
        }
    }

    // Fallback: prima parte separata da spazio se esistente, altrimenti il primo token.
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_whitespace() {
            let cand = s[..i].trim();
            if Path::new(cand).is_file() {
                return Some(cand.to_string());
            }
        }
    }

    let first_tok = s.split_whitespace().next().unwrap_or("");
    (!first_tok.is_empty()).then(|| first_tok.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LnkTarget {
    path: String,
    args: String,
}

/// Estrae percorso destinazione e argomenti da un collegamento Windows (`.lnk`, formato `[MS-SHLLINK]`).
fn parse_lnk_target(bytes: &[u8]) -> Option<LnkTarget> {
    const LNK_CLSID: [u8; 16] =
        [0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

    if bytes.len() < 76 {
        return None;
    }
    let hdr_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if hdr_size != 0x4C || bytes[4..20] != LNK_CLSID {
        return None;
    }

    let flags = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    let mut pos: usize = 76;

    // Bit 0: HasLinkTargetIDList
    if (flags & 0x01) != 0 {
        let id_hdr = bytes.get(pos..pos + 2)?;
        let id_len = u16::from_le_bytes([id_hdr[0], id_hdr[1]]) as usize;
        pos = pos.checked_add(2 + id_len)?;
    }

    let mut path: Option<String> = None;

    // Bit 1: HasLinkInfo
    if (flags & 0x02) != 0
        && let Some(li_hdr) = bytes.get(pos..pos + 4)
    {
        let li_size = u32::from_le_bytes([li_hdr[0], li_hdr[1], li_hdr[2], li_hdr[3]]) as usize;
        if let Some(li) = bytes.get(pos..pos.checked_add(li_size)?) {
            if li.len() >= 28 {
                let li_hdr_size = u32::from_le_bytes([li[4], li[5], li[6], li[7]]) as usize;
                let li_flags = u32::from_le_bytes([li[8], li[9], li[10], li[11]]);
                if (li_flags & 0x01) != 0 {
                    if li_hdr_size >= 36 && li.len() >= 32 {
                        let uoff = u32::from_le_bytes([li[28], li[29], li[30], li[31]]) as usize;
                        if uoff > 0 && uoff < li.len() {
                            path = read_c_utf16(&li[uoff..]);
                        }
                    }
                    if path.is_none() {
                        let aoff = u32::from_le_bytes([li[16], li[17], li[18], li[19]]) as usize;
                        if aoff > 0 && aoff < li.len() {
                            path = read_c_ansi(&li[aoff..]);
                        }
                    }
                }
            }
            pos = pos.checked_add(li_size)?;
        }
    }

    let is_unicode = (flags & 0x80) != 0;
    let read_str_data = |p: &mut usize| -> Option<String> {
        let cnt_bytes = bytes.get(*p..(*p).checked_add(2)?)?;
        let count = u16::from_le_bytes([cnt_bytes[0], cnt_bytes[1]]) as usize;
        *p += 2;
        if is_unicode {
            let byte_len = count.checked_mul(2)?;
            let slice = bytes.get(*p..(*p).checked_add(byte_len)?)?;
            *p += byte_len;
            Some(decode_utf16_bytes(slice))
        } else {
            let slice = bytes.get(*p..(*p).checked_add(count)?)?;
            *p += count;
            Some(String::from_utf8_lossy(slice).to_string())
        }
    };

    if (flags & 0x04) != 0 {
        // HasName
        let _ = read_str_data(&mut pos);
    }
    let rel_path = if (flags & 0x08) != 0 { read_str_data(&mut pos) } else { None };
    let work_dir = if (flags & 0x10) != 0 { read_str_data(&mut pos) } else { None };
    let args = if (flags & 0x20) != 0 { read_str_data(&mut pos).unwrap_or_default() } else { String::new() };
    if (flags & 0x40) != 0 {
        // HasIconLocation
        let _ = read_str_data(&mut pos);
    }

    // ExtraData: cerca EnvironmentVariableDataBlock (0xA0000001) se LinkInfo mancava.
    if path.is_none() {
        while let Some(blk_hdr) = bytes.get(pos..pos + 4) {
            let blk_size = u32::from_le_bytes([blk_hdr[0], blk_hdr[1], blk_hdr[2], blk_hdr[3]]) as usize;
            if blk_size < 4 {
                break;
            }
            let Some(blk) = bytes.get(pos..pos.saturating_add(blk_size)) else { break };
            if blk.len() >= 8 {
                let sig = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
                if sig == 0xA000_0001 && blk.len() >= 788 {
                    if let Some(u_target) = read_c_utf16(&blk[268..788])
                        && !u_target.is_empty()
                    {
                        path = Some(expand_env_strings(&u_target));
                        break;
                    }
                    if let Some(a_target) = read_c_ansi(&blk[8..268])
                        && !a_target.is_empty()
                    {
                        path = Some(expand_env_strings(&a_target));
                        break;
                    }
                }
            }
            pos = match pos.checked_add(blk_size) {
                Some(next) => next,
                None => break,
            };
        }
    }

    if path.is_none()
        && let Some(rp) = rel_path
        && !rp.is_empty()
    {
        if let Some(wd) = work_dir
            && !wd.is_empty()
        {
            let joined = Path::new(&wd).join(&rp);
            if joined.exists() {
                path = Some(joined.to_string_lossy().to_string());
            }
        }
        if path.is_none() && Path::new(&rp).is_absolute() {
            path = Some(rp);
        }
    }

    path.filter(|p| !p.is_empty()).map(|p| LnkTarget { path: p, args })
}

fn read_c_ansi(slice: &[u8]) -> Option<String> {
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    let s = String::from_utf8_lossy(&slice[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn read_c_utf16(slice: &[u8]) -> Option<String> {
    let mut u16s = Vec::new();
    for ch in slice.as_chunks::<2>().0 {
        let v = u16::from_le_bytes(*ch);
        if v == 0 {
            break;
        }
        u16s.push(v);
    }
    let s = String::from_utf16_lossy(&u16s).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestStartupTask {
    task_id: String,
    display_name: Option<String>,
    executable: Option<String>,
}

#[derive(Debug, Default)]
struct ManifestInfo {
    pkg_display_name: Option<String>,
    publisher: Option<String>,
    tasks: Vec<ManifestStartupTask>,
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

fn xml_attr(tag: &str, attr: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let ab = attr.as_bytes();
    let mut i = 0;
    while i + ab.len() < bytes.len() {
        let prev_ok = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if prev_ok && bytes[i..i + ab.len()].eq_ignore_ascii_case(ab) {
            let mut j = i + ab.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'=') {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if let Some(&q) = bytes.get(j)
                    && (q == b'"' || q == b'\'')
                {
                    j += 1;
                    if let Some(rel_end) = bytes[j..].iter().position(|&b| b == q) {
                        let raw = &tag[j..j + rel_end];
                        return Some(unescape_xml(raw));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn xml_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let after_open = xml[start..].find('>')? + start + 1;
    let end = xml[after_open..].find(&close)? + after_open;
    let s = unescape_xml(xml[after_open..end].trim());
    (!s.is_empty()).then_some(s)
}

fn parse_appx_startup_tasks(xml: &str) -> ManifestInfo {
    let mut info = ManifestInfo {
        pkg_display_name: xml_tag_text(xml, "DisplayName"),
        publisher: xml_tag_text(xml, "PublisherDisplayName"),
        tasks: Vec::new(),
    };

    let mut cur_app_exe: Option<String> = None;
    let mut cur_app_dn: Option<String> = None;
    let mut cur_ext_exe: Option<String> = None;
    let mut in_startup_ext = false;

    let mut pos = 0;
    while let Some(lt_rel) = xml[pos..].find('<') {
        let lt = pos + lt_rel;
        let Some(gt_rel) = xml[lt..].find('>') else { break };
        let gt = lt + gt_rel;
        let tag = &xml[lt + 1..gt];
        pos = gt + 1;

        let trimmed = tag.trim();
        if trimmed.starts_with('!') || trimmed.starts_with('?') {
            continue;
        }
        let is_close = trimmed.starts_with('/');
        let tag_body = trimmed.trim_start_matches('/').trim_end_matches('/');
        let tag_name_full = tag_body.split_whitespace().next().unwrap_or("");
        let local_name = tag_name_full.rsplit(':').next().unwrap_or(tag_name_full);

        if is_close {
            if local_name.eq_ignore_ascii_case("Extension") {
                in_startup_ext = false;
                cur_ext_exe = None;
            } else if local_name.eq_ignore_ascii_case("Application") {
                cur_app_exe = None;
                cur_app_dn = None;
            }
            continue;
        }

        let self_closing = trimmed.ends_with('/');
        if local_name.eq_ignore_ascii_case("Application") {
            cur_app_exe = xml_attr(tag_body, "Executable");
            cur_app_dn = None;
            if self_closing {
                cur_app_exe = None;
            }
        } else if local_name.eq_ignore_ascii_case("VisualElements") {
            if let Some(dn) = xml_attr(tag_body, "DisplayName") {
                cur_app_dn = Some(dn);
            }
        } else if local_name.eq_ignore_ascii_case("Extension") {
            if xml_attr(tag_body, "Category").is_some_and(|c| c.eq_ignore_ascii_case("windows.startupTask")) {
                in_startup_ext = !self_closing;
                cur_ext_exe = xml_attr(tag_body, "Executable");
            }
        } else if local_name.eq_ignore_ascii_case("StartupTask")
            && (in_startup_ext || xml_attr(tag_body, "TaskId").is_some())
            && let Some(task_id) = xml_attr(tag_body, "TaskId")
        {
            let task_dn = xml_attr(tag_body, "DisplayName").filter(|s| !s.is_empty());
            let task_exe =
                xml_attr(tag_body, "Executable").or_else(|| cur_ext_exe.clone()).or_else(|| cur_app_exe.clone());
            info.tasks.push(ManifestStartupTask {
                task_id,
                display_name: task_dn.or_else(|| cur_app_dn.clone()),
                executable: task_exe,
            });
        }
    }

    info
}

fn approved_state_from_bytes(bytes: Option<&Vec<u8>>) -> StartupState {
    match bytes.and_then(|v| v.first()) {
        Some(&b) if (b & 1) == 1 => StartupState::Disabled,
        _ => StartupState::Enabled,
    }
}

/// Scanner e controller per le app di avvio di Windows.
pub struct StartupScanner;

impl Default for StartupScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupScanner {
    pub fn new() -> Self {
        Self
    }

    /// Esegue la scansione completa di tutte le origini di avvio automatico Windows.
    pub fn scan(&self, out: &mut Vec<StartupEntry>) {
        out.clear();

        let Some(reg) = RegLib::load() else { return };
        let mut ver = VersionReader::load();
        let mut res_resolver: Option<ResourceResolver> = None;

        // 1. HKCU Run
        self.scan_registry_run(
            &reg,
            ver.as_mut(),
            HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ | KEY_WOW64_64KEY,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run",
            StartupSource::HkcuRun,
            false,
            out,
        );

        // 2. HKLM Run (64-bit)
        self.scan_registry_run(
            &reg,
            ver.as_mut(),
            HKEY_LOCAL_MACHINE,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ | KEY_WOW64_64KEY,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run",
            StartupSource::HklmRun,
            true,
            out,
        );

        // 3. HKLM Run (32-bit WOW6432Node)
        self.scan_registry_run(
            &reg,
            ver.as_mut(),
            HKEY_LOCAL_MACHINE,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ | KEY_WOW64_32KEY,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run32",
            StartupSource::HklmRun32,
            true,
            out,
        );

        // 4. Cartella Esecuzione Automatica Utente
        let user_dir = self.user_startup_folder(&reg);
        if let Some(dir) = user_dir {
            self.scan_startup_folder(
                &reg,
                ver.as_mut(),
                &dir,
                HKEY_CURRENT_USER,
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder",
                StartupSource::UserStartupFolder,
                false,
                out,
            );
        }

        // 5. Cartella Esecuzione Automatica Comune (Tutti gli utenti)
        let common_dir = self.common_startup_folder(&reg);
        if let Some(dir) = common_dir {
            self.scan_startup_folder(
                &reg,
                ver.as_mut(),
                &dir,
                HKEY_LOCAL_MACHINE,
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder",
                StartupSource::CommonStartupFolder,
                true,
                out,
            );
        }

        // 6. App Windows Store / MSIX (SystemAppData)
        self.scan_store_apps(&reg, &mut res_resolver, out);
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_registry_run(
        &self,
        reg: &RegLib,
        mut ver: Option<&mut VersionReader>,
        root: Hkey,
        run_subkey: &str,
        run_access: u32,
        approved_subkey: &str,
        source: StartupSource,
        requires_admin: bool,
        out: &mut Vec<StartupEntry>,
    ) {
        let Ok(hk_run) = reg.open(root, run_subkey, run_access) else { return };
        let values = reg.enum_string_values(hk_run.hkey);
        drop(hk_run);

        let approved_map = if let Ok(hk_app) = reg.open(root, approved_subkey, KEY_READ | KEY_WOW64_64KEY) {
            reg.enum_binary_values(hk_app.hkey)
        } else {
            BTreeMap::new()
        };

        for (val_name, raw_cmd, _is_expand) in values {
            let expanded = expand_env_strings(&raw_cmd);
            let trimmed = expanded.trim();
            if trimmed.is_empty() || trimmed == "\"\"" || trimmed == "''" {
                continue;
            }

            let target_path = extract_exe_path(trimmed);
            let meta = match (ver.as_deref_mut(), target_path.as_deref()) {
                (Some(vr), Some(p)) => vr.read(p),
                _ => ExeMeta::default(),
            };

            let friendly_name = meta.description.unwrap_or_else(|| val_name.clone());
            let publisher = meta.company.unwrap_or_default();
            let state = approved_state_from_bytes(approved_map.get(&val_name.to_lowercase()));

            out.push(StartupEntry {
                name: friendly_name,
                publisher,
                state,
                source,
                command: trimmed.to_string(),
                target_path,
                raw_key: val_name,
                requires_admin,
            });
        }
    }

    fn user_startup_folder(&self, reg: &RegLib) -> Option<PathBuf> {
        if let Some(p) = reg.get_string(
            HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders",
            "Startup",
        ) {
            let exp = expand_env_strings(&p);
            if !exp.is_empty() {
                return Some(PathBuf::from(exp));
            }
        }
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"))
    }

    fn common_startup_folder(&self, reg: &RegLib) -> Option<PathBuf> {
        if let Some(p) = reg.get_string(
            HKEY_LOCAL_MACHINE,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders",
            "Common Startup",
        ) {
            let exp = expand_env_strings(&p);
            if !exp.is_empty() {
                return Some(PathBuf::from(exp));
            }
        }
        std::env::var_os("ProgramData")
            .map(|p| PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"))
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_startup_folder(
        &self,
        reg: &RegLib,
        mut ver: Option<&mut VersionReader>,
        dir: &Path,
        approved_root: Hkey,
        approved_subkey: &str,
        source: StartupSource,
        requires_admin: bool,
        out: &mut Vec<StartupEntry>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };

        let approved_map = if let Ok(hk_app) = reg.open(approved_root, approved_subkey, KEY_READ | KEY_WOW64_64KEY) {
            reg.enum_binary_values(hk_app.hkey)
        } else {
            BTreeMap::new()
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let Some(file_name_os) = path.file_name() else { continue };
            let file_name = file_name_os.to_string_lossy().to_string();
            if file_name.eq_ignore_ascii_case("desktop.ini") || file_name.starts_with('.') {
                continue;
            }

            let display_stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| file_name.clone());

            let ext_lower = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();

            let (command, target_path) = if ext_lower == "lnk"
                && let Ok(bytes) = std::fs::read(&path)
                && let Some(lnk) = parse_lnk_target(&bytes)
            {
                let cmd = if lnk.args.trim().is_empty() {
                    lnk.path.clone()
                } else {
                    format!("\"{}\" {}", lnk.path, lnk.args.trim())
                };
                (cmd, Some(lnk.path))
            } else {
                let full_str = path.to_string_lossy().to_string();
                (full_str.clone(), Some(full_str))
            };

            let meta = match (ver.as_deref_mut(), target_path.as_deref()) {
                (Some(vr), Some(tp)) if tp.to_ascii_lowercase().ends_with(".exe") => vr.read(tp),
                _ => ExeMeta::default(),
            };

            let publisher = meta.company.unwrap_or_default();
            let state = approved_state_from_bytes(approved_map.get(&file_name.to_lowercase()));

            out.push(StartupEntry {
                name: display_stem,
                publisher,
                state,
                source,
                command,
                target_path,
                raw_key: file_name,
                requires_admin,
            });
        }
    }

    fn scan_store_apps(&self, reg: &RegLib, res_resolver: &mut Option<ResourceResolver>, out: &mut Vec<StartupEntry>) {
        const SYSTEM_APP_DATA: &str =
            "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData";
        const REPO_PACKAGES: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages";

        let Ok(hk_sad) = reg.open(HKEY_CURRENT_USER, SYSTEM_APP_DATA, KEY_READ) else { return };
        let families = reg.enum_subkeys(hk_sad.hkey);
        drop(hk_sad);

        for family in families {
            let fam_sub = format!("{SYSTEM_APP_DATA}\\{family}");
            let schemas_sub = format!("{fam_sub}\\Schemas");
            let Some(pkg_full) = reg.get_string(HKEY_CURRENT_USER, &schemas_sub, "PackageFullName") else {
                continue;
            };
            if pkg_full.is_empty() {
                continue;
            }

            let Ok(hk_fam) = reg.open(HKEY_CURRENT_USER, &fam_sub, KEY_READ) else { continue };
            let subkeys = reg.enum_subkeys(hk_fam.hkey);
            drop(hk_fam);

            let mut state_tasks: Vec<(String, u32)> = Vec::new();
            for sk in subkeys {
                let sk_sub = format!("{fam_sub}\\{sk}");
                if let Some(st_val) = reg.get_dword(HKEY_CURRENT_USER, &sk_sub, "State") {
                    state_tasks.push((sk, st_val));
                }
            }
            if state_tasks.is_empty() {
                continue;
            }

            let repo_sub = format!("{REPO_PACKAGES}\\{pkg_full}");
            let pkg_root = reg.get_string(HKEY_CURRENT_USER, &repo_sub, "PackageRootFolder");
            let repo_dn = reg.get_string(HKEY_CURRENT_USER, &repo_sub, "DisplayName");

            let manifest = pkg_root
                .as_deref()
                .and_then(|root| std::fs::read_to_string(Path::new(root).join("AppxManifest.xml")).ok())
                .map(|xml| parse_appx_startup_tasks(&xml));

            let resolve_res = |resolver: &mut Option<ResourceResolver>, raw: &str| -> Option<String> {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if !trimmed.contains("ms-resource:") {
                    return Some(trimmed.to_string());
                }
                if resolver.is_none() {
                    *resolver = ResourceResolver::load();
                }
                resolver.as_ref().and_then(|r| r.resolve(&pkg_full, &family, trimmed))
            };

            let pkg_publisher = manifest
                .as_ref()
                .and_then(|m| m.publisher.as_deref())
                .and_then(|p| resolve_res(res_resolver, p))
                .unwrap_or_default();

            let pkg_fallback_dn = manifest
                .as_ref()
                .and_then(|m| m.pkg_display_name.as_deref())
                .and_then(|d| resolve_res(res_resolver, d))
                .or_else(|| repo_dn.as_deref().and_then(|d| resolve_res(res_resolver, d)))
                .unwrap_or_else(|| family.split('_').next().unwrap_or(&family).to_string());

            for (task_id, state_dword) in state_tasks {
                // Se il manifest è leggibile e dichiara StartupTask, filtriamo vecchi task orfani (es. vecchie versioni app).
                let matched_manifest_task = if let Some(ref mf) = manifest {
                    if !mf.tasks.is_empty() {
                        let Some(found) = mf.tasks.iter().find(|t| t.task_id.eq_ignore_ascii_case(&task_id)) else {
                            continue;
                        };
                        Some(found)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let task_name = matched_manifest_task
                    .and_then(|t| t.display_name.as_deref())
                    .and_then(|d| resolve_res(res_resolver, d))
                    .unwrap_or_else(|| pkg_fallback_dn.clone());

                let target_path =
                    match (pkg_root.as_deref(), matched_manifest_task.and_then(|t| t.executable.as_deref())) {
                        (Some(root), Some(exe)) => {
                            let candidate = Path::new(root).join(exe);
                            if candidate.exists() {
                                Some(candidate.to_string_lossy().to_string())
                            } else {
                                Some(root.to_string())
                            }
                        }
                        (Some(root), None) => Some(root.to_string()),
                        _ => None,
                    };

                let command = matched_manifest_task
                    .and_then(|t| t.executable.as_deref())
                    .and_then(|exe| pkg_root.as_deref().map(|r| Path::new(r).join(exe).to_string_lossy().to_string()))
                    .unwrap_or_else(|| format!("{pkg_full}!{task_id}"));

                let state = match state_dword {
                    2 => StartupState::Enabled,
                    4 => StartupState::EnabledByPolicy,
                    3 => StartupState::DisabledByPolicy,
                    _ => StartupState::Disabled,
                };

                out.push(StartupEntry {
                    name: task_name,
                    publisher: pkg_publisher.clone(),
                    state,
                    source: StartupSource::StoreApp,
                    command,
                    target_path,
                    raw_key: format!("{family}\\{task_id}"),
                    requires_admin: false,
                });
            }
        }
    }

    /// Abilita o disabilita l'avvio automatico per la voce selezionata.
    pub fn set_enabled(&self, entry: &StartupEntry, enabled: bool) -> StartupToggleOutcome {
        if entry.state.is_policy() {
            return StartupToggleOutcome::PolicyLocked;
        }

        let Some(reg) = RegLib::load() else { return StartupToggleOutcome::Failed };

        if entry.source == StartupSource::StoreApp {
            let sub = format!(
                "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData\\{}",
                entry.raw_key
            );
            if let Some(cur) = reg.get_dword(HKEY_CURRENT_USER, &sub, "State")
                && (cur == 3 || cur == 4)
            {
                return StartupToggleOutcome::PolicyLocked;
            }
            let hk = match reg.open(HKEY_CURRENT_USER, &sub, KEY_SET_VALUE | KEY_QUERY_VALUE) {
                Ok(h) => h,
                Err(ERROR_ACCESS_DENIED) => return StartupToggleOutcome::AccessDenied,
                Err(_) => return StartupToggleOutcome::Failed,
            };
            let new_state = if enabled { 2u32 } else { 1u32 };
            match reg.set_dword(hk.hkey, "State", new_state) {
                Ok(()) => {
                    if enabled {
                        let _ = reg.set_dword(hk.hkey, "UserEnabledStartupOnce", 1);
                    }
                    return StartupToggleOutcome::Ok;
                }
                Err(ERROR_ACCESS_DENIED) => return StartupToggleOutcome::AccessDenied,
                Err(_) => return StartupToggleOutcome::Failed,
            }
        }

        let (root, approved_sub) = match entry.source {
            StartupSource::HkcuRun => {
                (HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run")
            }
            StartupSource::HkcuRun32 => {
                (HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run32")
            }
            StartupSource::HklmRun => {
                (HKEY_LOCAL_MACHINE, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run")
            }
            StartupSource::HklmRun32 => {
                (HKEY_LOCAL_MACHINE, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run32")
            }
            StartupSource::UserStartupFolder => (
                HKEY_CURRENT_USER,
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder",
            ),
            StartupSource::CommonStartupFolder => (
                HKEY_LOCAL_MACHINE,
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder",
            ),
            StartupSource::StoreApp => return StartupToggleOutcome::Failed,
        };

        let hk = match reg.create(root, approved_sub, KEY_SET_VALUE | KEY_WOW64_64KEY) {
            Ok(h) => h,
            Err(ERROR_ACCESS_DENIED) => return StartupToggleOutcome::AccessDenied,
            Err(_) => return StartupToggleOutcome::Failed,
        };

        let payload = build_startup_approved_bytes(enabled);
        match reg.set_binary(hk.hkey, &entry.raw_key, &payload) {
            Ok(()) => StartupToggleOutcome::Ok,
            Err(ERROR_ACCESS_DENIED) => StartupToggleOutcome::AccessDenied,
            Err(_) => StartupToggleOutcome::Failed,
        }
    }
}

/// Costruisce i 12 byte `StartupApproved` compatibili con Gestione attività di Windows 11.
pub fn build_startup_approved_bytes(enabled: bool) -> [u8; 12] {
    if enabled {
        [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    } else {
        let mut ft: FILETIME = unsafe { zeroed() };
        // SAFETY: `ft` è una struttura `FILETIME` valida e scrivibile.
        unsafe { GetSystemTimeAsFileTime(&mut ft) };
        let lo = ft.dwLowDateTime.to_le_bytes();
        let hi = ft.dwHighDateTime.to_le_bytes();
        [0x03, 0, 0, 0, lo[0], lo[1], lo[2], lo[3], hi[0], hi[1], hi[2], hi[3]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quoted_and_unquoted_exe_paths() {
        assert_eq!(
            extract_exe_path(r#""C:\Program Files\LGHUB\lghub.exe" --minimized"#).as_deref(),
            Some(r"C:\Program Files\LGHUB\lghub.exe")
        );
        assert_eq!(
            extract_exe_path(r"C:\Program Files\App Folder\my app.exe /Autorun").as_deref(),
            Some(r"C:\Program Files\App Folder\my app.exe")
        );
        assert_eq!(
            extract_exe_path(r"C:\Windows\system32\SecurityHealthSystray.exe").as_deref(),
            Some(r"C:\Windows\system32\SecurityHealthSystray.exe")
        );
        assert_eq!(extract_exe_path(r#""""#), None);
        assert_eq!(extract_exe_path("   "), None);
    }

    #[test]
    fn expands_windows_env_vars() {
        let out = expand_env_strings(r"%SystemRoot%\system32\cmd.exe");
        assert!(out.to_ascii_lowercase().ends_with(r"\system32\cmd.exe"));
        assert!(!out.contains('%'));
    }

    #[test]
    fn parses_appx_manifest_startup_tasks() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Properties>
    <DisplayName>ms-resource:AppTitle</DisplayName>
    <PublisherDisplayName>Spotify AB</PublisherDisplayName>
  </Properties>
  <Applications>
    <Application Id="Spotify" Executable="Spotify.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Spotify" />
      <Extensions>
        <desktop:Extension Category="windows.startupTask" Executable="SpotifyStartupTask.exe">
          <desktop:StartupTask TaskId="Spotify" Enabled="false" DisplayName="Spotify" />
        </desktop:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>"#;
        let info = parse_appx_startup_tasks(xml);
        assert_eq!(info.publisher.as_deref(), Some("Spotify AB"));
        assert_eq!(info.tasks.len(), 1);
        assert_eq!(info.tasks[0].task_id, "Spotify");
        assert_eq!(info.tasks[0].display_name.as_deref(), Some("Spotify"));
        assert_eq!(info.tasks[0].executable.as_deref(), Some("SpotifyStartupTask.exe"));
    }

    #[test]
    fn startup_approved_bytes_encode_enabled_and_disabled() {
        let on = build_startup_approved_bytes(true);
        assert_eq!(on, [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(on[0] & 1, 0);

        let off = build_startup_approved_bytes(false);
        assert_eq!(off[0], 0x03);
        assert_eq!(off[0] & 1, 1);
    }

    #[test]
    fn parses_synthetic_ms_shllink_with_linkinfo() {
        let mut lnk = vec![0u8; 76];
        lnk[0..4].copy_from_slice(&0x4Cu32.to_le_bytes());
        lnk[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
        ]);
        // HasLinkInfo (0x02) | HasArguments (0x20) | IsUnicode (0x80)
        lnk[20..24].copy_from_slice(&0xA2u32.to_le_bytes());

        let path_bytes = b"C:\\Tools\\demo.exe\0";
        let li_hdr_size: u32 = 28;
        let li_size = li_hdr_size + path_bytes.len() as u32;
        lnk.extend_from_slice(&li_size.to_le_bytes());
        lnk.extend_from_slice(&li_hdr_size.to_le_bytes());
        lnk.extend_from_slice(&1u32.to_le_bytes()); // VolumeIDAndLocalBasePath
        lnk.extend_from_slice(&0u32.to_le_bytes()); // VolumeIDOffset
        lnk.extend_from_slice(&li_hdr_size.to_le_bytes()); // LocalBasePathOffset
        lnk.extend_from_slice(&0u32.to_le_bytes()); // CommonNetworkRelativeLinkOffset
        lnk.extend_from_slice(&0u32.to_le_bytes()); // CommonPathSuffixOffset
        lnk.extend_from_slice(path_bytes);

        let args: Vec<u16> = "--tray".encode_utf16().collect();
        lnk.extend_from_slice(&(args.len() as u16).to_le_bytes());
        for u in args {
            lnk.extend_from_slice(&u.to_le_bytes());
        }

        let parsed = parse_lnk_target(&lnk).expect("valid lnk");
        assert_eq!(parsed.path, r"C:\Tools\demo.exe");
        assert_eq!(parsed.args, "--tray");
    }

    #[test]
    fn scans_live_windows_startup_entries() {
        let scanner = StartupScanner::new();
        let mut entries = Vec::new();
        scanner.scan(&mut entries);
        assert!(!entries.is_empty(), "almeno una voce di avvio dovrebbe essere presente su Windows");
    }
}
