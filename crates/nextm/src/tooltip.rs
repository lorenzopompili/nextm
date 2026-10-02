//! Testo del tooltip dell'icona.

use crate::strings::Strings;
use crate::wide::WBuf;
use nextm_metrics::saturation::SatState;

/// Il tooltip ha al massimo 127 caratteri più lo zero finale (`szTip`).
pub type TipBuf = WBuf<128>;

pub struct TooltipData<'a> {
    pub cpu_active: bool,
    pub cpu_percent: Option<u8>,
    pub cpu_utility: Option<u8>,
    pub sat_state: SatState,
    pub sat_top_proc: Option<(&'a str, u32)>,
    pub cpu_per_core: Option<&'a [u8]>,
    pub ram_active: bool,
    pub ram_percent: Option<u8>,
    pub ram_used_x10: Option<u32>,
    pub ram_total_x10: Option<u32>,
    pub net_active: bool,
    pub net_label: Option<&'a str>,
    pub net_down: Option<&'a str>,
    pub net_up: Option<&'a str>,
    // Temperature
    pub temp_acpi_active: bool,
    pub temp_acpi_c: Option<i16>,
    pub temp_acpi_fixed: bool,
    pub temp_gpu_active: bool,
    pub temp_gpu_c: Option<i16>,
    pub temp_gpu_age_secs: u32,
    pub temp_disk_active: bool,
    pub temp_disk_c: Option<i16>,
    pub temp_disk_label: Option<&'a str>,
    pub temp_disk_age_secs: u32,
}

