//! Menu contestuale, creato a ogni apertura e distrutto subito dopo.

use core::ptr::null;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreatePopupMenu, DestroyMenu, HMENU, MF_BYCOMMAND, MF_CHECKED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_NULL,
};

pub struct PopupMenu(HMENU);

#[allow(dead_code)]
impl PopupMenu {
    pub fn new() -> Option<PopupMenu> {
        // SAFETY: nessun parametro.
        let h = unsafe { CreatePopupMenu() };
        (!h.is_null()).then_some(PopupMenu(h))
    }

    /// Voce semplice, con spunta opzionale. `text` termina con zero.
    pub fn item(&self, id: u32, text: &[u16], checked: bool) {
        let flags = MF_STRING | if checked { MF_CHECKED } else { 0 };
        // SAFETY: menu valido, testo terminato da zero.
        unsafe { AppendMenuW(self.0, flags, id as usize, text.as_ptr()) };
    }

    /// Voce di un gruppo a scelta singola (pallino al posto della spunta).
    pub fn radio(&self, id: u32, text: &[u16], selected: bool) {
        self.item(id, text, false);
        if selected {
            // SAFETY: menu valido; la voce `id` esiste.
            unsafe { CheckMenuRadioItem(self.0, id, id, id, MF_BYCOMMAND) };
        }
    }

    pub fn separator(&self) {
        // SAFETY: menu valido.
        unsafe { AppendMenuW(self.0, MF_SEPARATOR, 0, null()) };
    }

    /// Aggiunge un sotto-menu; da qui in poi lo possiede (e lo distrugge) questo menu.
    pub fn submenu(&self, text: &[u16], sub: PopupMenu) {
        let h = sub.0;
        core::mem::forget(sub);
        // SAFETY: menu validi, testo terminato da zero.
        unsafe { AppendMenuW(self.0, MF_STRING | MF_POPUP, h as usize, text.as_ptr()) };
    }

    /// Mostra il menu nel punto (x, y) e restituisce il comando scelto (0 = nessuno).
    /// `SetForegroundWindow` e il `WM_NULL` finale servono perché il menu si chiuda
    /// correttamente cliccando altrove (comportamento documentato per le icone della tray).
    pub fn track(&self, hwnd: HWND, x: i32, y: i32) -> u32 {
        // SAFETY: menu e finestra validi.
        unsafe {
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenuEx(self.0, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, x, y, hwnd, null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            cmd as u32
        }
    }
}

impl Drop for PopupMenu {
    fn drop(&mut self) {
        // SAFETY: menu creato da noi; distrugge anche i sotto-menu.
        unsafe { DestroyMenu(self.0) };
    }
}
