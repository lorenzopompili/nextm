//! Composizione delle icone della tray.

use crate::canvas::Canvas;
use crate::color::Color;
use crate::pixfont::{FontKind, PixFont, font_for};
use crate::text::draw_text;

/// Taglia più grande disegnata a mano; oltre si raddoppia pixel per pixel.
const MAX_DESIGNED: u32 = 32;

/// Altezza della barra di stato in basso (core saturo) per un'icona di `size` pixel.
/// Lo spazio è riservato in tutte le icone "valore", così le cifre non si spostano mai.
pub fn bar_height(size: u32) -> u32 {
    (size / 8).max(2)
}

/// Font e fattore di ingrandimento per un'icona di `size` pixel. Fino a 32 px si usa il font
/// disegnato per quella taglia; oltre (scale del 250-300%) il font per metà taglia raddoppiato,
/// che resta nitido: 36 → 16×2, 40 → 20×2, 48 → 24×2, 64 → 32×2.
pub fn font_and_scale(size: u32, kind: FontKind) -> (&'static PixFont, u32) {
    if size > MAX_DESIGNED { (font_for(size / 2, kind), 2) } else { (font_for(size, kind), 1) }
}

/// Massimo numero di tacche della barra di stato.
const MAX_SEGMENTS: u32 = 4;

/// Barra di stato in fondo a un'icona "valore" (spec §4.1, core saturo).
///
/// Occupa le ultime [`bar_height`] righe, a tutta larghezza, divisa in `segments` tacche separate da 1 px
/// (2 px oltre i 32 px, dove il disegno è raddoppiato).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusBar {
    pub color: Color,
    /// Numero di tacche, da 1 a 4: 0 vale 1 e oltre 4 vale 4.
    pub segments: u8,
}

/// Disegna un'icona "valore": `text` centrato col font grande, sopra lo spazio della barra.
pub fn draw_value_icon(canvas: &mut Canvas, text: &str, color: Color) {
    draw_value_icon_with_bar(canvas, text, color, None);
}

/// Come [`draw_value_icon`], più la barra di stato nelle ultime [`bar_height`] righe se `bar` è presente.
/// Le cifre restano dove sono con o senza barra: lo spazio è riservato comunque.
pub fn draw_value_icon_with_bar(canvas: &mut Canvas, text: &str, color: Color, bar: Option<StatusBar>) {
    canvas.clear();
    let size = canvas.width().min(canvas.height());
    let (font, scale) = font_and_scale(size, FontKind::Large);
    let w = font.measure(text) * scale;
    let h = u32::from(font.height) * scale;
    // Area delle cifre: 1 riga di margine in alto, poi fino alla riga di distacco dalla barra.
    let area_h = size.saturating_sub(bar_height(size) + 2);
    let x = (size.saturating_sub(w) / 2) as i32;
    let y = 1 + (area_h.saturating_sub(h) / 2) as i32;
    draw_text(canvas, font, x, y, text, color, scale);
    if let Some(bar) = bar {
        draw_status_bar(canvas, size, scale, bar);
    }
}

/// Disegna la barra: `bar.segments` tacche nelle ultime `bar_height(size)` righe, separate da `gap` pixel,
/// che insieme coprono tutta la larghezza `size` dell'icona.
fn draw_status_bar(canvas: &mut Canvas, size: u32, gap: u32, bar: StatusBar) {
    let n = u32::from(bar.segments).clamp(1, MAX_SEGMENTS);
    let height = bar_height(size);
    let y = size.saturating_sub(height) as i32;
    // Larghezza da spartire fra le tacche, tolti i distacchi. I bordi si arrotondano al pixel più vicino:
    // le larghezze differiscono al più di 1 e con 3 tacche restano simmetriche (5-4-5 a 16 px).
    let avail = size.saturating_sub((n - 1) * gap);
    let edge = |i: u32| (i * avail + n / 2) / n;
    for i in 0..n {
        let (start, end) = (edge(i), edge(i + 1));
        canvas.fill_rect((start + i * gap) as i32, y, end - start, height, bar.color);
    }
}

