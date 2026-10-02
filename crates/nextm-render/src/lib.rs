//! Disegno delle icone di nextm su buffer BGRA in memoria.
//!
//! Nessuna chiamata di sistema: tutto è testabile confrontando le immagini prodotte
//! con immagini di riferimento testuali (`tests/golden`).

pub mod ascii;
pub mod canvas;
pub mod color;
pub mod icon;
pub mod pixfont;
pub mod text;

pub use canvas::Canvas;
pub use color::{Color, Palette, Theme};
