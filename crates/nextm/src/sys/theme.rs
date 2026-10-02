//! Tema della taskbar: chiaro, scuro o alto contrasto.

use nextm_render::{Color, Theme};
use windows_sys::Win32::Graphics::Gdi::{COLOR_WINDOWTEXT, GetSysColor};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};

use crate::sys::registry;

const PERSONALIZE: &[u16] = wide!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

/// Il tema su cui disegnare le icone. La taskbar segue la "modalità Windows"
/// (`SystemUsesLightTheme`), non la modalità delle app.
pub fn taskbar_theme() -> Theme {
    if high_contrast_on() {
        // SAFETY: indice di colore di sistema valido.
        let c = unsafe { GetSysColor(COLOR_WINDOWTEXT) };
        return Theme::HighContrast { text: colorref(c) };
    }
    match registry::get_dword(PERSONALIZE, wide!("SystemUsesLightTheme")) {
        Some(1) => Theme::Light,
        _ => Theme::Dark,
    }
}

fn high_contrast_on() -> bool {
    // SAFETY: struttura azzerata con cbSize corretto.
    unsafe {
        let mut hc: HIGHCONTRASTW = core::mem::zeroed();
        hc.cbSize = size_of::<HIGHCONTRASTW>() as u32;
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&raw mut hc).cast(), 0) != 0
            && hc.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

/// Da COLORREF (0x00BBGGRR) a colore opaco.
fn colorref(c: u32) -> Color {
    Color::rgb((c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_order() {
        assert_eq!(colorref(0x0011_2233), Color::rgb(0x33, 0x22, 0x11));
    }

    #[test]
    fn reads_some_theme() {
        let _ = taskbar_theme();
    }
}