struct SymbolDef {
    width: u8,
    height: u8,
    rows: &'static [u16],
}

const CPU_SYMBOL: SymbolDef = SymbolDef {
    width: 9,
    height: 5,
    rows: &[
        0b010010010, // . # . . # . . # .
        0b111111111, // # # # # # # # # #
        0b110111011, // # # . # # # . # #
        0b111111111, // # # # # # # # # #
        0b010010010, // . # . . # . . # .
    ],
};

const RAM_SYMBOL: SymbolDef = SymbolDef {
    width: 11,
    height: 5,
    rows: &[
        0b11111111111, // # # # # # # # # # # #
        0b10110110111, // # . # # . # # . # # #
        0b11111111111, // # # # # # # # # # # #
        0b11110111111, // # # # # . # # # # # #
        0b10110010101, // # . # # . . # . # . #
    ],
};

const TEMP_ACPI_SYMBOL: SymbolDef = SymbolDef {
    width: 5,
    height: 5,
    rows: &[
        0b00100, // . . # . .
        0b00100, // . . # . .
        0b00100, // . . # . .
        0b01110, // . # # # .
        0b01110, // . # # # .
    ],
};

const TEMP_GPU_SYMBOL: SymbolDef = SymbolDef {
    width: 11,
    height: 5,
    rows: &[
        0b11111111111, // # # # # # # # # # # #
        0b11011101100, // # # . # # # . # # . .
        0b10101010100, // # . # . # . # . # . .
        0b11011101100, // # # . # # # . # # . .
        0b11110001111, // # # # # . . . # # # #
    ],
};

const TEMP_DISK_SYMBOL: SymbolDef = SymbolDef {
    width: 11,
    height: 5,
    rows: &[
        0b11111111111, // # # # # # # # # # # #
        0b10000000001, // # . . . . . . . . . #
        0b10000000001, // # . . . . . . . . . #
        0b11111111111, // # # # # # # # # # # #
        0b10000001101, // # . . . . . . # # . #
    ],
};

fn draw_symbol(canvas: &mut Canvas, sym: &SymbolDef, x0: i32, y0: i32, color: Color, scale: u32) {
    for (r, &bits) in sym.rows.iter().enumerate() {
        for c in 0..sym.width {
            let shift = sym.width - 1 - c;
            if (bits >> shift) & 1 == 1 {
                canvas.fill_rect(x0 + (c as u32 * scale) as i32, y0 + (r as u32 * scale) as i32, scale, scale, color);
            }
        }
    }
}

/// Simbolo identificativo della metrica sopra il valore numerico.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricSymbol {
    Cpu,
    Ram,
    TempAcpi,
    TempGpu,
    TempDisk,
}

/// Disegna un'icona metrica: mini-icona (stile FontAwesome) in alto e valore compatto in basso.
pub fn draw_metric_icon(canvas: &mut Canvas, symbol: MetricSymbol, text: &str, color: Color, bar: Option<StatusBar>) {
    canvas.clear();
    let size = canvas.width().min(canvas.height());
    let (font, font_scale) = font_and_scale(size, FontKind::Small);
    let text_w = font.measure(text) * font_scale;
    let text_h = u32::from(font.height) * font_scale;

    let bar_h = bar_height(size);
    let has_bar_slot = symbol == MetricSymbol::Cpu;
    let bar_space = if has_bar_slot { bar_h + 1 } else { 0 };

    // Posizione testo: allineato in basso (sopra lo spazio della barra se CPU)
    let text_y = size.saturating_sub(text_h + bar_space);
    let text_x = (size.saturating_sub(text_w) / 2) as i32;
    draw_text(canvas, font, text_x, text_y as i32, text, color, font_scale);

    // Posizione simbolo mini-icona: centrato nello spazio sopra il testo
    let sym_def = match symbol {
        MetricSymbol::Cpu => &CPU_SYMBOL,
        MetricSymbol::Ram => &RAM_SYMBOL,
        MetricSymbol::TempAcpi => &TEMP_ACPI_SYMBOL,
        MetricSymbol::TempGpu => &TEMP_GPU_SYMBOL,
        MetricSymbol::TempDisk => &TEMP_DISK_SYMBOL,
    };
    let sym_scale = if size >= 28 { 2 } else { 1 };
    let sym_w = u32::from(sym_def.width) * sym_scale;
    let sym_h = u32::from(sym_def.height) * sym_scale;
    let sym_x = (size.saturating_sub(sym_w) / 2) as i32;
    let sym_y = (text_y.saturating_sub(sym_h) / 2).max(1) as i32;
    draw_symbol(canvas, sym_def, sym_x, sym_y, color, sym_scale);

    // Barra di stato CPU
    if let Some(bar) = bar {
        draw_status_bar(canvas, size, font_scale, bar);
    }
}

