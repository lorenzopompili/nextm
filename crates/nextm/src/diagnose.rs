//! `nextm --diagnose`: rapporto su ciò che nextm rileva su questo PC, da allegare alle segnalazioni.

use nextm_metrics::sys::cpu::{CpuSampler, CpuSource};
use nextm_render::Theme;

use crate::sys::{autostart, console, shell, store::Store, theme, version};

/// Testo del rapporto.
pub fn report() -> String {
    let (major, minor, build) = version::windows_version();
    let cpu = CpuSampler::new();
    let taskbar = shell::taskbar();
    let dpi = shell::dpi_of(taskbar);
    let detected = Store::detect();
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let theme = match theme::taskbar_theme() {
        Theme::Dark => "scuro",
        Theme::Light => "chiaro",
        Theme::HighContrast { .. } => "alto contrasto",
    };
    let store = match &detected.store {
        Store::Registry if detected.portable_readonly => "registro (nextm.ini presente ma non scrivibile)".to_string(),
        Store::Registry => "registro (HKCU\\Software\\nextm)".to_string(),
        Store::Portable(p) => format!("portable ({})", p.display()),
    };
    format!(
        "nextm {} - diagnostica\n\
         Windows: {major}.{minor}.{build}\n\
         Eseguibile: {exe}\n\
         CPU logiche: {} in {} gruppi; lettura: {}\n\
         Taskbar: {}; DPI {dpi}; icona {} px; tema {theme}\n\
         Impostazioni: {store}\n\
         Avvio con Windows: {}\n",
        env!("CARGO_PKG_VERSION"),
        cpu.logical_count(),
        cpu.group_count(),
        match cpu.source() {
            CpuSource::Performance => "SystemPerformanceInformation",
            CpuSource::PerProcessor => "SystemProcessorPerformanceInformation (ripiego)",
        },
        if taskbar.is_null() { "non trovata" } else { "trovata" },
        shell::small_icon_size(dpi),
        if autostart::is_enabled(&exe) { "sì" } else { "no" },
    )
}

/// Stampa il rapporto nella console di partenza; senza console lo salva in
/// `%TEMP%\nextm-diagnose.txt` e lo apre con l'editor predefinito.
pub fn run() {
    let text = report();
    if console::write_to_parent_console(&format!("\n{text}")) {
        return;
    }
    let path = std::env::temp_dir().join("nextm-diagnose.txt");
    if std::fs::write(&path, &text).is_ok() {
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        shell::open_with_explorer(&wide);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_has_the_main_sections() {
        let r = report();
        for key in ["Windows:", "CPU logiche:", "Taskbar:", "Impostazioni:", "Avvio con Windows:"] {
            assert!(r.contains(key), "manca {key} in:\n{r}");
        }
    }
}
