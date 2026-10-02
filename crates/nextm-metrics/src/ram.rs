//! RAM: campione della memoria fisica e conversione in GiB con una cifra decimale.

/// Un'istantanea della memoria fisica, come la restituisce `GlobalMemoryStatusEx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamSample {
    /// RAM fisica utilizzabile da Windows (`ullTotalPhys`), in byte. Non è la RAM installata:
    /// sulla macchina di sviluppo sono 32 043 MB contro 32 768 MB montati.
    pub total: u64,
    /// RAM disponibile (`ullAvailPhys` = Standby + Free + Zero), in byte. Le pagine della lista
    /// Modified (da scrivere su disco) non ci sono: contano come "in uso", come in Task Manager.
    pub available: u64,
    /// `dwMemoryLoad`: floor(100·(total − available)/total), da 0 a 100. Vale 100 anche quando
    /// restano meno di 100 pagine disponibili. È il numero che Task Manager media sugli ultimi
    /// 3 campioni per la tray e per l'intestazione della scheda Processi.
    pub load_percent: u8,
}

impl RamSample {
    /// RAM in uso, in byte: totale meno disponibile, senza andare sotto zero. Con due valori
    /// incoerenti (disponibile oltre il totale) il risultato è 0 e non un numero enorme.
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// Byte in GiB (1024³) con una cifra decimale, cioè GiB × 10 arrotondati al decimo più vicino
/// (la metà sale): 20,1 GiB -> 201, 1 GiB -> 10.
///
/// Restituisce un intero perché il separatore decimale dipende dalla lingua (§4.7): chi compone
/// il testo scrive `v / 10`, il separatore e `v % 10`. Task Manager scrive "GB" ma conta a
/// multipli di 1024, e nextm fa lo stesso. Oltre 429 milioni di GiB il risultato satura a
/// `u32::MAX`: un valore che nessuna macchina raggiunge, ma che non può andare in overflow.
pub fn gib_x10(bytes: u64) -> u32 {
    const GIB: u128 = 1 << 30;
    // u128 solo per il prodotto: `bytes × 10` supera u64 sopra 1,6 EiB.
    let tenths = (u128::from(bytes) * 10 + GIB / 2) / GIB;
    tenths.min(u128::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;

    fn sample(total: u64, available: u64) -> RamSample {
        RamSample { total, available, load_percent: 0 }
    }

    #[test]
    fn used_is_total_minus_available() {
        assert_eq!(sample(32 * GIB, 12 * GIB).used(), 20 * GIB);
        assert_eq!(sample(100, 0).used(), 100);
        assert_eq!(sample(100, 100).used(), 0);
    }

    #[test]
    fn used_saturates_instead_of_wrapping() {
        // Disponibile oltre il totale (letture incoerenti): 0, mai un underflow.
        assert_eq!(sample(100, 101).used(), 0);
        assert_eq!(sample(0, u64::MAX).used(), 0);
        assert_eq!(sample(0, 0).used(), 0);
    }

    #[test]
    fn used_ignores_load_percent() {
        let s = RamSample { total: 1_000, available: 400, load_percent: 99 };
        assert_eq!(s.used(), 600);
    }

    #[test]
    fn gib_x10_of_zero_and_whole_gib() {
        assert_eq!(gib_x10(0), 0);
        assert_eq!(gib_x10(GIB), 10);
        assert_eq!(gib_x10(16 * GIB), 160);
        assert_eq!(gib_x10(1024 * GIB), 10_240);
    }

    #[test]
    fn gib_x10_of_twenty_point_one_gib() {
        // 20,1 GiB = 21 582 210 662,4 byte.
        assert_eq!(gib_x10(21_582_210_662), 201);
        // La RAM utilizzabile della macchina di sviluppo: 32 043 MiB = 31,29 GiB -> 31,3.
        assert_eq!(gib_x10(32_043 * MIB), 313);
    }

    #[test]
    fn gib_x10_rounds_half_up() {
        // 256 MiB = 0,25 GiB è esattamente a metà fra 0,2 e 0,3; 768 MiB = 0,75 GiB fra 0,7 e 0,8.
        assert_eq!(gib_x10(256 * MIB - 1), 2);
        assert_eq!(gib_x10(256 * MIB), 3);
        assert_eq!(gib_x10(768 * MIB - 1), 7);
        assert_eq!(gib_x10(768 * MIB), 8);
        // Metà di 0,1 GiB: 0,05 GiB = 53 687 091,2 byte.
        assert_eq!(gib_x10(53_687_091), 0);
        assert_eq!(gib_x10(53_687_092), 1);
    }

    #[test]
    fn gib_x10_changes_exactly_at_every_rounding_boundary() {
        // Il risultato passa da t − 1 a t al primo byte che vale almeno (t − 0,5) decimi di GiB,
        // cioè ceil((2t − 1) · 2³⁰ / 20). Confronto con un calcolo indipendente per 0..=300 GiB.
        for t in 1..=3_000u32 {
            let first = (u128::from(2 * t - 1) * u128::from(GIB)).div_ceil(20) as u64;
            assert_eq!(gib_x10(first), t, "primo byte del decimo {t}");
            assert_eq!(gib_x10(first - 1), t - 1, "ultimo byte del decimo {}", t - 1);
        }
    }

    #[test]
    fn gib_x10_saturates_without_overflow() {
        assert_eq!(gib_x10(u64::MAX), u32::MAX);
        // Poco sotto il tetto: 2³² − 1 decimi di GiB stanno ancora in u32.
        let max_ok = (u128::from(u32::MAX) * u128::from(GIB) / 10) as u64;
        assert_eq!(gib_x10(max_ok), u32::MAX);
        assert_eq!(gib_x10(max_ok - GIB), u32::MAX - 10);
    }
}
