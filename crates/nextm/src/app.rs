//! Il cuore di nextm: stato dell'applicazione e gestione dei messaggi.
//!
//! Tutto gira sul thread principale. Lo stato vive in un `RefCell` locale al thread: la
//! procedura della finestra lo prende in prestito solo per il tempo di un evento. Menu e
//! finestre di messaggio (che hanno un loro ciclo di messaggi) si mostrano FUORI dal
//! prestito, così un `WM_TIMER` che arriva mentre sono aperti non trova lo stato bloccato.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use nextm_metrics::cpu::{CpuSample, busy_bp};
use nextm_metrics::level::{Level, LevelTracker};
use nextm_metrics::net::{
    IfInfo, NetMode as MetricsNetMode, RateMeter, Rates, Selection, Units, compact_rate, format_rate, menu_candidates,
    select,
};
use nextm_metrics::ram::gib_x10;
use nextm_metrics::saturation::{SatReport, SatState, SaturationDetector, ThreadRead, friendly_name};
use nextm_metrics::smooth::{Mean3, Steady, bp_to_percent};
use nextm_metrics::sys::cpu::CpuSampler;
use nextm_metrics::sys::net::NetSampler;
use nextm_metrics::sys::ram::read_ram;
use nextm_metrics::sys::temp_acpi::AcpiSampler;
use nextm_metrics::sys::temp_disk::DiskSampler;
use nextm_metrics::sys::temp_gpu::GpuSampler;
use nextm_metrics::sys::threads::ThreadScanner;
use nextm_metrics::sys::utility::{UtilitySample, UtilitySampler, utility_bp};
use nextm_metrics::temp::{GpuAdapter, temp_icon_text};
use nextm_render::icon::{
    StatusBar, draw_cpu_icon as render_cpu_icon, draw_dual_icon, draw_ram_icon, draw_static_icon, draw_temp_acpi_icon,
    draw_temp_disk_icon, draw_temp_gpu_icon, draw_value_icon, draw_value_icon_with_bar,
};
use nextm_render::{Canvas, Color, Palette, Theme};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Shell::{NIN_BALLOONUSERCLICK, NIN_SELECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, KillTimer, PBT_APMRESUMEAUTOMATIC, PBT_POWERSETTINGCHANGE,
    PostQuitMessage, SPI_SETHIGHCONTRAST, SetCoalescableTimer, WM_CONTEXTMENU, WM_DESTROY, WM_DEVICECHANGE,
    WM_DISPLAYCHANGE, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MOUSEMOVE, WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_TIMER,
};

use crate::settings::{
    CpuMode, ICON_CPU, ICON_NET, ICON_RAM, ICON_SYMBOL_CPU, ICON_SYMBOL_RAM, ICON_SYMBOL_TEMP_ACPI,
    ICON_SYMBOL_TEMP_DISK, ICON_SYMBOL_TEMP_GPU, ICON_TEMP_ACPI, ICON_TEMP_DISK, ICON_TEMP_GPU, METRIC_CPU, METRIC_NET,
    METRIC_RAM, METRIC_TEMP_ACPI, METRIC_TEMP_DISK, METRIC_TEMP_GPU, NetMode, SaturationMode, Settings,
};
use crate::strings::{self, Strings};
use crate::sys::autostart;
use crate::sys::darkmenu;
use crate::sys::hover::{HoverSnapshot, HoverWindow};
use crate::sys::icon::Icon;
use crate::sys::inspect_win::InspectWindow;
use crate::sys::menu::PopupMenu;
use crate::sys::power::{
    self, GUID_ENERGY_SAVER_STATUS, GUID_POWER_SAVING_STATUS, GUID_SESSION_DISPLAY_STATUS, PowerNotify,
};
use crate::sys::shell;
use crate::sys::single::{ACTIVATE_MESSAGE, EXIT_MESSAGE};
use crate::sys::store::Store;
use crate::sys::theme;
use crate::sys::tray::{self, NIN_KEYSELECT, TrayIcon};
use crate::sys::window::{self, WM_TRAY};
use crate::tooltip::{TipBuf, TooltipData, build_tooltip, icon_text};

const ID_CPU: u32 = 1;
const ID_RAM: u32 = 2;
const ID_NET: u32 = 3;
const ID_TEMP_ACPI: u32 = 4;
const ID_TEMP_GPU: u32 = 5;
const ID_TEMP_DISK: u32 = 6;
const ID_STATIC: u32 = 1;

const TIMER_TICK: usize = 1;
const TIMER_THEME: usize = 2;
const TIMER_NET_REFRESH: usize = 3;
const TIMER_HOVER_TRACK: usize = 4;

const HOVER_TRACK_MS: u32 = 120;

/// Primo campione poco dopo l'avvio o la ripresa, per non mostrare "—" a lungo.
const FIRST_TICK_MS: u32 = 300;
/// Per quanto tempo dopo l'ultimo movimento del mouse si aggiorna il tooltip.
const HOVER_MS: u64 = 5_000;
/// Attesa prima di rileggere il tema (Windows invia raffiche di WM_SETTINGCHANGE).
const THEME_DEBOUNCE_MS: u32 = 300;
/// Refresh dell'elenco schede di rete ogni 60 s.
const NET_REFRESH_MS: u32 = 60_000;

pub const CMD_INTERVAL_1S: u32 = 101;
pub const CMD_INTERVAL_2S: u32 = 102;
pub const CMD_INTERVAL_5S: u32 = 103;
pub const CMD_PAUSE_DISPLAY: u32 = 110;
pub const CMD_SLOW_SAVER: u32 = 111;
pub const CMD_ECOQOS: u32 = 112;
pub const CMD_AUTOSTART: u32 = 120;
pub const CMD_SHOW_ICON: u32 = 121;
pub const CMD_RESTART_ADMIN: u32 = 122;
pub const CMD_AUTOSTART_ADMIN: u32 = 123;
pub const CMD_ABOUT: u32 = 130;
pub const CMD_EXIT: u32 = 131;
pub const CMD_CHECK_UPDATES: u32 = 132;
pub const CMD_LANG_AUTO: u32 = 140;
pub const CMD_LANG_IT: u32 = 141;
pub const CMD_LANG_EN: u32 = 142;

pub const CMD_CPU_ACTIVE: u32 = 201;
pub const CMD_CPU_ICON: u32 = 202;
pub const CMD_CPU_UTILITY: u32 = 203;
pub const CMD_CPU_PER_CORE: u32 = 204;
pub const CMD_CPU_ICON_SYMBOL: u32 = 205;
pub const CMD_ICON_SYMBOLS_OFF: u32 = 206;
pub const CMD_ICON_SYMBOLS_ON: u32 = 207;
pub const CMD_DISK_SPACE_HOVER: u32 = 208;
pub const CMD_SAT_ALWAYS: u32 = 210;
pub const CMD_SAT_ONDEMAND: u32 = 211;
pub const CMD_SAT_OFF: u32 = 212;

pub const CMD_RAM_ACTIVE: u32 = 220;
pub const CMD_RAM_ICON: u32 = 221;
pub const CMD_RAM_ICON_SYMBOL: u32 = 222;

pub const CMD_NET_ACTIVE: u32 = 230;
pub const CMD_NET_ICON: u32 = 231;
pub const CMD_NET_BITS: u32 = 232;
pub const CMD_NET_SUM: u32 = 240;
pub const CMD_NET_AUTO: u32 = 241;
pub const CMD_NET_SPECIFIC_BASE: u32 = 250;

pub const CMD_TEMP_ACPI_ACTIVE: u32 = 260;
pub const CMD_TEMP_ACPI_ICON: u32 = 261;
pub const CMD_TEMP_ACPI_ICON_SYMBOL: u32 = 262;
pub const CMD_TEMP_GPU_ACTIVE: u32 = 270;
pub const CMD_TEMP_GPU_ICON: u32 = 271;
pub const CMD_TEMP_GPU_ICON_SYMBOL: u32 = 272;
pub const CMD_TEMP_GPU_SELECT_BASE: u32 = 280;
pub const CMD_TEMP_DISK_ACTIVE: u32 = 290;
pub const CMD_TEMP_DISK_ICON: u32 = 291;
pub const CMD_TEMP_DISK_ICON_SYMBOL: u32 = 292;