/// Disegna l'icona della CPU: mini microchip in alto, percentuale al centro, barra di saturazione in basso.
pub fn draw_cpu_icon(canvas: &mut Canvas, text: &str, color: Color, bar: Option<StatusBar>) {
    draw_metric_icon(canvas, MetricSymbol::Cpu, text, color, bar);
}

/// Disegna l'icona della RAM: mini modulo DIMM in alto, percentuale di carico in basso.
pub fn draw_ram_icon(canvas: &mut Canvas, text: &str, color: Color) {
    draw_metric_icon(canvas, MetricSymbol::Ram, text, color, None);
}

/// Disegna l'icona della temperatura ACPI: mini termometro in alto, valore con ° in basso.
pub fn draw_temp_acpi_icon(canvas: &mut Canvas, text: &str, color: Color) {
    draw_metric_icon(canvas, MetricSymbol::TempAcpi, text, color, None);
}

/// Disegna l'icona della temperatura GPU: mini scheda video con ventole in alto, valore con ° in basso.
pub fn draw_temp_gpu_icon(canvas: &mut Canvas, text: &str, color: Color) {
    draw_metric_icon(canvas, MetricSymbol::TempGpu, text, color, None);
}

/// Disegna l'icona della temperatura Disco: mini disco SSD/HDD con LED in alto, valore con ° in basso.
pub fn draw_temp_disk_icon(canvas: &mut Canvas, text: &str, color: Color) {
    draw_metric_icon(canvas, MetricSymbol::TempDisk, text, color, None);
}

/// Disegna l'icona della rete: due righe di testo piccolo, `top` sopra e `bottom` sotto, ciascuna nel suo
/// colore (spec §4.1: upload attenuato sopra, download nel colore principale sotto).
///
/// Le righe sono centrate in verticale con una riga di stacco; l'eventuale pixel avanzato resta in fondo.
/// Il testo è allineato a destra con 1 px di margine, così le unità (K, M, G) restano incolonnate. Oltre i
/// 32 px il disegno è raddoppiato come in [`draw_value_icon`], stacco e margine compresi.
pub fn draw_dual_icon(canvas: &mut Canvas, top: &str, top_color: Color, bottom: &str, bottom_color: Color) {
    canvas.clear();
    let size = canvas.width().min(canvas.height());
    let (font, scale) = font_and_scale(size, FontKind::Small);
    let h = u32::from(font.height) * scale;
    let y0 = size.saturating_sub(2 * h + scale) / 2;
    draw_right_aligned(canvas, font, scale, size, y0, top, top_color);
    draw_right_aligned(canvas, font, scale, size, y0 + h + scale, bottom, bottom_color);
}

/// Disegna `text` con il bordo destro a `scale` pixel dal lato destro di un'icona larga `size`.
/// Se non entra, parte dal bordo sinistro e viene tagliato a destra.
fn draw_right_aligned(canvas: &mut Canvas, font: &PixFont, scale: u32, size: u32, y: u32, text: &str, color: Color) {
    let w = font.measure(text).saturating_mul(scale);
    let x = size.saturating_sub(w.saturating_add(scale));
    draw_text(canvas, font, x as i32, y as i32, text, color, scale);
}

