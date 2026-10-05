//! Controllo degli aggiornamenti su GitHub Releases tramite WinHTTP dinamico (System32).
//!
//! Non carica alcuna DLL all'avvio: usa `winhttp.dll` solo durante la verifica e la scarica subito.
//! La verifica avviene in un thread di background per non bloccare mai l'interfaccia utente Win32.

use core::ffi::c_void;
use core::ptr::null;
use std::sync::Mutex;

use nextm_metrics::sys::dll::Library;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;

type FnWinHttpOpen = unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> *mut c_void;
type FnWinHttpConnect = unsafe extern "system" fn(*mut c_void, *const u16, u16, u32) -> *mut c_void;
type FnWinHttpOpenRequest = unsafe extern "system" fn(
    *mut c_void,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    *const *const u16,
    u32,
) -> *mut c_void;
type FnWinHttpSetTimeouts = unsafe extern "system" fn(*mut c_void, i32, i32, i32, i32) -> i32;
type FnWinHttpSendRequest =
    unsafe extern "system" fn(*mut c_void, *const u16, u32, *mut c_void, u32, u32, usize) -> i32;
type FnWinHttpReceiveResponse = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
type FnWinHttpQueryHeaders =
    unsafe extern "system" fn(*mut c_void, u32, *const u16, *mut c_void, *mut u32, *mut u32) -> i32;
type FnWinHttpReadData = unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> i32;
type FnWinHttpCloseHandle = unsafe extern "system" fn(*mut c_void) -> i32;

const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
const INTERNET_DEFAULT_HTTPS_PORT: u16 = 443;
const WINHTTP_FLAG_SECURE: u32 = 0x00800000;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x20000000;

const GITHUB_HOST: &[u16] = wide!("api.github.com");
const GITHUB_PATH: &[u16] = wide!("/repos/lorenzopompili/nextm/releases/latest");
const REPO_RELEASES_URL: &str = "https://github.com/lorenzopompili/nextm/releases";

/// Stato corrente del controllo aggiornamenti.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// Non ancora avviato.
    Idle,
    /// Richiesta HTTP in corso.
    Checking,
    /// La versione installata è la più recente.
    UpToDate { version: String },
    /// È disponibile una nuova versione su GitHub.
    NewVersion { version: String, url: String },
    /// Errore durante la verifica (es. rete assente, timeout, rate limit).
    Error(String),
}

static UPDATE_STATE: Mutex<UpdateState> = Mutex::new(UpdateState::Idle);

/// Legge lo stato corrente del controllo aggiornamenti.
pub fn get_update_state() -> UpdateState {
    UPDATE_STATE.lock().map(|s| s.clone()).unwrap_or(UpdateState::Idle)
}

/// Imposta lo stato del controllo aggiornamenti.
pub fn set_update_state(state: UpdateState) {
    if let Ok(mut s) = UPDATE_STATE.lock() {
        *s = state;
    }
}

/// Estrae il valore stringa di un campo chiave da una risposta JSON minimale.
pub fn extract_json_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let mut search_from = 0;
    while let Some(rel_idx) = json[search_from..].find(key) {
        let key_idx = search_from + rel_idx;
        // Verifica che sia tra virgolette: "key"
        if key_idx > 0 && json.as_bytes()[key_idx - 1] == b'"' {
            let after_key = key_idx + key.len();
            if after_key < json.len() && json.as_bytes()[after_key] == b'"' {
                let rest = &json[after_key + 1..];
                let colon_idx = rest.find(':')?;
                let after_colon = rest[colon_idx + 1..].trim_start();
                if let Some(stripped) = after_colon.strip_prefix('"') {
                    let end_quote = stripped.find('"')?;
                    return Some(&stripped[..end_quote]);
                }
            }
        }
        search_from = key_idx + key.len();
    }
    None
}

