//! Confronto delle icone disegnate con immagini di riferimento testuali (`tests/golden/*.txt`).
//!
//! Se un'immagine cambia apposta (per esempio dopo aver ritoccato il font), rigenera i file con:
//!     $env:NEXTM_BLESS = "1"; cargo test -p nextm-render --test golden; Remove-Item Env:NEXTM_BLESS
//! e controlla il diff prima di fare commit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use nextm_render::ascii::to_ascii;
use nextm_render::icon::{StatusBar, draw_dual_icon, draw_value_icon, draw_value_icon_with_bar};
use nextm_render::{Canvas, Color};

// Colori del tema scuro. Nelle immagini: `#` principale, `A` ambra (barra di stato), `o` attenuato (upload).
const WHITE: Color = Color::rgb(255, 255, 255);
const AMBER: Color = Color::rgb(0xFF, 0xB9, 0x00);
const DIM: Color = Color::rgb(0xB4, 0xB4, 0xB4);
const LEGEND: [(Color, char); 3] = [(WHITE, '#'), (AMBER, 'A'), (DIM, 'o')];

/// Le taglie delle icone con barra e doppie: le cinque disegnate a mano più il raddoppio a 40 e 48 px.
const SIZES: [u32; 7] = [16, 20, 24, 28, 32, 40, 48];

fn check(name: &str, canvas: &Canvas) {
    let actual = to_ascii(canvas, &LEGEND);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(format!("{name}.txt"));
    if std::env::var_os("NEXTM_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("manca {}: rigenera con NEXTM_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(actual, expected, "l'icona {name} è cambiata");
}

#[test]
fn value_icons_at_every_tray_size() {
    for size in [16, 20, 24, 28, 32, 36, 40, 48] {
        for text in ["0", "7", "42", "88", "100", "—"] {
            let mut c = Canvas::new(size, size);
            draw_value_icon(&mut c, text, WHITE);
            let label = if text == "—" { "dash" } else { text };
            check(&format!("value_{size}_{label}"), &c);
        }
    }
}

/// Core saturo: il numero con la barra ambra a 1, 2, 3 e 4 tacche (spec §4.1).
#[test]
fn value_icons_with_status_bar() {
    for size in SIZES {
        for segments in 1..=4 {
            let mut c = Canvas::new(size, size);
            draw_value_icon_with_bar(&mut c, "12", WHITE, Some(StatusBar { color: AMBER, segments }));
            check(&format!("value_{size}_12_bar{segments}"), &c);
        }
    }
}

/// Icona della rete: upload attenuato sopra, download nel colore principale sotto (spec §4.1).
#[test]
fn dual_icons_at_every_tray_size() {
    let cases = [("2M", "12M", "2M_12M"), (".3M", "99K", "dot3M_99K"), ("12m", "0", "12m_0")];
    for size in SIZES {
        for (top, bottom, label) in cases {
            let mut c = Canvas::new(size, size);
            draw_dual_icon(&mut c, top, DIM, bottom, WHITE);
            check(&format!("dual_{size}_{label}"), &c);
        }
    }
}
