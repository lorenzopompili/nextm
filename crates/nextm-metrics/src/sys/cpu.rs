//! Lettura del tempo di inattività di tutte le CPU logiche, senza svegliare i core.
//!
//! Fonte principale: `NtQuerySystemInformation(SystemPerformanceInformation)`, il cui primo campo
//! è `IdleProcessTime`, la somma dell'inattività di tutte le CPU logiche di tutti i gruppi.
//! Misurato sulla macchina di sviluppo (20 CPU logiche): stessi valori di `GetSystemTimes` entro
//! 0,16 punti in media (0,81 al massimo), ma circa 1,5 cambi di contesto per lettura invece di ~12.
//! `GetSystemTimes` e la classe 8 fanno girare il thread su più core per raccogliere i tempi di
//! ciascuno: sul portatile vuol dire svegliare core che dormono.
//!
//! Ripiego, se la classe 2 non risponde: `SystemProcessorPerformanceInformation` (classe 8),
//! un'interrogazione per gruppo di processori.

use core::ffi::c_void;
use core::mem::size_of;

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemPerformanceInformation,
    SystemProcessorPerformanceInformation,
};
use windows_sys::Win32::Foundation::NTSTATUS;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetActiveProcessorGroupCount,
};
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION as Sppi;

use crate::cpu::CpuSample;

// Non esposta da windows-sys: interrogazione dei contatori di un singolo gruppo di processori.
windows_link::link!("ntdll.dll" "system" fn NtQuerySystemInformationEx(
    systeminformationclass: SYSTEM_INFORMATION_CLASS,
    inputbuffer: *const c_void,
    inputbufferlength: u32,
    systeminformation: *mut c_void,
    systeminformationlength: u32,
    returnlength: *mut u32
) -> NTSTATUS);

/// Da dove arrivano i dati.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuSource {
    /// `SystemPerformanceInformation` (classe 2): il modo economico.
    Performance,
    /// `SystemProcessorPerformanceInformation` (classe 8), per gruppo: il ripiego.
    PerProcessor,
}

/// Legge istantanee `CpuSample` (inattività totale e orologio monotono).
pub struct CpuSampler {
    groups: u16,
    logical: u32,
    qpc_freq: i64,
    source: CpuSource,
    /// Buffer per la classe 2: la struttura è più grande del solo primo campo e cresce fra le build.
    perf: Vec<u64>,
    /// Buffer per il ripiego a classe 8 (vuoto finché non serve).
    per_cpu: Vec<Sppi>,
    /// Campione precedente per calcolare la percentuale per core.
    prev_per_cpu: Vec<Sppi>,
}

