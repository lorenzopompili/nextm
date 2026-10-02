//! Media mobile degli ultimi campioni, come fa Task Manager.

/// Media degli ultimi 3 campioni (in centesimi di punto). Nessuna allocazione.
#[derive(Clone, Debug, Default)]
pub struct Mean3 {
    buf: [u16; 3],
    len: u8,
    next: u8,
}

impl Mean3 {
    pub const fn new() -> Mean3 {
        Mean3 { buf: [0; 3], len: 0, next: 0 }
    }

    /// Aggiunge un campione, sostituendo il più vecchio quando ce ne sono già 3.
    pub fn push(&mut self, value: u16) {
        self.buf[usize::from(self.next)] = value;
        self.next = (self.next + 1) % 3;
        if self.len < 3 {
            self.len += 1;
        }
    }

    /// Dimentica tutti i campioni (nuova base dopo pausa, sospensione o errore).
    pub fn reset(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    /// Media dei campioni presenti (da 1 a 3), arrotondata; `None` se non ce ne sono.
    pub fn mean(&self) -> Option<u16> {
        if self.len == 0 {
            return None;
        }
        let n = u32::from(self.len);
        let sum: u32 = self.buf[..usize::from(self.len)].iter().map(|&v| u32::from(v)).sum();
        Some(((sum + n / 2) / n) as u16)
    }
}

/// Stabilizza il numero mostrato: cambia subito se lo scarto è di almeno 2 punti, mentre uno
/// scarto di 1 punto (il rumore di misura vale circa ±1,5) deve durare `PATIENCE` campioni.
/// Così l'icona non si ridisegna, e Explorer non lavora, per il solo rumore.
#[derive(Clone, Debug, Default)]
pub struct Steady {
    shown: Option<u8>,
    pending: u8,
}

impl Steady {
    /// Campioni consecutivi dopo i quali uno scarto di 1 punto viene mostrato.
    pub const PATIENCE: u8 = 3;

    pub const fn new() -> Steady {
        Steady { shown: None, pending: 0 }
    }

    /// Dimentica il valore mostrato (nuova base).
    pub fn reset(&mut self) {
        self.shown = None;
        self.pending = 0;
    }

    /// Aggiorna con il nuovo valore e restituisce quello da mostrare.
    pub fn update(&mut self, value: u8) -> u8 {
        let shown = match self.shown {
            None => value,
            Some(s) => match s.abs_diff(value) {
                0 => s,
                1 => {
                    self.pending += 1;
                    if self.pending >= Steady::PATIENCE { value } else { s }
                }
                _ => value,
            },
        };
        if shown == value {
            self.pending = 0;
        }
        self.shown = Some(shown);
        shown
    }
}

/// Da centesimi di punto a percentuale intera arrotondata (0..=100).
pub fn bp_to_percent(bp: u16) -> u8 {
    ((u32::from(bp.min(10_000)) + 50) / 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_no_mean() {
        assert_eq!(Mean3::new().mean(), None);
    }

    #[test]
    fn mean_of_available_samples() {
        let mut m = Mean3::new();
        m.push(1_000);
        assert_eq!(m.mean(), Some(1_000));
        m.push(2_000);
        assert_eq!(m.mean(), Some(1_500));
        m.push(4_000);
        assert_eq!(m.mean(), Some(2_333));
    }

    #[test]
    fn keeps_only_last_three() {
        let mut m = Mean3::new();
        for v in [9_000, 1_000, 2_000, 3_000] {
            m.push(v);
        }
        assert_eq!(m.mean(), Some(2_000));
    }

    #[test]
    fn reset_forgets_samples() {
        let mut m = Mean3::new();
        m.push(5_000);
        m.reset();
        assert_eq!(m.mean(), None);
        m.push(100);
        assert_eq!(m.mean(), Some(100));
    }

    #[test]
    fn steady_shows_first_value_and_big_jumps_immediately() {
        let mut s = Steady::new();
        assert_eq!(s.update(10), 10);
        assert_eq!(s.update(12), 12);
        assert_eq!(s.update(40), 40);
        assert_eq!(s.update(38), 38);
    }

    #[test]
    fn steady_ignores_one_point_noise() {
        let mut s = Steady::new();
        s.update(10);
        for v in [11, 10, 9, 10, 11, 10] {
            assert_eq!(s.update(v), 10, "valore {v}");
        }
    }

    #[test]
    fn steady_follows_a_persistent_one_point_change() {
        let mut s = Steady::new();
        s.update(10);
        assert_eq!(s.update(11), 10);
        assert_eq!(s.update(11), 10);
        assert_eq!(s.update(11), 11);
        assert_eq!(s.update(11), 11);
    }

    #[test]
    fn steady_reset_starts_over() {
        let mut s = Steady::new();
        s.update(50);
        s.reset();
        assert_eq!(s.update(51), 51);
    }

    #[test]
    fn percent_rounding() {
        assert_eq!(bp_to_percent(0), 0);
        assert_eq!(bp_to_percent(49), 0);
        assert_eq!(bp_to_percent(50), 1);
        assert_eq!(bp_to_percent(4_249), 42);
        assert_eq!(bp_to_percent(4_250), 43);
        assert_eq!(bp_to_percent(10_000), 100);
        assert_eq!(bp_to_percent(12_000), 100);
    }
}
