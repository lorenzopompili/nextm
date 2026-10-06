//! Impostazioni di nextm: valori, default e conversione da/verso chiave-valore.

/// Versione dello schema delle impostazioni salvate.
pub const SCHEMA_VERSION: u32 = 3;

/// Intervalli di aggiornamento ammessi, in millisecondi.
pub const INTERVALS_MS: [u32; 3] = [1_000, 2_000, 5_000];

/// Quante volte al massimo si ripropone la notifica del primo avvio se non viene cliccata.
pub const FIRST_RUN_MAX_SHOWN: u32 = 2;

const POWER_PAUSE_DISPLAY: u32 = 1 << 0;
const POWER_SLOW_SAVER: u32 = 1 << 1;
const POWER_ECOQOS: u32 = 1 << 2;

pub const METRIC_CPU: u32 = 1 << 0;
pub const METRIC_RAM: u32 = 1 << 1;
pub const METRIC_NET: u32 = 1 << 2;
pub const METRIC_TEMP_ACPI: u32 = 1 << 3;
pub const METRIC_TEMP_GPU: u32 = 1 << 4;
pub const METRIC_TEMP_DISK: u32 = 1 << 5;
pub const METRIC_GPU: u32 = 1 << 6;

pub const ICON_CPU: u32 = 1 << 0;
pub const ICON_RAM: u32 = 1 << 1;
pub const ICON_NET: u32 = 1 << 2;
pub const ICON_TEMP_ACPI: u32 = 1 << 3;
pub const ICON_TEMP_GPU: u32 = 1 << 4;
pub const ICON_TEMP_DISK: u32 = 1 << 5;
pub const ICON_GPU: u32 = 1 << 6;

pub const ICON_SYMBOL_CPU: u32 = 1 << 0;
pub const ICON_SYMBOL_RAM: u32 = 1 << 1;
pub const ICON_SYMBOL_TEMP_ACPI: u32 = 1 << 2;
pub const ICON_SYMBOL_TEMP_GPU: u32 = 1 << 3;
pub const ICON_SYMBOL_TEMP_DISK: u32 = 1 << 4;
pub const ICON_SYMBOL_GPU: u32 = 1 << 5;

/// Modalità di calcolo della CPU (tempo CPU o Utilità).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuMode {
    Standard,
    Utility,
}

/// Modalità di scansione del core saturo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaturationMode {
    Always,
    OnDemand,
    Off,
}

/// Modalità di conteggio della rete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetMode {
    Sum,
    Auto,
    Specific,
}

/// Modalità di selezione della lingua.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Auto,
    Italian,
    English,
}

