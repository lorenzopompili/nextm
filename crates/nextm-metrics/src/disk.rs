//! Rappresentazione e formattazione dello spazio su disco per unità montate.

/// Informazioni sullo spazio di un'unità disco montata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedDisk {
    pub letter: char,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl MountedDisk {
    pub const fn new(letter: char, total_bytes: u64, free_bytes: u64) -> Self {
        Self { letter, total_bytes, free_bytes }
    }

    /// Restituisce i byte occupati sull'unità.
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    /// Percentuale di spazio occupato (da 0 a 100).
    pub fn used_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            0
        } else {
            ((self.used_bytes() as u128 * 100) / self.total_bytes as u128).min(100) as u8
        }
    }
}

/// Formatta una quantità di byte in una stringa leggibile (es. "749 GB", "1,5 TB" o "512 MB").
pub fn format_disk_size(bytes: u64, is_it: bool) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB {
        let int = bytes / TIB;
        let dec = ((bytes % TIB) * 10) / TIB;
        let sep = if is_it { ',' } else { '.' };
        format!("{int}{sep}{dec} TB")
    } else if bytes >= GIB {
        let gb = (bytes + GIB / 2) / GIB;
        format!("{gb} GB")
    } else {
        let mb = (bytes + MIB / 2) / MIB;
        format!("{mb} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_used_and_percent() {
        let d = MountedDisk::new('C', 1_000_000_000, 200_000_000);
        assert_eq!(d.used_bytes(), 800_000_000);
        assert_eq!(d.used_percent(), 80);

        let empty = MountedDisk::new('D', 0, 0);
        assert_eq!(empty.used_bytes(), 0);
        assert_eq!(empty.used_percent(), 0);

        let full = MountedDisk::new('E', 500_000, 0);
        assert_eq!(full.used_bytes(), 500_000);
        assert_eq!(full.used_percent(), 100);
    }

    #[test]
    fn formats_sizes_in_different_units() {
        // MB
        assert_eq!(format_disk_size(500 * 1024 * 1024, true), "500 MB");
        // GB
        assert_eq!(format_disk_size(200 * 1024 * 1024 * 1024, true), "200 GB");
        // TB (Italiano con virgola, Inglese con punto)
        let tb_bytes = 1536 * 1024 * 1024 * 1024; // 1.5 TB
        assert_eq!(format_disk_size(tb_bytes, true), "1,5 TB");
        assert_eq!(format_disk_size(tb_bytes, false), "1.5 TB");
    }
}