impl CpuSampler {
    pub fn new() -> CpuSampler {
        // SAFETY: funzioni senza parametri puntatore.
        let groups = unsafe { GetActiveProcessorGroupCount() }.max(1);
        let logical = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1);
        let mut qpc_freq = 0i64;
        // SAFETY: puntatore a una variabile locale.
        unsafe { QueryPerformanceFrequency(&mut qpc_freq) };
        let mut sampler = CpuSampler {
            groups,
            logical,
            qpc_freq: qpc_freq.max(1),
            source: CpuSource::Performance,
            perf: vec![0; 128],
            per_cpu: Vec::new(),
            prev_per_cpu: Vec::new(),
        };
        if sampler.idle_performance().is_none() {
            sampler.source = CpuSource::PerProcessor;
            sampler.per_cpu = vec![Sppi::default(); logical as usize];
        }
        sampler
    }

    /// Numero di CPU logiche attive (tutti i gruppi).
    pub fn logical_count(&self) -> u32 {
        self.logical
    }

    /// Numero di gruppi di processori.
    pub fn group_count(&self) -> u16 {
        self.groups
    }

    /// La fonte in uso.
    pub fn source(&self) -> CpuSource {
        self.source
    }

    /// Un'istantanea; `None` se la lettura fallisce.
    pub fn read(&mut self) -> Option<CpuSample> {
        let idle = match self.source {
            CpuSource::Performance => self.idle_performance()?,
            CpuSource::PerProcessor => self.idle_per_processor()?,
        };
        Some(CpuSample { idle, wall: self.wall_100ns() })
    }

    fn wall_100ns(&self) -> u64 {
        let mut t = 0i64;
        // SAFETY: puntatore a una variabile locale.
        unsafe { QueryPerformanceCounter(&mut t) };
        (i128::from(t.max(0)) * 10_000_000 / i128::from(self.qpc_freq)) as u64
    }

    fn idle_performance(&mut self) -> Option<u64> {
        let mut returned = 0u32;
        // SAFETY: il buffer ha `size_of_val` byte scrivibili; `returned` è valido.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemPerformanceInformation,
                self.perf.as_mut_ptr().cast::<c_void>(),
                size_of_val(self.perf.as_slice()) as u32,
                &mut returned,
            )
        };
        // Il primo campo (IdleProcessTime, LARGE_INTEGER) deve essere stato scritto.
        if status < 0 || (returned as usize) < size_of::<u64>() {
            return None;
        }
        self.perf.first().copied()
    }

    fn idle_per_processor(&mut self) -> Option<u64> {
        let mut idle = 0u64;
        let mut offset = 0usize;
        for group in 0..self.groups {
            let rest = self.per_cpu.get_mut(offset..)?;
            if rest.is_empty() {
                return None;
            }
            let bytes = size_of_val(rest) as u32;
            let mut returned = 0u32;
            let out = rest.as_mut_ptr().cast::<c_void>();
            // SAFETY: `out` punta a `bytes` byte scrivibili del buffer; `returned` è valido.
            let status = unsafe {
                if self.groups == 1 {
                    NtQuerySystemInformation(SystemProcessorPerformanceInformation, out, bytes, &mut returned)
                } else {
                    let input = group;
                    NtQuerySystemInformationEx(
                        SystemProcessorPerformanceInformation,
                        (&raw const input).cast::<c_void>(),
                        size_of::<u16>() as u32,
                        out,
                        bytes,
                        &mut returned,
                    )
                }
            };
            if status < 0 {
                return None;
            }
            let count = (returned as usize / size_of::<Sppi>()).min(rest.len());
            idle = rest[..count].iter().fold(idle, |acc, p| acc.wrapping_add(p.IdleTime as u64));
            offset += count;
        }
        Some(idle)
    }

    /// Azzera la base di calcolo del campionamento per core.
    pub fn reset_per_core(&mut self) {
        self.prev_per_cpu.clear();
    }

    /// Legge la percentuale di utilizzo di ciascuna CPU logica (0..100%).
    /// Restituisce `None` al primo campione (serve una base di confronto) o in caso di errore.
    pub fn read_per_core(&mut self) -> Option<Vec<u8>> {
        if self.per_cpu.len() < self.logical as usize {
            self.per_cpu.resize(self.logical as usize, Sppi::default());
        }

        let mut offset = 0usize;
        for group in 0..self.groups {
            let rest = self.per_cpu.get_mut(offset..)?;
            if rest.is_empty() {
                return None;
            }
            let bytes = size_of_val(rest) as u32;
            let mut returned = 0u32;
            let out = rest.as_mut_ptr().cast::<c_void>();
            // SAFETY: `out` punta a `bytes` byte scrivibili del buffer; `returned` è valido.
            let status = unsafe {
                if self.groups == 1 {
                    NtQuerySystemInformation(SystemProcessorPerformanceInformation, out, bytes, &mut returned)
                } else {
                    let input = group;
                    NtQuerySystemInformationEx(
                        SystemProcessorPerformanceInformation,
                        (&raw const input).cast::<c_void>(),
                        size_of::<u16>() as u32,
                        out,
                        bytes,
                        &mut returned,
                    )
                }
            };
            if status < 0 {
                return None;
            }
            let count = (returned as usize / size_of::<Sppi>()).min(rest.len());
            offset += count;
        }

        if self.prev_per_cpu.len() != self.logical as usize {
            self.prev_per_cpu = self.per_cpu.clone();
            return None;
        }

        let mut per_core = Vec::with_capacity(self.logical as usize);
        for i in 0..self.logical as usize {
            let cur = &self.per_cpu[i];
            let prev = &self.prev_per_cpu[i];
            let delta_kernel = cur.KernelTime.wrapping_sub(prev.KernelTime);
            let delta_user = cur.UserTime.wrapping_sub(prev.UserTime);
            let delta_idle = cur.IdleTime.wrapping_sub(prev.IdleTime);
            let delta_total = delta_kernel.wrapping_add(delta_user);

            let pct = if delta_total > 0 {
                let delta_busy = delta_total.saturating_sub(delta_idle).max(0);
                ((delta_busy * 100) / delta_total).min(100) as u8
            } else {
                0
            };
            per_core.push(pct);
        }

        self.prev_per_cpu.clone_from(&self.per_cpu);
        Some(per_core)
    }
}

impl Default for CpuSampler {
    fn default() -> CpuSampler {
        CpuSampler::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::busy_bp;

    #[test]
    fn reads_plausible_values() {
        let mut s = CpuSampler::new();
        assert!(s.logical_count() >= 1);
        let a = s.read().expect("prima lettura");
        std::thread::sleep(std::time::Duration::from_millis(250));
        let b = s.read().expect("seconda lettura");
        assert!(b.wall > a.wall);
        assert!(busy_bp(a, b, s.logical_count()).expect("tempo trascorso") <= 10_000);
    }

    #[test]
    fn performance_source_is_available_on_windows_11() {
        assert_eq!(CpuSampler::new().source(), CpuSource::Performance);
    }

    #[test]
    fn both_sources_agree() {
        let mut fast = CpuSampler::new();
        let mut slow = CpuSampler::new();
        slow.source = CpuSource::PerProcessor;
        slow.per_cpu = vec![Sppi::default(); slow.logical as usize];
        let (a1, b1) = (fast.read().expect("classe 2"), slow.read().expect("classe 8"));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (a2, b2) = (fast.read().expect("classe 2"), slow.read().expect("classe 8"));
        let n = fast.logical_count();
        let x = i32::from(busy_bp(a1, a2, n).expect("classe 2"));
        let y = i32::from(busy_bp(b1, b2, n).expect("classe 8"));
        // Entro 5 punti anche su una finestra breve (i contatori hanno granularità propria).
        assert!((x - y).abs() <= 500, "classe 2 = {x} bp, classe 8 = {y} bp");
    }

    #[test]
    fn reads_per_core_percentages() {
        let mut s = CpuSampler::new();
        let first = s.read_per_core();
        assert!(first.is_none(), "il primo campione inizializza la base");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let second = s.read_per_core().expect("secondo campione per core");
        assert_eq!(second.len(), s.logical_count() as usize);
        for &pct in &second {
            assert!(pct <= 100, "percentuale core <= 100: {pct}");
        }
    }
}