/// Converte una stringa di versione semantica ("v0.1.0", "0.2.1") in una tupla (major, minor, patch).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut parts = s.split('.');
    let major_str = parts.next()?;
    let minor_str = parts.next().unwrap_or("0");
    let patch_str = parts.next().unwrap_or("0");

    let clean_num = |p: &str| -> Option<u32> {
        let num_str: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        num_str.parse().ok()
    };

    Some((clean_num(major_str)?, clean_num(minor_str)?, clean_num(patch_str)?))
}

/// Restituisce `true` se `remote` è più recente di `current`.
pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

struct WinHttpHandle {
    h: *mut c_void,
    close: FnWinHttpCloseHandle,
}

impl WinHttpHandle {
    fn new(h: *mut c_void, close: FnWinHttpCloseHandle) -> Option<Self> {
        if h.is_null() { None } else { Some(Self { h, close }) }
    }
}

impl Drop for WinHttpHandle {
    fn drop(&mut self) {
        if !self.h.is_null() {
            unsafe { (self.close)(self.h) };
        }
    }
}

/// Esegue la query HTTP verso l'API di GitHub in modo sincrono.
fn check_github_releases_sync() -> Result<UpdateState, String> {
    let lib = Library::load("winhttp.dll").ok_or_else(|| "winhttp.dll non disponibile".to_string())?;

    let fn_open: FnWinHttpOpen =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpOpen\0").ok_or("WinHttpOpen non trovata")?) };
    let fn_connect: FnWinHttpConnect =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpConnect\0").ok_or("WinHttpConnect non trovata")?) };
    let fn_open_req: FnWinHttpOpenRequest =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpOpenRequest\0").ok_or("WinHttpOpenRequest non trovata")?) };
    let fn_set_timeouts: FnWinHttpSetTimeouts =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpSetTimeouts\0").ok_or("WinHttpSetTimeouts non trovata")?) };
    let fn_send_req: FnWinHttpSendRequest =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpSendRequest\0").ok_or("WinHttpSendRequest non trovata")?) };
    let fn_recv_resp: FnWinHttpReceiveResponse = unsafe {
        core::mem::transmute(lib.proc(b"WinHttpReceiveResponse\0").ok_or("WinHttpReceiveResponse non trovata")?)
    };
    let fn_query_hdr: FnWinHttpQueryHeaders =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpQueryHeaders\0").ok_or("WinHttpQueryHeaders non trovata")?) };
    let fn_read_data: FnWinHttpReadData =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpReadData\0").ok_or("WinHttpReadData non trovata")?) };
    let fn_close: FnWinHttpCloseHandle =
        unsafe { core::mem::transmute(lib.proc(b"WinHttpCloseHandle\0").ok_or("WinHttpCloseHandle non trovata")?) };

    let user_agent = wide!("nextm/0.1.0");
    let session = unsafe { fn_open(user_agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, null(), null(), 0) };
    let session = WinHttpHandle::new(session, fn_close).ok_or("Inizializzazione sessione WinHTTP fallita")?;

    // Timeout: 5000 ms per risoluzione, connessione, invio e ricezione
    unsafe { fn_set_timeouts(session.h, 5000, 5000, 5000, 5000) };

    let connect = unsafe { fn_connect(session.h, GITHUB_HOST.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0) };
    let connect = WinHttpHandle::new(connect, fn_close).ok_or("Connessione a api.github.com fallita")?;

    let verb = wide!("GET");
    let req = unsafe {
        fn_open_req(connect.h, verb.as_ptr(), GITHUB_PATH.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE)
    };
    let req = WinHttpHandle::new(req, fn_close).ok_or("Creazione richiesta HTTP fallita")?;

    let headers = wide!("Accept: application/vnd.github.v3+json\r\nUser-Agent: nextm-updater\r\n");
    let sent = unsafe {
        fn_send_req(req.h, headers.as_ptr(), headers.len().saturating_sub(1) as u32, core::ptr::null_mut(), 0, 0, 0)
    };
    if sent == 0 {
        return Err("Invio richiesta HTTP fallito".to_string());
    }

    let recvd = unsafe { fn_recv_resp(req.h, core::ptr::null_mut()) };
    if recvd == 0 {
        return Err("Ricezione risposta HTTP fallita".to_string());
    }

    let mut status_code: u32 = 0;
    let mut status_len = size_of::<u32>() as u32;
    let mut header_idx = 0;
    let query_res = unsafe {
        fn_query_hdr(
            req.h,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&raw mut status_code).cast(),
            &mut status_len,
            &mut header_idx,
        )
    };
    if query_res == 0 || status_code != 200 {
        return Err(format!("Risposta server: HTTP {status_code}"));
    }

    let mut body = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut bytes_read = 0;
        let read_res = unsafe { fn_read_data(req.h, buf.as_mut_ptr(), buf.len() as u32, &mut bytes_read) };
        if read_res == 0 || bytes_read == 0 {
            break;
        }
        body.extend_from_slice(&buf[..bytes_read as usize]);
        if body.len() > 65536 {
            // Risposta troppo grande, interrompi
            break;
        }
    }

    let json = core::str::from_utf8(&body).map_err(|_| "Formato risposta non UTF-8 valido")?;
    let tag = extract_json_string(json, "tag_name").ok_or("Tag di versione non trovato nel rilascio")?;
    let html_url = extract_json_string(json, "html_url").unwrap_or(REPO_RELEASES_URL);

    let current = env!("CARGO_PKG_VERSION");
    if is_newer(tag, current) {
        Ok(UpdateState::NewVersion { version: tag.to_string(), url: html_url.to_string() })
    } else {
        Ok(UpdateState::UpToDate { version: current.to_string() })
    }
}

