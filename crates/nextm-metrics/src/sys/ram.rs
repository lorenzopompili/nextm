//! Lettura della memoria fisica con `GlobalMemoryStatusEx`.
//!
//! Una sola chiamata di sistema, senza allocazioni e senza DLL in più: la funzione sta in
//! kernel32, già collegata. Misurata sulla macchina di sviluppo con questo codice in release:
//! circa 0,7 µs a caldo e 6–11 µs quando la chiamata è una al secondo (cache fredde). Secondo
//! la ricerca non genera interrupt fra processori.
//!
//! Da non usare al suo posto:
//! - `GetPerformanceInfo`: a ogni tick costa circa 91 µs perché enumera processi, thread e handle;
//! - il contatore "% Committed Bytes In Use" di PDH: misura il commit, non la RAM fisica.

use core::mem::size_of;

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::ram::RamSample;

/// Un'istantanea della memoria fisica; `None` se la chiamata fallisce (nella pratica mai).
///
/// `total` è la RAM che Windows può usare, `available` è Standby + Free + Zero e
/// `load_percent` è il `dwMemoryLoad` che il sistema calcola sulla stessa lettura.
pub fn read_ram() -> Option<RamSample> {
    // Gli altri campi partono da zero e li riempie la chiamata.
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` è una variabile locale valida per la scrittura, con `dwLength` impostato
    // alla dimensione della struttura come richiede l'API.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    Some(RamSample {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,
        // Documentato fra 0 e 100: il limite garantisce che il cast non tronchi mai.
        load_percent: status.dwMemoryLoad.min(100) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// floor(100·(T−A)/T), la formula di `dwMemoryLoad`.
    fn expected_load(s: &RamSample) -> u128 {
        u128::from(s.used()) * 100 / u128::from(s.total)
    }

    fn assert_plausible(s: &RamSample) {
        assert!(s.total > 0, "totale = 0");
        assert!(s.available <= s.total, "disponibile {} oltre il totale {}", s.available, s.total);
        assert!(s.load_percent <= 100, "dwMemoryLoad = {}", s.load_percent);
        // La stessa formula, entro 1 punto: `dwMemoryLoad` vale 100 anche con meno di 100 pagine libere.
        let expected = expected_load(s);
        assert!(
            u128::from(s.load_percent).abs_diff(expected) <= 1,
            "dwMemoryLoad = {}, floor(100·(T−A)/T) = {expected} ({s:?})",
            s.load_percent
        );
    }

    #[test]
    fn reads_plausible_values() {
        let s = read_ram().expect("GlobalMemoryStatusEx");
        assert_plausible(&s);
    }

    #[test]
    fn repeated_reads_stay_plausible() {
        for _ in 0..200 {
            assert_plausible(&read_ram().expect("GlobalMemoryStatusEx"));
        }
    }

    #[test]
    fn values_are_bytes_in_whole_pages() {
        // Windows 11 richiede 4 GB: sotto 256 MiB i valori sarebbero in KiB o in pagine, non in byte.
        // Totale e disponibile sono numeri di pagine da 4 KiB (x64 e ARM64) moltiplicati per la pagina.
        let s = read_ram().expect("GlobalMemoryStatusEx");
        assert!(s.total >= 256 << 20, "totale = {} byte", s.total);
        assert_eq!(s.total % 4096, 0);
        assert_eq!(s.available % 4096, 0);
    }
}
