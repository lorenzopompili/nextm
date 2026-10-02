//! Tipi e formattazione per le temperature (ACPI, GPU, disco).
//!
//! Logica pura senza chiamate di sistema, testabile ovunque.

/// Zona termica ACPI (tipicamente la CPU dall'EC o una zona della scheda madre).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThermalZone {
    /// Nome ACPI dell'istanza (es. `\_TZ.TZ01`).
    pub name: String,
    /// Etichetta mostrata (es. "CPU (EC)" su modelli noti, o "Zona ACPI TZ01").
    pub label: String,
    /// Temperatura in °C interi.
    pub temp_c: i16,
    /// `true` se la zona è verificata essere la CPU dall'embedded controller.
    pub is_cpu_ec: bool,
    /// `true` se il valore è rimasto identico per più di 10 minuti (sensore fisso/finto).
    pub is_fixed: bool,
}

/// Scheda grafica e relativa temperatura.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuAdapter {
    /// Nome del modello (es. "AMD Radeon(TM) 880M Graphics").
    pub name: String,
    /// LUID dell'adattatore.
    pub luid: u64,
    /// Temperatura in °C interi; `None` se il driver non restituisce il dato (temp = 0).
    pub temp_c: Option<i16>,
}

/// Unità disco e relativa temperatura.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskDevice {
    /// Etichetta descrittiva (es. "SSD NVMe", "SSD", "Disco").
    pub label: String,
    /// Lettera del volume (es. 'C').
    pub drive_letter: char,
    /// Temperatura in °C interi (sensore composito per NVMe).
    pub temp_c: i16,
    /// Soglia di allarme (warning), se disponibile.
    pub warn_c: Option<i16>,
    /// Soglia critica, se disponibile.
    pub crit_c: Option<i16>,
}

/// Stato di freschezza di una lettura lenta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadingAge {
    pub seconds: u32,
}

impl ReadingAge {
    pub fn new(seconds: u32) -> ReadingAge {
        ReadingAge { seconds }
    }

    /// Formatta l'età in un buffer fisso, es. "3 s fa" (IT) o "3s ago" (EN).
    pub fn format(self, is_it: bool) -> [u8; 16] {
        let mut buf = [0u8; 16];
        let mut pos = 0;
        let mut val = self.seconds;
        if val == 0 {
            let s = if is_it { b"ora" } else { b"now" };
            buf[..s.len()].copy_from_slice(s);
            return buf;
        }

        // Cifre dei secondi
        let mut digits = [0u8; 10];
        let mut d_len = 0;
        while val > 0 {
            digits[d_len] = b'0' + (val % 10) as u8;
            val /= 10;
            d_len += 1;
        }
        for &d in digits[..d_len].iter().rev() {
            if pos < 15 {
                buf[pos] = d;
                pos += 1;
            }
        }

        let suffix = if is_it { b" s fa" } else { b"s ago" };
        let end = (pos + suffix.len()).min(15);
        let copy_len = end - pos;
        buf[pos..end].copy_from_slice(&suffix[..copy_len]);
        buf
    }
}

/// Testo dell'icona per una temperatura: "0°".."99°", o "—" se manca il dato.
pub fn temp_icon_text(temp_c: Option<i16>, buf: &mut [u8; 8]) -> &str {
    let Some(t) = temp_c else { return "\u{2014}" };
    if t <= 0 {
        return "\u{2014}";
    }
    let val = (t as u32).min(99);
    let d1 = (val / 10) as u8;
    let d0 = (val % 10) as u8;
    let mut pos = 0;
    if d1 > 0 {
        buf[pos] = b'0' + d1;
        pos += 1;
    }
    buf[pos] = b'0' + d0;
    pos += 1;
    buf[pos] = 0xC2;
    buf[pos + 1] = 0xB0;
    pos += 2;
    core::str::from_utf8(&buf[..pos]).unwrap_or("\u{2014}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_formatting() {
        let age0 = ReadingAge::new(0);
        let b0 = age0.format(true);
        let s0 = core::str::from_utf8(&b0).unwrap();
        assert!(s0.starts_with("ora"));

        let age3 = ReadingAge::new(3);
        let b3 = age3.format(true);
        let s3 = core::str::from_utf8(&b3).unwrap();
        assert!(s3.starts_with("3 s fa"));

        let b3_en = age3.format(false);
        let s3_en = core::str::from_utf8(&b3_en).unwrap();
        assert!(s3_en.starts_with("3s ago"));
    }

    #[test]
    fn temp_texts() {
        let mut buf = [0u8; 8];
        assert_eq!(temp_icon_text(None, &mut buf), "—");
        assert_eq!(temp_icon_text(Some(0), &mut buf), "—");
        assert_eq!(temp_icon_text(Some(-5), &mut buf), "—");
        assert_eq!(temp_icon_text(Some(7), &mut buf), "7°");
        assert_eq!(temp_icon_text(Some(55), &mut buf), "55°");
        assert_eq!(temp_icon_text(Some(68), &mut buf), "68°");
        assert_eq!(temp_icon_text(Some(105), &mut buf), "99°");
    }
}
