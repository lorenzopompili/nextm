//! Campionamento e aggregazione dei processi top per CPU, RAM e Rete/IO.

use std::collections::BTreeMap;

use crate::inspect::ProcessRecord;
use crate::sys::processes::ProcessScanner;

/// Risultato del campionamento dei processi top per CPU, RAM e Rete/IO.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopProcessResult {
    pub cpu: Vec<(String, String)>,
    pub ram: Vec<(String, String)>,
    pub net: Vec<(String, String)>,
}

/// Tracciatore e calcolatore dei processi a maggior consumo di risorse.
pub struct TopProcessTracker {
    scanner: ProcessScanner,
    prev_cpu: BTreeMap<u32, u64>,
    next_prev_cpu: BTreeMap<u32, u64>,
    prev_io: BTreeMap<u32, u64>,
    next_prev_io: BTreeMap<u32, u64>,
    last_tick_ms: u64,
    num_logical_cpus: u32,
    scratch: Vec<ProcessRecord>,
}

impl Default for TopProcessTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TopProcessTracker {
    /// Inizializza il tracciatore dei top process.
    pub fn new() -> Self {
        let num_logical_cpus = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        }
        .max(1);

        Self {
            scanner: ProcessScanner::new(),
            prev_cpu: BTreeMap::new(),
            next_prev_cpu: BTreeMap::new(),
            prev_io: BTreeMap::new(),
            next_prev_io: BTreeMap::new(),
            last_tick_ms: 0,
            num_logical_cpus,
            scratch: Vec::with_capacity(512),
        }
    }

    /// Rilascia memoria e reimposta il baseline temporale.
    pub fn reset(&mut self) {
        self.prev_cpu.clear();
        self.next_prev_cpu.clear();
        self.prev_io.clear();
        self.next_prev_io.clear();
        self.last_tick_ms = 0;
        self.scratch.clear();
        self.scanner.release();
    }

    /// Campiona i processi correnti e restituisce i top N per CPU, RAM e Rete/IO (max 25 ciascuno).
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        n_cpu: usize,
        n_ram: usize,
        n_net: usize,
        want_cpu: bool,
        want_ram: bool,
        want_net: bool,
        is_it: bool,
    ) -> TopProcessResult {
        let n_cpu = n_cpu.clamp(1, 25);
        let n_ram = n_ram.clamp(1, 25);
        let n_net = n_net.clamp(1, 25);
        if (!want_cpu && !want_ram && !want_net) || (n_cpu == 0 && n_ram == 0 && n_net == 0) {
            return TopProcessResult::default();
        }

        if !self.scanner.scan(&mut self.scratch) {
            return TopProcessResult::default();
        }

        let now_ms = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        let delta_ms =
            if self.last_tick_ms > 0 && now_ms > self.last_tick_ms { now_ms - self.last_tick_ms } else { 1000 };
        self.last_tick_ms = now_ms;

        let delta_time_100ns = (delta_ms as f32) * 10_000.0 * (self.num_logical_cpus as f32);

        self.next_prev_cpu.clear();
        self.next_prev_io.clear();

        struct Acc {
            display_name: String,
            count: usize,
            cpu_pct: f32,
            ram_bytes: u64,
            io_rate_bps: u64,
        }

        let mut groups: Vec<Acc> = Vec::with_capacity(64);

        for p in &self.scratch {
            self.next_prev_cpu.insert(p.pid, p.cpu_time_100ns);
            let io_total = p.io_read_bytes.saturating_add(p.io_write_bytes);
            self.next_prev_io.insert(p.pid, io_total);

            // Escludi PID 0 ("System Idle Process")
            if p.pid == 0 {
                continue;
            }

            let cpu_pct = if want_cpu {
                if let Some(&old_cpu) = self.prev_cpu.get(&p.pid) {
                    let cpu_delta = p.cpu_time_100ns.saturating_sub(old_cpu) as f32;
                    if delta_time_100ns > 0.0 {
                        ((cpu_delta / delta_time_100ns) * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            } else {
                0.0
            };

            let io_rate_bps = if want_net {
                if let Some(&old_io) = self.prev_io.get(&p.pid) {
                    let io_delta = io_total.saturating_sub(old_io);
                    (io_delta * 1000).checked_div(delta_ms).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };

            if let Some(entry) = groups.iter_mut().find(|g| g.display_name.eq_ignore_ascii_case(&p.name)) {
                entry.count += 1;
                entry.cpu_pct += cpu_pct;
                entry.ram_bytes = entry.ram_bytes.saturating_add(p.working_set_bytes);
                entry.io_rate_bps = entry.io_rate_bps.saturating_add(io_rate_bps);
            } else {
                groups.push(Acc {
                    display_name: p.name.clone(),
                    count: 1,
                    cpu_pct,
                    ram_bytes: p.working_set_bytes,
                    io_rate_bps,
                });
            }
        }

        std::mem::swap(&mut self.prev_cpu, &mut self.next_prev_cpu);
        std::mem::swap(&mut self.prev_io, &mut self.next_prev_io);

        // 1. Top CPU
        let cpu_list = if want_cpu {
            groups.sort_by(|a, b| b.cpu_pct.partial_cmp(&a.cpu_pct).unwrap_or(core::cmp::Ordering::Equal));
            groups
                .iter()
                .take(n_cpu)
                .map(|item| {
                    let name = format_display_name(&item.display_name, item.count);
                    let val = format_cpu_pct(item.cpu_pct, is_it);
                    (name, val)
                })
                .collect()
        } else {
            Vec::new()
        };

        // 2. Top RAM
        let ram_list = if want_ram {
            groups.sort_by_key(|a| core::cmp::Reverse(a.ram_bytes));
            groups
                .iter()
                .take(n_ram)
                .map(|item| {
                    let name = format_display_name(&item.display_name, item.count);
                    let val = format_ram_bytes(item.ram_bytes, is_it);
                    (name, val)
                })
                .collect()
        } else {
            Vec::new()
        };

        // 3. Top Net/IO
        let net_list = if want_net {
            groups.sort_by_key(|a| core::cmp::Reverse(a.io_rate_bps));
            groups
                .iter()
                .filter(|item| item.io_rate_bps > 0)
                .take(n_net)
                .map(|item| {
                    let name = format_display_name(&item.display_name, item.count);
                    let val = format_io_rate(item.io_rate_bps, is_it);
                    (name, val)
                })
                .collect()
        } else {
            Vec::new()
        };

        TopProcessResult { cpu: cpu_list, ram: ram_list, net: net_list }
    }
}

pub fn format_display_name(raw_name: &str, count: usize) -> String {
    let base = if count > 1 { format!("{raw_name} ({count})") } else { raw_name.to_string() };
    if base.chars().count() > 24 {
        let mut s: String = base.chars().take(22).collect();
        s.push('…');
        s
    } else {
        base
    }
}

pub fn format_cpu_pct(pct: f32, is_it: bool) -> String {
    let int_x10 = (pct * 10.0).round() as u32;
    let sep = if is_it { ',' } else { '.' };
    format!("{}{}{}%", int_x10 / 10, sep, int_x10 % 10)
}

pub fn format_ram_bytes(bytes: u64, is_it: bool) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    let sep = if is_it { ',' } else { '.' };
    if bytes >= GIB {
        let gib_x10 = (bytes * 10 + GIB / 2) / GIB;
        format!("{}{}{} GB", gib_x10 / 10, sep, gib_x10 % 10)
    } else {
        let mib = (bytes + MIB / 2) / MIB;
        format!("{mib} MB")
    }
}

pub fn format_io_rate(bytes_sec: u64, is_it: bool) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    let sep = if is_it { ',' } else { '.' };
    if bytes_sec >= MIB {
        let mib_x10 = (bytes_sec * 10 + MIB / 2) / MIB;
        format!("{}{}{} MB/s", mib_x10 / 10, sep, mib_x10 % 10)
    } else if bytes_sec >= KIB {
        let kib = (bytes_sec + KIB / 2) / KIB;
        format!("{kib} KB/s")
    } else {
        format!("{bytes_sec} B/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_names_and_counts() {
        assert_eq!(format_display_name("chrome.exe", 1), "chrome.exe");
        assert_eq!(format_display_name("chrome.exe", 5), "chrome.exe (5)");
        assert_eq!(format_display_name("VeryLongProcessNameToTruncate.exe", 1), "VeryLongProcessNameToT…");
    }

    #[test]
    fn formats_cpu_percentages() {
        assert_eq!(format_cpu_pct(12.34, true), "12,3%");
        assert_eq!(format_cpu_pct(12.34, false), "12.3%");
        assert_eq!(format_cpu_pct(0.0, true), "0,0%");
    }

    #[test]
    fn formats_ram_units() {
        assert_eq!(format_ram_bytes(512 * 1024 * 1024, true), "512 MB");
        assert_eq!(format_ram_bytes(1536 * 1024 * 1024, true), "1,5 GB");
        assert_eq!(format_ram_bytes(1536 * 1024 * 1024, false), "1.5 GB");
    }

    #[test]
    fn formats_io_rate_units() {
        assert_eq!(format_io_rate(500, true), "500 B/s");
        assert_eq!(format_io_rate(150 * 1024, true), "150 KB/s");
        assert_eq!(format_io_rate(1500 * 1024, true), "1,5 MB/s");
        assert_eq!(format_io_rate(1500 * 1024, false), "1.5 MB/s");
    }

    #[test]
    fn live_tracker_samples_processes() {
        let mut tracker = TopProcessTracker::new();
        let res = tracker.sample(5, 5, 5, true, true, true, true);
        assert!(!res.ram.is_empty(), "RAM top processes should not be empty");
        tracker.reset();
    }
}