/// Le impostazioni dell'applicazione.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Periodo base del campionamento (1, 2 o 5 s).
    pub interval_ms: u32,
    /// Lingua dell'interfaccia (Auto / OS, Italiano, English).
    pub language: Language,
    /// Nessun lavoro a schermo spento.
    pub pause_display: bool,
    /// Periodo almeno di 2 s con il risparmio energia di Windows attivo.
    pub slow_energy_saver: bool,
    /// EcoQoS: Windows fa girare nextm sui core efficienti.
    pub ecoqos: bool,
    /// L'utente ha cliccato la notifica del primo avvio.
    pub first_run_done: bool,
    /// Quante volte è già stata mostrata la notifica del primo avvio.
    pub first_run_shown: u32,
    /// Bitmask delle metriche attive.
    pub metrics: u32,
    /// Bitmask delle icone visibili.
    pub icons: u32,
    /// Bitmask per visualizzare l'icona con mini simbolo e numero invece di solo numero.
    pub icon_symbols: u32,
    /// Modalità CPU.
    pub cpu_mode: CpuMode,
    /// Modalità core saturo.
    pub saturation_mode: SaturationMode,
    /// Mostra l'impegno di ciascun core logico al passaggio del mouse sulla CPU.
    pub cpu_per_core_hover: bool,
    /// Mostra lo spazio occupato e rimanente per ciascuna unità disco montata al passaggio del mouse.
    pub disk_space_hover: bool,
    /// Modalità di selezione dell'interfaccia di rete.
    pub net_mode: NetMode,
    /// Identificatore LUID dell'interfaccia specifica (se `net_mode == Specific`).
    pub net_luid: u64,
    /// Rete in bit/s anziché byte/s.
    pub net_bits: bool,
    /// LUID della GPU selezionata (se ce n'è più di una).
    pub gpu_luid: u64,
    /// Mostra i processi con maggior consumo CPU all'hover sul riquadro.
    pub hover_top_cpu: bool,
    /// Mostra i processi con maggior consumo RAM all'hover sul riquadro.
    pub hover_top_ram: bool,
    /// Numero N di processi top da mostrare (retrocompatibilità).
    pub hover_top_n: u8,
    /// Numero N di processi top CPU (campo libero 1..=25).
    pub hover_top_n_cpu: u8,
    /// Numero N di processi top RAM (campo libero 1..=25).
    pub hover_top_n_ram: u8,
    /// Numero N di processi top Rete/IO (campo libero 1..=25).
    pub hover_top_n_net: u8,
    /// Mostra i mini grafici sparkline dell'andamento recente (30s) su hover.
    pub hover_sparklines: bool,
    /// Mostra i processi con maggior traffico di rete/I/O all'hover sul riquadro.
    pub hover_top_net: bool,
    /// Mostra la sezione GPU e VRAM all'hover sul riquadro.
    pub hover_gpu: bool,
    /// Avviso proattivo per carico CPU elevato.
    pub alert_cpu: bool,
    /// Soglia critica CPU % (campo libero 1..=100).
    pub alert_cpu_threshold: u8,
    /// Avviso proattivo per memoria RAM elevata.
    pub alert_ram: bool,
    /// Soglia critica RAM % (campo libero 1..=100).
    pub alert_ram_threshold: u8,
    /// Avviso proattivo per disco in esaurimento.
    pub alert_disk: bool,
    /// Soglia critica spazio occupato disco % (campo libero 1..=100).
    pub alert_disk_threshold: u8,
    /// Registratore blackbox (session flight recorder su CSV).
    pub blackbox_recording: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            interval_ms: 1_000,
            language: Language::Auto,
            pause_display: true,
            slow_energy_saver: true,
            ecoqos: true,
            first_run_done: false,
            first_run_shown: 0,
            metrics: METRIC_CPU | METRIC_RAM | METRIC_NET,
            icons: ICON_CPU,
            icon_symbols: ICON_SYMBOL_CPU
                | ICON_SYMBOL_RAM
                | ICON_SYMBOL_TEMP_ACPI
                | ICON_SYMBOL_TEMP_GPU
                | ICON_SYMBOL_TEMP_DISK
                | ICON_SYMBOL_GPU,
            cpu_mode: CpuMode::Standard,
            saturation_mode: SaturationMode::Always,
            cpu_per_core_hover: false,
            disk_space_hover: true,
            net_mode: NetMode::Sum,
            net_luid: 0,
            net_bits: false,
            gpu_luid: 0,
            hover_top_cpu: false,
            hover_top_ram: false,
            hover_top_n: 5,
            hover_top_n_cpu: 5,
            hover_top_n_ram: 5,
            hover_top_n_net: 5,
            hover_sparklines: true,
            hover_top_net: false,
            hover_gpu: true,
            alert_cpu: false,
            alert_cpu_threshold: 90,
            alert_ram: false,
            alert_ram_threshold: 90,
            alert_disk: false,
            alert_disk_threshold: 90,
            blackbox_recording: false,
        }
    }
}

