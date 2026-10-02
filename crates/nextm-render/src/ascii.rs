//! Rappresentazione testuale di un'immagine, per i test con immagini di riferimento.

use crate::canvas::Canvas;
use crate::color::Color;

/// Un carattere per pixel e una riga di testo per riga di pixel.
/// Trasparente = `.`, colori della legenda = il loro carattere, altri colori = `?`.
pub fn to_ascii(canvas: &Canvas, legend: &[(Color, char)]) -> String {
    let mut out = String::with_capacity(((canvas.width() + 1) * canvas.height()) as usize);
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let p = canvas.pixel(x, y);
            let ch = if p.a == 0 { '.' } else { legend.iter().find(|(c, _)| *c == p).map_or('?', |(_, ch)| *ch) };
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_colors_and_transparency() {
        let red = Color::rgb(255, 0, 0);
        let mut c = Canvas::new(3, 2);
        c.set(0, 0, red);
        c.set(2, 1, Color::rgb(1, 1, 1));
        assert_eq!(to_ascii(&c, &[(red, '#')]), "#..\n..?\n");
    }
}
