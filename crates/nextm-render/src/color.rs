//! Colori e tavolozze per tema della taskbar.

/// Colore RGBA con alfa non premoltiplicato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };

    /// Colore opaco.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
}

/// Tema della taskbar su cui disegnare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Taskbar scura (`SystemUsesLightTheme` = 0).
    Dark,
    /// Taskbar chiara (`SystemUsesLightTheme` = 1).
    Light,
    /// Alto contrasto attivo: si usa il colore del testo di sistema, niente colori di stato.
    HighContrast { text: Color },
}

/// Colori delle cifre per livello.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub normal: Color,
    pub warn: Color,
    pub full: Color,
    /// Tono attenuato, meno vistoso di `normal`: l'upload nell'icona della rete (spec §4.1).
    pub dim: Color,
}

impl Palette {
    pub fn for_theme(theme: Theme) -> Palette {
        match theme {
            Theme::Dark => Palette {
                normal: Color::rgb(0xFF, 0xFF, 0xFF),
                warn: Color::rgb(0xFF, 0xB9, 0x00),
                full: Color::rgb(0xFF, 0x5F, 0x5F),
                dim: Color::rgb(0xB4, 0xB4, 0xB4),
            },
            Theme::Light => Palette {
                normal: Color::rgb(0x1B, 0x1B, 0x1B),
                warn: Color::rgb(0x9D, 0x5D, 0x00),
                full: Color::rgb(0xC4, 0x2B, 0x1C),
                dim: Color::rgb(0x5C, 0x5C, 0x5C),
            },
            // In alto contrasto niente sfumature: contano solo i colori di sistema.
            Theme::HighContrast { text } => Palette { normal: text, warn: text, full: text, dim: text },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sfondo tipico della taskbar di Windows 11 per tema: scuro e chiaro.
    const DARK_TASKBAR: Color = Color::rgb(0x20, 0x20, 0x20);
    const LIGHT_TASKBAR: Color = Color::rgb(0xF3, 0xF3, 0xF3);

    /// Luminanza relativa secondo WCAG 2.x.
    fn luminance(c: Color) -> f32 {
        let lin = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    /// Rapporto di contrasto WCAG 2.x fra due colori (da 1 a 21).
    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn high_contrast_uses_system_text_everywhere() {
        let text = Color::rgb(1, 2, 3);
        let p = Palette::for_theme(Theme::HighContrast { text });
        assert_eq!((p.normal, p.warn, p.full, p.dim), (text, text, text, text));
    }

    #[test]
    fn themes_have_distinct_levels() {
        for t in [Theme::Dark, Theme::Light] {
            let p = Palette::for_theme(t);
            assert_ne!(p.normal, p.warn);
            assert_ne!(p.warn, p.full);
            assert_ne!(p.normal, p.full);
        }
    }

    #[test]
    fn dim_is_distinct_from_every_other_level() {
        for t in [Theme::Dark, Theme::Light] {
            let p = Palette::for_theme(t);
            assert_ne!(p.dim, p.normal);
            assert_ne!(p.dim, p.warn);
            assert_ne!(p.dim, p.full);
        }
    }

    #[test]
    fn dim_values_follow_the_spec() {
        assert_eq!(Palette::for_theme(Theme::Dark).dim, Color::rgb(0xB4, 0xB4, 0xB4));
        assert_eq!(Palette::for_theme(Theme::Light).dim, Color::rgb(0x5C, 0x5C, 0x5C));
    }

    #[test]
    fn dim_is_quieter_than_normal_but_still_readable() {
        // Attenuato = più vicino allo sfondo del colore principale, ma con contrasto da testo (WCAG AA, 4,5:1).
        for (theme, taskbar) in [(Theme::Dark, DARK_TASKBAR), (Theme::Light, LIGHT_TASKBAR)] {
            let p = Palette::for_theme(theme);
            let (dim, normal) = (contrast(p.dim, taskbar), contrast(p.normal, taskbar));
            assert!(dim < normal, "{theme:?}: l'attenuato ({dim}) deve avere meno contrasto del principale ({normal})");
            assert!(dim >= 4.5, "{theme:?}: l'attenuato ha contrasto {dim}, sotto 4,5:1");
        }
    }
}