/// Disegna l'icona statica neutra di nextm: simbolo di indicatore/tachimetro (spec §4.1: `gauge-high`).
pub fn draw_static_icon(canvas: &mut Canvas, color: Color) {
    canvas.clear();
    let size = canvas.width().min(canvas.height());
    let (cx, cy) = (size as i32 / 2, (size as i32 * 5) / 8);
    let r_out = (size as i32 * 3) / 8;
    let r_in = r_out.saturating_sub(if size > MAX_DESIGNED { 3 } else { 2 });
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let dx = x - cx;
            let dy = y - cy;
            let d2 = dx * dx + dy * dy;
            // Arco superiore e laterale
            if d2 <= r_out * r_out && d2 >= r_in * r_in && dy <= (r_out / 2) {
                canvas.set(x, y, color);
            }
            // Perno centrale
            if d2 <= (if size > MAX_DESIGNED { 4 } else { 2 }) {
                canvas.set(x, y, color);
            }
            // Lancetta verso l'alto a destra (gauge-high)
            if dx >= 0 && dy <= 0 && (dx + dy).abs() <= 1 && dx - dy < r_out {
                canvas.set(x, y, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn designed_sizes_are_not_scaled() {
        for size in [16, 20, 24, 28, 32] {
            let (font, scale) = font_and_scale(size, FontKind::Large);
            assert_eq!(scale, 1);
            assert!(core::ptr::eq(font, font_for(size, FontKind::Large)));
        }
    }

    #[test]
    fn large_scales_double_half_size_fonts() {
        for (size, half) in [(36, 16), (40, 20), (48, 24), (64, 32)] {
            let (font, scale) = font_and_scale(size, FontKind::Large);
            assert_eq!(scale, 2);
            assert!(core::ptr::eq(font, font_for(half, FontKind::Large)));
        }
    }

    #[test]
    fn digits_fit_above_the_bar_at_every_size() {
        for size in [16, 20, 24, 28, 32, 36, 40, 48, 64] {
            let (font, scale) = font_and_scale(size, FontKind::Large);
            let h = u32::from(font.height) * scale;
            assert!(h + bar_height(size) + 2 <= size, "altezza a {size} px");
            assert!(font.measure("100") * scale + 2 <= size, "larghezza di 100 a {size} px");
        }
    }

    #[test]
    fn nothing_is_drawn_in_the_bar_area() {
        for size in [16, 24, 32, 48] {
            let mut c = Canvas::new(size, size);
            draw_value_icon(&mut c, "88", Color::rgb(255, 255, 255));
            for y in size - bar_height(size)..size {
                for x in 0..size {
                    assert_eq!(c.pixel(x, y), Color::TRANSPARENT, "pixel ({x},{y}) a {size} px");
                }
            }
        }
    }

    const WHITE: Color = Color::rgb(255, 255, 255);
    const AMBER: Color = Color::rgb(0xFF, 0xB9, 0x00);
    const DIM: Color = Color::rgb(0xB4, 0xB4, 0xB4);

    /// Le taglie della tray: le cinque disegnate a mano, il raddoppio e alcune intermedie.
    const SIZES: [u32; 10] = [16, 20, 24, 28, 32, 36, 40, 48, 56, 64];

    fn bar(segments: u8) -> Option<StatusBar> {
        Some(StatusBar { color: AMBER, segments })
    }

    /// I tratti continui di pixel accesi della riga `y`, come (x iniziale, larghezza).
    fn runs(c: &Canvas, y: u32) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        for x in 0..c.width() {
            if c.pixel(x, y).a == 0 {
                continue;
            }
            match out.last_mut() {
                Some((start, len)) if *start + *len == x => *len += 1,
                _ => out.push((x, 1)),
            }
        }
        out
    }

    #[test]
    fn value_icon_without_bar_is_the_plain_one() {
        for size in SIZES {
            let (mut plain, mut none) = (Canvas::new(size, size), Canvas::new(size, size));
            draw_value_icon(&mut plain, "42", WHITE);
            draw_value_icon_with_bar(&mut none, "42", WHITE, None);
            assert_eq!(plain.bgra(), none.bgra(), "a {size} px");
        }
    }

    #[test]
    fn bar_fills_the_last_rows_edge_to_edge() {
        for size in SIZES {
            let gap = if size > MAX_DESIGNED { 2 } else { 1 };
            for segments in 1..=4u8 {
                let mut c = Canvas::new(size, size);
                draw_value_icon_with_bar(&mut c, "12", WHITE, bar(segments));
                let rows = size - bar_height(size)..size;
                let first = runs(&c, rows.start);
                assert_eq!(first.len(), usize::from(segments), "numero di tacche a {size} px");
                assert_eq!(first[0].0, 0, "la barra parte dal bordo sinistro a {size} px");
                let (last_x, last_w) = first[first.len() - 1];
                assert_eq!(last_x + last_w, size, "la barra arriva al bordo destro a {size} px");
                for pair in first.windows(2) {
                    assert_eq!(pair[1].0 - (pair[0].0 + pair[0].1), gap, "distacco fra le tacche a {size} px");
                }
                // Tutte le righe della barra sono uguali e del colore della barra.
                for y in rows {
                    assert_eq!(runs(&c, y), first, "riga {y} a {size} px");
                    for &(x, w) in &first {
                        assert!((x..x + w).all(|px| c.pixel(px, y) == AMBER), "colore a {size} px");
                    }
                }
            }
        }
    }

    #[test]
    fn segments_are_balanced_and_three_are_symmetric() {
        for size in SIZES {
            for segments in 1..=4u8 {
                let mut c = Canvas::new(size, size);
                draw_value_icon_with_bar(&mut c, "", WHITE, bar(segments));
                let widths: Vec<u32> = runs(&c, size - 1).iter().map(|&(_, w)| w).collect();
                let (min, max) = (widths.iter().min().unwrap(), widths.iter().max().unwrap());
                assert!(max - min <= 1, "tacche sbilanciate {widths:?} a {size} px");
                if segments == 3 {
                    assert_eq!(widths[0], widths[2], "3 tacche non simmetriche {widths:?} a {size} px");
                }
            }
        }
    }

    #[test]
    fn segments_are_clamped_to_one_through_four() {
        for size in [16, 24, 40] {
            let draw = |segments: u8| {
                let mut c = Canvas::new(size, size);
                draw_value_icon_with_bar(&mut c, "12", WHITE, bar(segments));
                c
            };
            assert_eq!(draw(0).bgra(), draw(1).bgra(), "0 vale 1 a {size} px");
            for over in [5, 9, 255] {
                assert_eq!(draw(over).bgra(), draw(4).bgra(), "{over} vale 4 a {size} px");
            }
        }
    }

    #[test]
    fn bar_leaves_the_digits_and_the_gap_row_alone() {
        for size in SIZES {
            let (mut plain, mut with_bar) = (Canvas::new(size, size), Canvas::new(size, size));
            draw_value_icon(&mut plain, "100", WHITE);
            draw_value_icon_with_bar(&mut with_bar, "100", WHITE, bar(4));
            let bar_top = size - bar_height(size);
            for y in 0..bar_top {
                for x in 0..size {
                    assert_eq!(with_bar.pixel(x, y), plain.pixel(x, y), "pixel ({x},{y}) a {size} px");
                }
            }
            // La riga fra le cifre e la barra resta vuota.
            assert!((0..size).all(|x| with_bar.pixel(x, bar_top - 1) == Color::TRANSPARENT), "distacco a {size} px");
        }
    }

    #[test]
    fn bar_survives_tiny_canvases() {
        // Nessun panic né sottrazioni che vanno sotto zero, anche se l'icona è più piccola della barra.
        for size in 0..6 {
            let mut c = Canvas::new(size, size);
            draw_value_icon_with_bar(&mut c, "12", WHITE, bar(4));
            draw_dual_icon(&mut c, "12M", DIM, "99K", WHITE);
        }
    }

    #[test]
    fn dual_icon_stacks_two_right_aligned_rows() {
        for size in SIZES {
            let (font, scale) = font_and_scale(size, FontKind::Small);
            let h = u32::from(font.height) * scale;
            let top_rows = {
                let y0 = (size - (2 * h + scale)) / 2;
                y0..y0 + h
            };
            let bottom_rows = top_rows.end + scale..top_rows.end + scale + h;
            for (top, bottom) in [("2M", "12M"), (".3M", "99K"), ("12m", "0"), ("99k", "5g")] {
                let mut c = Canvas::new(size, size);
                draw_dual_icon(&mut c, top, DIM, bottom, WHITE);
                // Ogni riga ha il suo colore; lo stacco e i pixel avanzati restano vuoti.
                for y in 0..size {
                    for x in 0..size {
                        let want = if top_rows.contains(&y) {
                            [DIM, Color::TRANSPARENT]
                        } else if bottom_rows.contains(&y) {
                            [WHITE, Color::TRANSPARENT]
                        } else {
                            [Color::TRANSPARENT; 2]
                        };
                        assert!(want.contains(&c.pixel(x, y)), "pixel ({x},{y}) di \"{top}\"/\"{bottom}\" a {size} px");
                    }
                }
                // Il bordo destro del testo sta a `scale` pixel dal margine, con qualunque unità.
                for (rows, text) in [(top_rows.clone(), top), (bottom_rows.clone(), bottom)] {
                    let right =
                        rows.flat_map(|y| (0..size).map(move |x| (x, y))).filter(|&(x, y)| c.pixel(x, y).a != 0);
                    assert_eq!(right.map(|(x, _)| x).max(), Some(size - 1 - scale), "\"{text}\" a {size} px");
                }
            }
        }
    }

    #[test]
    fn dual_icon_text_fits_at_every_size() {
        // Le forme compatte più larghe (spec §4.1) entrano con il margine destro, a ogni taglia.
        for size in 16..=64 {
            let (font, scale) = font_and_scale(size, FontKind::Small);
            assert!((2 * u32::from(font.height) + 1) * scale <= size, "altezza a {size} px");
            for text in ["99M", "99m", "99K", "99k", ".9G", ".9g", "12M", "12m", "0"] {
                assert!(font.measure(text) * scale + scale <= size, "\"{text}\" a {size} px");
            }
        }
    }

    #[test]
    fn dual_icon_replaces_the_previous_content() {
        let mut c = Canvas::new(16, 16);
        c.fill_rect(0, 0, 16, 16, AMBER);
        draw_dual_icon(&mut c, "", DIM, "", WHITE);
        assert!(c.bgra().iter().all(|&b| b == 0));
    }

    #[test]
    fn static_icon_draws_something_at_every_size() {
        for size in [16, 20, 24, 28, 32, 40, 48] {
            let mut c = Canvas::new(size, size);
            draw_static_icon(&mut c, WHITE);
            assert!(c.bgra().iter().any(|&b| b != 0), "static icon empty at {size} px");
        }
    }

    #[test]
    fn distinct_metric_icons_draw_at_every_size() {
        for size in [16, 20, 24, 28, 32, 40, 48] {
            let mut c_cpu = Canvas::new(size, size);
            draw_cpu_icon(&mut c_cpu, "23", WHITE, None);
            assert!(c_cpu.bgra().iter().any(|&b| b != 0), "cpu icon empty at {size} px");

            let mut c_ram = Canvas::new(size, size);
            draw_ram_icon(&mut c_ram, "63", WHITE);
            assert!(c_ram.bgra().iter().any(|&b| b != 0), "ram icon empty at {size} px");

            let mut c_acpi = Canvas::new(size, size);
            draw_temp_acpi_icon(&mut c_acpi, "50°", WHITE);
            assert!(c_acpi.bgra().iter().any(|&b| b != 0), "acpi temp icon empty at {size} px");

            let mut c_gpu = Canvas::new(size, size);
            draw_temp_gpu_icon(&mut c_gpu, "55°", WHITE);
            assert!(c_gpu.bgra().iter().any(|&b| b != 0), "gpu temp icon empty at {size} px");

            let mut c_disk = Canvas::new(size, size);
            draw_temp_disk_icon(&mut c_disk, "44°", WHITE);
            assert!(c_disk.bgra().iter().any(|&b| b != 0), "disk temp icon empty at {size} px");
        }
    }
}
