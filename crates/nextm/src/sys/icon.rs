//! HICON creata da un'immagine BGRA in memoria.

use core::ptr::null_mut;

use nextm_render::Canvas;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON};

/// Dimensione massima gestita: 64 px (8 byte per riga di maschera × 64 righe).
const MAX_SIZE: u32 = 64;
/// Maschera AND a 1 bit tutta a zero: con 32 bit per pixel decide l'alfa del colore.
static ZERO_MASK: [u8; 512] = [0; 512];

/// Icona di sistema, distrutta automaticamente. La shell ne fa una copia a ogni
/// `NIM_ADD`/`NIM_MODIFY`, quindi si può distruggere subito dopo averla consegnata.
pub struct Icon(HICON);

impl Icon {
    /// Crea l'icona a 32 bit dall'immagine (al massimo 64×64).
    pub fn from_canvas(c: &Canvas) -> Option<Icon> {
        if c.width() == 0 || c.width() > MAX_SIZE || c.height() == 0 || c.height() > MAX_SIZE {
            return None;
        }
        // SAFETY: maschera e colore hanno almeno le dimensioni richieste da CreateIcon
        // (maschera: righe allineate a 16 bit; colore: 4 byte per pixel).
        let h = unsafe {
            CreateIcon(null_mut(), c.width() as i32, c.height() as i32, 1, 32, ZERO_MASK.as_ptr(), c.bgra().as_ptr())
        };
        (!h.is_null()).then_some(Icon(h))
    }

    pub fn handle(&self) -> HICON {
        self.0
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: l'icona è stata creata da noi e si distrugge una sola volta.
        unsafe { DestroyIcon(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nextm_render::Color;

    #[test]
    fn creates_icons_of_tray_sizes() {
        for size in [16, 20, 24, 28, 32] {
            let mut c = Canvas::new(size, size);
            c.set(1, 1, Color::rgb(255, 255, 255));
            assert!(Icon::from_canvas(&c).is_some(), "dimensione {size}");
        }
    }

    #[test]
    fn rejects_empty_and_huge_images() {
        assert!(Icon::from_canvas(&Canvas::new(0, 0)).is_none());
        assert!(Icon::from_canvas(&Canvas::new(65, 65)).is_none());
    }
}