/// Costruisce il tooltip composito con le metriche attive (spec §4.2 e §5).
pub fn build_tooltip(out: &mut TipBuf, s: &Strings, is_it: bool, data: &TooltipData) {
    out.clear();

    let has_any_temp = data.temp_acpi_active || data.temp_gpu_active || data.temp_disk_active;
    if !data.cpu_active && !data.ram_active && !data.net_active && !has_any_temp {
        out.push_str("nextm");
        return;
    }

    let mut first_line = true;

    // 1. CPU
    if data.cpu_active {
        first_line = false;
        out.push_wide(s.cpu);
        out.push(b' ' as u16);
        if let Some(util) = data.cpu_utility {
            out.push_u32(u32::from(util));
            out.push(b'%' as u16);
            out.push_str(if is_it { " utilità · tempo CPU " } else { " utility · CPU time " });
            if let Some(p) = data.cpu_percent {
                out.push_u32(u32::from(p));
                out.push(b'%' as u16);
            } else {
                out.push(0x2014);
            }
        } else if let Some(p) = data.cpu_percent {
            out.push_u32(u32::from(p));
            out.push(b'%' as u16);
        } else {
            out.push(0x2014);
        }

        match data.sat_state {
            SatState::Normal => {}
            SatState::Full => {
                out.push_str(if is_it { " · piena" } else { " · full" });
            }
            SatState::Saturated(k) => {
                out.push_str(" · ");
                out.push_u32(u32::from(k));
                if k == 1 {
                    out.push_str(if is_it { " core saturo" } else { " core saturated" });
                } else {
                    out.push_str(if is_it { " core saturi" } else { " cores saturated" });
                }
            }
        }

        if let Some((name, core100)) = data.sat_top_proc {
            out.push(b'\n' as u16);
            let truncated_name = if name.len() > 15 {
                let mut end = 15;
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                &name[..end]
            } else {
                name
            };
            out.push_str(truncated_name);
            out.push_str(": ");
            out.push_u32(core100 / 100);
            out.push(if is_it { b',' as u16 } else { b'.' as u16 });
            out.push_u32((core100 % 100) / 10);
            out.push_str(if is_it { " core" } else { " cores" });
        }

        if let Some(cores) = data.cpu_per_core
            && cores.len() <= 4
        {
            out.push(b'\n' as u16);
            for (i, &p) in cores.iter().enumerate() {
                if i > 0 {
                    out.push(b' ' as u16);
                }
                out.push(b'C' as u16);
                out.push_u32(i as u32);
                out.push(b':' as u16);
                out.push_u32(u32::from(p));
                out.push(b'%' as u16);
            }
        }
    }

    // 2. RAM
    if data.ram_active {
        if !first_line {
            out.push(b'\n' as u16);
        }
        first_line = false;
        out.push_wide(s.ram);
        out.push(b' ' as u16);
        if let Some(p) = data.ram_percent {
            out.push_u32(u32::from(p));
            out.push(b'%' as u16);
        } else {
            out.push(0x2014);
        }
        if let (Some(used), Some(total)) = (data.ram_used_x10, data.ram_total_x10) {
            out.push_str(" · ");
            push_gib(out, used, is_it);
            out.push(b'/' as u16);
            push_gib(out, total, is_it);
            out.push_str(" GB");
        }
    }

    // 3. Rete
    if data.net_active {
        if !first_line {
            out.push(b'\n' as u16);
        }
        first_line = false;
        out.push_wide(s.net);
        if let Some(lbl) = data.net_label {
            out.push_str(" (");
            out.push_str(lbl);
            out.push(b')' as u16);
        }
        if let Some(down) = data.net_down {
            out.push_str(" \u{2193} ");
            out.push_str(down);
        }
        if let Some(up) = data.net_up {
            out.push_str(" \u{2191} ");
            out.push_str(up);
        }
    }

    // 4. Temperature
    if has_any_temp {
        if !first_line {
            out.push(b'\n' as u16);
        }
        let mut first_item = true;

        if data.temp_acpi_active {
            first_item = false;
            out.push_str("CPU ");
            if let Some(t) = data.temp_acpi_c {
                out.push_u32(t.max(0) as u32);
                out.push(0x00B0); // °
                if data.temp_acpi_fixed {
                    out.push(b' ' as u16);
                    out.push(b'(' as u16);
                    out.push_wide(s.temp_fixed);
                    out.push(b')' as u16);
                }
            } else {
                out.push(0x2014); // —
            }
        }

        if data.temp_gpu_active {
            if !first_item {
                out.push_str(" · ");
            }
            first_item = false;
            out.push_str("GPU ");
            if let Some(t) = data.temp_gpu_c {
                out.push_u32(t.max(0) as u32);
                out.push(0x00B0);
                if data.temp_gpu_age_secs >= 2 {
                    out.push_str(" (");
                    out.push_u32(data.temp_gpu_age_secs);
                    out.push(b' ' as u16);
                    out.push_wide(s.seconds_ago);
                    out.push(b')' as u16);
                }
            } else {
                out.push(0x2014);
            }
        }

        if data.temp_disk_active {
            if !first_item {
                out.push_str(" · ");
            }
            let lbl = data.temp_disk_label.unwrap_or("Disco");
            out.push_str(lbl);
            out.push(b' ' as u16);
            if let Some(t) = data.temp_disk_c {
                out.push_u32(t.max(0) as u32);
                out.push(0x00B0);
                if data.temp_disk_age_secs >= 2 {
                    out.push_str(" (");
                    out.push_u32(data.temp_disk_age_secs);
                    out.push(b' ' as u16);
                    out.push_wide(s.seconds_ago);
                    out.push(b')' as u16);
                }
            } else {
                out.push(0x2014);
            }
        }
    }
}

fn push_gib(out: &mut TipBuf, val_x10: u32, is_it: bool) {
    out.push_u32(val_x10 / 10);
    out.push(if is_it { b',' as u16 } else { b'.' as u16 });
    out.push_u32(val_x10 % 10);
}

/// Tooltip compatibilità M1 per la sola CPU.
#[allow(dead_code)]
pub fn cpu_tooltip(out: &mut TipBuf, s: &Strings, percent: Option<u8>) {
    out.clear();
    out.push_wide(s.cpu);
    out.push(b' ' as u16);
    match percent {
        Some(p) => {
            out.push_u32(u32::from(p));
            out.push(b'%' as u16);
        }
        None => {
            out.push(0x2014);
        }
    }
}

