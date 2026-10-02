//! Livello di allarme di una percentuale mostrata (colore delle cifre).

/// Livello di una percentuale: normale, ambra, rosso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warn,
    Full,
}

/// Da questa percentuale le cifre diventano ambra.
pub const WARN_PERCENT: u8 = 70;
/// Da questa percentuale le cifre diventano rosse.
pub const FULL_ENTER_PERCENT: u8 = 90;
/// Una volta rosse, tornano ambra solo sotto questa percentuale (isteresi).
pub const FULL_EXIT_PERCENT: u8 = 80;

/// Calcola il livello con isteresi sul rosso, così il colore non sfarfalla attorno al 90%.
/// Lavora sul valore mostrato, che è già la media degli ultimi 3 campioni.
#[derive(Clone, Debug, Default)]
pub struct LevelTracker {
    full: bool,
}

impl LevelTracker {
    pub const fn new() -> LevelTracker {
        LevelTracker { full: false }
    }

    /// Dimentica lo stato (nuova base).
    pub fn reset(&mut self) {
        self.full = false;
    }

    /// Aggiorna lo stato con la percentuale mostrata e restituisce il livello.
    pub fn update(&mut self, shown_percent: u8) -> Level {
        if self.full {
            if shown_percent < FULL_EXIT_PERCENT {
                self.full = false;
            }
        } else if shown_percent >= FULL_ENTER_PERCENT {
            self.full = true;
        }
        if self.full {
            Level::Full
        } else if shown_percent >= WARN_PERCENT {
            Level::Warn
        } else {
            Level::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_without_history() {
        let mut t = LevelTracker::new();
        assert_eq!(t.update(69), Level::Normal);
        assert_eq!(t.update(70), Level::Warn);
        assert_eq!(t.update(89), Level::Warn);
        assert_eq!(t.update(90), Level::Full);
    }

    #[test]
    fn red_stays_until_below_eighty() {
        let mut t = LevelTracker::new();
        assert_eq!(t.update(95), Level::Full);
        assert_eq!(t.update(85), Level::Full);
        assert_eq!(t.update(80), Level::Full);
        assert_eq!(t.update(79), Level::Warn);
        assert_eq!(t.update(85), Level::Warn);
        assert_eq!(t.update(90), Level::Full);
    }

    #[test]
    fn drop_from_red_to_low_value_is_normal() {
        let mut t = LevelTracker::new();
        t.update(99);
        assert_eq!(t.update(10), Level::Normal);
    }

    #[test]
    fn reset_clears_red() {
        let mut t = LevelTracker::new();
        t.update(99);
        t.reset();
        assert_eq!(t.update(85), Level::Warn);
    }
}
