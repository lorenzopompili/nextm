//! CPU globale calcolata dal tempo di inattività del sistema e dal tempo trascorso.

/// Un'istantanea: tempo di inattività cumulativo di tutte le CPU logiche e un orologio
/// monotono, entrambi in unità da 100 ns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuSample {
    pub idle: u64,
    pub wall: u64,
}

/// Occupazione della CPU fra due istantanee, in centesimi di punto (0..=10000).
///
/// Formula: 1 − Δinattività / (Δtempo × CPU logiche). Restituisce `None` se non è trascorso
/// tempo, se un contatore è tornato indietro o se `logical` è 0: il chiamante riparte da una
/// nuova base. Il risultato è limitato a 0..=10000 (i contatori hanno una granularità propria).
pub fn busy_bp(prev: CpuSample, cur: CpuSample, logical: u32) -> Option<u16> {
    let d_idle = cur.idle.checked_sub(prev.idle)?;
    let d_wall = cur.wall.checked_sub(prev.wall)?;
    let capacity = u128::from(d_wall) * u128::from(logical);
    if capacity == 0 {
        return None;
    }
    let busy = capacity.saturating_sub(u128::from(d_idle));
    let bp = (busy * 10_000 + capacity / 2) / capacity;
    Some(bp.min(10_000) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(idle: u64, wall: u64) -> CpuSample {
        CpuSample { idle, wall }
    }

    #[test]
    fn quarter_busy_on_four_cpus() {
        // 1 s su 4 CPU = 4 s di capacità; 3 s inattivi -> 25,00%.
        assert_eq!(busy_bp(s(0, 0), s(30_000_000, 10_000_000), 4), Some(2_500));
    }

    #[test]
    fn fully_idle_and_fully_busy() {
        assert_eq!(busy_bp(s(0, 0), s(20_000_000, 10_000_000), 2), Some(0));
        assert_eq!(busy_bp(s(5, 0), s(5, 10_000_000), 2), Some(10_000));
    }

    #[test]
    fn idle_above_capacity_is_clamped_to_zero() {
        // Granularità dei contatori: l'inattività può superare di poco la capacità.
        assert_eq!(busy_bp(s(0, 0), s(21_000_000, 10_000_000), 2), Some(0));
    }

    #[test]
    fn no_time_or_no_cpus_gives_none() {
        assert_eq!(busy_bp(s(1, 5), s(2, 5), 4), None);
        assert_eq!(busy_bp(s(0, 0), s(1, 10), 0), None);
    }

    #[test]
    fn counters_going_backwards_give_none() {
        assert_eq!(busy_bp(s(10, 0), s(5, 100), 1), None);
        assert_eq!(busy_bp(s(0, 100), s(5, 50), 1), None);
    }

    #[test]
    fn rounds_half_up() {
        // 1 inattivo su 3 di capacità -> 66,666...% -> 6667.
        assert_eq!(busy_bp(s(0, 0), s(1, 3), 1), Some(6_667));
    }
}