static MSG_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(u32::MAX);
static MSG_ACTIVATE: AtomicU32 = AtomicU32::new(u32::MAX);
static MSG_EXIT: AtomicU32 = AtomicU32::new(u32::MAX);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Esegue `f` sullo stato, se disponibile e non già in prestito (evento rientrante).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

/// Cosa fare dopo un comando del menu, fuori dal prestito dello stato.
enum After {
    Nothing,
    About,
    Exit,
}

/// Quale notifica è stata mostrata per ultima (per sapere cosa fare al clic).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Balloon {
    FirstRun,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawKeyCpu {
    percent: Option<u8>,
    level: Level,
    bar: Option<StatusBar>,
    size: u32,
    palette: Palette,
    symbol: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawKeyRam {
    percent: Option<u8>,
    level: Level,
    size: u32,
    palette: Palette,
    symbol: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawKeyNet {
    down: [u8; 4],
    up: [u8; 4],
    size: u32,
    palette: Palette,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawKeyTemp {
    temp_c: Option<i16>,
    size: u32,
    palette: Palette,
    symbol: bool,
}

pub struct App {
    hwnd: HWND,
    s: &'static Strings,
    store: Store,
    settings: Settings,
    exe: String,

    tray_cpu: Option<TrayIcon>,
    tray_ram: Option<TrayIcon>,
    tray_net: Option<TrayIcon>,
    tray_temp_acpi: Option<TrayIcon>,
    tray_temp_gpu: Option<TrayIcon>,
    tray_temp_disk: Option<TrayIcon>,
    tray_static: Option<TrayIcon>,

    canvas: Canvas,
    taskbar: HWND,
    dpi: u32,
    theme: Theme,
    palette: Palette,

    // CPU
    cpu: CpuSampler,
    prev_cpu: Option<CpuSample>,
    mean_cpu: Mean3,
    steady_cpu: Steady,
    level_cpu: LevelTracker,
    percent_cpu: Option<u8>,
    drawn_cpu: Option<DrawKeyCpu>,
    cpu_per_core: Option<Vec<u8>>,

    // Utilità
    utility_sampler: Option<UtilitySampler>,
    prev_utility: Option<UtilitySample>,
    percent_utility: Option<u8>,

    // Core saturo
    sat_detector: SaturationDetector,
    sat_scanner: Option<ThreadScanner>,
    sat_report: SatReport,
    sat_threads_buf: Vec<ThreadRead>,
    sat_top_proc: Option<(String, u32)>,

    // RAM
    mean_ram: Mean3,
    steady_ram: Steady,
    level_ram: LevelTracker,
    percent_ram: Option<u8>,
    ram_used_x10: Option<u32>,
    ram_total_x10: Option<u32>,
    drawn_ram: Option<DrawKeyRam>,

    // Rete
    net_sampler: Option<NetSampler>,
    net_meter: RateMeter,
    net_interfaces: Vec<IfInfo>,
    net_selection: Selection,
    net_rates: Option<Rates>,
    net_counters_buf: Vec<(u64, u64, u64)>,
    net_candidates: Vec<(u64, String)>,
    net_needs_refresh: bool,
    drawn_net: Option<DrawKeyNet>,

    // Temperature (M3)
    acpi_sampler: Option<AcpiSampler>,
    temp_acpi_c: Option<i16>,
    temp_acpi_fixed: bool,
    temp_acpi_label: Option<String>,
    drawn_temp_acpi: Option<DrawKeyTemp>,

    gpu_sampler: Option<GpuSampler>,
    temp_gpu_c: Option<i16>,
    gpu_adapters: Vec<GpuAdapter>,
    last_gpu_tick: u64,
    drawn_temp_gpu: Option<DrawKeyTemp>,

    disk_sampler: Option<DiskSampler>,
    temp_disk_c: Option<i16>,
    temp_disk_label: Option<String>,
    last_disk_tick: u64,
    drawn_temp_disk: Option<DrawKeyTemp>,

    hover_win: Option<HoverWindow>,
    inspect_win: Option<InspectWindow>,
    last_inspect_toggle: u64,
    tip_sent: TipBuf,
    hover_until: u64,
    timer_ms: u32,
    first_tick: bool,
    display_on: bool,
    energy_saver: bool,
    display_notify: Option<PowerNotify>,
    saver_notify: Option<PowerNotify>,
    balloon: Option<Balloon>,
    portable_readonly: bool,
}

/// Avvia nextm sul thread corrente; restituisce il codice di uscita.
pub fn run() -> i32 {
    run_internal(false)
}

/// Avvia nextm sul thread corrente aprendo immediatamente la finestra di ispezione.
pub fn run_with_inspect(show_inspect: bool) -> i32 {
    run_internal(show_inspect)
}

fn run_internal(show_inspect: bool) -> i32 {
    let msg_taskbar = window::register_message(wide!("TaskbarCreated"));
    let msg_activate = window::register_message(ACTIVATE_MESSAGE);
    let msg_exit = window::register_message(EXIT_MESSAGE);
    MSG_TASKBAR_CREATED.store(msg_taskbar, Ordering::Relaxed);
    MSG_ACTIVATE.store(msg_activate, Ordering::Relaxed);
    MSG_EXIT.store(msg_exit, Ordering::Relaxed);
    let Some(hwnd) = window::create_hidden(Some(wndproc)) else { return 1 };
    window::allow_message_through_uipi(hwnd, msg_activate);
    window::allow_message_through_uipi(hwnd, msg_exit);
    window::allow_message_through_uipi(hwnd, msg_taskbar);
    let app = App::new(hwnd);
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(App::start);
    if show_inspect {
        with_app(App::show_inspect);
    }
    window::run_message_loop()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            on_tray(hwnd, tray::decode(wparam, lparam));
            0
        }
        WM_TIMER => {
            with_app(|a| a.on_timer(wparam));
            0
        }
        WM_SETTINGCHANGE => {
            let area = unsafe { setting_area(lparam) };
            with_app(|a| a.on_setting_change(wparam as u32, area));
            0
        }
        WM_DISPLAYCHANGE => {
            with_app(App::on_display_change);
            0
        }
        WM_DEVICECHANGE => {
            with_app(App::on_device_change);
            0
        }
        WM_POWERBROADCAST => {
            if wparam as u32 == PBT_POWERSETTINGCHANGE {
                if let Some((guid, data)) = unsafe { power::setting_from_lparam(lparam) } {
                    with_app(|a| a.on_power_setting(&guid, data));
                }
            } else if wparam as u32 == PBT_APMRESUMEAUTOMATIC {
                with_app(App::on_resume);
            }
            1
        }
        WM_DESTROY => {
            APP.with(|cell| {
                if let Ok(mut guard) = cell.try_borrow_mut() {
                    guard.take();
                }
            });
            unsafe { PostQuitMessage(0) };
            0
        }
        m if m == MSG_TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_app(App::on_taskbar_created);
            0
        }
        m if m == MSG_ACTIVATE.load(Ordering::Relaxed) => {
            with_app(App::on_activate);
            0
        }
        m if m == MSG_EXIT.load(Ordering::Relaxed) => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Riconosce le aree di `WM_SETTINGCHANGE` che interessano il tema.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingArea {
    ImmersiveColorSet,
    Other,
}

unsafe fn setting_area(lparam: LPARAM) -> SettingArea {
    let p = lparam as *const u16;
    if p.is_null() {
        return SettingArea::Other;
    }
    let target = wide!("ImmersiveColorSet");
    for (i, &expected) in target.iter().enumerate() {
        let c = unsafe { *p.add(i) };
        if c != expected {
            return SettingArea::Other;
        }
        if c == 0 {
            break;
        }
    }
    SettingArea::ImmersiveColorSet
}

fn on_tray(hwnd: HWND, ev: tray::TrayEvent) {
    match ev.event {
        WM_CONTEXTMENU => {
            with_app(App::hide_hover);
            show_tray_menu(hwnd, ev.x, ev.y);
        }
        WM_LBUTTONDBLCLK => {
            with_app(App::hide_hover);
            with_app(App::show_inspect);
        }
        WM_MOUSEMOVE => {
            with_app(|a| a.on_hover(ev.id, ev.x, ev.y));
        }
        NIN_BALLOONUSERCLICK => {
            with_app(App::on_balloon_click);
        }
        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP => {
            with_app(App::hide_hover);
            with_app(App::toggle_inspect);
        }
        _ => {}
    }
}

fn show_tray_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(menu) = PopupMenu::new() else { return };
    let Some(s) = with_app(|a| a.s) else { return };
    let dark = with_app(|a| a.theme != Theme::Light).unwrap_or(true);
    menu.item(CMD_CHECK_UPDATES, s.menu_check_updates, false);
    menu.separator();
    menu.item(CMD_EXIT, s.menu_exit, false);
    darkmenu::set_menu_theme(dark);
    let cmd = menu.track(hwnd, x, y);
    if cmd == CMD_EXIT {
        unsafe { DestroyWindow(hwnd) };
    } else if cmd == CMD_CHECK_UPDATES {
        execute_command(CMD_CHECK_UPDATES);
    }
}

/// Ottiene uno snapshot delle impostazioni correnti, dello stato autostart utente e admin.
pub fn get_settings_snapshot() -> Option<(Settings, bool, bool)> {
    with_app(|a| (a.settings, autostart::is_enabled(&a.exe), crate::sys::elevation::is_task_scheduler_enabled()))
}

/// Esegue un comando di impostazione inviato dalla pagina Impostazioni o da tastiera.
pub fn execute_command(cmd: u32) {
    let after = with_app(|a| a.on_command(cmd));
    match after {
        Some(After::About) => {
            with_app(|a| a.show_inspect_tab(crate::sys::inspect_win::InspectTab::Info));
        }
        Some(After::Exit) => {
            if let Some(hwnd) = with_app(|a| a.hwnd) {
                unsafe { DestroyWindow(hwnd) };
            }
        }
        _ => {}
    }
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

impl App {
    fn new(hwnd: HWND) -> App {
        let detected = Store::detect();
        let settings = detected.store.load();
        let taskbar = shell::taskbar();
        let dpi = shell::dpi_of(taskbar);
        let size = shell::small_icon_size(dpi);
        let theme = theme::taskbar_theme();
        let logical = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        }
        .max(1);
        let s = match settings.language {
            crate::settings::Language::Italian => &strings::IT,
            crate::settings::Language::English => &strings::EN,
            crate::settings::Language::Auto => strings::for_langid(crate::sys::ui_langid()),
        };

        App {
            hwnd,
            s,
            store: detected.store,
            settings,
            exe: std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            tray_cpu: None,
            tray_ram: None,
            tray_net: None,
            tray_temp_acpi: None,
            tray_temp_gpu: None,
            tray_temp_disk: None,
            tray_static: None,
            canvas: Canvas::new(size, size),
            taskbar,
            dpi,
            theme,
            palette: Palette::for_theme(theme),
            cpu: CpuSampler::new(),
            prev_cpu: None,
            mean_cpu: Mean3::new(),
            steady_cpu: Steady::new(),
            level_cpu: LevelTracker::new(),
            percent_cpu: None,
            drawn_cpu: None,
            cpu_per_core: None,
            utility_sampler: None,
            prev_utility: None,
            percent_utility: None,
            sat_detector: SaturationDetector::new(logical),
            sat_scanner: None,
            sat_report: SatReport { state: SatState::Normal, top_thread: None, top_process: None },
            sat_threads_buf: Vec::new(),
            sat_top_proc: None,
            mean_ram: Mean3::new(),
            steady_ram: Steady::new(),
            level_ram: LevelTracker::new(),
            percent_ram: None,
            ram_used_x10: None,
            ram_total_x10: None,
            drawn_ram: None,
            net_sampler: None,
            net_meter: RateMeter::new(),
            net_interfaces: Vec::new(),
            net_selection: Selection { luids: Vec::new(), label: String::new(), fallback: false },
            net_rates: None,
            net_counters_buf: Vec::new(),
            net_candidates: Vec::new(),
            net_needs_refresh: true,
            drawn_net: None,
            acpi_sampler: None,
            temp_acpi_c: None,
            temp_acpi_fixed: false,
            temp_acpi_label: None,
            drawn_temp_acpi: None,
            gpu_sampler: None,
            temp_gpu_c: None,
            gpu_adapters: Vec::new(),
            last_gpu_tick: 0,
            drawn_temp_gpu: None,
            disk_sampler: None,
            temp_disk_c: None,
            temp_disk_label: None,
            last_disk_tick: 0,
            drawn_temp_disk: None,
            hover_win: HoverWindow::new(hwnd),
            inspect_win: InspectWindow::new(s),
            last_inspect_toggle: 0,
            tip_sent: TipBuf::new(),
            hover_until: 0,
            timer_ms: 0,
            first_tick: true,
            display_on: true,
            energy_saver: false,
            display_notify: None,
            saver_notify: None,
            balloon: None,
            portable_readonly: detected.portable_readonly,
        }
    }

    fn start(&mut self) {
        if self.settings.is_very_first_run() && !self.store.is_portable() && !self.exe.is_empty() {
            autostart::enable(&self.exe);
        }
        if self.settings.ecoqos {
            power::set_ecoqos(true);
        }
        self.update_power_registrations();
        self.sync_tray_icons();
        self.retime();

        unsafe {
            SetCoalescableTimer(self.hwnd, TIMER_NET_REFRESH, NET_REFRESH_MS, None, 5_000);
        }

        if self.portable_readonly {
            self.show_balloon(self.s.balloon_portable_readonly, Balloon::Other);
        } else if self.settings.should_show_first_run() {
            self.settings.first_run_shown = self.settings.first_run_shown.saturating_add(1);
            self.save();
            self.show_balloon(self.s.balloon_first_run, Balloon::FirstRun);
        }
    }

    fn on_device_change(&mut self) {
        self.net_needs_refresh = true;
    }

    fn show_balloon(&mut self, text: &[u16], kind: Balloon) {
        let title = self.s.balloon_title;
        let sent = self
            .tray_cpu
            .as_ref()
            .or(self.tray_static.as_ref())
            .or(self.tray_ram.as_ref())
            .or(self.tray_net.as_ref())
            .or(self.tray_temp_acpi.as_ref())
            .or(self.tray_temp_gpu.as_ref())
            .or(self.tray_temp_disk.as_ref())
            .is_some_and(|t| t.balloon(title, text));
        if sent {
            self.balloon = Some(kind);
        }
    }

    fn explorer_tip(&self) -> &[u16] {
        if self.hover_win.is_some() { &[] } else { self.tip_sent.as_wide() }
    }

    fn sync_tray_icons(&mut self) {
        if !self.settings.has_any_icon() {
            self.tray_cpu = None;
            self.tray_ram = None;
            self.tray_net = None;
            self.tray_temp_acpi = None;
            self.tray_temp_gpu = None;
            self.tray_temp_disk = None;
            self.drawn_cpu = None;
            self.drawn_ram = None;
            self.drawn_net = None;
            self.drawn_temp_acpi = None;
            self.drawn_temp_gpu = None;
            self.drawn_temp_disk = None;

            if self.tray_static.is_none() {
                draw_static_icon(&mut self.canvas, self.palette.normal);
                let icon = Icon::from_canvas(&self.canvas);
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_static = TrayIcon::add(self.hwnd, ID_STATIC, &icon, tip);
                }
            }
            return;
        }

        self.tray_static = None;

        if self.settings.is_icon_active(ICON_CPU) {
            if self.tray_cpu.is_none() {
                let level = self.level_cpu.update(self.percent_cpu.unwrap_or(0));
                let icon = self.draw_cpu_icon(level);
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_cpu = TrayIcon::add(self.hwnd, ID_CPU, &icon, tip);
                }
            }
        } else {
            self.tray_cpu = None;
            self.drawn_cpu = None;
        }

        if self.settings.is_icon_active(ICON_RAM) {
            if self.tray_ram.is_none() {
                let level = self.level_ram.update(self.percent_ram.unwrap_or(0));
                let icon = self.draw_ram_icon(level);
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_ram = TrayIcon::add(self.hwnd, ID_RAM, &icon, tip);
                }
            }
        } else {
            self.tray_ram = None;
            self.drawn_ram = None;
        }

        if self.settings.is_icon_active(ICON_NET) {
            if self.tray_net.is_none() {
                let icon = self.draw_net_icon();
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_net = TrayIcon::add(self.hwnd, ID_NET, &icon, tip);
                }
            }
        } else {
            self.tray_net = None;
            self.drawn_net = None;
        }

        if self.settings.is_icon_active(ICON_TEMP_ACPI) {
            if self.tray_temp_acpi.is_none() {
                let icon = self.draw_temp_acpi_icon();
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_temp_acpi = TrayIcon::add(self.hwnd, ID_TEMP_ACPI, &icon, tip);
                }
            }
        } else {
            self.tray_temp_acpi = None;
            self.drawn_temp_acpi = None;
        }

        if self.settings.is_icon_active(ICON_TEMP_GPU) {
            if self.tray_temp_gpu.is_none() {
                let icon = self.draw_temp_gpu_icon();
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_temp_gpu = TrayIcon::add(self.hwnd, ID_TEMP_GPU, &icon, tip);
                }
            }
        } else {
            self.tray_temp_gpu = None;
            self.drawn_temp_gpu = None;
        }

        if self.settings.is_icon_active(ICON_TEMP_DISK) {
            if self.tray_temp_disk.is_none() {
                let icon = self.draw_temp_disk_icon();
                if let Some(icon) = icon {
                    let tip = self.explorer_tip();
                    self.tray_temp_disk = TrayIcon::add(self.hwnd, ID_TEMP_DISK, &icon, tip);
                }
            }
        } else {
            self.tray_temp_disk = None;
            self.drawn_temp_disk = None;
        }
    }

    fn arm_timer(&mut self, ms: u32) {
        if self.timer_ms == ms {
            return;
        }
        self.timer_ms = ms;
        let tolerance = ms / 10;
        unsafe { SetCoalescableTimer(self.hwnd, TIMER_TICK, ms, None, tolerance) };
    }

    fn kill_timer(&mut self) {
        if self.timer_ms == 0 {
            return;
        }
        self.timer_ms = 0;
        unsafe { KillTimer(self.hwnd, TIMER_TICK) };
    }

    fn retime(&mut self) {
        if !self.display_on && self.settings.pause_display {
            self.kill_timer();
            return;
        }
        if !self.settings.is_metric_active(METRIC_CPU)
            && !self.settings.is_metric_active(METRIC_RAM)
            && !self.settings.is_metric_active(METRIC_NET)
            && !self.settings.is_metric_active(METRIC_TEMP_ACPI)
            && !self.settings.is_metric_active(METRIC_TEMP_GPU)
            && !self.settings.is_metric_active(METRIC_TEMP_DISK)
            && !self.settings.has_any_icon()
        {
            self.kill_timer();
            return;
        }
        if self.first_tick {
            self.arm_timer(FIRST_TICK_MS);
            return;
        }
        let now = now_ms();
        let hovering = now < self.hover_until;
        let base = if hovering {
            1_000
        } else if self.energy_saver && self.settings.slow_energy_saver {
            self.settings.interval_ms.max(2_000)
        } else {
            self.settings.interval_ms
        };
        self.arm_timer(base);
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_TICK => {
                if self.first_tick {
                    self.first_tick = false;
                    self.retime();
                }
                self.tick();
            }
            TIMER_THEME => {
                unsafe { KillTimer(self.hwnd, TIMER_THEME) };
                self.apply_theme();
            }
            TIMER_NET_REFRESH => {
                self.net_needs_refresh = true;
            }
            TIMER_HOVER_TRACK => {
                let mut pt: POINT = unsafe { core::mem::zeroed() };
                unsafe { GetCursorPos(&mut pt) };
                let mut should_hide = false;
                if let Some(hover) = &self.hover_win {
                    if hover.anchor_distance(pt) > 50 && !hover.contains_point(pt, 16) {
                        should_hide = true;
                    }
                } else {
                    should_hide = true;
                }
                if should_hide {
                    self.hide_hover();
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self) {
        let now = now_ms();
        let hovering = now < self.hover_until;

        // 1. CPU
        if self.settings.is_metric_active(METRIC_CPU) {
            let cur = self.cpu.read();
            if let (Some(prev), Some(cur)) = (self.prev_cpu, cur)
                && let Some(bp) = busy_bp(prev, cur, self.cpu.logical_count())
            {
                self.mean_cpu.push(bp);
                if let Some(m) = self.mean_cpu.mean() {
                    self.percent_cpu = Some(self.steady_cpu.update(bp_to_percent(m)));
                }
            }
            self.prev_cpu = cur;

            // Utilità opzionale
            if self.settings.cpu_mode == CpuMode::Utility {
                if self.utility_sampler.is_none() {
                    self.utility_sampler = UtilitySampler::new();
                }
                if let Some(sampler) = self.utility_sampler.as_mut() {
                    let u_cur = sampler.read();
                    if let (Some(u_prev), Some(u_cur)) = (self.prev_utility, u_cur)
                        && let Some(bp) = utility_bp(u_prev, u_cur)
                    {
                        self.percent_utility = Some(bp_to_percent(bp.min(10_000) as u16));
                    }
                    self.prev_utility = u_cur;
                }
            }

            // Core saturo
            let gate = self.sat_detector.gate_percent();
            let should_scan = match self.settings.saturation_mode {
                SaturationMode::Always => self.percent_cpu.unwrap_or(0) >= gate || hovering,
                SaturationMode::OnDemand => hovering,
                SaturationMode::Off => false,
            };

            if should_scan {
                if self.sat_scanner.is_none() {
                    self.sat_scanner = Some(ThreadScanner::new());
                }
                if let Some(scanner) = self.sat_scanner.as_mut()
                    && scanner.scan(&mut self.sat_threads_buf)
                {
                    let full = self.level_cpu.update(self.percent_cpu.unwrap_or(0)) == Level::Full;
                    let wall_100ns = now.saturating_mul(10_000);
                    self.sat_report = self.sat_detector.update(wall_100ns, &self.sat_threads_buf, full);

                    if let Some((pid, core100)) = self.sat_report.top_process {
                        let mut name = String::new();
                        if scanner.process_name(pid, &mut name) {
                            let friendly = friendly_name(&name).to_string();
                            self.sat_top_proc = Some((friendly, core100));
                        } else {
                            self.sat_top_proc = Some((format!("PID {pid}"), core100));
                        }
                    } else {
                        self.sat_top_proc = None;
                    }
                }
            } else {
                if let Some(scanner) = self.sat_scanner.as_mut() {
                    scanner.release();
                }
                self.sat_report.state = SatState::Normal;
                self.sat_top_proc = None;
            }

            if self.settings.cpu_per_core_hover {
                self.cpu_per_core = self.cpu.read_per_core();
            } else {
                self.cpu.reset_per_core();
                self.cpu_per_core = None;
            }
        } else {
            self.cpu.reset_per_core();
            self.cpu_per_core = None;
        }

        // 2. RAM
        if self.settings.is_metric_active(METRIC_RAM)
            && let Some(ram) = read_ram()
        {
            self.mean_ram.push(u16::from(ram.load_percent) * 100);
            if let Some(m) = self.mean_ram.mean() {
                self.percent_ram = Some(self.steady_ram.update(bp_to_percent(m)));
            }
            self.ram_used_x10 = Some(gib_x10(ram.used()));
            self.ram_total_x10 = Some(gib_x10(ram.total));
        }

        // 3. Rete
        if self.settings.is_metric_active(METRIC_NET) {
            if self.net_sampler.is_none() {
                self.net_sampler = NetSampler::new();
            }
            if let Some(sampler) = self.net_sampler.as_mut() {
                if self.net_needs_refresh || self.net_interfaces.is_empty() || sampler.needs_refresh() {
                    self.net_interfaces = sampler.interfaces();
                    self.net_candidates = menu_candidates(&self.net_interfaces, Some(self.settings.net_luid));
                    self.net_needs_refresh = false;
                }

                let mode = match self.settings.net_mode {
                    NetMode::Sum => MetricsNetMode::Sum,
                    NetMode::Auto => MetricsNetMode::Auto,
                    NetMode::Specific => MetricsNetMode::Specific(self.settings.net_luid),
                };
                self.net_selection = select(&self.net_interfaces, mode);

                if sampler.counters(&self.net_selection.luids, &mut self.net_counters_buf) {
                    let wall_100ns = now.saturating_mul(10_000);
                    if let Some(rates) = self.net_meter.update(&self.net_counters_buf, wall_100ns) {
                        self.net_rates = Some(rates);
                    }
                }
            }
        }

        // 4. ACPI Temp
        if self.settings.is_metric_active(METRIC_TEMP_ACPI) {
            if self.acpi_sampler.is_none() {
                self.acpi_sampler = AcpiSampler::new();
            }
            if let Some(sampler) = self.acpi_sampler.as_mut()
                && let Some(zone) = sampler.read()
            {
                self.temp_acpi_c = Some(zone.temp_c);
                self.temp_acpi_fixed = zone.is_fixed;
                self.temp_acpi_label = Some(zone.label);
            }
        } else {
            self.acpi_sampler = None;
            self.temp_acpi_c = None;
        }

        // 5. GPU Temp (ogni 5s, o hovering, o primo tick)
        if self.settings.is_metric_active(METRIC_TEMP_GPU) {
            if self.gpu_sampler.is_none() {
                self.gpu_sampler = GpuSampler::new();
            }
            if let Some(sampler) = self.gpu_sampler.as_mut()
                && (self.last_gpu_tick == 0 || now >= self.last_gpu_tick + 5_000 || hovering)
            {
                self.gpu_adapters = sampler.list_adapters();
                let selected = self
                    .gpu_adapters
                    .iter()
                    .find(|a| {
                        if self.settings.gpu_luid != 0 { a.luid == self.settings.gpu_luid } else { a.temp_c.is_some() }
                    })
                    .or_else(|| self.gpu_adapters.first());
                self.temp_gpu_c = selected.and_then(|a| a.temp_c);
                self.last_gpu_tick = now;
            }
        } else {
            self.gpu_sampler = None;
            self.temp_gpu_c = None;
            self.gpu_adapters.clear();
            self.last_gpu_tick = 0;
        }

        // 6. Disco Temp (ogni 60s o primo tick)
        if self.settings.is_metric_active(METRIC_TEMP_DISK) {
            if self.disk_sampler.is_none() {
                self.disk_sampler = DiskSampler::new();
            }
            if let Some(sampler) = self.disk_sampler.as_mut()
                && (self.last_disk_tick == 0 || now >= self.last_disk_tick + 60_000)
            {
                if let Some(dev) = sampler.read() {
                    self.temp_disk_c = Some(dev.temp_c);
                    self.temp_disk_label = Some(dev.label);
                }
                self.last_disk_tick = now;
            }
        } else {
            self.disk_sampler = None;
            self.temp_disk_c = None;
            self.temp_disk_label = None;
            self.last_disk_tick = 0;
        }

        // 7. Aggiorna Tooltip
        let is_it = core::ptr::eq(self.s, &strings::IT);
        let units = if self.settings.net_bits { Units::Bits } else { Units::Bytes };
        let (down_buf, up_buf) = if let Some(rates) = self.net_rates {
            (Some(format_rate(rates.down, units, is_it)), Some(format_rate(rates.up, units, is_it)))
        } else {
            (None, None)
        };

        let gpu_age = if self.last_gpu_tick > 0 { (now.saturating_sub(self.last_gpu_tick) / 1000) as u32 } else { 0 };
        let disk_age =
            if self.last_disk_tick > 0 { (now.saturating_sub(self.last_disk_tick) / 1000) as u32 } else { 0 };

        let tooltip_data = TooltipData {
            cpu_active: self.settings.is_metric_active(METRIC_CPU),
            cpu_percent: self.percent_cpu,
            cpu_utility: self.percent_utility,
            sat_state: self.sat_report.state,
            sat_top_proc: self.sat_top_proc.as_ref().map(|(n, c)| (n.as_str(), *c)),
            cpu_per_core: self.cpu_per_core.as_deref(),
            ram_active: self.settings.is_metric_active(METRIC_RAM),
            ram_percent: self.percent_ram,
            ram_used_x10: self.ram_used_x10,
            ram_total_x10: self.ram_total_x10,
            net_active: self.settings.is_metric_active(METRIC_NET),
            net_label: if self.net_selection.label.is_empty() { None } else { Some(&self.net_selection.label) },
            net_down: down_buf.as_ref().map(|b| b.as_str()),
            net_up: up_buf.as_ref().map(|b| b.as_str()),
            temp_acpi_active: self.settings.is_metric_active(METRIC_TEMP_ACPI),
            temp_acpi_c: self.temp_acpi_c,
            temp_acpi_fixed: self.temp_acpi_fixed,
            temp_gpu_active: self.settings.is_metric_active(METRIC_TEMP_GPU),
            temp_gpu_c: self.temp_gpu_c,
            temp_gpu_age_secs: gpu_age,
            temp_disk_active: self.settings.is_metric_active(METRIC_TEMP_DISK),
            temp_disk_c: self.temp_disk_c,
            temp_disk_label: self.temp_disk_label.as_deref(),
            temp_disk_age_secs: disk_age,
        };
        build_tooltip(&mut self.tip_sent, self.s, is_it, &tooltip_data);

        // 8. Aggiorna la finestra di hover se visibile
        if self.hover_win.as_ref().is_some_and(|h| h.is_visible()) {
            let snapshot = self.make_hover_snapshot();
            if let Some(hover) = &mut self.hover_win {
                hover.refresh_if_visible(snapshot);
            }
        }

        // 9. Aggiorna le icone
        self.update_tray_content();
    }

    fn color_for_level(palette: &Palette, level: Level) -> Color {
        match level {
            Level::Normal => palette.normal,
            Level::Warn => palette.warn,
            Level::Full => palette.full,
        }
    }

    fn draw_cpu_icon(&mut self, level: Level) -> Option<Icon> {
        let mut buf = [0u8; 3];
        let val_to_show = if self.settings.cpu_mode == CpuMode::Utility {
            self.percent_utility.or(self.percent_cpu)
        } else {
            self.percent_cpu
        };
        let text = icon_text(val_to_show, &mut buf);
        let color = Self::color_for_level(&self.palette, level);

        let bar = match self.sat_report.state {
            SatState::Normal | SatState::Full => None,
            SatState::Saturated(k) => Some(StatusBar { color: self.palette.warn, segments: k }),
        };

        if self.settings.is_icon_symbol_active(ICON_SYMBOL_CPU) {
            render_cpu_icon(&mut self.canvas, text, color, bar);
        } else {
            draw_value_icon_with_bar(&mut self.canvas, text, color, bar);
        }
        Icon::from_canvas(&self.canvas)
    }

    fn draw_ram_icon(&mut self, level: Level) -> Option<Icon> {
        let mut buf = [0u8; 3];
        let text = icon_text(self.percent_ram, &mut buf);
        let color = Self::color_for_level(&self.palette, level);
        if self.settings.is_icon_symbol_active(ICON_SYMBOL_RAM) {
            draw_ram_icon(&mut self.canvas, text, color);
        } else {
            draw_value_icon(&mut self.canvas, text, color);
        }
        Icon::from_canvas(&self.canvas)
    }

    fn draw_net_icon(&mut self) -> Option<Icon> {
        let units = if self.settings.net_bits { Units::Bits } else { Units::Bytes };
        let (up_text, down_text) = if let Some(rates) = self.net_rates {
            (compact_rate(rates.up, units), compact_rate(rates.down, units))
        } else {
            (compact_rate(0, units), compact_rate(0, units))
        };
        draw_dual_icon(&mut self.canvas, up_text.as_str(), self.palette.dim, down_text.as_str(), self.palette.normal);
        Icon::from_canvas(&self.canvas)
    }

    fn draw_temp_acpi_icon(&mut self) -> Option<Icon> {
        let mut buf = [0u8; 8];
        let text = temp_icon_text(self.temp_acpi_c, &mut buf);
        if self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_ACPI) {
            draw_temp_acpi_icon(&mut self.canvas, text, self.palette.normal);
        } else {
            draw_value_icon(&mut self.canvas, text, self.palette.normal);
        }
        Icon::from_canvas(&self.canvas)
    }

    fn draw_temp_gpu_icon(&mut self) -> Option<Icon> {
        let mut buf = [0u8; 8];
        let text = temp_icon_text(self.temp_gpu_c, &mut buf);
        if self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_GPU) {
            draw_temp_gpu_icon(&mut self.canvas, text, self.palette.normal);
        } else {
            draw_value_icon(&mut self.canvas, text, self.palette.normal);
        }
        Icon::from_canvas(&self.canvas)
    }

    fn draw_temp_disk_icon(&mut self) -> Option<Icon> {
        let mut buf = [0u8; 8];
        let text = temp_icon_text(self.temp_disk_c, &mut buf);
        if self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_DISK) {
            draw_temp_disk_icon(&mut self.canvas, text, self.palette.normal);
        } else {
            draw_value_icon(&mut self.canvas, text, self.palette.normal);
        }
        Icon::from_canvas(&self.canvas)
    }

    fn update_tray_content(&mut self) {
        let size = shell::small_icon_size(self.dpi);

        // Icona statica
        if let Some(tray) = &self.tray_static {
            tray.set_tip(self.explorer_tip());
            return;
        }

        // CPU
        if self.tray_cpu.is_some() {
            let bar = match self.sat_report.state {
                SatState::Normal | SatState::Full => None,
                SatState::Saturated(k) => Some(StatusBar { color: self.palette.warn, segments: k }),
            };
            let level = self.level_cpu.update(self.percent_cpu.unwrap_or(0));
            let key = DrawKeyCpu {
                percent: self.percent_cpu,
                level,
                bar,
                size,
                palette: self.palette,
                symbol: self.settings.is_icon_symbol_active(ICON_SYMBOL_CPU),
            };
            if self.drawn_cpu != Some(key) {
                let icon = self.draw_cpu_icon(level);
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_cpu {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_cpu = Some(key);
                }
            } else if let Some(tray) = &self.tray_cpu {
                tray.set_tip(self.explorer_tip());
            }
        }

        // RAM
        if self.tray_ram.is_some() {
            let level = self.level_ram.update(self.percent_ram.unwrap_or(0));
            let key = DrawKeyRam {
                percent: self.percent_ram,
                level,
                size,
                palette: self.palette,
                symbol: self.settings.is_icon_symbol_active(ICON_SYMBOL_RAM),
            };
            if self.drawn_ram != Some(key) {
                let icon = self.draw_ram_icon(level);
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_ram {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_ram = Some(key);
                }
            } else if let Some(tray) = &self.tray_ram {
                tray.set_tip(self.explorer_tip());
            }
        }

        // Rete
        if self.tray_net.is_some() {
            let units = if self.settings.net_bits { Units::Bits } else { Units::Bytes };
            let (up_text, down_text) = if let Some(rates) = self.net_rates {
                (compact_rate(rates.up, units), compact_rate(rates.down, units))
            } else {
                (compact_rate(0, units), compact_rate(0, units))
            };
            let mut down = [0u8; 4];
            let mut up = [0u8; 4];
            let ds = down_text.as_str().as_bytes();
            let us = up_text.as_str().as_bytes();
            down[..ds.len().min(4)].copy_from_slice(&ds[..ds.len().min(4)]);
            up[..us.len().min(4)].copy_from_slice(&us[..us.len().min(4)]);

            let key = DrawKeyNet { down, up, size, palette: self.palette };
            if self.drawn_net != Some(key) {
                let icon = self.draw_net_icon();
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_net {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_net = Some(key);
                }
            } else if let Some(tray) = &self.tray_net {
                tray.set_tip(self.explorer_tip());
            }
        }

        // ACPI Temp
        if self.tray_temp_acpi.is_some() {
            let key = DrawKeyTemp {
                temp_c: self.temp_acpi_c,
                size,
                palette: self.palette,
                symbol: self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_ACPI),
            };
            if self.drawn_temp_acpi != Some(key) {
                let icon = self.draw_temp_acpi_icon();
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_temp_acpi {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_temp_acpi = Some(key);
                }
            } else if let Some(tray) = &self.tray_temp_acpi {
                tray.set_tip(self.explorer_tip());
            }
        }

        // GPU Temp
        if self.tray_temp_gpu.is_some() {
            let key = DrawKeyTemp {
                temp_c: self.temp_gpu_c,
                size,
                palette: self.palette,
                symbol: self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_GPU),
            };
            if self.drawn_temp_gpu != Some(key) {
                let icon = self.draw_temp_gpu_icon();
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_temp_gpu {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_temp_gpu = Some(key);
                }
            } else if let Some(tray) = &self.tray_temp_gpu {
                tray.set_tip(self.explorer_tip());
            }
        }

        // Disco Temp
        if self.tray_temp_disk.is_some() {
            let key = DrawKeyTemp {
                temp_c: self.temp_disk_c,
                size,
                palette: self.palette,
                symbol: self.settings.is_icon_symbol_active(ICON_SYMBOL_TEMP_DISK),
            };
            if self.drawn_temp_disk != Some(key) {
                let icon = self.draw_temp_disk_icon();
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray_temp_disk {
                        tray.update_or_readd(&icon, self.explorer_tip());
                    }
                    self.drawn_temp_disk = Some(key);
                }
            } else if let Some(tray) = &self.tray_temp_disk {
                tray.set_tip(self.explorer_tip());
            }
        }
    }

    fn on_hover(&mut self, _id: u32, x: i32, y: i32) {
        let now = now_ms();
        let was_hovering = now < self.hover_until;
        self.hover_until = now.saturating_add(HOVER_MS);
        if !was_hovering {
            self.retime();
        }

        let snapshot = self.make_hover_snapshot();
        if let Some(hover) = &mut self.hover_win {
            hover.show_or_update(snapshot, x, y);
        }

        unsafe {
            SetCoalescableTimer(self.hwnd, TIMER_HOVER_TRACK, HOVER_TRACK_MS, None, 20);
        }
    }

    fn hide_hover(&mut self) {
        if let Some(hover) = &mut self.hover_win {
            hover.hide();
        }
        unsafe {
            KillTimer(self.hwnd, TIMER_HOVER_TRACK);
        }
    }

    fn show_inspect(&mut self) {
        self.last_inspect_toggle = unsafe { GetTickCount64() };
        if let Some(win) = self.inspect_win.as_mut() {
            win.show(self.theme != Theme::Light);
        }
    }

    fn show_inspect_tab(&mut self, tab: crate::sys::inspect_win::InspectTab) {
        self.last_inspect_toggle = unsafe { GetTickCount64() };
        if let Some(win) = self.inspect_win.as_mut() {
            win.show_tab(tab, self.theme != Theme::Light);
        }
    }

    fn toggle_inspect(&mut self) {
        let now = unsafe { GetTickCount64() };
        if now.saturating_sub(self.last_inspect_toggle) < 300 {
            return;
        }
        self.last_inspect_toggle = now;
        if let Some(win) = self.inspect_win.as_mut() {
            win.toggle(self.theme != Theme::Light);
        }
    }

    fn make_hover_snapshot(&self) -> HoverSnapshot {
        let is_it = core::ptr::eq(self.s, &strings::IT);
        let units = if self.settings.net_bits { Units::Bits } else { Units::Bytes };
        let (down_fmt, up_fmt) = if let Some(rates) = self.net_rates {
            (
                Some(format_rate(rates.down, units, is_it).as_str().to_string()),
                Some(format_rate(rates.up, units, is_it).as_str().to_string()),
            )
        } else {
            (None, None)
        };

        let selected_gpu =
            self.gpu_adapters.iter().find(|a| a.luid == self.settings.gpu_luid).or_else(|| self.gpu_adapters.first());

        let mounted_disks =
            if self.settings.disk_space_hover { nextm_metrics::sys::disk::scan_mounted_disks() } else { Vec::new() };

        HoverSnapshot {
            is_dark: self.theme != Theme::Light,
            is_it,
            dpi: self.dpi,
            cpu_active: self.settings.is_metric_active(METRIC_CPU),
            cpu_percent: self.percent_cpu,
            cpu_utility: self.percent_utility,
            sat_state: self.sat_report.state,
            sat_top_proc: self.sat_top_proc.clone(),
            cpu_cores: self.cpu_per_core.clone(),
            ram_active: self.settings.is_metric_active(METRIC_RAM),
            ram_percent: self.percent_ram,
            ram_used_x10: self.ram_used_x10,
            ram_total_x10: self.ram_total_x10,
            net_active: self.settings.is_metric_active(METRIC_NET),
            net_label: if self.net_selection.label.is_empty() { None } else { Some(self.net_selection.label.clone()) },
            net_down: down_fmt,
            net_up: up_fmt,
            temp_acpi_active: self.settings.is_metric_active(METRIC_TEMP_ACPI),
            temp_acpi_c: self.temp_acpi_c,
            temp_acpi_fixed: self.temp_acpi_fixed,
            temp_gpu_active: self.settings.is_metric_active(METRIC_TEMP_GPU),
            temp_gpu_c: self.temp_gpu_c,
            temp_gpu_name: selected_gpu.map(|a| a.name.clone()),
            temp_disk_active: self.settings.is_metric_active(METRIC_TEMP_DISK),
            temp_disk_c: self.temp_disk_c,
            temp_disk_label: self.temp_disk_label.clone(),
            disk_space_active: self.settings.disk_space_hover,
            mounted_disks,
        }
    }

    fn on_balloon_click(&mut self) {
        if self.balloon == Some(Balloon::FirstRun) {
            self.settings.first_run_done = true;
            self.save();
            shell::open_with_explorer(wide!("ms-settings:taskbar"));
        }
        self.balloon = None;
    }

    fn on_activate(&mut self) {
        self.tray_cpu = None;
        self.tray_ram = None;
        self.tray_net = None;
        self.tray_temp_acpi = None;
        self.tray_temp_gpu = None;
        self.tray_temp_disk = None;
        self.tray_static = None;
        self.drawn_cpu = None;
        self.drawn_ram = None;
        self.drawn_net = None;
        self.drawn_temp_acpi = None;
        self.drawn_temp_gpu = None;
        self.drawn_temp_disk = None;
        self.sync_tray_icons();
        self.tick();
        self.show_inspect();
    }

    fn on_taskbar_created(&mut self) {
        self.taskbar = shell::taskbar();
        self.dpi = shell::dpi_of(self.taskbar);
        let size = shell::small_icon_size(self.dpi);
        self.canvas.resize(size, size);
        self.tray_cpu = None;
        self.tray_ram = None;
        self.tray_net = None;
        self.tray_temp_acpi = None;
        self.tray_temp_gpu = None;
        self.tray_temp_disk = None;
        self.tray_static = None;
        self.drawn_cpu = None;
        self.drawn_ram = None;
        self.drawn_net = None;
        self.drawn_temp_acpi = None;
        self.drawn_temp_gpu = None;
        self.drawn_temp_disk = None;
        self.sync_tray_icons();
        self.tick();
    }

    fn on_display_change(&mut self) {
        self.on_taskbar_created();
    }

    fn on_setting_change(&mut self, action: u32, area: SettingArea) {
        if action == SPI_SETHIGHCONTRAST || area == SettingArea::ImmersiveColorSet {
            unsafe {
                SetCoalescableTimer(self.hwnd, TIMER_THEME, THEME_DEBOUNCE_MS, None, 50);
            }
        }
    }

    fn apply_theme(&mut self) {
        let t = theme::taskbar_theme();
        if self.theme != t {
            self.theme = t;
            self.palette = Palette::for_theme(t);
            self.drawn_cpu = None;
            self.drawn_ram = None;
            self.drawn_net = None;
            self.drawn_temp_acpi = None;
            self.drawn_temp_gpu = None;
            self.drawn_temp_disk = None;
            self.update_tray_content();
        }
    }

    fn on_power_setting(&mut self, guid: &windows_sys::core::GUID, data: u32) {
        if power::guid_eq(guid, &GUID_SESSION_DISPLAY_STATUS) {
            let on = data != 0;
            if self.display_on != on {
                self.display_on = on;
                if on {
                    self.reset_baseline();
                    self.first_tick = true;
                    self.retime();
                } else if self.settings.pause_display {
                    self.kill_timer();
                }
            }
        } else if power::guid_eq(guid, &GUID_ENERGY_SAVER_STATUS) || power::guid_eq(guid, &GUID_POWER_SAVING_STATUS) {
            let saver = data != 0;
            if self.energy_saver != saver {
                self.energy_saver = saver;
                self.retime();
            }
        }
    }

    fn on_resume(&mut self) {
        self.reset_baseline();
        self.net_needs_refresh = true;
        self.first_tick = true;
        self.retime();
    }

    fn reset_baseline(&mut self) {
        self.prev_cpu = None;
        self.mean_cpu.reset();
        self.steady_cpu.reset();
        self.level_cpu.reset();

        self.prev_utility = None;
        self.percent_utility = None;

        self.mean_ram.reset();
        self.steady_ram.reset();
        self.level_ram.reset();

        self.net_meter.reset();
        self.sat_detector.reset();
        if let Some(scanner) = self.sat_scanner.as_mut() {
            scanner.release();
        }

        self.last_gpu_tick = 0;
        self.last_disk_tick = 0;
        self.cpu.reset_per_core();
        self.cpu_per_core = None;
    }

    fn update_power_registrations(&mut self) {
        if self.settings.pause_display {
            if self.display_notify.is_none() {
                self.display_notify = PowerNotify::register(self.hwnd, &GUID_SESSION_DISPLAY_STATUS);
            }
        } else {
            self.display_notify = None;
            if !self.display_on {
                self.display_on = true;
                self.reset_baseline();
                self.first_tick = true;
                self.arm_timer(FIRST_TICK_MS);
            }
        }
        if self.settings.slow_energy_saver {
            if self.saver_notify.is_none() {
                self.saver_notify = PowerNotify::register(self.hwnd, &GUID_ENERGY_SAVER_STATUS)
                    .or_else(|| PowerNotify::register(self.hwnd, &GUID_POWER_SAVING_STATUS));
            }
        } else {
            self.saver_notify = None;
            self.energy_saver = false;
        }
    }

    fn on_command(&mut self, cmd: u32) -> After {
        match cmd {
            CMD_CPU_ACTIVE => {
                self.settings.metrics ^= METRIC_CPU;
                if !self.settings.is_metric_active(METRIC_CPU) {
                    self.settings.icons &= !ICON_CPU;
                }
                self.on_settings_modified();
            }
            CMD_CPU_ICON => {
                self.settings.icons ^= ICON_CPU;
                if self.settings.is_icon_active(ICON_CPU) {
                    self.settings.metrics |= METRIC_CPU;
                }
                self.on_settings_modified();
            }
            CMD_CPU_ICON_SYMBOL => {
                self.settings.icon_symbols ^= ICON_SYMBOL_CPU;
                self.on_settings_modified();
            }
            CMD_ICON_SYMBOLS_OFF => {
                self.settings.icon_symbols = 0;
                self.drawn_cpu = None;
                self.drawn_ram = None;
                self.drawn_temp_acpi = None;
                self.drawn_temp_gpu = None;
                self.drawn_temp_disk = None;
                self.on_settings_modified();
            }
            CMD_ICON_SYMBOLS_ON => {
                self.settings.icon_symbols = ICON_SYMBOL_CPU
                    | ICON_SYMBOL_RAM
                    | ICON_SYMBOL_TEMP_ACPI
                    | ICON_SYMBOL_TEMP_GPU
                    | ICON_SYMBOL_TEMP_DISK;
                self.drawn_cpu = None;
                self.drawn_ram = None;
                self.drawn_temp_acpi = None;
                self.drawn_temp_gpu = None;
                self.drawn_temp_disk = None;
                self.on_settings_modified();
            }
            CMD_CPU_UTILITY => {
                self.settings.cpu_mode =
                    if self.settings.cpu_mode == CpuMode::Utility { CpuMode::Standard } else { CpuMode::Utility };
                self.prev_utility = None;
                self.percent_utility = None;
                self.on_settings_modified();
            }
            CMD_CPU_PER_CORE => {
                self.settings.cpu_per_core_hover = !self.settings.cpu_per_core_hover;
                if !self.settings.cpu_per_core_hover {
                    self.cpu.reset_per_core();
                    self.cpu_per_core = None;
                }
                self.on_settings_modified();
            }
            CMD_DISK_SPACE_HOVER => {
                self.settings.disk_space_hover = !self.settings.disk_space_hover;
                self.on_settings_modified();
            }
            CMD_SAT_ALWAYS => {
                self.settings.saturation_mode = SaturationMode::Always;
                self.on_settings_modified();
            }
            CMD_SAT_ONDEMAND => {
                self.settings.saturation_mode = SaturationMode::OnDemand;
                self.on_settings_modified();
            }
            CMD_SAT_OFF => {
                self.settings.saturation_mode = SaturationMode::Off;
                if let Some(scanner) = self.sat_scanner.as_mut() {
                    scanner.release();
                }
                self.sat_report.state = SatState::Normal;
                self.on_settings_modified();
            }
            CMD_RAM_ACTIVE => {
                self.settings.metrics ^= METRIC_RAM;
                if !self.settings.is_metric_active(METRIC_RAM) {
                    self.settings.icons &= !ICON_RAM;
                }
                self.on_settings_modified();
            }
            CMD_RAM_ICON => {
                self.settings.icons ^= ICON_RAM;
                if self.settings.is_icon_active(ICON_RAM) {
                    self.settings.metrics |= METRIC_RAM;
                }
                self.on_settings_modified();
            }
            CMD_RAM_ICON_SYMBOL => {
                self.settings.icon_symbols ^= ICON_SYMBOL_RAM;
                self.on_settings_modified();
            }
            CMD_NET_ACTIVE => {
                self.settings.metrics ^= METRIC_NET;
                if !self.settings.is_metric_active(METRIC_NET) {
                    self.settings.icons &= !ICON_NET;
                }
                self.on_settings_modified();
            }
            CMD_NET_ICON => {
                self.settings.icons ^= ICON_NET;
                if self.settings.is_icon_active(ICON_NET) {
                    self.settings.metrics |= METRIC_NET;
                }
                self.on_settings_modified();
            }
            CMD_NET_BITS => {
                self.settings.net_bits = !self.settings.net_bits;
                self.on_settings_modified();
            }
            CMD_NET_SUM => {
                self.settings.net_mode = NetMode::Sum;
                self.on_settings_modified();
            }
            CMD_NET_AUTO => {
                self.settings.net_mode = NetMode::Auto;
                self.on_settings_modified();
            }
            c if (CMD_NET_SPECIFIC_BASE..CMD_NET_SPECIFIC_BASE + 10).contains(&c) => {
                let idx = (c - CMD_NET_SPECIFIC_BASE) as usize;
                if let Some(cand) = self.net_candidates.get(idx) {
                    self.settings.net_mode = NetMode::Specific;
                    self.settings.net_luid = cand.0;
                    self.on_settings_modified();
                }
            }
            CMD_TEMP_ACPI_ACTIVE => {
                self.settings.metrics ^= METRIC_TEMP_ACPI;
                if !self.settings.is_metric_active(METRIC_TEMP_ACPI) {
                    self.settings.icons &= !ICON_TEMP_ACPI;
                    self.acpi_sampler = None;
                    self.temp_acpi_c = None;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_ACPI_ICON => {
                self.settings.icons ^= ICON_TEMP_ACPI;
                if self.settings.is_icon_active(ICON_TEMP_ACPI) {
                    self.settings.metrics |= METRIC_TEMP_ACPI;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_ACPI_ICON_SYMBOL => {
                self.settings.icon_symbols ^= ICON_SYMBOL_TEMP_ACPI;
                self.on_settings_modified();
            }
            CMD_TEMP_GPU_ACTIVE => {
                self.settings.metrics ^= METRIC_TEMP_GPU;
                if !self.settings.is_metric_active(METRIC_TEMP_GPU) {
                    self.settings.icons &= !ICON_TEMP_GPU;
                    self.gpu_sampler = None;
                    self.temp_gpu_c = None;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_GPU_ICON => {
                self.settings.icons ^= ICON_TEMP_GPU;
                if self.settings.is_icon_active(ICON_TEMP_GPU) {
                    self.settings.metrics |= METRIC_TEMP_GPU;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_GPU_ICON_SYMBOL => {
                self.settings.icon_symbols ^= ICON_SYMBOL_TEMP_GPU;
                self.on_settings_modified();
            }
            c if (CMD_TEMP_GPU_SELECT_BASE..CMD_TEMP_GPU_SELECT_BASE + 8).contains(&c) => {
                let idx = (c - CMD_TEMP_GPU_SELECT_BASE) as usize;
                if let Some(adapter) = self.gpu_adapters.get(idx) {
                    self.settings.gpu_luid = adapter.luid;
                    self.temp_gpu_c = adapter.temp_c;
                    self.on_settings_modified();
                }
            }
            CMD_TEMP_DISK_ACTIVE => {
                self.settings.metrics ^= METRIC_TEMP_DISK;
                if !self.settings.is_metric_active(METRIC_TEMP_DISK) {
                    self.settings.icons &= !ICON_TEMP_DISK;
                    self.disk_sampler = None;
                    self.temp_disk_c = None;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_DISK_ICON => {
                self.settings.icons ^= ICON_TEMP_DISK;
                if self.settings.is_icon_active(ICON_TEMP_DISK) {
                    self.settings.metrics |= METRIC_TEMP_DISK;
                }
                self.on_settings_modified();
            }
            CMD_TEMP_DISK_ICON_SYMBOL => {
                self.settings.icon_symbols ^= ICON_SYMBOL_TEMP_DISK;
                self.on_settings_modified();
            }
            CMD_INTERVAL_1S | CMD_INTERVAL_2S | CMD_INTERVAL_5S => {
                self.settings.interval_ms = match cmd {
                    CMD_INTERVAL_1S => 1_000,
                    CMD_INTERVAL_2S => 2_000,
                    _ => 5_000,
                };
                self.on_settings_modified();
            }
            CMD_PAUSE_DISPLAY => {
                self.settings.pause_display = !self.settings.pause_display;
                self.save();
                self.update_power_registrations();
            }
            CMD_SLOW_SAVER => {
                self.settings.slow_energy_saver = !self.settings.slow_energy_saver;
                self.save();
                self.update_power_registrations();
                self.retime();
            }
            CMD_ECOQOS => {
                self.settings.ecoqos = !self.settings.ecoqos;
                self.save();
                power::set_ecoqos(self.settings.ecoqos);
            }
            CMD_AUTOSTART => {
                if autostart::is_enabled(&self.exe) {
                    autostart::disable();
                } else if !self.exe.is_empty() {
                    autostart::enable(&self.exe);
                    crate::sys::elevation::set_task_scheduler_enabled(false);
                }
            }
            CMD_RESTART_ADMIN => {
                if crate::sys::elevation::restart_as_admin() {
                    return After::Exit;
                }
                return After::Nothing;
            }
            CMD_AUTOSTART_ADMIN => {
                let enabled = crate::sys::elevation::is_task_scheduler_enabled();
                if crate::sys::elevation::set_task_scheduler_enabled(!enabled) && !enabled {
                    autostart::disable();
                }
            }
            CMD_LANG_AUTO | CMD_LANG_IT | CMD_LANG_EN => {
                self.settings.language = match cmd {
                    CMD_LANG_IT => crate::settings::Language::Italian,
                    CMD_LANG_EN => crate::settings::Language::English,
                    _ => crate::settings::Language::Auto,
                };
                self.s = match self.settings.language {
                    crate::settings::Language::Italian => &strings::IT,
                    crate::settings::Language::English => &strings::EN,
                    crate::settings::Language::Auto => strings::for_langid(crate::sys::ui_langid()),
                };
                self.save();
                if let Some(win) = self.inspect_win.as_mut() {
                    win.update_strings(self.s);
                }
                self.sync_tray_icons();
                self.tick();
            }
            CMD_SHOW_ICON => shell::open_with_explorer(wide!("ms-settings:taskbar")),
            CMD_ABOUT => return After::About,
            CMD_CHECK_UPDATES => {
                self.show_inspect_tab(crate::sys::inspect_win::InspectTab::Info);
                let hwnd = self.inspect_win.as_ref().map(|w| w.hwnd());
                crate::sys::update::check_for_updates_async(hwnd);
                return After::About;
            }
            CMD_EXIT => return After::Exit,
            _ => {}
        }
        After::Nothing
    }

    fn on_settings_modified(&mut self) {
        self.save();
        self.sync_tray_icons();
        self.retime();
        self.tick();
    }

    fn save(&mut self) {
        self.store.save(&self.settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_immersive_color_set() {
        let s = wide!("ImmersiveColorSet");
        assert!(unsafe { setting_area(s.as_ptr() as LPARAM) } == SettingArea::ImmersiveColorSet);
        let other = wide!("Policy");
        assert!(unsafe { setting_area(other.as_ptr() as LPARAM) } == SettingArea::Other);
        let longer = wide!("ImmersiveColorSetX");
        assert!(unsafe { setting_area(longer.as_ptr() as LPARAM) } == SettingArea::Other);
        assert!(unsafe { setting_area(0) } == SettingArea::Other);
    }
}