/// Avvia il controllo degli aggiornamenti in background.
/// Se `notify_hwnd` è fornito, invia un `InvalidateRect` al completamento per ridisegnare la finestra.
pub fn check_for_updates_async(notify_hwnd: Option<HWND>) {
    {
        let Ok(mut state) = UPDATE_STATE.lock() else { return };
        if *state == UpdateState::Checking {
            return;
        }
        *state = UpdateState::Checking;
    }

    // Se la finestra è aperta, ridisegna subito per mostrare lo stato "Verifica in corso..."
    if let Some(hwnd) = notify_hwnd {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }

    let hwnd_raw = notify_hwnd.map(|h| h as isize);
    std::thread::spawn(move || {
        let result = match check_github_releases_sync() {
            Ok(new_state) => new_state,
            Err(e) => UpdateState::Error(e),
        };
        set_update_state(result);
        if let Some(h) = hwnd_raw {
            unsafe { InvalidateRect(h as HWND, null(), 0) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_versions() {
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("V1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.2.0-rc1"), Some((0, 2, 0)));
    }

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
    }

    #[test]
    fn json_string_extraction() {
        let sample = r#"{"tag_name":"v0.1.0","name":"First release","html_url":"https://github.com/lorenzopompili/nextm/releases/tag/v0.1.0"}"#;
        assert_eq!(extract_json_string(sample, "tag_name"), Some("v0.1.0"));
        assert_eq!(
            extract_json_string(sample, "html_url"),
            Some("https://github.com/lorenzopompili/nextm/releases/tag/v0.1.0")
        );
        assert_eq!(extract_json_string(sample, "not_present"), None);
    }

    #[test]
    fn live_check_github_releases() {
        let res = check_github_releases_sync();
        assert!(res.is_ok(), "live check failed: {:?}", res.err());
        match res.expect("res") {
            UpdateState::UpToDate { version } => {
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
            }
            UpdateState::NewVersion { version, url } => {
                assert!(!version.is_empty());
                assert!(!url.is_empty());
            }
            other => panic!("unexpected state: {:?}", other),
        }
    }
}