/// Testo dell'icona per una percentuale: "0".."100", oppure "—" se manca il dato.
/// Scrive in `buf` e restituisce la stringa pronta per il disegno.
pub fn icon_text(percent: Option<u8>, buf: &mut [u8; 3]) -> &str {
    let Some(p) = percent else { return "\u{2014}" };
    let p = p.min(100);
    let n = if p >= 100 {
        buf.copy_from_slice(b"100");
        3
    } else if p >= 10 {
        buf[0] = b'0' + p / 10;
        buf[1] = b'0' + p % 10;
        2
    } else {
        buf[0] = b'0' + p;
        1
    };
    core::str::from_utf8(&buf[..n]).unwrap_or("\u{2014}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{EN, IT};

    fn text(b: &TipBuf) -> String {
        String::from_utf16_lossy(b.as_slice())
    }

    #[test]
    fn cpu_line() {
        let mut b = TipBuf::new();
        cpu_tooltip(&mut b, &IT, Some(42));
        assert_eq!(text(&b), "CPU 42%");
        cpu_tooltip(&mut b, &EN, None);
        assert_eq!(text(&b), "CPU —");
    }

    #[test]
    fn composite_tooltip_format() {
        let mut b = TipBuf::new();
        let data = TooltipData {
            cpu_active: true,
            cpu_percent: Some(12),
            cpu_utility: None,
            sat_state: SatState::Saturated(1),
            sat_top_proc: Some(("pwsh.exe", 100)),
            cpu_per_core: None,
            ram_active: true,
            ram_percent: Some(63),
            ram_used_x10: Some(201),
            ram_total_x10: Some(313),
            net_active: true,
            net_label: Some("somma: Ethernet 7"),
            net_down: Some("2,1 MB/s"),
            net_up: Some("340 kB/s"),
            temp_acpi_active: false,
            temp_acpi_c: None,
            temp_acpi_fixed: false,
            temp_gpu_active: false,
            temp_gpu_c: None,
            temp_gpu_age_secs: 0,
            temp_disk_active: false,
            temp_disk_c: None,
            temp_disk_label: None,
            temp_disk_age_secs: 0,
        };
        build_tooltip(&mut b, &IT, true, &data);
        let s = text(&b);
        assert!(s.contains("CPU 12% · 1 core saturo"));
        assert!(s.contains("pwsh.exe: 1,0 core"));
        assert!(s.contains("RAM 63% · 20,1/31,3 GB"));
        assert!(s.contains("Rete (somma: Ethernet 7) ↓ 2,1 MB/s ↑ 340 kB/s"));
    }

    #[test]
    fn per_core_hover_tooltip_format() {
        let mut b = TipBuf::new();
        let data = TooltipData {
            cpu_active: true,
            cpu_percent: Some(25),
            cpu_utility: None,
            sat_state: SatState::Normal,
            sat_top_proc: None,
            cpu_per_core: Some(&[10, 95, 4, 1]),
            ram_active: false,
            ram_percent: None,
            ram_used_x10: None,
            ram_total_x10: None,
            net_active: false,
            net_label: None,
            net_down: None,
            net_up: None,
            temp_acpi_active: false,
            temp_acpi_c: None,
            temp_acpi_fixed: false,
            temp_gpu_active: false,
            temp_gpu_c: None,
            temp_gpu_age_secs: 0,
            temp_disk_active: false,
            temp_disk_c: None,
            temp_disk_label: None,
            temp_disk_age_secs: 0,
        };
        build_tooltip(&mut b, &IT, true, &data);
        let s = text(&b);
        assert_eq!(s, "CPU 25%\nC0:10% C1:95% C2:4% C3:1%");
    }

    #[test]
    fn temperature_tooltip_format() {
        let mut b = TipBuf::new();
        let data = TooltipData {
            cpu_active: true,
            cpu_percent: Some(8),
            cpu_utility: None,
            sat_state: SatState::Normal,
            sat_top_proc: None,
            cpu_per_core: None,
            ram_active: false,
            ram_percent: None,
            ram_used_x10: None,
            ram_total_x10: None,
            net_active: false,
            net_label: None,
            net_down: None,
            net_up: None,
            temp_acpi_active: true,
            temp_acpi_c: Some(68),
            temp_acpi_fixed: false,
            temp_gpu_active: true,
            temp_gpu_c: Some(55),
            temp_gpu_age_secs: 3,
            temp_disk_active: true,
            temp_disk_c: Some(44),
            temp_disk_label: Some("SSD NVMe"),
            temp_disk_age_secs: 40,
        };
        build_tooltip(&mut b, &IT, true, &data);
        let s = text(&b);
        assert!(s.contains("CPU 8%"));
        assert!(s.contains("CPU 68° · GPU 55° (3 s fa) · SSD NVMe 44° (40 s fa)"));

        // EN test
        build_tooltip(&mut b, &EN, false, &data);
        let s_en = text(&b);
        assert!(s_en.contains("CPU 68° · GPU 55° (3 s ago) · SSD NVMe 44° (40 s ago)"));
    }

    #[test]
    fn fixed_acpi_temp_format() {
        let mut b = TipBuf::new();
        let data = TooltipData {
            cpu_active: false,
            cpu_percent: None,
            cpu_utility: None,
            sat_state: SatState::Normal,
            sat_top_proc: None,
            cpu_per_core: None,
            ram_active: false,
            ram_percent: None,
            ram_used_x10: None,
            ram_total_x10: None,
            net_active: false,
            net_label: None,
            net_down: None,
            net_up: None,
            temp_acpi_active: true,
            temp_acpi_c: Some(65),
            temp_acpi_fixed: true,
            temp_gpu_active: false,
            temp_gpu_c: None,
            temp_gpu_age_secs: 0,
            temp_disk_active: false,
            temp_disk_c: None,
            temp_disk_label: None,
            temp_disk_age_secs: 0,
        };
        build_tooltip(&mut b, &IT, true, &data);
        assert_eq!(text(&b), "CPU 65° (fissa)");
        build_tooltip(&mut b, &EN, false, &data);
        assert_eq!(text(&b), "CPU 65° (fixed)");
    }

    #[test]
    fn empty_metrics_shows_nextm() {
        let mut b = TipBuf::new();
        let data = TooltipData {
            cpu_active: false,
            cpu_percent: None,
            cpu_utility: None,
            sat_state: SatState::Normal,
            sat_top_proc: None,
            cpu_per_core: None,
            ram_active: false,
            ram_percent: None,
            ram_used_x10: None,
            ram_total_x10: None,
            net_active: false,
            net_label: None,
            net_down: None,
            net_up: None,
            temp_acpi_active: false,
            temp_acpi_c: None,
            temp_acpi_fixed: false,
            temp_gpu_active: false,
            temp_gpu_c: None,
            temp_gpu_age_secs: 0,
            temp_disk_active: false,
            temp_disk_c: None,
            temp_disk_label: None,
            temp_disk_age_secs: 0,
        };
        build_tooltip(&mut b, &IT, true, &data);
        assert_eq!(text(&b), "nextm");
    }

    #[test]
    fn icon_texts() {
        let mut buf = [0u8; 3];
        assert_eq!(icon_text(Some(0), &mut buf), "0");
        assert_eq!(icon_text(Some(7), &mut buf), "7");
        assert_eq!(icon_text(Some(42), &mut buf), "42");
        assert_eq!(icon_text(Some(100), &mut buf), "100");
        assert_eq!(icon_text(Some(250), &mut buf), "100");
        assert_eq!(icon_text(None, &mut buf), "—");
    }
}
