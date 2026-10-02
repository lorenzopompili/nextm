//! Icona nell'area di notifica (tray).

use core::mem::zeroed;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// Selezione da tastiera dell'icona (non definita da windows-sys): `NIN_SELECT | NINF_KEY`.
pub const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

use crate::sys::icon::Icon;
use crate::sys::window::WM_TRAY;
use crate::wide::copy_to_fixed;

/// Un'icona nella tray, rimossa automaticamente. Identità: finestra + `id` fisso.
pub struct TrayIcon {
    hwnd: HWND,
    id: u32,
}

/// Evento ricevuto dalla shell (formato `NOTIFYICON_VERSION_4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayEvent {
    /// `WM_CONTEXTMENU`, `NIN_SELECT`, `WM_LBUTTONDBLCLK`, `WM_MOUSEMOVE`, ...
    pub event: u32,
    pub id: u32,
    /// Punto di riferimento (coordinate schermo) per menu e pannelli.
    pub x: i32,
    pub y: i32,
}

/// Decodifica i parametri di `WM_TRAY`: evento in LOWORD(lParam), id in HIWORD(lParam),
/// coordinate in wParam (x nella parola bassa, y in quella alta, con segno).
pub fn decode(wparam: WPARAM, lparam: LPARAM) -> TrayEvent {
    TrayEvent {
        event: (lparam as u32) & 0xFFFF,
        id: ((lparam as u32) >> 16) & 0xFFFF,
        x: i32::from((wparam & 0xFFFF) as u16 as i16),
        y: i32::from(((wparam >> 16) & 0xFFFF) as u16 as i16),
    }
}

fn base(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW è una struttura C valida se azzerata.
    let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = id;
    nid
}

fn notify(message: u32, nid: &NOTIFYICONDATAW) -> bool {
    // SAFETY: `nid` è una struttura completa e valida per la durata della chiamata.
    unsafe { Shell_NotifyIconW(message, nid) != 0 }
}

impl TrayIcon {
    /// Aggiunge l'icona con il tooltip `tip` e chiede il comportamento `NOTIFYICON_VERSION_4`.
    pub fn add(hwnd: HWND, id: u32, icon: &Icon, tip: &[u16]) -> Option<TrayIcon> {
        let mut nid = base(hwnd, id);
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon.handle();
        copy_to_fixed(&mut nid.szTip, tip);
        if !notify(NIM_ADD, &nid) {
            return None;
        }
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        notify(NIM_SETVERSION, &nid);
        Some(TrayIcon { hwnd, id })
    }

    /// Sostituisce immagine e tooltip in una sola chiamata: quando l'immagine cambia,
    /// anche il testo resta aggiornato senza chiamate in più verso Explorer.
    pub fn update(&self, icon: &Icon, tip: &[u16]) -> bool {
        let mut nid = base(self.hwnd, self.id);
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.hIcon = icon.handle();
        copy_to_fixed(&mut nid.szTip, tip);
        notify(NIM_MODIFY, &nid)
    }

    /// Sostituisce immagine e tooltip; se fallisce (es. Explorer riavviato), tenta di riaggiungerla.
    pub fn update_or_readd(&self, icon: &Icon, tip: &[u16]) -> bool {
        if self.update(icon, tip) {
            return true;
        }
        let mut nid = base(self.hwnd, self.id);
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon.handle();
        copy_to_fixed(&mut nid.szTip, tip);
        if notify(NIM_ADD, &nid) {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            notify(NIM_SETVERSION, &nid);
            true
        } else {
            false
        }
    }

    /// Sostituisce il testo del tooltip.
    pub fn set_tip(&self, tip: &[u16]) -> bool {
        let mut nid = base(self.hwnd, self.id);
        nid.uFlags = NIF_TIP | NIF_SHOWTIP;
        copy_to_fixed(&mut nid.szTip, tip);
        notify(NIM_MODIFY, &nid)
    }

    /// Mostra una notifica di Windows legata all'icona (rispetta le ore di silenzio).
    pub fn balloon(&self, title: &[u16], text: &[u16]) -> bool {
        let mut nid = base(self.hwnd, self.id);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        copy_to_fixed(&mut nid.szInfoTitle, title);
        copy_to_fixed(&mut nid.szInfo, text);
        notify(NIM_MODIFY, &nid)
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        notify(NIM_DELETE, &base(self.hwnd, self.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU;

    #[test]
    fn decodes_event_id_and_coordinates() {
        let lparam = ((1u32 << 16) | WM_CONTEXTMENU) as LPARAM;
        let wparam = ((720u32 << 16) | 1500) as WPARAM;
        assert_eq!(decode(wparam, lparam), TrayEvent { event: WM_CONTEXTMENU, id: 1, x: 1500, y: 720 });
    }

    #[test]
    fn decodes_negative_coordinates_of_left_monitors() {
        let lparam = ((1u32 << 16) | NIN_SELECT) as LPARAM;
        let wparam = ((100u32 << 16) | (-1200i16 as u16 as u32)) as WPARAM;
        assert_eq!(decode(wparam, lparam).x, -1200);
    }
}