impl Settings {
    /// Le coppie da salvare, sempre nello stesso ordine.
    pub fn to_pairs(self) -> [(&'static str, u32); 35] {
        let mut power = 0;
        if self.pause_display {
            power |= POWER_PAUSE_DISPLAY;
        }
        if self.slow_energy_saver {
            power |= POWER_SLOW_SAVER;
        }
        if self.ecoqos {
            power |= POWER_ECOQOS;
        }
        [
            ("SchemaVersion", SCHEMA_VERSION),
            ("IntervalMs", self.interval_ms),
            (
                "Language",
                match self.language {
                    Language::Auto => 0,
                    Language::Italian => 1,
                    Language::English => 2,
                },
            ),
            ("Power", power),
            ("FirstRunDone", u32::from(self.first_run_done)),
            ("FirstRunShown", self.first_run_shown),
            ("Metrics", self.metrics),
            ("Icons", self.icons),
            ("IconSymbols", self.icon_symbols),
            (
                "CpuMode",
                match self.cpu_mode {
                    CpuMode::Standard => 0,
                    CpuMode::Utility => 1,
                },
            ),
            (
                "SaturationMode",
                match self.saturation_mode {
                    SaturationMode::Always => 0,
                    SaturationMode::OnDemand => 1,
                    SaturationMode::Off => 2,
                },
            ),
            ("CpuPerCoreHover", u32::from(self.cpu_per_core_hover)),
            ("DiskSpaceHover", u32::from(self.disk_space_hover)),
            (
                "NetMode",
                match self.net_mode {
                    NetMode::Sum => 0,
                    NetMode::Auto => 1,
                    NetMode::Specific => 2,
                },
            ),
            ("NetLuidLo", (self.net_luid & 0xFFFF_FFFF) as u32),
            ("NetLuidHi", (self.net_luid >> 32) as u32),
            ("NetBits", u32::from(self.net_bits)),
            ("GpuLuidLo", (self.gpu_luid & 0xFFFF_FFFF) as u32),
            ("GpuLuidHi", (self.gpu_luid >> 32) as u32),
            ("HoverTopCpu", u32::from(self.hover_top_cpu)),
            ("HoverTopRam", u32::from(self.hover_top_ram)),
            ("HoverTopN", self.hover_top_n as u32),
            ("HoverSparklines", u32::from(self.hover_sparklines)),
            ("HoverTopNet", u32::from(self.hover_top_net)),
            ("HoverGpu", u32::from(self.hover_gpu)),
            ("HoverTopNCpu", self.hover_top_n_cpu as u32),
            ("HoverTopNRam", self.hover_top_n_ram as u32),
            ("HoverTopNNet", self.hover_top_n_net as u32),
            ("AlertCpu", u32::from(self.alert_cpu)),
            ("AlertCpuThreshold", self.alert_cpu_threshold as u32),
            ("AlertRam", u32::from(self.alert_ram)),
            ("AlertRamThreshold", self.alert_ram_threshold as u32),
            ("AlertDisk", u32::from(self.alert_disk)),
            ("AlertDiskThreshold", self.alert_disk_threshold as u32),
            ("BlackboxRecording", u32::from(self.blackbox_recording)),
        ]
    }

    /// Ricostruisce le impostazioni leggendo ogni chiave con `get`.
    /// Valori mancanti o non validi prendono il default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<u32>) -> Settings {
        let d = Settings::default();
        let interval_ms = get("IntervalMs").filter(|v| INTERVALS_MS.contains(v)).unwrap_or(d.interval_ms);
        let language = match get("Language").unwrap_or(0) {
            1 => Language::Italian,
            2 => Language::English,
            _ => Language::Auto,
        };
        let power = get("Power").filter(|&p| p <= 0b111);
        let metrics = get("Metrics").unwrap_or(d.metrics);
        let icons = get("Icons").unwrap_or(d.icons);
        let icon_symbols = get("IconSymbols").unwrap_or(d.icon_symbols);
        let cpu_mode = match get("CpuMode").unwrap_or(0) {
            1 => CpuMode::Utility,
            _ => CpuMode::Standard,
        };
        let saturation_mode = match get("SaturationMode").unwrap_or(0) {
            1 => SaturationMode::OnDemand,
            2 => SaturationMode::Off,
            _ => SaturationMode::Always,
        };
        let cpu_per_core_hover = get("CpuPerCoreHover").is_some_and(|v| v != 0);
        let disk_space_hover = get("DiskSpaceHover").map_or(d.disk_space_hover, |v| v != 0);
        let net_mode = match get("NetMode").unwrap_or(0) {
            1 => NetMode::Auto,
            2 => NetMode::Specific,
            _ => NetMode::Sum,
        };
        let lo = get("NetLuidLo").unwrap_or(0);
        let hi = get("NetLuidHi").unwrap_or(0);
        let net_luid = (u64::from(hi) << 32) | u64::from(lo);
        let net_bits = get("NetBits").is_some_and(|v| v != 0);
        let gpu_lo = get("GpuLuidLo").unwrap_or(0);
        let gpu_hi = get("GpuLuidHi").unwrap_or(0);
        let gpu_luid = (u64::from(gpu_hi) << 32) | u64::from(gpu_lo);
        let hover_top_cpu = get("HoverTopCpu").is_some_and(|v| v != 0);
        let hover_top_ram = get("HoverTopRam").is_some_and(|v| v != 0);
        let hover_top_n_cpu = get("HoverTopNCpu").unwrap_or(get("HoverTopN").unwrap_or(5)).clamp(1, 25) as u8;
        let hover_top_n_ram = get("HoverTopNRam").unwrap_or(get("HoverTopN").unwrap_or(5)).clamp(1, 25) as u8;
        let hover_top_n_net = get("HoverTopNNet").unwrap_or(get("HoverTopN").unwrap_or(5)).clamp(1, 25) as u8;
        let hover_top_n = hover_top_n_cpu;
        let hover_sparklines = get("HoverSparklines").map_or(d.hover_sparklines, |v| v != 0);
        let hover_top_net = get("HoverTopNet").is_some_and(|v| v != 0);
        let hover_gpu = get("HoverGpu").map_or(d.hover_gpu, |v| v != 0);
        let alert_cpu = get("AlertCpu").is_some_and(|v| v != 0);
        let alert_cpu_threshold = get("AlertCpuThreshold").unwrap_or(90).clamp(1, 100) as u8;
        let alert_ram = get("AlertRam").is_some_and(|v| v != 0);
        let alert_ram_threshold = get("AlertRamThreshold").unwrap_or(90).clamp(1, 100) as u8;
        let alert_disk = get("AlertDisk").is_some_and(|v| v != 0);
        let alert_disk_threshold = get("AlertDiskThreshold").unwrap_or(90).clamp(1, 100) as u8;
        let blackbox_recording = get("BlackboxRecording").is_some_and(|v| v != 0);

        Settings {
            interval_ms,
            language,
            pause_display: power.map_or(d.pause_display, |p| p & POWER_PAUSE_DISPLAY != 0),
            slow_energy_saver: power.map_or(d.slow_energy_saver, |p| p & POWER_SLOW_SAVER != 0),
            ecoqos: power.map_or(d.ecoqos, |p| p & POWER_ECOQOS != 0),
            first_run_done: get("FirstRunDone").is_some_and(|v| v != 0),
            first_run_shown: get("FirstRunShown").unwrap_or(0).min(FIRST_RUN_MAX_SHOWN),
            metrics,
            icons,
            icon_symbols,
            cpu_mode,
            saturation_mode,
            cpu_per_core_hover,
            disk_space_hover,
            net_mode,
            net_luid,
            net_bits,
            gpu_luid,
            hover_top_cpu,
            hover_top_ram,
            hover_top_n,
            hover_top_n_cpu,
            hover_top_n_ram,
            hover_top_n_net,
            hover_sparklines,
            hover_top_net,
            hover_gpu,
            alert_cpu,
            alert_cpu_threshold,
            alert_ram,
            alert_ram_threshold,
            alert_disk,
            alert_disk_threshold,
            blackbox_recording,
        }
    }

