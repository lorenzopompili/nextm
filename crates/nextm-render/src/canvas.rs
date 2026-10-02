//! Immagine BGRA a 32 bit con alfa non premoltiplicato, righe dall'alto verso il basso.

use crate::color::Color;

/// Immagine in memoria. I byte di ogni pixel sono nell'ordine B, G, R, A, il formato che
/// `CreateIcon` si aspetta per un'icona a 32 bit.
pub struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Canvas {
    /// Crea un'immagine completamente trasparente.
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, height, data: vec![0; (width * height * 4) as usize] }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Cambia dimensione (e rialloca) solo se serve; il contenuto diventa trasparente.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width != self.width || height != self.height {
            self.width = width;
            self.height = height;
            self.data = vec![0; (width * height * 4) as usize];
        } else {
            self.clear();
        }
    }

    /// Rende trasparente tutta l'immagine, senza riallocare.
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Colore del pixel (x, y); trasparente fuori dai bordi.
    pub fn pixel(&self, x: u32, y: u32) -> Color {
        if x >= self.width || y >= self.height {
            return Color::TRANSPARENT;
        }
        let i = ((y * self.width + x) * 4) as usize;
        Color { b: self.data[i], g: self.data[i + 1], r: self.data[i + 2], a: self.data[i + 3] }
    }

    /// Colora il pixel (x, y); le coordinate fuori dai bordi vengono ignorate.
    pub fn set(&mut self, x: i32, y: i32, c: Color) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.data[i] = c.b;
        self.data[i + 1] = c.g;
        self.data[i + 2] = c.r;
        self.data[i + 3] = c.a;
    }

    /// Riempie un rettangolo, ritagliato ai bordi dell'immagine.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Color) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                self.set(x + dx, y + dy, c);
            }
        }
    }

    /// I byte BGRA dell'immagine, riga per riga dall'alto.
    pub fn bgra(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(255, 0, 0);

    #[test]
    fn new_is_transparent() {
        let c = Canvas::new(4, 3);
        assert_eq!(c.bgra().len(), 4 * 3 * 4);
        assert!(c.bgra().iter().all(|&b| b == 0));
    }

    #[test]
    fn set_writes_bgra_order() {
        let mut c = Canvas::new(2, 2);
        c.set(1, 0, Color { r: 1, g: 2, b: 3, a: 4 });
        assert_eq!(&c.bgra()[4..8], &[3, 2, 1, 4]);
        assert_eq!(c.pixel(1, 0), Color { r: 1, g: 2, b: 3, a: 4 });
    }

    #[test]
    fn out_of_bounds_is_ignored() {
        let mut c = Canvas::new(2, 2);
        c.set(-1, 0, RED);
        c.set(0, 2, RED);
        assert!(c.bgra().iter().all(|&b| b == 0));
        assert_eq!(c.pixel(5, 5), Color::TRANSPARENT);
    }

    #[test]
    fn fill_rect_is_clipped() {
        let mut c = Canvas::new(3, 3);
        c.fill_rect(1, 1, 5, 5, RED);
        assert_eq!(c.pixel(0, 0), Color::TRANSPARENT);
        assert_eq!(c.pixel(2, 2), RED);
    }

    #[test]
    fn resize_same_size_clears_without_reallocating() {
        let mut c = Canvas::new(2, 2);
        c.set(0, 0, RED);
        c.resize(2, 2);
        assert_eq!(c.pixel(0, 0), Color::TRANSPARENT);
        c.resize(3, 1);
        assert_eq!((c.width(), c.height(), c.bgra().len()), (3, 1, 12));
    }
}
