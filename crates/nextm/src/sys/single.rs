//! Istanza singola: un mutex con nome per sessione utente.

use core::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

use crate::sys::window::{CLASS_NAME, register_message};

const MUTEX_NAME: &[u16] = wide!("Local\\nextm-3f6c2a9e-8d41-4b7a-9f0e-6b1d5c7a2e44");
/// Messaggio con cui una seconda istanza chiede alla prima di farsi vedere.
pub const ACTIVATE_MESSAGE: &[u16] = wide!("nextm-activate");
/// Messaggio con cui `nextm --quit` chiede all'istanza attiva di chiudersi.
pub const EXIT_MESSAGE: &[u16] = wide!("nextm-exit");

use core::sync::atomic::{AtomicPtr, Ordering};

static ACTIVE_MUTEX: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());

/// Tiene il mutex finché il programma è in esecuzione.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// `None` se un'altra istanza di nextm è già attiva in questa sessione.
    pub fn acquire() -> Option<SingleInstance> {
        // SAFETY: nome terminato da zero; il risultato si controlla subito.
        unsafe {
            let h = CreateMutexW(null(), 0, MUTEX_NAME.as_ptr());
            if h.is_null() {
                // Senza mutex non si può escludere un'altra istanza: si parte comunque.
                return Some(SingleInstance(h));
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(h);
                return None;
            }
            ACTIVE_MUTEX.store(h, Ordering::SeqCst);
            Some(SingleInstance(h))
        }
    }

    /// Attende fino a `timeout` che un'eventuale istanza precedente si chiuda
    /// e rilasci il mutex di sessione.
    pub fn acquire_with_retry(timeout: core::time::Duration) -> Option<SingleInstance> {
        let start = std::time::Instant::now();
        loop {
            if let Some(instance) = Self::acquire() {
                return Some(instance);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(core::time::Duration::from_millis(50));
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let active = ACTIVE_MUTEX.swap(core::ptr::null_mut(), Ordering::SeqCst);
        let h = if !active.is_null() {
            self.0 = core::ptr::null_mut();
            active
        } else {
            let direct = self.0;
            self.0 = core::ptr::null_mut();
            direct
        };
        if !h.is_null() {
            // SAFETY: handle mutex chiuso esattamente una volta.
            unsafe { CloseHandle(h) };
        }
    }
}

/// Rilascia esplicitamente e immediatamente il mutex di sessione (ad esempio prima di un riavvio elevato).
pub fn release_current() {
    let h = ACTIVE_MUTEX.swap(core::ptr::null_mut(), Ordering::SeqCst);
    if !h.is_null() {
        unsafe { CloseHandle(h) };
    }
}

/// Chiede all'istanza già attiva di farsi vedere; `false` se non la trova.
pub fn notify_existing() -> bool {
    post_to_existing(ACTIVATE_MESSAGE)
}

/// Attende fino a `timeout` che un'eventuale istanza precedente si chiuda
/// e rilasci il mutex di sessione.
pub fn acquire_with_retry(timeout: core::time::Duration) -> Option<SingleInstance> {
    SingleInstance::acquire_with_retry(timeout)
}

/// Chiede all'istanza attiva di chiudersi (`nextm --quit`, prima di aggiornare l'exe);
/// `false` se nessuna istanza è attiva.
pub fn request_quit() -> bool {
    post_to_existing(EXIT_MESSAGE)
}

fn post_to_existing(message: &[u16]) -> bool {
    // SAFETY: stringhe terminate da zero; PostMessageW accetta una finestra di un altro processo.
    unsafe {
        let hwnd = FindWindowW(CLASS_NAME.as_ptr(), null());
        if hwnd.is_null() {
            return false;
        }
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX);
        PostMessageW(hwnd, register_message(message), 0, 0) != 0
    }
}