    pub fn is_metric_active(&self, metric: u32) -> bool {
        self.metrics & metric != 0
    }

    pub fn is_icon_active(&self, icon: u32) -> bool {
        self.icons & icon != 0
    }

    pub fn is_icon_symbol_active(&self, symbol_flag: u32) -> bool {
        self.icon_symbols & symbol_flag != 0
    }

    pub fn has_any_icon(&self) -> bool {
        self.icons & (ICON_CPU | ICON_RAM | ICON_NET | ICON_TEMP_ACPI | ICON_TEMP_GPU | ICON_TEMP_DISK | ICON_GPU) != 0
    }

    /// `true` se la notifica del primo avvio va mostrata a questo avvio.
    pub fn should_show_first_run(&self) -> bool {
        !self.first_run_done && self.first_run_shown < FIRST_RUN_MAX_SHOWN
    }

    /// `true` solo al primissimo avvio (per attivare l'avvio con Windows una volta sola).
    pub fn is_very_first_run(&self) -> bool {
        !self.first_run_done && self.first_run_shown == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(s: Settings) -> Settings {
        let pairs = s.to_pairs();
        Settings::from_lookup(|k| pairs.iter().find(|(name, _)| *name == k).map(|(_, v)| *v))
    }

    #[test]
    fn default_values() {
        let d = Settings::default();
        assert_eq!(d.interval_ms, 1_000);
        assert_eq!(d.language, Language::Auto);
        assert!(d.pause_display);
        assert!(d.slow_energy_saver);
        assert!(d.ecoqos);
        assert!(!d.first_run_done);
        assert_eq!(d.first_run_shown, 0);
        assert!(d.is_metric_active(METRIC_CPU));
        assert!(d.is_metric_active(METRIC_RAM));
        assert!(d.is_metric_active(METRIC_NET));
        assert!(!d.is_metric_active(METRIC_TEMP_ACPI));
        assert!(!d.is_metric_active(METRIC_TEMP_GPU));
        assert!(!d.is_metric_active(METRIC_TEMP_DISK));
        assert!(d.is_icon_active(ICON_CPU));
        assert!(!d.is_icon_active(ICON_RAM));
        assert!(!d.is_icon_active(ICON_NET));
        assert!(!d.is_icon_active(ICON_TEMP_ACPI));
        assert!(!d.is_icon_active(ICON_TEMP_GPU));
        assert!(!d.is_icon_active(ICON_TEMP_DISK));
        assert!(d.is_icon_symbol_active(ICON_SYMBOL_CPU));
        assert!(d.is_icon_symbol_active(ICON_SYMBOL_RAM));
        assert_eq!(d.cpu_mode, CpuMode::Standard);
        assert_eq!(d.saturation_mode, SaturationMode::Always);
        assert!(!d.cpu_per_core_hover);
        assert!(d.disk_space_hover);
        assert_eq!(d.net_mode, NetMode::Sum);
        assert_eq!(d.net_luid, 0);
        assert!(!d.net_bits);
        assert_eq!(d.gpu_luid, 0);
        assert!(!d.hover_top_cpu);
        assert!(!d.hover_top_ram);
        assert_eq!(d.hover_top_n, 5);
        assert_eq!(d.hover_top_n_cpu, 5);
        assert_eq!(d.hover_top_n_ram, 5);
        assert_eq!(d.hover_top_n_net, 5);
        assert!(d.hover_sparklines);
        assert!(!d.hover_top_net);
        assert!(d.hover_gpu);
        assert!(!d.alert_cpu);
        assert_eq!(d.alert_cpu_threshold, 90);
        assert!(!d.alert_ram);
        assert_eq!(d.alert_ram_threshold, 90);
        assert!(!d.alert_disk);
        assert_eq!(d.alert_disk_threshold, 90);
        assert!(!d.blackbox_recording);
    }

    #[test]
    fn roundtrip_default_and_custom() {
        assert_eq!(roundtrip(Settings::default()), Settings::default());
        let custom = Settings {
            interval_ms: 5_000,
            language: Language::English,
            pause_display: false,
            slow_energy_saver: false,
            ecoqos: false,
            first_run_done: true,
            first_run_shown: 2,
            metrics: METRIC_CPU | METRIC_NET | METRIC_TEMP_ACPI | METRIC_TEMP_GPU | METRIC_GPU,
            icons: ICON_CPU | ICON_RAM | ICON_NET | ICON_TEMP_GPU | ICON_GPU,
            icon_symbols: ICON_SYMBOL_CPU | ICON_SYMBOL_TEMP_GPU | ICON_SYMBOL_GPU,
            cpu_mode: CpuMode::Utility,
            saturation_mode: SaturationMode::OnDemand,
            cpu_per_core_hover: true,
            disk_space_hover: false,
            net_mode: NetMode::Specific,
            net_luid: 0x1234_5678_9ABC_DEF0,
            net_bits: true,
            gpu_luid: 0xCAFE_BABE_DEAD_BEEF,
            hover_top_cpu: true,
            hover_top_ram: true,
            hover_top_n: 7,
            hover_top_n_cpu: 7,
            hover_top_n_ram: 12,
            hover_top_n_net: 4,
            hover_sparklines: false,
            hover_top_net: true,
            hover_gpu: false,
            alert_cpu: true,
            alert_cpu_threshold: 85,
            alert_ram: true,
            alert_ram_threshold: 92,
            alert_disk: true,
            alert_disk_threshold: 95,
            blackbox_recording: true,
        };
        assert_eq!(roundtrip(custom), custom);
    }

    #[test]
    fn bad_interval_takes_default() {
        let s = Settings::from_lookup(|k| match k {
            "IntervalMs" => Some(3_000),
            _ => None,
        });
        assert_eq!(s.interval_ms, 1_000);
    }
}
