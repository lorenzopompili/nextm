//! Disegno di testo con i font a pixel.

use crate::canvas::Canvas;
use crate::color::Color;
use crate::pixfont::PixFont;

/// Disegna `text` con `font`; (x, y) è l'angolo in alto a sinistra del primo glifo.
/// `scale` ingrandisce ogni pixel del font in un quadrato `scale`×`scale` (1 = dimensione reale).
/// I caratteri senza glifo vengono saltati, come in `PixFont::measure`.
pub fn draw_text(canvas: &mut Canvas, font: &PixFont, x: i32, y: i32, text: &str, color: Color, scale: u32) {
    let s = scale.max(1) as i32;
    let mut pen = x;
    let mut first = true;
    for ch in text.chars() {
        let Some(glyph) = font.glyph(ch) else { continue };
        if !first {
            pen += i32::from(font.spacing) * s;
        }
        first = false;
        let w = u32::from(glyph.width);
        for (row, bits) in glyph.rows.iter().enumerate() {
            for col in 0..w {
                if (bits >> (w - 1 - col)) & 1 == 1 {
                    canvas.fill_rect(pen + col as i32 * s, y + row as i32 * s, s as u32, s as u32, color);
                }
            }
        }
        pen += i32::from(glyph.width) * s;
    }
}
