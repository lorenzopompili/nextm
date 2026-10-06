//! Registratore di sessione a scatola nera (Blackbox Flight Recorder).
//!
//! Registra metriche hardware e di sistema su file CSV circolare per analisi post-mortem
//! e diagnosi di picchi o blocchi anomali del computer.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Registratore di sessione su file CSV.
pub struct BlackboxRecorder {
    file_path: PathBuf,
    last_write_ms: u64,
}

impl BlackboxRecorder {
    /// Inizializza il registratore blackbox nella cartella dati di nextm.
    pub fn new() -> Self {
        let dir = Self::log_dir();
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("nextm_blackbox.csv");

        let recorder = Self { file_path, last_write_ms: 0 };
        recorder.ensure_header();
        recorder
    }

    /// Percorso della cartella contenente i log blackbox.
    pub fn log_dir() -> PathBuf {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            PathBuf::from(local_app_data).join("nextm").join("blackbox")
        } else {
            PathBuf::from(".").join("blackbox")
        }
    }

    /// Assicura che l'intestazione CSV sia presente all'inizio del file.
    fn ensure_header(&self) {
        if let Ok(metadata) = std::fs::metadata(&self.file_path)
            && metadata.len() > 0
        {
            return;
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.file_path) {
            let _ = writeln!(
                f,
                "timestamp_unix_ms,cpu_pct,ram_pct,net_down_bps,net_up_bps,gpu_pct,temp_cpu_c,temp_gpu_c,temp_disk_c,top_cpu_proc,top_ram_proc"
            );
        }
    }

    /// Controlla la dimensione del file ed esegue una rotazione se supera 5 MB.
    fn check_rotation(&self) {
        if let Ok(metadata) = std::fs::metadata(&self.file_path)
            && metadata.len() >= 5 * 1024 * 1024
        {
            let prev_path = self.file_path.with_file_name("nextm_blackbox.prev.csv");
            let _ = std::fs::rename(&self.file_path, prev_path);
            self.ensure_header();
        }
    }

    /// Scrive una riga di telemetria se è trascorso l'intervallo specificato (es. 2 secondi).
    #[allow(clippy::too_many_arguments)]
    pub fn record_sample(
        &mut self,
        now_ms: u64,
        cpu_pct: u8,
        ram_pct: u8,
        net_down_bps: u64,
        net_up_bps: u64,
        gpu_pct: Option<u8>,
        temp_cpu_c: Option<i16>,
        temp_gpu_c: Option<i16>,
        temp_disk_c: Option<i16>,
        top_cpu_proc: Option<&str>,
        top_ram_proc: Option<&str>,
    ) {
        if self.last_write_ms > 0 && now_ms.saturating_sub(self.last_write_ms) < 2000 {
            return;
        }
        self.last_write_ms = now_ms;
        self.check_rotation();

        let epoch_now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);

        let gpu_str = gpu_pct.map(|g| g.to_string()).unwrap_or_default();
        let cpu_t_str = temp_cpu_c.map(|t| t.to_string()).unwrap_or_default();
        let gpu_t_str = temp_gpu_c.map(|t| t.to_string()).unwrap_or_default();
        let disk_t_str = temp_disk_c.map(|t| t.to_string()).unwrap_or_default();
        let top_cpu = top_cpu_proc.unwrap_or_default().replace(',', " ");
        let top_ram = top_ram_proc.unwrap_or_default().replace(',', " ");

        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.file_path) {
            let _ = writeln!(
                f,
                "{epoch_now},{cpu_pct},{ram_pct},{net_down_bps},{net_up_bps},{gpu_str},{cpu_t_str},{gpu_t_str},{disk_t_str},{top_cpu},{top_ram}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blackbox_recorder_writes_line() {
        let mut recorder = BlackboxRecorder::new();
        recorder.record_sample(
            1000,
            25,
            60,
            10240,
            2048,
            Some(15),
            Some(45),
            Some(50),
            Some(38),
            Some("explorer.exe"),
            Some("chrome.exe"),
        );
        assert!(recorder.file_path.exists());
    }
}
