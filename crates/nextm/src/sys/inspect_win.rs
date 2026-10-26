//! Finestra unificata di ispezione di sistema ("Il Grande Passo").
//!
//! Integra in un'unica interfaccia fluida e ultraleggera:
//! 1. Gestione Processi (Task Manager) con consumi CPU %, Working Set, Memoria Privata, Thread
//!    e correlazione con i relativi servizi e socket di rete.
//! 2. Vista Connessioni di Rete (TCPView) con porte locali/remote, protocolli e stati TCP/UDP,
//!    e chiusura forzata connessioni.
//! 3. Vista Servizi Windows con stato, avvio/arresto e correlazione al processo host (svchost).
//!
//! Nessuna dipendenza runtime esterna, zero driver, DWM dark mode, doppio buffer GDI senza sfarfallio.

#![allow(unsafe_op_in_unsafe_fn)]

use core::mem::zeroed;
use core::ptr::{null, null_mut};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use windows_sys::Win32::Foundation::{COLORREF, CloseHandle, FreeLibrary, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, ETO_CLIPPED, EndPaint, ExtTextOutW, FillRect, FrameRect, GetTextExtentPoint32W, HBRUSH,
    HDC, HFONT, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
    TextOutW,
};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BringWindowToTop, CreatePopupMenu, CreateWindowExW, DI_NORMAL, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DrawIconEx, ES_AUTOHSCROLL, GetClientRect, GetForegroundWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, HICON, HTCLIENT, HWND_TOP, IDC_ARROW, IDC_HAND, IDC_SIZEWE, IMAGE_ICON,
    KillTimer, LR_DEFAULTCOLOR, LoadCursorW, LoadImageW, MF_SEPARATOR, MF_STRING, MINMAXINFO, RegisterClassExW,
    SPI_GETWORKAREA, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageW,
    SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFONT, WM_SETICON, WM_SIZE, WM_TIMER,
    WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use crate::strings::Strings;
use nextm_metrics::inspect::{IpAddrKind, ServiceState, SocketProtocol, TcpState};
use nextm_metrics::sys::inspect_engine::InspectEngine;

#[inline]
fn cmp_ignore_ascii_case(a: &str, b: &str) -> core::cmp::Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

#[inline]
fn contains_ignore_ascii_case(haystack: &str, needle_ascii_lower: &str) -> bool {
    if needle_ascii_lower.is_empty() {
        return true;
    }
    if needle_ascii_lower.len() > haystack.len() {
        return false;
    }
    let n_bytes = needle_ascii_lower.as_bytes();
    let h_bytes = haystack.as_bytes();
    h_bytes
        .windows(n_bytes.len())
        .any(|window| window.iter().zip(n_bytes.iter()).all(|(h, n)| h.to_ascii_lowercase() == *n))
}

#[inline]
fn pid_contains(pid: u32, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut buf = [0u8; 12];
    let mut val = pid;
    if val == 0 {
        return query == "0";
    }
    let mut rev = [0u8; 12];
    let mut len = 0;
    while val > 0 {
        rev[len] = b'0' + (val % 10) as u8;
        val /= 10;
        len += 1;
    }
    for i in 0..len {
        buf[i] = rev[len - 1 - i];
    }
    if let Ok(s) = core::str::from_utf8(&buf[..len]) { s.contains(query) } else { false }
}

pub const INSPECT_CLASS: &[u16] = wide!("nextm-inspect");
const IDC_SEARCH: u32 = 1001;
const TIMER_REFRESH: usize = 1;

const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const VK_ESCAPE: u32 = 0x1B;
const VK_DELETE: u32 = 0x2E;
const VK_F5: u32 = 0x74;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectTab {
    Processes,
    Sockets,
    Services,
    Settings,
    Info,
}

#[derive(Clone, Debug)]
struct ProcessRow {
    pid: u32,
    name: String,
    cpu_percent: f32,
    ws_bytes: u64,
    priv_bytes: u64,
    threads: u32,
    services_summary: String,
    sockets_count: usize,
    is_hung: bool,
    is_leak: bool,
    mem_delta_mb: i32,
}

#[derive(Clone, Debug)]
struct SocketRow {
    proto: &'static str,
    local_addr: String,
    remote_addr: String,
    scope_badge: &'static str,
    remote_ip: Option<String>,
    is_public: bool,
    state: &'static str,
    proc_name: String,
    pid: u32,
    raw_local_ip_v4: Option<[u8; 4]>,
    raw_local_port: u16,
    raw_remote_ip_v4: Option<[u8; 4]>,
    raw_remote_port: u16,
}

#[derive(Clone, Debug)]
struct ServiceRow {
    name: String,
    display_name: String,
    state_str: String,
    is_running: bool,
    pid: u32,
    proc_name: String,
}

#[derive(Clone, Debug)]
enum ProcItem {
    Group {
        name: String,
        pids: Vec<u32>,
        count: usize,
        cpu_percent: f32,
        ws_bytes: u64,
        priv_bytes: u64,
        threads: u32,
        services_summary: String,
        sockets_count: usize,
        is_hung: bool,
        is_leak: bool,
        is_expanded: bool,
    },
    Child {
        proc_idx: usize,
    },
    Single {
        proc_idx: usize,
    },
}

#[derive(Clone, Debug)]
enum SocketItem {
    Group {
        proc_name: String,
        pids: Vec<u32>,
        count: usize,
        tcp_count: usize,
        udp_count: usize,
        is_expanded: bool,
        socket_indices: Vec<usize>,
    },
    Child {
        socket_idx: usize,
    },
    Single {
        socket_idx: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SelectedKey {
    ProcessGroup(String),
    ProcessPid(u32),
    SocketGroup(String),
    SocketEntry { pid: u32, proto: String, local_addr: String, remote_addr: String },
    Service(String),
}

struct InspectFonts {
    dpi: u32,
    pub title: HFONT,
    pub body: HFONT,
    pub mono: HFONT,
    pub btn: HFONT,
    pub badge: HFONT,
    pub info_sub: HFONT,
    pub hero: HFONT,
    pub title_lg: HFONT,
}

impl InspectFonts {
    unsafe fn new(dpi: u32) -> Self {
        Self {
            dpi,
            title: create_gdi_font(dpi, 10, 600, false),
            body: create_gdi_font(dpi, 9, 400, false),
            mono: create_gdi_font(dpi, 9, 400, true),
            btn: create_gdi_font(dpi, 8, 600, false),
            badge: create_gdi_font(dpi, 9, 600, false),
            info_sub: create_gdi_font(dpi, 11, 400, false),
            hero: create_gdi_font(dpi, 18, 700, false),
            title_lg: create_gdi_font(dpi, 15, 700, false),
        }
    }

    unsafe fn destroy(&mut self) {
        if !self.title.is_null() {
            DeleteObject(self.title);
            self.title = null_mut();
        }
        if !self.body.is_null() {
            DeleteObject(self.body);
            self.body = null_mut();
        }
        if !self.mono.is_null() {
            DeleteObject(self.mono);
            self.mono = null_mut();
        }
        if !self.btn.is_null() {
            DeleteObject(self.btn);
            self.btn = null_mut();
        }
        if !self.badge.is_null() {
            DeleteObject(self.badge);
            self.badge = null_mut();
        }
        if !self.info_sub.is_null() {
            DeleteObject(self.info_sub);
            self.info_sub = null_mut();
        }
        if !self.hero.is_null() {
            DeleteObject(self.hero);
            self.hero = null_mut();
        }
        if !self.title_lg.is_null() {
            DeleteObject(self.title_lg);
            self.title_lg = null_mut();
        }
    }

    unsafe fn update_dpi(&mut self, new_dpi: u32) {
        if self.dpi != new_dpi {
            self.destroy();
            *self = Self::new(new_dpi);
        }
    }
}

impl Drop for InspectFonts {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}

struct InspectState {
    hwnd: HWND,
    search_edit: HWND,
    engine: InspectEngine,
    active_tab: InspectTab,
    procs: Vec<ProcessRow>,
    proc_items: Vec<ProcItem>,
    group_procs: bool,
    expanded_groups: BTreeSet<String>,
    sockets: Vec<SocketRow>,
    socket_items: Vec<SocketItem>,
    group_sockets: bool,
    expanded_socket_groups: BTreeSet<String>,
    services: Vec<ServiceRow>,
    filtered_services: Vec<usize>,
    hung_windows: Vec<crate::sys::hung::HungWindow>,
    hung_kill_rect: Option<(RECT, u32)>,
    proc_mem_history: BTreeMap<u32, Vec<u64>>,
    filter_text: String,
    selected_index: Option<usize>,
    selected_key: Option<SelectedKey>,
    scroll_y: i32,
    hover_row: Option<usize>,
    sort_col: usize,
    sort_asc: bool,
    status_msg: Option<(String, Instant)>,
    is_dark: bool,
    dpi: u32,
    fonts: InspectFonts,
    s: &'static Strings,

    col_widths_procs: [i32; 6],
    col_widths_sockets: [i32; 5],
    col_widths_services: [i32; 4],
    hover_col_sep: Option<usize>,
    resizing_col: Option<(usize, i32, i32)>,

    tab_rects: [RECT; 5],
    group_btn_rect: RECT,
    bottom_btn_rects: [RECT; 3],

    is_dragging_scrollbar: bool,
    scrollbar_drag_start_y: i32,
    scrollbar_drag_start_scroll: i32,
    last_mouse_x: i32,
    last_mouse_y: i32,
    info_author_rect: RECT,
    info_repo_rect: RECT,
    info_update_btn_rect: RECT,
    info_download_btn_rect: RECT,
    info_card2_author_rect: RECT,

    edit_brush_dark: HBRUSH,
    edit_brush_light: HBRUSH,
    icon_96: HICON,
}

impl InspectState {
    #[inline]
    pub fn scale(&self, val: i32) -> i32 {
        (val * self.dpi as i32 + 48) / 96
    }

    #[inline]
    pub fn row_height(&self) -> i32 {
        self.scale(26).max(24)
    }

    #[inline]
    pub fn header_height(&self) -> i32 {
        self.scale(28).max(26)
    }

    #[inline]
    pub fn top_bar_height(&self) -> i32 {
        self.scale(46).max(42)
    }

    #[inline]
    pub fn bottom_bar_height(&self) -> i32 {
        self.scale(42).max(38)
    }
}

impl Drop for InspectState {
    fn drop(&mut self) {
        unsafe {
            if !self.edit_brush_dark.is_null() {
                DeleteObject(self.edit_brush_dark);
            }
            if !self.edit_brush_light.is_null() {
                DeleteObject(self.edit_brush_light);
            }
            if !self.icon_96.is_null() {
                DestroyIcon(self.icon_96);
            }
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<InspectState>> = const { RefCell::new(None) };
}

impl InspectState {
    fn release_memory(&mut self) {
        self.procs.clear();
        self.procs.shrink_to_fit();
        self.proc_items.clear();
        self.proc_items.shrink_to_fit();
        self.sockets.clear();
        self.sockets.shrink_to_fit();
        self.socket_items.clear();
        self.socket_items.shrink_to_fit();
        self.services.clear();
        self.services.shrink_to_fit();
        self.filtered_services.clear();
        self.filtered_services.shrink_to_fit();
        self.hung_windows.clear();
        self.hung_windows.shrink_to_fit();
        self.proc_mem_history.clear();
        self.engine.release();
    }

    fn is_italian(&self) -> bool {
        core::ptr::eq(self.s, &crate::strings::IT)
    }

    fn reload_data(&mut self) {
        let snapshot = self.engine.scan();
        self.hung_windows = crate::sys::hung::scan_hung_windows();

        // 1. Processi
        let current_pids: BTreeSet<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
        self.proc_mem_history.retain(|pid, _| current_pids.contains(pid));

        self.procs.clear();
        for p in &snapshot.processes {
            let mut svc_summary = String::new();
            for (idx, svc) in p.services.iter().take(3).enumerate() {
                if idx > 0 {
                    svc_summary.push_str(", ");
                }
                svc_summary.push_str(&svc.name);
            }
            if p.services.len() > 3 {
                svc_summary.push_str(&format!(" (+{})", p.services.len() - 3));
            }

            let is_hung = self.hung_windows.iter().any(|h| h.pid == p.pid);
            let history = self.proc_mem_history.entry(p.pid).or_default();
            history.push(p.private_bytes);
            if history.len() > 10 {
                history.remove(0);
            }
            let (is_leak, mem_delta_mb) = if history.len() >= 4 {
                let is_monotonic = history.windows(2).all(|w| w[1] >= w[0]);
                let growth = history.last().unwrap_or(&0).saturating_sub(*history.first().unwrap_or(&0));
                let growth_mb = (growth / (1024 * 1024)) as i32;
                if is_monotonic && growth >= 15 * 1024 * 1024 { (true, growth_mb) } else { (false, growth_mb) }
            } else {
                (false, 0)
            };

            self.procs.push(ProcessRow {
                pid: p.pid,
                name: p.name.clone(),
                cpu_percent: p.cpu_percent,
                ws_bytes: p.working_set_bytes,
                priv_bytes: p.private_bytes,
                threads: p.threads,
                services_summary: svc_summary,
                sockets_count: p.sockets.len(),
                is_hung,
                is_leak,
                mem_delta_mb,
            });
        }

        // 2. Sockets
        self.sockets.clear();
        let mut add_sock = |s: &nextm_metrics::inspect::SocketInfo, proc_name: &str| {
            let proto = s.protocol.as_str();
            let (local_addr, raw_v4_local) = match s.local_addr {
                IpAddrKind::V4(b) => (format!("{}.{}.{}.{}:{}", b[0], b[1], b[2], b[3], s.local_port), Some(b)),
                IpAddrKind::V6(_) => (format!("[{}]:{}", s.local_addr, s.local_port), None),
            };

            let (remote_addr, raw_v4_remote, raw_remote_port) = match (s.remote_addr, s.remote_port) {
                (Some(IpAddrKind::V4(b)), Some(port)) if port > 0 || s.state == Some(TcpState::Established) => {
                    (format!("{}.{}.{}.{}:{}", b[0], b[1], b[2], b[3], port), Some(b), port)
                }
                (Some(IpAddrKind::V6(b)), Some(port)) if port > 0 || s.state == Some(TcpState::Established) => {
                    (format!("[{}]:{}", IpAddrKind::V6(b), port), None, port)
                }
                _ => ("*:*".to_string(), None, 0),
            };

            let state_str = match s.state {
                Some(state) => state.as_str(),
                None => {
                    if s.protocol == SocketProtocol::Udp {
                        "UDP"
                    } else {
                        "—"
                    }
                }
            };

            let (scope_badge, is_public, remote_ip) = match s.remote_addr {
                Some(ip) => {
                    let sc = ip.scope();
                    (sc.badge(), sc.is_public(), Some(ip.to_string()))
                }
                None => ("", false, None),
            };

            self.sockets.push(SocketRow {
                proto,
                local_addr,
                remote_addr,
                scope_badge,
                remote_ip,
                is_public,
                state: state_str,
                proc_name: proc_name.to_string(),
                pid: s.pid,
                raw_local_ip_v4: raw_v4_local,
                raw_local_port: s.local_port,
                raw_remote_ip_v4: raw_v4_remote,
                raw_remote_port,
            });
        };

        for p in &snapshot.processes {
            for sock in &p.sockets {
                add_sock(sock, &p.name);
            }
        }
        for sock in &snapshot.orphaned_sockets {
            add_sock(sock, "—");
        }

        // 3. Servizi
        let is_it = self.is_italian();
        self.services.clear();
        for p in &snapshot.processes {
            for svc in &p.services {
                let (state_str, is_running) = match svc.state {
                    ServiceState::Running => (if is_it { "In esecuzione" } else { "Running" }, true),
                    ServiceState::Stopped => (if is_it { "Arrestato" } else { "Stopped" }, false),
                    ServiceState::Paused => (if is_it { "In pausa" } else { "Paused" }, false),
                    ServiceState::StartPending | ServiceState::ContinuePending => {
                        (if is_it { "Avvio..." } else { "Starting..." }, false)
                    }
                    ServiceState::StopPending | ServiceState::PausePending => {
                        (if is_it { "Arresto..." } else { "Stopping..." }, false)
                    }
                    ServiceState::Unknown(_) => (if is_it { "Sconosciuto" } else { "Unknown" }, false),
                };
                self.services.push(ServiceRow {
                    name: svc.name.clone(),
                    display_name: svc.display_name.clone(),
                    state_str: state_str.to_string(),
                    is_running,
                    pid: svc.pid,
                    proc_name: p.name.clone(),
                });
            }
        }

        self.apply_sorting();
        self.apply_filter();
        self.clamp_scroll();
    }

    fn apply_sorting(&mut self) {
        let asc = self.sort_asc;
        match self.active_tab {
            InspectTab::Processes => {
                match self.sort_col {
                    0 => self.procs.sort_by(|a, b| {
                        if asc {
                            cmp_ignore_ascii_case(&a.name, &b.name)
                        } else {
                            cmp_ignore_ascii_case(&b.name, &a.name)
                        }
                    }),
                    1 => self.procs.sort_by(|a, b| if asc { a.pid.cmp(&b.pid) } else { b.pid.cmp(&a.pid) }),
                    2 => self.procs.sort_by(|a, b| {
                        if asc {
                            a.cpu_percent.partial_cmp(&b.cpu_percent).unwrap_or(core::cmp::Ordering::Equal)
                        } else {
                            b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(core::cmp::Ordering::Equal)
                        }
                    }),
                    3 => self
                        .procs
                        .sort_by(|a, b| if asc { a.ws_bytes.cmp(&b.ws_bytes) } else { b.ws_bytes.cmp(&a.ws_bytes) }),
                    4 => self.procs.sort_by(|a, b| {
                        if asc { a.priv_bytes.cmp(&b.priv_bytes) } else { b.priv_bytes.cmp(&a.priv_bytes) }
                    }),
                    5 => self
                        .procs
                        .sort_by(|a, b| if asc { a.threads.cmp(&b.threads) } else { b.threads.cmp(&a.threads) }),
                    _ => {}
                }
                self.update_proc_items();
            }
            InspectTab::Sockets => {
                match self.sort_col {
                    0 => self.sockets.sort_by(|a, b| {
                        if asc {
                            cmp_ignore_ascii_case(&a.proc_name, &b.proc_name)
                        } else {
                            cmp_ignore_ascii_case(&b.proc_name, &a.proc_name)
                        }
                    }),
                    1 => self.sockets.sort_by(|a, b| if asc { a.pid.cmp(&b.pid) } else { b.pid.cmp(&a.pid) }),
                    2 => self.sockets.sort_by(|a, b| if asc { a.proto.cmp(b.proto) } else { b.proto.cmp(a.proto) }),
                    3 => self.sockets.sort_by(|a, b| {
                        if asc { a.local_addr.cmp(&b.local_addr) } else { b.local_addr.cmp(&a.local_addr) }
                    }),
                    4 => self.sockets.sort_by(|a, b| {
                        if asc { a.remote_addr.cmp(&b.remote_addr) } else { b.remote_addr.cmp(&a.remote_addr) }
                    }),
                    5 => self.sockets.sort_by(|a, b| if asc { a.state.cmp(b.state) } else { b.state.cmp(a.state) }),
                    _ => {}
                }
                self.update_socket_items();
            }
            InspectTab::Services => match self.sort_col {
                0 => self.services.sort_by(|a, b| {
                    if asc { cmp_ignore_ascii_case(&a.name, &b.name) } else { cmp_ignore_ascii_case(&b.name, &a.name) }
                }),
                1 => self.services.sort_by(|a, b| {
                    if asc {
                        cmp_ignore_ascii_case(&a.display_name, &b.display_name)
                    } else {
                        cmp_ignore_ascii_case(&b.display_name, &a.display_name)
                    }
                }),
                2 => self
                    .services
                    .sort_by(|a, b| if asc { a.state_str.cmp(&b.state_str) } else { b.state_str.cmp(&a.state_str) }),
                3 => self.services.sort_by(|a, b| if asc { a.pid.cmp(&b.pid) } else { b.pid.cmp(&a.pid) }),
                _ => {}
            },
            InspectTab::Info | InspectTab::Settings => {}
        }
    }

    fn update_proc_items(&mut self) {
        self.proc_items.clear();
        let query = self.filter_text.trim().to_lowercase();

        if !self.group_procs {
            for (idx, p) in self.procs.iter().enumerate() {
                let matches = if query.is_empty() {
                    true
                } else if query == "leak" || query == "leak:" {
                    p.is_leak
                } else if query == "hung" || query == "bloccato" || query == "bloccati" {
                    p.is_hung
                } else {
                    contains_ignore_ascii_case(&p.name, &query)
                        || pid_contains(p.pid, &query)
                        || contains_ignore_ascii_case(&p.services_summary, &query)
                };
                if matches {
                    self.proc_items.push(ProcItem::Single { proc_idx: idx });
                }
            }
            return;
        }

        struct GroupAcc {
            name: String,
            indices: Vec<usize>,
            pids: Vec<u32>,
            total_cpu: f32,
            total_ws: u64,
            total_priv: u64,
            total_threads: u32,
            total_sockets: usize,
            services: Vec<String>,
        }

        let mut map: BTreeMap<String, GroupAcc> = BTreeMap::new();
        for (idx, p) in self.procs.iter().enumerate() {
            let key = p.name.to_lowercase();
            let entry = map.entry(key).or_insert_with(|| GroupAcc {
                name: p.name.clone(),
                indices: Vec::new(),
                pids: Vec::new(),
                total_cpu: 0.0,
                total_ws: 0,
                total_priv: 0,
                total_threads: 0,
                total_sockets: 0,
                services: Vec::new(),
            });
            entry.indices.push(idx);
            entry.pids.push(p.pid);
            entry.total_cpu += p.cpu_percent;
            entry.total_ws += p.ws_bytes;
            entry.total_priv += p.priv_bytes;
            entry.total_threads += p.threads;
            entry.total_sockets += p.sockets_count;
            if !p.services_summary.is_empty() && !entry.services.contains(&p.services_summary) {
                entry.services.push(p.services_summary.clone());
            }
        }

        let mut groups: Vec<GroupAcc> = map
            .into_values()
            .filter(|g| {
                if query.is_empty() {
                    return true;
                }
                if query == "leak" || query == "leak:" {
                    return g.indices.iter().any(|&idx| self.procs[idx].is_leak);
                }
                if query == "hung" || query == "bloccato" || query == "bloccati" {
                    return g.indices.iter().any(|&idx| self.procs[idx].is_hung);
                }
                if contains_ignore_ascii_case(&g.name, &query) {
                    return true;
                }
                for &idx in &g.indices {
                    let p = &self.procs[idx];
                    if pid_contains(p.pid, &query) || contains_ignore_ascii_case(&p.services_summary, &query) {
                        return true;
                    }
                }
                false
            })
            .collect();

        let asc = self.sort_asc;
        match self.sort_col {
            0 => groups.sort_by(|a, b| {
                if asc { cmp_ignore_ascii_case(&a.name, &b.name) } else { cmp_ignore_ascii_case(&b.name, &a.name) }
            }),
            1 => groups.sort_by(|a, b| {
                if asc { a.indices.len().cmp(&b.indices.len()) } else { b.indices.len().cmp(&a.indices.len()) }
            }),
            2 => groups.sort_by(|a, b| {
                if asc {
                    a.total_cpu.partial_cmp(&b.total_cpu).unwrap_or(core::cmp::Ordering::Equal)
                } else {
                    b.total_cpu.partial_cmp(&a.total_cpu).unwrap_or(core::cmp::Ordering::Equal)
                }
            }),
            3 => groups.sort_by(|a, b| if asc { a.total_ws.cmp(&b.total_ws) } else { b.total_ws.cmp(&a.total_ws) }),
            4 => groups
                .sort_by(|a, b| if asc { a.total_priv.cmp(&b.total_priv) } else { b.total_priv.cmp(&a.total_priv) }),
            5 => groups.sort_by(|a, b| {
                if asc { a.total_threads.cmp(&b.total_threads) } else { b.total_threads.cmp(&a.total_threads) }
            }),
            _ => {}
        }

        for g in &mut groups {
            let procs = &self.procs;
            match self.sort_col {
                0 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        cmp_ignore_ascii_case(&procs[a].name, &procs[b].name)
                    } else {
                        cmp_ignore_ascii_case(&procs[b].name, &procs[a].name)
                    }
                }),
                1 => g.indices.sort_by(|&a, &b| {
                    if asc { procs[a].pid.cmp(&procs[b].pid) } else { procs[b].pid.cmp(&procs[a].pid) }
                }),
                2 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        procs[a].cpu_percent.partial_cmp(&procs[b].cpu_percent).unwrap_or(core::cmp::Ordering::Equal)
                    } else {
                        procs[b].cpu_percent.partial_cmp(&procs[a].cpu_percent).unwrap_or(core::cmp::Ordering::Equal)
                    }
                }),
                3 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        procs[a].ws_bytes.cmp(&procs[b].ws_bytes)
                    } else {
                        procs[b].ws_bytes.cmp(&procs[a].ws_bytes)
                    }
                }),
                4 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        procs[a].priv_bytes.cmp(&procs[b].priv_bytes)
                    } else {
                        procs[b].priv_bytes.cmp(&procs[a].priv_bytes)
                    }
                }),
                5 => g.indices.sort_by(|&a, &b| {
                    if asc { procs[a].threads.cmp(&procs[b].threads) } else { procs[b].threads.cmp(&procs[a].threads) }
                }),
                _ => {}
            }
        }

        for g in groups {
            let count = g.indices.len();
            if count == 1 {
                self.proc_items.push(ProcItem::Single { proc_idx: g.indices[0] });
            } else {
                let is_expanded = self.expanded_groups.contains(&g.name.to_lowercase());
                let svc_summary = g.services.join(", ");
                let has_hung = g.indices.iter().any(|&idx| self.procs[idx].is_hung);
                let has_leak = g.indices.iter().any(|&idx| self.procs[idx].is_leak);
                self.proc_items.push(ProcItem::Group {
                    name: g.name.clone(),
                    pids: g.pids,
                    count,
                    cpu_percent: g.total_cpu,
                    ws_bytes: g.total_ws,
                    priv_bytes: g.total_priv,
                    threads: g.total_threads,
                    services_summary: svc_summary,
                    sockets_count: g.total_sockets,
                    is_hung: has_hung,
                    is_leak: has_leak,
                    is_expanded,
                });
                if is_expanded {
                    for &idx in &g.indices {
                        self.proc_items.push(ProcItem::Child { proc_idx: idx });
                    }
                }
            }
        }
    }

    fn update_socket_items(&mut self) {
        self.socket_items.clear();
        let query = self.filter_text.trim().to_lowercase();

        if !self.group_sockets {
            for (idx, s) in self.sockets.iter().enumerate() {
                if query.is_empty()
                    || contains_ignore_ascii_case(&s.proc_name, &query)
                    || pid_contains(s.pid, &query)
                    || s.local_addr.contains(&query)
                    || s.remote_addr.contains(&query)
                    || contains_ignore_ascii_case(s.state, &query)
                    || contains_ignore_ascii_case(s.proto, &query)
                {
                    self.socket_items.push(SocketItem::Single { socket_idx: idx });
                }
            }
            return;
        }

        struct SockGroupAcc {
            name: String,
            indices: Vec<usize>,
            pids: Vec<u32>,
            tcp_count: usize,
            udp_count: usize,
        }

        let mut map: BTreeMap<String, SockGroupAcc> = BTreeMap::new();
        for (idx, s) in self.sockets.iter().enumerate() {
            let key = s.proc_name.to_lowercase();
            let entry = map.entry(key).or_insert_with(|| SockGroupAcc {
                name: s.proc_name.clone(),
                indices: Vec::new(),
                pids: Vec::new(),
                tcp_count: 0,
                udp_count: 0,
            });
            entry.indices.push(idx);
            if !entry.pids.contains(&s.pid) {
                entry.pids.push(s.pid);
            }
            if s.proto == "TCP" {
                entry.tcp_count += 1;
            } else {
                entry.udp_count += 1;
            }
        }

        let mut groups: Vec<SockGroupAcc> = map
            .into_values()
            .filter(|g| {
                if query.is_empty() {
                    return true;
                }
                if contains_ignore_ascii_case(&g.name, &query) {
                    return true;
                }
                for &idx in &g.indices {
                    let s = &self.sockets[idx];
                    if pid_contains(s.pid, &query)
                        || s.local_addr.contains(&query)
                        || s.remote_addr.contains(&query)
                        || contains_ignore_ascii_case(s.state, &query)
                        || contains_ignore_ascii_case(s.proto, &query)
                    {
                        return true;
                    }
                }
                false
            })
            .collect();

        let asc = self.sort_asc;
        match self.sort_col {
            0 => groups.sort_by(|a, b| {
                if asc { cmp_ignore_ascii_case(&a.name, &b.name) } else { cmp_ignore_ascii_case(&b.name, &a.name) }
            }),
            1 => groups.sort_by(|a, b| {
                let a_pid = a.pids.first().copied().unwrap_or(0);
                let b_pid = b.pids.first().copied().unwrap_or(0);
                if asc { a_pid.cmp(&b_pid) } else { b_pid.cmp(&a_pid) }
            }),
            2 => groups.sort_by(|a, b| if asc { a.tcp_count.cmp(&b.tcp_count) } else { b.tcp_count.cmp(&a.tcp_count) }),
            3 => groups.sort_by(|a, b| {
                if asc { a.indices.len().cmp(&b.indices.len()) } else { b.indices.len().cmp(&a.indices.len()) }
            }),
            4 => groups.sort_by(|a, b| {
                if asc { a.indices.len().cmp(&b.indices.len()) } else { b.indices.len().cmp(&a.indices.len()) }
            }),
            5 => groups.sort_by(|a, b| if asc { a.tcp_count.cmp(&b.tcp_count) } else { b.tcp_count.cmp(&a.tcp_count) }),
            _ => {}
        }

        for g in &mut groups {
            let sockets = &self.sockets;
            match self.sort_col {
                0 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        cmp_ignore_ascii_case(&sockets[a].proc_name, &sockets[b].proc_name)
                    } else {
                        cmp_ignore_ascii_case(&sockets[b].proc_name, &sockets[a].proc_name)
                    }
                }),
                1 => g.indices.sort_by(|&a, &b| {
                    if asc { sockets[a].pid.cmp(&sockets[b].pid) } else { sockets[b].pid.cmp(&sockets[a].pid) }
                }),
                2 => g.indices.sort_by(|&a, &b| {
                    if asc { sockets[a].proto.cmp(sockets[b].proto) } else { sockets[b].proto.cmp(sockets[a].proto) }
                }),
                3 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        sockets[a].local_addr.cmp(&sockets[b].local_addr)
                    } else {
                        sockets[b].local_addr.cmp(&sockets[a].local_addr)
                    }
                }),
                4 => g.indices.sort_by(|&a, &b| {
                    if asc {
                        sockets[a].remote_addr.cmp(&sockets[b].remote_addr)
                    } else {
                        sockets[b].remote_addr.cmp(&sockets[a].remote_addr)
                    }
                }),
                5 => g.indices.sort_by(|&a, &b| {
                    if asc { sockets[a].state.cmp(sockets[b].state) } else { sockets[b].state.cmp(sockets[a].state) }
                }),
                _ => {}
            }
        }

        for g in groups {
            let count = g.indices.len();
            if count == 1 {
                self.socket_items.push(SocketItem::Single { socket_idx: g.indices[0] });
            } else {
                let is_expanded = self.expanded_socket_groups.contains(&g.name.to_lowercase());
                self.socket_items.push(SocketItem::Group {
                    proc_name: g.name.clone(),
                    pids: g.pids,
                    count,
                    tcp_count: g.tcp_count,
                    udp_count: g.udp_count,
                    is_expanded,
                    socket_indices: g.indices.clone(),
                });
                if is_expanded {
                    for &idx in &g.indices {
                        self.socket_items.push(SocketItem::Child { socket_idx: idx });
                    }
                }
            }
        }
    }

    fn apply_filter(&mut self) {
        match self.active_tab {
            InspectTab::Processes => {
                self.update_proc_items();
            }
            InspectTab::Sockets => {
                self.update_socket_items();
            }
            InspectTab::Services => {
                let query = self.filter_text.trim().to_lowercase();
                self.filtered_services = self
                    .services
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        if query.is_empty() {
                            true
                        } else {
                            contains_ignore_ascii_case(&s.name, &query)
                                || contains_ignore_ascii_case(&s.display_name, &query)
                                || contains_ignore_ascii_case(&s.proc_name, &query)
                                || pid_contains(s.pid, &query)
                                || contains_ignore_ascii_case(&s.state_str, &query)
                        }
                    })
                    .map(|(i, _)| i)
                    .collect();
            }
            InspectTab::Info | InspectTab::Settings => {}
        }
        self.restore_selection_index();
        self.clamp_scroll();
    }

    fn row_count(&self) -> usize {
        match self.active_tab {
            InspectTab::Processes => self.proc_items.len(),
            InspectTab::Sockets => self.socket_items.len(),
            InspectTab::Services => self.filtered_services.len(),
            InspectTab::Info | InspectTab::Settings => 0,
        }
    }

    fn set_status(&mut self, msg: String) {
        self.status_msg = Some((msg, Instant::now()));
    }

    fn selected_proc_item(&self) -> Option<&ProcItem> {
        let sel = self.selected_index?;
        self.proc_items.get(sel)
    }

    fn kill_selected(&mut self) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel).cloned() else { return };
        let is_it = self.is_italian();
        match item {
            ProcItem::Group { name, pids, count, .. } => {
                let mut killed = 0;
                for pid in pids {
                    if self.engine.kill_process(pid) {
                        killed += 1;
                    }
                }
                if is_it {
                    self.set_status(format!("Terminati {killed}/{count} processi di {name}"));
                } else {
                    self.set_status(format!("Terminated {killed}/{count} processes of {name}"));
                }
                self.reload_data();
            }
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                if let Some(p) = self.procs.get(proc_idx) {
                    let pid = p.pid;
                    let name = p.name.clone();
                    if self.engine.kill_process(pid) {
                        if is_it {
                            self.set_status(format!("Processo terminato: {name} ({pid})"));
                        } else {
                            self.set_status(format!("Process terminated: {name} ({pid})"));
                        }
                    } else {
                        if is_it {
                            self.set_status(format!("Impossibile terminare: {name} ({pid})"));
                        } else {
                            self.set_status(format!("Unable to terminate: {name} ({pid})"));
                        }
                    }
                    self.reload_data();
                }
            }
        }
    }

    fn suspend_selected(&mut self) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel).cloned() else { return };
        let is_it = self.is_italian();
        match item {
            ProcItem::Group { name, pids, count, .. } => {
                let mut suspended = 0;
                for pid in pids {
                    if nextm_metrics::sys::processes::ProcessScanner::suspend_process(pid) {
                        suspended += 1;
                    }
                }
                if is_it {
                    self.set_status(format!("Sospesi {suspended}/{count} processi di {name}"));
                } else {
                    self.set_status(format!("Suspended {suspended}/{count} processes of {name}"));
                }
            }
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                if let Some(p) = self.procs.get(proc_idx) {
                    let pid = p.pid;
                    let name = p.name.clone();
                    if nextm_metrics::sys::processes::ProcessScanner::suspend_process(pid) {
                        if is_it {
                            self.set_status(format!("Processo sospeso: {name} ({pid})"));
                        } else {
                            self.set_status(format!("Process suspended: {name} ({pid})"));
                        }
                    } else if is_it {
                        self.set_status(format!("Impossibile sospendere: {name} ({pid})"));
                    } else {
                        self.set_status(format!("Unable to suspend: {name} ({pid})"));
                    }
                }
            }
        }
    }

    fn resume_selected(&mut self) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel).cloned() else { return };
        let is_it = self.is_italian();
        match item {
            ProcItem::Group { name, pids, count, .. } => {
                let mut resumed = 0;
                for pid in pids {
                    if nextm_metrics::sys::processes::ProcessScanner::resume_process(pid) {
                        resumed += 1;
                    }
                }
                if is_it {
                    self.set_status(format!("Ripresi {resumed}/{count} processi di {name}"));
                } else {
                    self.set_status(format!("Resumed {resumed}/{count} processes of {name}"));
                }
            }
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                if let Some(p) = self.procs.get(proc_idx) {
                    let pid = p.pid;
                    let name = p.name.clone();
                    if nextm_metrics::sys::processes::ProcessScanner::resume_process(pid) {
                        if is_it {
                            self.set_status(format!("Processo ripreso: {name} ({pid})"));
                        } else {
                            self.set_status(format!("Process resumed: {name} ({pid})"));
                        }
                    } else if is_it {
                        self.set_status(format!("Impossibile riprendere: {name} ({pid})"));
                    } else {
                        self.set_status(format!("Unable to resume: {name} ({pid})"));
                    }
                }
            }
        }
    }

    fn ecoqos_selected(&mut self, enable: bool) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel).cloned() else { return };
        let is_it = self.is_italian();
        let state_str = if enable {
            if is_it { "attivata" } else { "enabled" }
        } else {
            if is_it { "disattivata" } else { "disabled" }
        };
        match item {
            ProcItem::Group { name, pids, count, .. } => {
                let mut changed = 0;
                for pid in pids {
                    if nextm_metrics::sys::processes::ProcessScanner::set_process_ecoqos(pid, enable) {
                        changed += 1;
                    }
                }
                if is_it {
                    self.set_status(format!(
                        "Modalità Efficienza {state_str} per {changed}/{count} processi di {name}"
                    ));
                } else {
                    self.set_status(format!("Efficiency mode {state_str} for {changed}/{count} processes of {name}"));
                }
            }
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                if let Some(p) = self.procs.get(proc_idx) {
                    let pid = p.pid;
                    let name = p.name.clone();
                    if nextm_metrics::sys::processes::ProcessScanner::set_process_ecoqos(pid, enable) {
                        if is_it {
                            self.set_status(format!("Modalità Efficienza {state_str}: {name} ({pid})"));
                        } else {
                            self.set_status(format!("Efficiency mode {state_str}: {name} ({pid})"));
                        }
                    } else if is_it {
                        self.set_status(format!("Impossibile impostare EcoQoS: {name} ({pid})"));
                    } else {
                        self.set_status(format!("Unable to set EcoQoS: {name} ({pid})"));
                    }
                }
            }
        }
    }

    fn open_location_selected(&mut self) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel) else { return };
        let is_it = self.is_italian();
        let pid = match item {
            ProcItem::Group { pids, .. } => pids.first().copied(),
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                self.procs.get(*proc_idx).map(|p| p.pid)
            }
        };
        if let Some(pid) = pid
            && !open_process_file_location(pid)
        {
            if is_it {
                self.set_status(format!("Impossibile aprire il percorso del processo ({pid})"));
            } else {
                self.set_status(format!("Unable to open process path ({pid})"));
            }
        }
    }

    fn copy_pid_selected(&mut self, hwnd: HWND) {
        if self.active_tab != InspectTab::Processes {
            return;
        }
        let Some(sel) = self.selected_index else { return };
        let Some(item) = self.proc_items.get(sel) else { return };
        let is_it = self.is_italian();
        let text = match item {
            ProcItem::Group { pids, .. } => pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "),
            ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                self.procs.get(*proc_idx).map(|p| p.pid.to_string()).unwrap_or_default()
            }
        };
        if !text.is_empty() {
            copy_to_clipboard(hwnd, &text);
            if is_it {
                self.set_status(format!("PID copiato negli appunti: {text}"));
            } else {
                self.set_status(format!("PID copied to clipboard: {text}"));
            }
        }
    }

    fn selected_socket_item(&self) -> Option<&SocketItem> {
        let sel = self.selected_index?;
        self.socket_items.get(sel)
    }

    fn selected_service(&self) -> Option<&ServiceRow> {
        let sel = self.selected_index?;
        let &svc_idx = self.filtered_services.get(sel)?;
        self.services.get(svc_idx)
    }

    fn key_for_row(&self, idx: usize) -> Option<SelectedKey> {
        match self.active_tab {
            InspectTab::Processes => {
                let item = self.proc_items.get(idx)?;
                match item {
                    ProcItem::Group { name, .. } => Some(SelectedKey::ProcessGroup(name.clone())),
                    ProcItem::Child { proc_idx } | ProcItem::Single { proc_idx } => {
                        self.procs.get(*proc_idx).map(|p| SelectedKey::ProcessPid(p.pid))
                    }
                }
            }
            InspectTab::Sockets => {
                let item = self.socket_items.get(idx)?;
                match item {
                    SocketItem::Group { proc_name, .. } => Some(SelectedKey::SocketGroup(proc_name.clone())),
                    SocketItem::Child { socket_idx } | SocketItem::Single { socket_idx } => {
                        self.sockets.get(*socket_idx).map(|s| SelectedKey::SocketEntry {
                            pid: s.pid,
                            proto: s.proto.to_string(),
                            local_addr: s.local_addr.clone(),
                            remote_addr: s.remote_addr.clone(),
                        })
                    }
                }
            }
            InspectTab::Services => {
                let &svc_idx = self.filtered_services.get(idx)?;
                self.services.get(svc_idx).map(|s| SelectedKey::Service(s.name.clone()))
            }
            InspectTab::Info | InspectTab::Settings => None,
        }
    }

    fn set_selected_index(&mut self, idx: Option<usize>) {
        self.selected_index = idx;
        self.selected_key = idx.and_then(|i| self.key_for_row(i));
    }

    fn restore_selection_index(&mut self) {
        let Some(key) = &self.selected_key else {
            self.selected_index = None;
            return;
        };
        match self.active_tab {
            InspectTab::Processes => match key {
                SelectedKey::ProcessPid(target_pid) => {
                    let found = self.proc_items.iter().position(|item| match item {
                        ProcItem::Child { proc_idx } | ProcItem::Single { proc_idx } => {
                            self.procs.get(*proc_idx).map(|p| p.pid == *target_pid).unwrap_or(false)
                        }
                        _ => false,
                    });
                    if let Some(i) = found {
                        self.selected_index = Some(i);
                    } else {
                        self.selected_index = None;
                        self.selected_key = None;
                    }
                }
                SelectedKey::ProcessGroup(target_name) => {
                    let found = self.proc_items.iter().position(|item| match item {
                        ProcItem::Group { name, .. } => name.eq_ignore_ascii_case(target_name),
                        ProcItem::Single { proc_idx } => {
                            self.procs.get(*proc_idx).map(|p| p.name.eq_ignore_ascii_case(target_name)).unwrap_or(false)
                        }
                        _ => false,
                    });
                    if let Some(i) = found {
                        self.selected_index = Some(i);
                    } else {
                        self.selected_index = None;
                        self.selected_key = None;
                    }
                }
                _ => {
                    self.selected_index = None;
                    self.selected_key = None;
                }
            },
            InspectTab::Sockets => match key {
                SelectedKey::SocketGroup(target_name) => {
                    let found = self.socket_items.iter().position(|item| match item {
                        SocketItem::Group { proc_name, .. } => proc_name.eq_ignore_ascii_case(target_name),
                        _ => false,
                    });
                    if let Some(i) = found {
                        self.selected_index = Some(i);
                    } else {
                        self.selected_index = None;
                        self.selected_key = None;
                    }
                }
                SelectedKey::SocketEntry { pid, proto, local_addr, remote_addr } => {
                    let found = self.socket_items.iter().position(|item| match item {
                        SocketItem::Child { socket_idx } | SocketItem::Single { socket_idx } => {
                            if let Some(s) = self.sockets.get(*socket_idx) {
                                s.pid == *pid
                                    && s.proto == proto
                                    && s.local_addr == *local_addr
                                    && s.remote_addr == *remote_addr
                            } else {
                                false
                            }
                        }
                        _ => false,
                    });
                    if let Some(i) = found {
                        self.selected_index = Some(i);
                    } else {
                        self.selected_index = None;
                        self.selected_key = None;
                    }
                }
                _ => {
                    self.selected_index = None;
                    self.selected_key = None;
                }
            },
            InspectTab::Services => match key {
                SelectedKey::Service(target_name) => {
                    let found = self.filtered_services.iter().position(|&svc_idx| {
                        self.services.get(svc_idx).map(|s| s.name.eq_ignore_ascii_case(target_name)).unwrap_or(false)
                    });
                    if let Some(i) = found {
                        self.selected_index = Some(i);
                    } else {
                        self.selected_index = None;
                        self.selected_key = None;
                    }
                }
                _ => {
                    self.selected_index = None;
                    self.selected_key = None;
                }
            },
            InspectTab::Info | InspectTab::Settings => {
                self.selected_index = None;
                self.selected_key = None;
            }
        }
    }

    fn set_active_tab(&mut self, tab: InspectTab) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        if tab == InspectTab::Info || tab == InspectTab::Settings {
            unsafe { ShowWindow(self.search_edit, SW_HIDE) };
        } else {
            unsafe { ShowWindow(self.search_edit, SW_SHOW) };
        }
        self.sort_col = match tab {
            InspectTab::Processes => 2,
            _ => 0,
        };
        self.sort_asc = tab != InspectTab::Processes;
        self.scroll_y = 0;
        self.set_selected_index(None);
        self.apply_sorting();
        self.apply_filter();
        self.clamp_scroll();
    }

    fn client_dimensions(&self) -> (i32, i32) {
        let mut rc: RECT = unsafe { zeroed() };
        unsafe { GetClientRect(self.hwnd, &mut rc) };
        (rc.right, rc.bottom)
    }

    fn max_scroll_y(&self, h: i32) -> i32 {
        let top_h = self.top_bar_height();
        let hdr_h = self.header_height();
        let btm_h = self.bottom_bar_height();
        let viewport_h = (h - top_h - hdr_h - btm_h).max(0);
        let content_h = self.row_count() as i32 * self.row_height();
        (content_h - viewport_h).max(0)
    }

    fn clamp_scroll(&mut self) {
        let (_, h) = self.client_dimensions();
        let max_s = self.max_scroll_y(h);
        self.scroll_y = self.scroll_y.clamp(0, max_s);
    }

    fn get_col_widths_mut(&mut self) -> &mut [i32] {
        match self.active_tab {
            InspectTab::Processes => &mut self.col_widths_procs,
            InspectTab::Sockets => &mut self.col_widths_sockets,
            InspectTab::Services => &mut self.col_widths_services,
            InspectTab::Info | InspectTab::Settings => &mut [],
        }
    }

    fn col_bounds(&self, total_w: i32) -> Vec<(i32, i32)> {
        let start_x = self.scale(14);
        let scrollbar_reserve = self.scale(22);
        let right_limit = (total_w - scrollbar_reserve).max(start_x + self.scale(600));
        let avail = (right_limit - start_x).max(self.scale(600));

        match self.active_tab {
            InspectTab::Processes => {
                let base_widths = [
                    self.scale(self.col_widths_procs[0]),
                    self.scale(self.col_widths_procs[1]),
                    self.scale(self.col_widths_procs[2]),
                    self.scale(self.col_widths_procs[3]),
                    self.scale(self.col_widths_procs[4]),
                    self.scale(self.col_widths_procs[5]),
                    self.scale(180),
                ];
                let base_sum: i32 = base_widths.iter().sum();
                let extra = (avail - base_sum).max(0);

                let w0 = base_widths[0] + (extra * 40) / 100;
                let w1 = base_widths[1];
                let w2 = base_widths[2];
                let w3 = base_widths[3] + (extra * 2) / 100;
                let w4 = base_widths[4] + (extra * 2) / 100;
                let w5 = base_widths[5];
                let w6 = (avail - (w0 + w1 + w2 + w3 + w4 + w5)).max(self.scale(140));

                let widths = [w0, w1, w2, w3, w4, w5, w6];
                let mut bounds = Vec::with_capacity(7);
                let mut cur = start_x;
                for &w in &widths {
                    bounds.push((cur, cur + w));
                    cur += w;
                }
                bounds
            }
            InspectTab::Sockets => {
                let base_widths = [
                    self.scale(self.col_widths_sockets[0]),
                    self.scale(self.col_widths_sockets[1]),
                    self.scale(self.col_widths_sockets[2]),
                    self.scale(self.col_widths_sockets[3]),
                    self.scale(self.col_widths_sockets[4]),
                    self.scale(110),
                ];
                let base_sum: i32 = base_widths.iter().sum();
                let extra = (avail - base_sum).max(0);

                let w0 = base_widths[0] + (extra * 25) / 100;
                let w1 = base_widths[1];
                let w2 = base_widths[2];
                let w3 = base_widths[3] + (extra * 35) / 100;
                let w4 = base_widths[4] + (extra * 35) / 100;
                let w5 = (avail - (w0 + w1 + w2 + w3 + w4)).max(self.scale(90));

                let widths = [w0, w1, w2, w3, w4, w5];
                let mut bounds = Vec::with_capacity(6);
                let mut cur = start_x;
                for &w in &widths {
                    bounds.push((cur, cur + w));
                    cur += w;
                }
                bounds
            }
            InspectTab::Services => {
                let base_widths = [
                    self.scale(self.col_widths_services[0]),
                    self.scale(self.col_widths_services[1]),
                    self.scale(self.col_widths_services[2]),
                    self.scale(self.col_widths_services[3]),
                    self.scale(100),
                ];
                let base_sum: i32 = base_widths.iter().sum();
                let extra = (avail - base_sum).max(0);

                let w0 = base_widths[0] + (extra * 35) / 100;
                let w2 = base_widths[2];
                let w3 = base_widths[3];
                let w4 = base_widths[4];
                let w1 = (avail - (w0 + w2 + w3 + w4)).max(self.scale(180));

                let widths = [w0, w1, w2, w3, w4];
                let mut bounds = Vec::with_capacity(5);
                let mut cur = start_x;
                for &w in &widths {
                    bounds.push((cur, cur + w));
                    cur += w;
                }
                bounds
            }
            InspectTab::Info | InspectTab::Settings => Vec::new(),
        }
    }
}

fn find_col_sep(bounds: &[(i32, i32)], x: i32) -> Option<usize> {
    for (i, &(_, x2)) in bounds.iter().take(bounds.len().saturating_sub(1)).enumerate() {
        if (x2 - 5..=x2 + 5).contains(&x) {
            return Some(i);
        }
    }
    None
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub struct InspectWindow {
    hwnd: HWND,
}

#[allow(clippy::manual_dangling_ptr)]
const fn make_int_resource(id: u16) -> *const u16 {
    id as usize as *const u16
}

impl InspectWindow {
    pub fn new(s: &'static Strings, dpi: u32) -> Option<Self> {
        let dpi = dpi.max(96);
        unsafe {
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSEXW = zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(inspect_wndproc);
            wc.hInstance = instance;
            wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
            let hicon_big = LoadImageW(instance, make_int_resource(1), IMAGE_ICON, 32, 32, LR_DEFAULTCOLOR);
            let hicon_sm = LoadImageW(instance, make_int_resource(1), IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR);
            if !hicon_big.is_null() {
                wc.hIcon = hicon_big as _;
            }
            if !hicon_sm.is_null() {
                wc.hIconSm = hicon_sm as _;
            }
            wc.lpszClassName = INSPECT_CLASS.as_ptr();
            let _ = RegisterClassExW(&wc);

            let mut work_area: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work_area as *mut _ as _, 0);
            let work_w = (work_area.right - work_area.left).max(800);
            let work_h = (work_area.bottom - work_area.top).max(600);

            let win_w = ((1040 * dpi as i32) / 96).clamp(840, (work_w - 40).max(600));
            let win_h = ((700 * dpi as i32) / 96).clamp(560, (work_h - 40).max(450));
            let win_x = work_area.left + (work_w - win_w) / 2;
            let win_y = work_area.top + (work_h - win_h) / 2;

            let title = if crate::sys::elevation::is_elevated() { s.inspect_title_admin } else { s.inspect_title };

            let hwnd = CreateWindowExW(
                0,
                INSPECT_CLASS.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                win_x,
                win_y,
                win_w,
                win_h,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );

            if hwnd.is_null() {
                return None;
            }

            if !hicon_big.is_null() {
                SendMessageW(hwnd, WM_SETICON, 1, hicon_big as LPARAM);
            }
            if !hicon_sm.is_null() {
                SendMessageW(hwnd, WM_SETICON, 0, hicon_sm as LPARAM);
            }

            let top_h = (46 * dpi as i32 + 48) / 96;
            let edit_h = (26 * dpi as i32 + 48) / 96;
            let edit_w = ((200 * dpi as i32) / 96).min(win_w / 4).max(120);
            let edit_x = win_w - edit_w - (14 * dpi as i32 + 48) / 96;
            let edit_y = (top_h - edit_h) / 2;

            let search_edit = CreateWindowExW(
                0,
                wide!("EDIT").as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | (ES_AUTOHSCROLL as u32),
                edit_x,
                edit_y,
                edit_w,
                edit_h,
                hwnd,
                IDC_SEARCH as usize as _,
                instance,
                null(),
            );

            SendMessageW(search_edit, 0x1501, 1, s.inspect_search_hint.as_ptr() as LPARAM);

            let actual_dpi = crate::sys::shell::dpi_of(hwnd).max(dpi);
            let fonts = InspectFonts::new(actual_dpi);
            SendMessageW(search_edit, WM_SETFONT, fonts.body as WPARAM, 1);

            let state = InspectState {
                hwnd,
                search_edit,
                engine: InspectEngine::new(),
                active_tab: InspectTab::Processes,
                procs: Vec::new(),
                proc_items: Vec::new(),
                group_procs: true,
                expanded_groups: BTreeSet::new(),
                sockets: Vec::new(),
                socket_items: Vec::new(),
                group_sockets: true,
                expanded_socket_groups: BTreeSet::new(),
                services: Vec::new(),
                filtered_services: Vec::new(),
                hung_windows: Vec::new(),
                hung_kill_rect: None,
                proc_mem_history: BTreeMap::new(),
                filter_text: String::new(),
                selected_index: None,
                selected_key: None,
                scroll_y: 0,
                hover_row: None,
                sort_col: 2,     // Default CPU %
                sort_asc: false, // Descending
                status_msg: None,
                is_dark: true,
                dpi: actual_dpi,
                fonts,
                s,
                col_widths_procs: [220, 80, 80, 110, 110, 70],
                col_widths_sockets: [200, 70, 60, 180, 180],
                col_widths_services: [200, 280, 120, 100],
                hover_col_sep: None,
                resizing_col: None,
                tab_rects: [zeroed(); 5],
                group_btn_rect: zeroed(),
                bottom_btn_rects: [zeroed(); 3],
                is_dragging_scrollbar: false,
                scrollbar_drag_start_y: 0,
                scrollbar_drag_start_scroll: 0,
                last_mouse_x: 0,
                last_mouse_y: 0,
                info_author_rect: zeroed(),
                info_repo_rect: zeroed(),
                info_update_btn_rect: zeroed(),
                info_download_btn_rect: zeroed(),
                info_card2_author_rect: zeroed(),
                edit_brush_dark: CreateSolidBrush(0x002B2B2B),
                edit_brush_light: CreateSolidBrush(0x00FFFFFF),
                icon_96: LoadImageW(instance, make_int_resource(1), IMAGE_ICON, 96, 96, LR_DEFAULTCOLOR) as HICON,
            };

            STATE.with(|cell| {
                *cell.borrow_mut() = Some(state);
            });

            apply_dwm_styling(hwnd, true);

            Some(InspectWindow { hwnd })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_visible(&self) -> bool {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.hwnd) != 0 }
    }

    pub fn show(&mut self, is_dark: bool) {
        STATE.with(|cell| {
            if let Some(st) = cell.borrow_mut().as_mut() {
                st.is_dark = is_dark;
                st.reload_data();
            }
        });

        unsafe {
            let mut wr: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut wr);
            let cur_w = wr.right - wr.left;
            let cur_h = wr.bottom - wr.top;
            let dpi = crate::sys::shell::dpi_of(self.hwnd).max(96);

            let mut work_area: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work_area as *mut _ as _, 0);
            let work_w = (work_area.right - work_area.left).max(800);
            let work_h = (work_area.bottom - work_area.top).max(600);
            let min_w = ((960 * dpi as i32) / 96).min(work_w - 40);
            let min_h = ((640 * dpi as i32) / 96).min(work_h - 40);

            let is_offscreen = wr.right <= work_area.left
                || wr.left >= work_area.right
                || wr.bottom <= work_area.top
                || wr.top >= work_area.bottom;

            if cur_w < min_w || cur_h < min_h || is_offscreen {
                let target_w = ((1040 * dpi as i32) / 96).clamp(840, (work_w - 40).max(600));
                let target_h = ((700 * dpi as i32) / 96).clamp(560, (work_h - 40).max(450));
                let win_x = work_area.left + (work_w - target_w) / 2;
                let win_y = work_area.top + (work_h - target_h) / 2;
                SetWindowPos(self.hwnd, HWND_TOP, win_x, win_y, target_w, target_h, SWP_SHOWWINDOW);
            }

            apply_dwm_styling(self.hwnd, is_dark);
            ShowWindow(self.hwnd, SW_RESTORE);
            ShowWindow(self.hwnd, SW_SHOW);
            SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, 0x0001 | 0x0002); // SWP_NOSIZE | SWP_NOMOVE
            SetForegroundWindow(self.hwnd);
            BringWindowToTop(self.hwnd);
            SetTimer(self.hwnd, TIMER_REFRESH, 1000, None);
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    pub fn show_tab(&mut self, tab: InspectTab, is_dark: bool) {
        STATE.with(|cell| {
            if let Some(st) = cell.borrow_mut().as_mut() {
                st.is_dark = is_dark;
                st.set_active_tab(tab);
                st.reload_data();
            }
        });

        unsafe {
            let mut wr: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut wr);
            let cur_w = wr.right - wr.left;
            let cur_h = wr.bottom - wr.top;
            let dpi = crate::sys::shell::dpi_of(self.hwnd).max(96);

            let mut work_area: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work_area as *mut _ as _, 0);
            let work_w = (work_area.right - work_area.left).max(800);
            let work_h = (work_area.bottom - work_area.top).max(600);
            let min_w = ((960 * dpi as i32) / 96).min(work_w - 40);
            let min_h = ((640 * dpi as i32) / 96).min(work_h - 40);

            let is_offscreen = wr.right <= work_area.left
                || wr.left >= work_area.right
                || wr.bottom <= work_area.top
                || wr.top >= work_area.bottom;

            if cur_w < min_w || cur_h < min_h || is_offscreen {
                let target_w = ((1040 * dpi as i32) / 96).clamp(840, (work_w - 40).max(600));
                let target_h = ((700 * dpi as i32) / 96).clamp(560, (work_h - 40).max(450));
                let win_x = work_area.left + (work_w - target_w) / 2;
                let win_y = work_area.top + (work_h - target_h) / 2;
                SetWindowPos(self.hwnd, HWND_TOP, win_x, win_y, target_w, target_h, SWP_SHOWWINDOW);
            }

            apply_dwm_styling(self.hwnd, is_dark);
            ShowWindow(self.hwnd, SW_RESTORE);
            ShowWindow(self.hwnd, SW_SHOW);
            SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, 0x0001 | 0x0002);
            SetForegroundWindow(self.hwnd);
            BringWindowToTop(self.hwnd);
            SetTimer(self.hwnd, TIMER_REFRESH, 1000, None);
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            KillTimer(self.hwnd, TIMER_REFRESH);
            ShowWindow(self.hwnd, SW_HIDE);
        }
        STATE.with(|cell| {
            if let Some(st) = cell.borrow_mut().as_mut() {
                st.release_memory();
            }
        });
    }

    pub fn toggle(&mut self, is_dark: bool) {
        let is_fg = unsafe { GetForegroundWindow() == self.hwnd };
        if self.is_visible() && is_fg {
            self.hide();
        } else {
            self.show(is_dark);
        }
    }

    pub fn update_strings(&mut self, s: &'static Strings) {
        let mut search_edit = null_mut();
        STATE.with(|cell| {
            if let Some(st) = cell.borrow_mut().as_mut() {
                st.s = s;
                st.reload_data();
                search_edit = st.search_edit;
            }
        });
        unsafe {
            let title = if crate::sys::elevation::is_elevated() { s.inspect_title_admin } else { s.inspect_title };
            SetWindowTextW(self.hwnd, title.as_ptr());
            if !search_edit.is_null() {
                SendMessageW(search_edit, 0x1501, 1, s.inspect_search_hint.as_ptr() as LPARAM);
            }
            InvalidateRect(self.hwnd, null(), 1);
        }
    }
}

impl Drop for InspectWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe {
                KillTimer(self.hwnd, TIMER_REFRESH);
                DestroyWindow(self.hwnd);
            }
        }
        STATE.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

unsafe fn apply_dwm_styling(hwnd: HWND, is_dark: bool) {
    let dwm = LoadLibraryExW(wide!("dwmapi.dll").as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
    if dwm.is_null() {
        return;
    }
    let p = GetProcAddress(dwm, c"DwmSetWindowAttribute".as_ptr().cast());
    if let Some(set_attr) = p {
        type FnDwmSetWindowAttribute = unsafe extern "system" fn(HWND, u32, *const core::ffi::c_void, u32) -> i32;
        let set_attr: FnDwmSetWindowAttribute = core::mem::transmute(set_attr);
        let corner: u32 = 2; // DWMWCP_ROUND
        set_attr(hwnd, 33, (&raw const corner).cast(), size_of::<u32>() as u32);
        let dark: u32 = if is_dark { 1 } else { 0 };
        set_attr(hwnd, 20, (&raw const dark).cast(), size_of::<u32>() as u32);
    }
    FreeLibrary(dwm);
}

unsafe extern "system" fn inspect_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        0x02E0 => {
            // WM_DPICHANGED: LOWORD(wparam) = X DPI, HIWORD(wparam) = Y DPI
            let new_dpi = (wparam & 0xFFFF) as u32;
            let prc = lparam as *const RECT;
            if !prc.is_null() {
                let r = unsafe { *prc };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            STATE.with(|cell| {
                if let Some(st) = cell.borrow_mut().as_mut() {
                    st.dpi = new_dpi;
                    unsafe {
                        st.fonts.update_dpi(new_dpi);
                        SendMessageW(st.search_edit, WM_SETFONT, st.fonts.body as WPARAM, 1);
                        let mut rc: RECT = zeroed();
                        GetClientRect(hwnd, &mut rc);
                        let w = rc.right;
                        let top_h = st.top_bar_height();
                        let edit_h = st.scale(26);
                        let edit_w = st.scale(200).min(w / 4).max(120);
                        let edit_x = w - edit_w - st.scale(14);
                        let edit_y = (top_h - edit_h) / 2;
                        SetWindowPos(
                            st.search_edit,
                            null_mut(),
                            edit_x,
                            edit_y,
                            edit_w,
                            edit_h,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let hdc = wparam as HDC;
            let (dark, brush) = STATE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| (s.is_dark, if s.is_dark { s.edit_brush_dark } else { s.edit_brush_light }))
                    .unwrap_or((true, null_mut()))
            });
            if dark {
                SetTextColor(hdc, 0x00E0E0E0);
                SetBkColor(hdc, 0x002B2B2B);
            } else {
                SetTextColor(hdc, 0x00101010);
                SetBkColor(hdc, 0x00FFFFFF);
            }
            brush as LRESULT
        }
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            let dpi = crate::sys::shell::dpi_of(hwnd).max(96);
            mmi.ptMinTrackSize.x = ((780 * dpi as i32) / 96).max(640);
            mmi.ptMinTrackSize.y = ((480 * dpi as i32) / 96).max(400);
            0
        }
        WM_SIZE => {
            let w = (lparam & 0xFFFF) as i32;
            let _h = ((lparam >> 16) & 0xFFFF) as i32;
            STATE.with(|cell| {
                if let Some(st) = cell.borrow().as_ref() {
                    let edit_w = st.scale(220).min(w - st.scale(550)).max(st.scale(120));
                    let edit_x = w - edit_w - st.scale(14);
                    let edit_h = st.scale(26);
                    let edit_y = (st.top_bar_height() - edit_h) / 2;
                    unsafe { SetWindowPos(st.search_edit, null_mut(), edit_x, edit_y, edit_w, edit_h, SWP_NOZORDER) };
                }
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as u32;
            let code = ((wparam >> 16) & 0xFFFF) as u16;
            if id == IDC_SEARCH && code == 0x0300 {
                // EN_CHANGE
                STATE.with(|cell| {
                    if let Some(st) = cell.borrow_mut().as_mut() {
                        let len = GetWindowTextLengthW(st.search_edit) as usize;
                        let mut buf = vec![0u16; len + 1];
                        GetWindowTextW(st.search_edit, buf.as_mut_ptr(), (len + 1) as i32);
                        buf.truncate(len);
                        st.filter_text = String::from_utf16_lossy(&buf);
                        st.scroll_y = 0;
                        st.selected_index = None;
                        st.apply_filter();
                    }
                });
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_TIMER => {
            if wparam == TIMER_REFRESH {
                STATE.with(|cell| {
                    if let Some(st) = cell.borrow_mut().as_mut() {
                        st.reload_data();
                    }
                });
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) as i16) as i32;
            let mut rc: RECT = unsafe { zeroed() };
            unsafe { GetClientRect(hwnd, &mut rc) };
            STATE.with(|cell| {
                if let Some(st) = cell.borrow_mut().as_mut() {
                    let line_h = st.row_height();
                    let max_s = st.max_scroll_y(rc.bottom);
                    st.scroll_y = (st.scroll_y - (delta / 120) * line_h * 3).clamp(0, max_s);
                }
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_mouse_down(hwnd, x, y);
            0
        }
        WM_LBUTTONUP => {
            on_mouse_up(hwnd);
            0
        }
        WM_RBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_right_click(hwnd, x, y);
            0
        }
        WM_LBUTTONDBLCLK => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_mouse_dblclk(hwnd, x, y);
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            on_mouse_move(hwnd, x, y);
            0
        }
        WM_SETCURSOR => {
            let hit_test = (lparam & 0xFFFF) as u32;
            if hit_test == HTCLIENT {
                let cursor = STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|st| {
                            let top_h = st.top_bar_height();
                            let hdr_h = st.header_height();
                            if st.resizing_col.is_some()
                                || (st.active_tab != InspectTab::Info
                                    && st.active_tab != InspectTab::Settings
                                    && (top_h..top_h + hdr_h).contains(&st.last_mouse_y)
                                    && st.hover_col_sep.is_some())
                            {
                                IDC_SIZEWE
                            } else if st.active_tab == InspectTab::Info {
                                let x = st.last_mouse_x;
                                let y = st.last_mouse_y;
                                if ((st.info_author_rect.left..=st.info_author_rect.right).contains(&x)
                                    && (st.info_author_rect.top..=st.info_author_rect.bottom).contains(&y))
                                    || ((st.info_repo_rect.left..=st.info_repo_rect.right).contains(&x)
                                        && (st.info_repo_rect.top..=st.info_repo_rect.bottom).contains(&y))
                                    || ((st.info_update_btn_rect.left..=st.info_update_btn_rect.right).contains(&x)
                                        && (st.info_update_btn_rect.top..=st.info_update_btn_rect.bottom).contains(&y))
                                    || (st.info_download_btn_rect.right > 0
                                        && (st.info_download_btn_rect.left..=st.info_download_btn_rect.right)
                                            .contains(&x)
                                        && (st.info_download_btn_rect.top..=st.info_download_btn_rect.bottom)
                                            .contains(&y))
                                    || (st.info_card2_author_rect.right > 0
                                        && (st.info_card2_author_rect.left..=st.info_card2_author_rect.right)
                                            .contains(&x)
                                        && (st.info_card2_author_rect.top..=st.info_card2_author_rect.bottom)
                                            .contains(&y))
                                {
                                    IDC_HAND
                                } else {
                                    IDC_ARROW
                                }
                            } else {
                                IDC_ARROW
                            }
                        })
                        .unwrap_or(IDC_ARROW)
                });
                unsafe { SetCursor(LoadCursorW(null_mut(), cursor)) };
                return 1;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYDOWN => {
            on_key_down(hwnd, wparam as u32);
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_null() {
                paint_window(hwnd, hdc);
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_CLOSE => {
            KillTimer(hwnd, TIMER_REFRESH);
            ShowWindow(hwnd, SW_HIDE);
            STATE.with(|cell| {
                if let Some(st) = cell.borrow_mut().as_mut() {
                    st.release_memory();
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn copy_to_clipboard(hwnd: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(core::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if hmem.is_null() {
            return false;
        }
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            return false;
        }
        core::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr as *mut u8, bytes);
        GlobalUnlock(hmem);

        if OpenClipboard(hwnd) == 0 {
            return false;
        }
        EmptyClipboard();
        const CF_UNICODETEXT: u32 = 13;
        let res = SetClipboardData(CF_UNICODETEXT, hmem as _);
        CloseClipboard();
        !res.is_null()
    }
}

fn open_process_file_location(pid: u32) -> bool {
    if pid == 0 || pid == 4 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut path_buf = [0u16; 1024];
        let mut len = path_buf.len() as u32;
        let success = QueryFullProcessImageNameW(handle, 0, path_buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if success != 0 && len > 0 {
            let path_str = String::from_utf16_lossy(&path_buf[..len as usize]);
            let _ = std::process::Command::new("explorer.exe").arg(format!("/select,\"{}\"", path_str)).spawn();
            return true;
        }
    }
    false
}

fn on_mouse_down(hwnd: HWND, x: i32, y: i32) {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;

    let mut needs_repaint = false;

    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };

        let top_h = st.top_bar_height();
        let hdr_h = st.header_height();
        let btm_h = st.bottom_bar_height();
        let row_h = st.row_height();
        let table_top = top_h + hdr_h;
        let table_bottom = h - btm_h;
        let sb_reserve = st.scale(20);

        // 1. Click sulle Tab o sul pulsante Raggruppa in alto (y: 0..top_h)
        if (0..top_h).contains(&y) {
            if let Some((rect, pid)) = st.hung_kill_rect
                && (rect.left..rect.right).contains(&x)
                && (rect.top..rect.bottom).contains(&y)
                && crate::sys::hung::quick_terminate_process(pid)
            {
                let is_it = st.is_italian();
                let msg = if is_it {
                    format!("Processo bloccato (PID {pid}) terminato!")
                } else {
                    format!("Hung process (PID {pid}) terminated!")
                };
                st.set_status(msg);
                st.reload_data();
                st.apply_filter();
                needs_repaint = true;
                return;
            }

            let gr = &st.group_btn_rect;
            if gr.left > 0
                && (st.active_tab == InspectTab::Processes || st.active_tab == InspectTab::Sockets)
                && (gr.left..gr.right).contains(&x)
                && (gr.top..gr.bottom).contains(&y)
            {
                if st.active_tab == InspectTab::Processes {
                    st.group_procs = !st.group_procs;
                    st.update_proc_items();
                } else {
                    st.group_sockets = !st.group_sockets;
                    st.update_socket_items();
                }
                st.clamp_scroll();
                needs_repaint = true;
                return;
            }

            for (idx, r) in st.tab_rects.iter().enumerate() {
                if (r.left..r.right).contains(&x) && (r.top..r.bottom).contains(&y) {
                    match idx {
                        0 => st.set_active_tab(InspectTab::Processes),
                        1 => st.set_active_tab(InspectTab::Sockets),
                        2 => st.set_active_tab(InspectTab::Services),
                        3 => st.set_active_tab(InspectTab::Settings),
                        4 => st.set_active_tab(InspectTab::Info),
                        _ => {}
                    }
                    needs_repaint = true;
                    return;
                }
            }
            return;
        }

        // 2. Click sulle colonne dell'header della tabella (y: top_h..table_top)
        if st.active_tab != InspectTab::Info && st.active_tab != InspectTab::Settings && (top_h..table_top).contains(&y)
        {
            let bounds = st.col_bounds(w);
            if let Some(sep_idx) = find_col_sep(&bounds, x) {
                let start_w = bounds[sep_idx].1 - bounds[sep_idx].0;
                st.resizing_col = Some((sep_idx, x, start_w));
                unsafe { SetCapture(hwnd) };
                return;
            }

            let col_clicked = bounds.iter().position(|&(x1, x2)| x >= x1 && x < x2);
            if let Some(col) = col_clicked {
                if st.sort_col == col {
                    st.sort_asc = !st.sort_asc;
                } else {
                    st.sort_col = col;
                    st.sort_asc = col != 2; // CPU % default desc, altre default asc
                }
                st.apply_sorting();
                st.apply_filter();
                needs_repaint = true;
            }
            return;
        }

        // 3. Scrollbar a destra
        let max_s = st.max_scroll_y(h);
        if max_s > 0 && (w - sb_reserve..w).contains(&x) && (table_top..table_bottom).contains(&y) {
            let viewport_h = (table_bottom - table_top).max(1);
            let total_h = st.row_count() as i32 * row_h;
            let thumb_h = (((viewport_h as f32) / (total_h as f32)) * viewport_h as f32) as i32;
            let thumb_h = thumb_h.clamp(st.scale(24).max(20), viewport_h);
            let avail = (viewport_h - thumb_h).max(1);
            let thumb_y = table_top + ((st.scroll_y as f32 / max_s as f32) * avail as f32) as i32;

            if y >= thumb_y && y <= thumb_y + thumb_h {
                st.is_dragging_scrollbar = true;
                st.scrollbar_drag_start_y = y;
                st.scrollbar_drag_start_scroll = st.scroll_y;
                unsafe { SetCapture(hwnd) };
            } else if y < thumb_y {
                st.scroll_y = (st.scroll_y - viewport_h).max(0);
                needs_repaint = true;
            } else {
                st.scroll_y = (st.scroll_y + viewport_h).min(max_s);
                needs_repaint = true;
            }
            return;
        }

        // 4. Click sulle righe della tabella (y: table_top..table_bottom)
        if st.active_tab != InspectTab::Info
            && st.active_tab != InspectTab::Settings
            && (table_top..table_bottom).contains(&y)
            && x < (w - sb_reserve)
        {
            let clicked_idx = ((y - table_top + st.scroll_y) / row_h) as usize;
            if clicked_idx < st.row_count() {
                if st.active_tab == InspectTab::Processes {
                    let bounds = st.col_bounds(w);
                    if let Some(ProcItem::Group { name, .. }) = st.proc_items.get(clicked_idx) {
                        // Click sulla freccia di espansione
                        if x >= bounds[0].0 && x < bounds[0].0 + st.scale(35) {
                            let key = name.to_lowercase();
                            if st.expanded_groups.contains(&key) {
                                st.expanded_groups.remove(&key);
                            } else {
                                st.expanded_groups.insert(key);
                            }
                            st.update_proc_items();
                            st.restore_selection_index();
                            st.clamp_scroll();
                            needs_repaint = true;
                            return;
                        }
                    }
                } else if st.active_tab == InspectTab::Sockets {
                    let bounds = st.col_bounds(w);
                    if let Some(SocketItem::Group { proc_name, .. }) = st.socket_items.get(clicked_idx) {
                        // Click sulla freccia di espansione
                        if x >= bounds[0].0 && x < bounds[0].0 + st.scale(35) {
                            let key = proc_name.to_lowercase();
                            if st.expanded_socket_groups.contains(&key) {
                                st.expanded_socket_groups.remove(&key);
                            } else {
                                st.expanded_socket_groups.insert(key);
                            }
                            st.update_socket_items();
                            st.restore_selection_index();
                            st.clamp_scroll();
                            needs_repaint = true;
                            return;
                        }
                    }
                }
                st.set_selected_index(Some(clicked_idx));
                needs_repaint = true;
            } else {
                st.set_selected_index(None);
                needs_repaint = true;
            }
            return;
        }

        // 5. Click sui bottoni in basso (y: table_bottom..h)
        if y >= table_bottom {
            let b1 = &st.bottom_btn_rects[0];
            if (b1.left..b1.right).contains(&x) && (b1.top..b1.bottom).contains(&y) {
                match st.active_tab {
                    InspectTab::Processes => {
                        st.kill_selected();
                        needs_repaint = true;
                    }
                    InspectTab::Sockets => {
                        let sel_item = st.selected_socket_item().cloned();
                        let is_it = st.is_italian();
                        match sel_item {
                            Some(SocketItem::Group { proc_name, socket_indices, .. }) => {
                                let mut closed = 0;
                                for idx in socket_indices {
                                    if let Some(s) = st.sockets.get(idx)
                                        && let (Some(loc_ip), Some(rem_ip)) = (s.raw_local_ip_v4, s.raw_remote_ip_v4)
                                        && st.engine.close_tcp_v4(loc_ip, s.raw_local_port, rem_ip, s.raw_remote_port)
                                    {
                                        closed += 1;
                                    }
                                }
                                if is_it {
                                    st.set_status(format!("Chiuse {closed} connessioni TCP di {proc_name}"));
                                } else {
                                    st.set_status(format!("Closed {closed} TCP connections of {proc_name}"));
                                }
                                st.reload_data();
                                needs_repaint = true;
                            }
                            Some(SocketItem::Child { socket_idx } | SocketItem::Single { socket_idx }) => {
                                let ips = st.sockets.get(socket_idx).and_then(|s| {
                                    match (s.raw_local_ip_v4, s.raw_remote_ip_v4) {
                                        (Some(loc_ip), Some(rem_ip)) => {
                                            Some((loc_ip, s.raw_local_port, rem_ip, s.raw_remote_port))
                                        }
                                        _ => None,
                                    }
                                });
                                if let Some((loc_ip, loc_port, rem_ip, rem_port)) = ips {
                                    if st.engine.close_tcp_v4(loc_ip, loc_port, rem_ip, rem_port) {
                                        st.set_status(if is_it {
                                            "Connessione TCP chiusa".to_string()
                                        } else {
                                            "TCP connection closed".to_string()
                                        });
                                    } else {
                                        st.set_status(if is_it {
                                            "Impossibile chiudere connessione".to_string()
                                        } else {
                                            "Unable to close connection".to_string()
                                        });
                                    }
                                    st.reload_data();
                                    needs_repaint = true;
                                }
                            }
                            None => {}
                        }
                    }
                    InspectTab::Services => {
                        if let Some(s) = st.selected_service() {
                            let name = s.name.clone();
                            let is_it = st.is_italian();
                            if s.is_running {
                                if st.engine.stop_service(&name) {
                                    st.set_status(if is_it {
                                        format!("Servizio arrestato: {name}")
                                    } else {
                                        format!("Service stopped: {name}")
                                    });
                                } else {
                                    st.set_status(if is_it {
                                        format!("Impossibile arrestare: {name}")
                                    } else {
                                        format!("Unable to stop: {name}")
                                    });
                                }
                            } else if st.engine.start_service(&name) {
                                st.set_status(if is_it {
                                    format!("Servizio avviato: {name}")
                                } else {
                                    format!("Service started: {name}")
                                });
                            } else {
                                st.set_status(if is_it {
                                    format!("Impossibile avviare: {name}")
                                } else {
                                    format!("Unable to start: {name}")
                                });
                            }
                            st.reload_data();
                            needs_repaint = true;
                        }
                    }
                    InspectTab::Info => {
                        crate::sys::shell::open_task_manager();
                    }
                    InspectTab::Settings => {}
                }
                return;
            }

            let b2 = &st.bottom_btn_rects[1];
            if (b2.left..b2.right).contains(&x) && (b2.top..b2.bottom).contains(&y) {
                st.reload_data();
                let is_it = st.is_italian();
                st.set_status(if is_it { "Dati aggiornati".to_string() } else { "Data refreshed".to_string() });
                needs_repaint = true;
                return;
            }

            let b3 = &st.bottom_btn_rects[2];
            if b3.left > 0
                && (b3.left..b3.right).contains(&x)
                && (b3.top..b3.bottom).contains(&y)
                && !crate::sys::elevation::is_elevated()
            {
                drop(guard);
                crate::app::execute_command(crate::app::CMD_RESTART_ADMIN);
                return;
            }
            return;
        }

        // 6. Click nel corpo della pagina Impostazioni
        if st.active_tab == InspectTab::Settings {
            let dpi = st.dpi;
            drop(guard);
            if handle_settings_click(x, y, w, h, dpi) {
                needs_repaint = true;
            }
        } else if st.active_tab == InspectTab::Info {
            drop(guard);
            if handle_info_click(hwnd, x, y) {
                needs_repaint = true;
            }
        }
    });

    if needs_repaint {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn on_mouse_dblclk(hwnd: HWND, _x: i32, y: i32) {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    let h = rc.bottom;

    let mut switched = false;
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        if st.active_tab == InspectTab::Info || st.active_tab == InspectTab::Settings {
            return;
        };

        let top_h = st.top_bar_height();
        let hdr_h = st.header_height();
        let btm_h = st.bottom_bar_height();
        let row_h = st.row_height();
        let table_top = top_h + hdr_h;
        let table_bottom = h - btm_h;

        // Doppio clic sull'header tabella -> ripristina organizzazione colonne predefinita!
        if (top_h..table_top).contains(&y) {
            st.col_widths_procs = [220, 80, 80, 110, 110, 70];
            st.col_widths_sockets = [200, 70, 60, 180, 180];
            st.col_widths_services = [200, 280, 120, 100];
            switched = true;
            return;
        }

        if (table_top..table_bottom).contains(&y) {
            let clicked_idx = ((y - table_top + st.scroll_y) / row_h) as usize;

            if st.active_tab == InspectTab::Processes {
                let Some(item) = st.proc_items.get(clicked_idx).cloned() else { return };
                match item {
                    ProcItem::Group { name, .. } => {
                        let key = name.to_lowercase();
                        if st.expanded_groups.contains(&key) {
                            st.expanded_groups.remove(&key);
                        } else {
                            st.expanded_groups.insert(key);
                        }
                        st.update_proc_items();
                        st.clamp_scroll();
                        switched = true;
                    }
                    ProcItem::Child { proc_idx, .. } | ProcItem::Single { proc_idx } => {
                        if let Some(p) = st.procs.get(proc_idx) {
                            let pid = p.pid;
                            if p.sockets_count > 0 {
                                st.set_active_tab(InspectTab::Sockets);
                                st.filter_text = pid.to_string();
                                unsafe {
                                    let wstr: Vec<u16> =
                                        st.filter_text.encode_utf16().chain(core::iter::once(0)).collect();
                                    SetWindowTextW(st.search_edit, wstr.as_ptr());
                                }
                                st.scroll_y = 0;
                                st.selected_index = None;
                                st.apply_filter();
                                switched = true;
                            } else if !p.services_summary.is_empty() {
                                st.set_active_tab(InspectTab::Services);
                                st.filter_text = pid.to_string();
                                unsafe {
                                    let wstr: Vec<u16> =
                                        st.filter_text.encode_utf16().chain(core::iter::once(0)).collect();
                                    SetWindowTextW(st.search_edit, wstr.as_ptr());
                                }
                                st.scroll_y = 0;
                                st.selected_index = None;
                                st.apply_filter();
                                switched = true;
                            }
                        }
                    }
                }
            } else if st.active_tab == InspectTab::Sockets {
                let Some(item) = st.socket_items.get(clicked_idx).cloned() else { return };
                match item {
                    SocketItem::Group { proc_name, .. } => {
                        let key = proc_name.to_lowercase();
                        if st.expanded_socket_groups.contains(&key) {
                            st.expanded_socket_groups.remove(&key);
                        } else {
                            st.expanded_socket_groups.insert(key);
                        }
                        st.update_socket_items();
                        st.clamp_scroll();
                        switched = true;
                    }
                    SocketItem::Child { socket_idx } | SocketItem::Single { socket_idx } => {
                        if let Some(s) = st.sockets.get(socket_idx) {
                            let pid = s.pid;
                            st.set_active_tab(InspectTab::Processes);
                            st.filter_text = pid.to_string();
                            unsafe {
                                let wstr: Vec<u16> = st.filter_text.encode_utf16().chain(core::iter::once(0)).collect();
                                SetWindowTextW(st.search_edit, wstr.as_ptr());
                            }
                            st.scroll_y = 0;
                            st.selected_index = None;
                            st.apply_filter();
                            switched = true;
                        }
                    }
                }
            }
        }
    });

    if switched {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn on_right_click(hwnd: HWND, x: i32, y: i32) {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;

    let mut clicked_idx = None;
    let mut is_processes = false;
    let mut is_sockets = false;
    let mut is_services = false;
    let mut is_it = false;

    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        if st.active_tab == InspectTab::Info || st.active_tab == InspectTab::Settings {
            return;
        }

        let top_h = st.top_bar_height();
        let hdr_h = st.header_height();
        let btm_h = st.bottom_bar_height();
        let row_h = st.row_height();
        let table_top = top_h + hdr_h;
        let table_bottom = h - btm_h;
        let sb_reserve = st.scale(20);

        if !(table_top..table_bottom).contains(&y) || x >= (w - sb_reserve) {
            return;
        }

        is_it = st.is_italian();
        let idx = ((y - table_top + st.scroll_y) / row_h) as usize;
        if idx < st.row_count() {
            st.set_selected_index(Some(idx));
            clicked_idx = Some(idx);
            is_processes = st.active_tab == InspectTab::Processes;
            is_sockets = st.active_tab == InspectTab::Sockets;
            is_services = st.active_tab == InspectTab::Services;
        }
    });

    if clicked_idx.is_none() {
        return;
    }

    unsafe { InvalidateRect(hwnd, null(), 0) };

    const IDM_SUSPEND: usize = 2001;
    const IDM_RESUME: usize = 2002;
    const IDM_ECOQOS_ON: usize = 2003;
    const IDM_ECOQOS_OFF: usize = 2004;
    const IDM_KILL: usize = 2005;
    const IDM_OPEN_LOCATION: usize = 2006;
    const IDM_COPY_PID: usize = 2007;

    const IDM_SOCK_CLOSE: usize = 2010;
    const IDM_SOCK_COPY_REMOTE: usize = 2011;
    const IDM_SOCK_GEOIP: usize = 2012;

    const IDM_SVC_START: usize = 2020;
    const IDM_SVC_STOP: usize = 2021;
    const IDM_SVC_COPY_NAME: usize = 2022;

    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() {
            return;
        }

        let append_item = |id: usize, text: &str| {
            let wstr: Vec<u16> = text.encode_utf16().chain(core::iter::once(0)).collect();
            AppendMenuW(hmenu, MF_STRING, id, wstr.as_ptr());
        };
        let append_sep = || {
            AppendMenuW(hmenu, MF_SEPARATOR, 0, null());
        };

        if is_processes {
            append_item(IDM_SUSPEND, if is_it { "⏸ Sospendi processo" } else { "⏸ Suspend process" });
            append_item(IDM_RESUME, if is_it { "▶ Ripristina processo" } else { "▶ Resume process" });
            append_sep();
            append_item(
                IDM_ECOQOS_ON,
                if is_it { "🍃 Modalità Efficienza: Attiva" } else { "🍃 Efficiency Mode: Enable" },
            );
            append_item(
                IDM_ECOQOS_OFF,
                if is_it { "⚡ Modalità Efficienza: Disattiva" } else { "⚡ Efficiency Mode: Disable" },
            );
            append_sep();
            append_item(IDM_KILL, if is_it { "❌ Termina processo (Canc)" } else { "❌ End process (Del)" });
            append_sep();
            append_item(IDM_OPEN_LOCATION, if is_it { "📁 Apri percorso file" } else { "📁 Open file location" });
            append_item(IDM_COPY_PID, if is_it { "📋 Copia PID" } else { "📋 Copy PID" });
        } else if is_sockets {
            append_item(IDM_SOCK_CLOSE, if is_it { "❌ Chiudi connessione TCP" } else { "❌ Close TCP connection" });
            append_sep();
            append_item(
                IDM_SOCK_COPY_REMOTE,
                if is_it { "📋 Copia indirizzo remoto" } else { "📋 Copy remote address" },
            );
            append_item(
                IDM_SOCK_GEOIP,
                if is_it { "🌐 Cerca WHOIS / Geo-IP (ipinfo.io)" } else { "🌐 Lookup WHOIS / Geo-IP (ipinfo.io)" },
            );
        } else if is_services {
            append_item(IDM_SVC_START, if is_it { "▶ Avvia servizio" } else { "▶ Start service" });
            append_item(IDM_SVC_STOP, if is_it { "⏹ Arresta servizio" } else { "⏹ Stop service" });
            append_sep();
            append_item(IDM_SVC_COPY_NAME, if is_it { "📋 Copia nome servizio" } else { "📋 Copy service name" });
        }

        let mut pt = POINT { x, y };
        ClientToScreen(hwnd, &mut pt);

        let cmd =
            TrackPopupMenu(hmenu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null()) as usize;
        DestroyMenu(hmenu);

        if cmd == 0 {
            return;
        }

        STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(st) = guard.as_mut() else { return };
            match cmd {
                IDM_SUSPEND => st.suspend_selected(),
                IDM_RESUME => st.resume_selected(),
                IDM_ECOQOS_ON => st.ecoqos_selected(true),
                IDM_ECOQOS_OFF => st.ecoqos_selected(false),
                IDM_KILL => st.kill_selected(),
                IDM_OPEN_LOCATION => st.open_location_selected(),
                IDM_COPY_PID => st.copy_pid_selected(hwnd),
                IDM_SOCK_CLOSE => {
                    if let Some(SocketItem::Single { socket_idx } | SocketItem::Child { socket_idx }) =
                        st.selected_socket_item().cloned()
                        && let Some(s) = st.sockets.get(socket_idx)
                        && let (Some(loc_ip), Some(rem_ip)) = (s.raw_local_ip_v4, s.raw_remote_ip_v4)
                    {
                        if st.engine.close_tcp_v4(loc_ip, s.raw_local_port, rem_ip, s.raw_remote_port) {
                            let msg = if is_it { "Connessione TCP chiusa" } else { "TCP connection closed" };
                            st.set_status(msg.to_string());
                            st.reload_data();
                        } else {
                            let msg = if is_it {
                                "Impossibile chiudere connessione (richiede Admin)"
                            } else {
                                "Unable to close connection (requires Admin)"
                            };
                            st.set_status(msg.to_string());
                        }
                    }
                }
                IDM_SOCK_COPY_REMOTE => {
                    if let Some(SocketItem::Single { socket_idx } | SocketItem::Child { socket_idx }) =
                        st.selected_socket_item()
                        && let Some(s) = st.sockets.get(*socket_idx)
                    {
                        copy_to_clipboard(hwnd, &s.remote_addr);
                        let msg = if is_it { "Indirizzo remoto copiato" } else { "Remote address copied" };
                        st.set_status(msg.to_string());
                    }
                }
                IDM_SOCK_GEOIP => {
                    if let Some(SocketItem::Single { socket_idx } | SocketItem::Child { socket_idx }) =
                        st.selected_socket_item()
                        && let Some(s) = st.sockets.get(*socket_idx)
                        && let Some(ip) = &s.remote_ip
                    {
                        let url = format!("https://ipinfo.io/{ip}");
                        let url_wstr: Vec<u16> = url.encode_utf16().chain(core::iter::once(0)).collect();
                        crate::sys::shell::open_with_explorer(&url_wstr);
                        let msg = if is_it {
                            format!("Apertura Geo-IP per {ip}")
                        } else {
                            format!("Opening Geo-IP lookup for {ip}")
                        };
                        st.set_status(msg);
                    }
                }
                IDM_SVC_START => {
                    if let Some(svc) = st.selected_service().cloned() {
                        if st.engine.start_service(&svc.name) {
                            let msg = if is_it {
                                format!("Servizio avviato: {}", svc.name)
                            } else {
                                format!("Service started: {}", svc.name)
                            };
                            st.set_status(msg);
                            st.reload_data();
                        } else {
                            let msg = if is_it {
                                format!("Impossibile avviare {} (richiede Admin)", svc.name)
                            } else {
                                format!("Unable to start {} (requires Admin)", svc.name)
                            };
                            st.set_status(msg);
                        }
                    }
                }
                IDM_SVC_STOP => {
                    if let Some(svc) = st.selected_service().cloned() {
                        if st.engine.stop_service(&svc.name) {
                            let msg = if is_it {
                                format!("Servizio arrestato: {}", svc.name)
                            } else {
                                format!("Service stopped: {}", svc.name)
                            };
                            st.set_status(msg);
                            st.reload_data();
                        } else {
                            let msg = if is_it {
                                format!("Impossibile arrestare {} (richiede Admin)", svc.name)
                            } else {
                                format!("Unable to stop {} (requires Admin)", svc.name)
                            };
                            st.set_status(msg);
                        }
                    }
                }
                IDM_SVC_COPY_NAME => {
                    if let Some(svc) = st.selected_service() {
                        copy_to_clipboard(hwnd, &svc.name);
                        let msg = if is_it {
                            format!("Nome servizio copiato: {}", svc.name)
                        } else {
                            format!("Service name copied: {}", svc.name)
                        };
                        st.set_status(msg);
                    }
                }
                _ => {}
            }
        });
        InvalidateRect(hwnd, null(), 0);
    }
}

fn on_mouse_move(hwnd: HWND, x: i32, y: i32) {
    let mut rc: RECT = unsafe { zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;

    let mut changed = false;
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        st.last_mouse_x = x;
        st.last_mouse_y = y;

        let top_h = st.top_bar_height();
        let hdr_h = st.header_height();
        let btm_h = st.bottom_bar_height();
        let row_h = st.row_height();
        let table_top = top_h + hdr_h;
        let table_bottom = h - btm_h;
        let sb_reserve = st.scale(20);

        // Trascinamento barra di scorrimento
        if st.is_dragging_scrollbar {
            let max_s = st.max_scroll_y(h);
            if max_s > 0 {
                let viewport_h = (table_bottom - table_top).max(1);
                let total_h = st.row_count() as i32 * row_h;
                let thumb_h = (((viewport_h as f32) / (total_h as f32)) * viewport_h as f32) as i32;
                let thumb_h = thumb_h.clamp(st.scale(24).max(20), viewport_h);
                let avail = (viewport_h - thumb_h).max(1);
                let dy = y - st.scrollbar_drag_start_y;
                let delta = ((dy as f32 / avail as f32) * max_s as f32) as i32;
                st.scroll_y = (st.scrollbar_drag_start_scroll + delta).clamp(0, max_s);
                changed = true;
            }
            return;
        }

        // Trascinamento ridimensionamento colonna
        if let Some((col_idx, start_x, start_w)) = st.resizing_col {
            let dx = x - start_x;
            let new_pixel_w = (start_w + dx).max(st.scale(35));
            let new_base_w = (new_pixel_w * 96 + (st.dpi as i32 / 2)) / st.dpi as i32;
            if col_idx < st.get_col_widths_mut().len() {
                st.get_col_widths_mut()[col_idx] = new_base_w;
            }
            changed = true;
            return;
        }

        // Hover su separatore colonna (y: top_h..table_top)
        if st.active_tab != InspectTab::Info && st.active_tab != InspectTab::Settings && (top_h..table_top).contains(&y)
        {
            let bounds = st.col_bounds(w);
            let sep = find_col_sep(&bounds, x);
            if st.hover_col_sep != sep {
                st.hover_col_sep = sep;
                changed = true;
            }
        } else if st.hover_col_sep.is_some() {
            st.hover_col_sep = None;
            changed = true;
        }

        // Hover su riga tabella
        let new_hover = if st.active_tab != InspectTab::Info
            && st.active_tab != InspectTab::Settings
            && (table_top..table_bottom).contains(&y)
            && x < (w - sb_reserve)
        {
            let idx = ((y - table_top + st.scroll_y) / row_h) as usize;
            if idx < st.row_count() { Some(idx) } else { None }
        } else {
            None
        };

        if st.hover_row != new_hover {
            st.hover_row = new_hover;
            changed = true;
        }
    });

    if changed {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn on_mouse_up(hwnd: HWND) {
    let mut changed = false;
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };
        if st.is_dragging_scrollbar || st.resizing_col.is_some() {
            st.is_dragging_scrollbar = false;
            st.resizing_col = None;
            unsafe { ReleaseCapture() };
            changed = true;
        }
    });

    if changed {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn on_key_down(hwnd: HWND, key: u32) {
    let mut repaint = false;
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };

        match key {
            0x71 => {
                st.set_active_tab(InspectTab::Settings);
                repaint = true;
            }
            VK_F5 => {
                st.reload_data();
                let is_it = st.is_italian();
                st.set_status(if is_it { "Aggiornato".to_string() } else { "Refreshed".to_string() });
                repaint = true;
            }
            VK_ESCAPE => unsafe {
                KillTimer(hwnd, TIMER_REFRESH);
                ShowWindow(hwnd, SW_HIDE);
            },
            VK_UP => {
                if let Some(curr) = st.selected_index {
                    if curr > 0 {
                        st.set_selected_index(Some(curr - 1));
                        let row_top = (curr - 1) as i32 * 24;
                        if row_top < st.scroll_y {
                            st.scroll_y = row_top;
                        }
                        st.clamp_scroll();
                        repaint = true;
                    }
                } else if st.row_count() > 0 {
                    st.set_selected_index(Some(0));
                    st.scroll_y = 0;
                    repaint = true;
                }
            }
            VK_DOWN => {
                let max = st.row_count();
                if let Some(curr) = st.selected_index {
                    if curr + 1 < max {
                        st.set_selected_index(Some(curr + 1));
                        let (_, h) = st.client_dimensions();
                        let viewport_h = (h - 42 - 74).max(0);
                        let row_bottom = (curr + 2) as i32 * 24;
                        if row_bottom > st.scroll_y + viewport_h {
                            st.scroll_y = row_bottom - viewport_h;
                        }
                        st.clamp_scroll();
                        repaint = true;
                    }
                } else if max > 0 {
                    st.set_selected_index(Some(0));
                    st.scroll_y = 0;
                    repaint = true;
                }
            }
            VK_PRIOR => {
                let step = 10;
                if let Some(curr) = st.selected_index {
                    st.set_selected_index(Some(curr.saturating_sub(step)));
                } else if st.row_count() > 0 {
                    st.set_selected_index(Some(0));
                }
                let (_, h) = st.client_dimensions();
                let viewport_h = (h - 42 - 74).max(0);
                st.scroll_y = (st.scroll_y - viewport_h).max(0);
                repaint = true;
            }
            VK_NEXT => {
                let max = st.row_count();
                let step = 10;
                if let Some(curr) = st.selected_index {
                    st.set_selected_index(Some((curr + step).min(max.saturating_sub(1))));
                } else if max > 0 {
                    st.set_selected_index(Some(0));
                }
                let (_, h) = st.client_dimensions();
                let viewport_h = (h - 42 - 74).max(0);
                let max_s = st.max_scroll_y(h);
                st.scroll_y = (st.scroll_y + viewport_h).min(max_s);
                repaint = true;
            }
            VK_DELETE => {
                if st.active_tab == InspectTab::Processes {
                    st.kill_selected();
                    repaint = true;
                } else if st.active_tab == InspectTab::Sockets {
                    let sel_item = st.selected_socket_item().cloned();
                    let is_it = st.is_italian();
                    if let Some(item) = sel_item {
                        match item {
                            SocketItem::Group { proc_name, socket_indices, .. } => {
                                let mut closed = 0;
                                for idx in socket_indices {
                                    if let Some(s) = st.sockets.get(idx)
                                        && let (Some(loc_ip), Some(rem_ip)) = (s.raw_local_ip_v4, s.raw_remote_ip_v4)
                                        && st.engine.close_tcp_v4(loc_ip, s.raw_local_port, rem_ip, s.raw_remote_port)
                                    {
                                        closed += 1;
                                    }
                                }
                                if is_it {
                                    st.set_status(format!("Chiuse {closed} connessioni TCP di {proc_name}"));
                                } else {
                                    st.set_status(format!("Closed {closed} TCP connections of {proc_name}"));
                                }
                                st.reload_data();
                                repaint = true;
                            }
                            SocketItem::Child { socket_idx } | SocketItem::Single { socket_idx } => {
                                let ips = st.sockets.get(socket_idx).and_then(|s| {
                                    match (s.raw_local_ip_v4, s.raw_remote_ip_v4) {
                                        (Some(loc_ip), Some(rem_ip)) => {
                                            Some((loc_ip, s.raw_local_port, rem_ip, s.raw_remote_port))
                                        }
                                        _ => None,
                                    }
                                });
                                if let Some((loc_ip, loc_port, rem_ip, rem_port)) = ips {
                                    if st.engine.close_tcp_v4(loc_ip, loc_port, rem_ip, rem_port) {
                                        st.set_status(if is_it {
                                            "Connessione TCP chiusa".to_string()
                                        } else {
                                            "TCP connection closed".to_string()
                                        });
                                    } else {
                                        st.set_status(if is_it {
                                            "Impossibile chiudere connessione".to_string()
                                        } else {
                                            "Unable to close connection".to_string()
                                        });
                                    }
                                    st.reload_data();
                                    repaint = true;
                                }
                            }
                        }
                    }
                }
            }
            0x20 => {
                // Barra spaziatrice: alterna espansione gruppo
                if st.active_tab == InspectTab::Processes
                    && let Some(ProcItem::Group { name, .. }) = st.selected_index.and_then(|sel| st.proc_items.get(sel))
                {
                    let key = name.to_lowercase();
                    if st.expanded_groups.contains(&key) {
                        st.expanded_groups.remove(&key);
                    } else {
                        st.expanded_groups.insert(key);
                    }
                    st.update_proc_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                } else if st.active_tab == InspectTab::Sockets
                    && let Some(SocketItem::Group { proc_name, .. }) =
                        st.selected_index.and_then(|sel| st.socket_items.get(sel))
                {
                    let key = proc_name.to_lowercase();
                    if st.expanded_socket_groups.contains(&key) {
                        st.expanded_socket_groups.remove(&key);
                    } else {
                        st.expanded_socket_groups.insert(key);
                    }
                    st.update_socket_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                }
            }
            0x27 => {
                // Freccia destra: espande gruppo
                if st.active_tab == InspectTab::Processes
                    && let Some(ProcItem::Group { name, is_expanded: false, .. }) =
                        st.selected_index.and_then(|sel| st.proc_items.get(sel))
                {
                    st.expanded_groups.insert(name.to_lowercase());
                    st.update_proc_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                } else if st.active_tab == InspectTab::Sockets
                    && let Some(SocketItem::Group { proc_name, is_expanded: false, .. }) =
                        st.selected_index.and_then(|sel| st.socket_items.get(sel))
                {
                    st.expanded_socket_groups.insert(proc_name.to_lowercase());
                    st.update_socket_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                }
            }
            0x25 => {
                // Freccia sinistra: chiude gruppo
                if st.active_tab == InspectTab::Processes
                    && let Some(ProcItem::Group { name, is_expanded: true, .. }) =
                        st.selected_index.and_then(|sel| st.proc_items.get(sel))
                {
                    st.expanded_groups.remove(&name.to_lowercase());
                    st.update_proc_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                } else if st.active_tab == InspectTab::Sockets
                    && let Some(SocketItem::Group { proc_name, is_expanded: true, .. }) =
                        st.selected_index.and_then(|sel| st.socket_items.get(sel))
                {
                    st.expanded_socket_groups.remove(&proc_name.to_lowercase());
                    st.update_socket_items();
                    st.restore_selection_index();
                    st.clamp_scroll();
                    repaint = true;
                }
            }
            _ => {}
        }
    });

    if repaint {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

unsafe fn paint_window(hwnd: HWND, hdc: HDC) {
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right;
    let h = rc.bottom;

    if w <= 0 || h <= 0 {
        return;
    }

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(st) = guard.as_mut() else { return };

        let is_dark = st.is_dark;
        let s = st.s;
        let is_it = core::ptr::eq(s, &crate::strings::IT);

        // Palette
        let bg_color = if is_dark { 0x001F1F1F } else { 0x00F3F3F3 };
        let header_bg = if is_dark { 0x00181818 } else { 0x00E5E5E5 };
        let active_tab_bg = 0x00D77800; // Accent Blue in BGR
        let inactive_tab_bg = if is_dark { 0x002B2B2B } else { 0x00DCDCDC };
        let table_header_bg = if is_dark { 0x00252525 } else { 0x00E0E0E0 };
        let row_even = if is_dark { 0x001F1F1F } else { 0x00FFFFFF };
        let row_odd = if is_dark { 0x00232323 } else { 0x00F8F8F8 };
        let row_hover = if is_dark { 0x002D2D2D } else { 0x00EAEAEA };
        let row_sel = if is_dark { 0x00714709 } else { 0x00CEE5FF }; // Selected blue in BGR
        let text_normal = if is_dark { 0x00E0E0E0 } else { 0x001A1A1A };
        let text_dim = if is_dark { 0x00888888 } else { 0x00666666 };
        let text_cyan = 0x00B0C94E; // BGR cyan
        let text_yellow = 0x00AADCD; // BGR yellow
        let text_red = 0x003333FF; // BGR red
        let border_color = if is_dark { 0x00333333 } else { 0x00CCCCCC };

        // Font
        let font_title = st.fonts.title;
        let font_body = st.fonts.body;
        let font_mono = st.fonts.mono;
        let old_font = SelectObject(mem_dc, font_title);

        SetBkMode(mem_dc, TRANSPARENT as _);

        // 1. Sfondo generale
        let bg_brush = CreateSolidBrush(bg_color);
        FillRect(mem_dc, &rc, bg_brush);
        DeleteObject(bg_brush);

        let top_h = st.top_bar_height();
        let hdr_h = st.header_height();
        let btm_h = st.bottom_bar_height();
        let row_h = st.row_height();

        // 2. Barra in alto (y: 0..top_h)
        let header_rc = RECT { left: 0, top: 0, right: w, bottom: top_h };
        let h_brush = CreateSolidBrush(header_bg);
        FillRect(mem_dc, &header_rc, h_brush);
        DeleteObject(h_brush);

        // Disegna Tabs con larghezza dinamica e DPI-aware
        let tab_h = st.scale(28).max(24);
        let tab_y1 = (top_h - tab_h) / 2;
        let tab_y2 = tab_y1 + tab_h;
        let tab_pad = st.scale(16);
        let tab_gap = st.scale(6);
        let mut cur_tab_x = st.scale(14);

        let tabs = [
            (s.inspect_tab_processes, st.active_tab == InspectTab::Processes),
            (s.inspect_tab_sockets, st.active_tab == InspectTab::Sockets),
            (s.inspect_tab_services, st.active_tab == InspectTab::Services),
            (s.inspect_tab_settings, st.active_tab == InspectTab::Settings),
            (s.inspect_tab_info, st.active_tab == InspectTab::Info),
        ];

        for (i, &(text, active)) in tabs.iter().enumerate() {
            let len = text.len().saturating_sub(1);
            let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
            GetTextExtentPoint32W(mem_dc, text.as_ptr(), len as i32, &mut sz);
            let tab_w = sz.cx + tab_pad * 2;
            let tab_rc = RECT { left: cur_tab_x, top: tab_y1, right: cur_tab_x + tab_w, bottom: tab_y2 };
            st.tab_rects[i] = tab_rc;

            let t_brush = CreateSolidBrush(if active { active_tab_bg } else { inactive_tab_bg });
            FillRect(mem_dc, &tab_rc, t_brush);
            DeleteObject(t_brush);

            SetTextColor(mem_dc, if active { 0x00FFFFFF } else { text_dim });
            let tx = cur_tab_x + (tab_w - sz.cx) / 2;
            let ty = tab_y1 + (tab_h - sz.cy) / 2;
            TextOutW(mem_dc, tx, ty, text.as_ptr(), len as i32);

            cur_tab_x += tab_w + tab_gap;
        }
        let tabs_end_x = cur_tab_x;

        let edit_h = st.scale(26).max(22);
        let edit_y = (top_h - edit_h) / 2;
        let edit_w = st.scale(200).min((w - tabs_end_x - st.scale(130)).max(st.scale(100)));
        let edit_x = w - edit_w - st.scale(14);
        SetWindowPos(st.search_edit, null_mut(), edit_x, edit_y, edit_w, edit_h, SWP_NOZORDER);

        if st.active_tab == InspectTab::Processes || st.active_tab == InspectTab::Sockets {
            let is_grouped = if st.active_tab == InspectTab::Processes { st.group_procs } else { st.group_sockets };
            let gb_w = st.scale(110).max(85);
            let gb_x2 = edit_x - st.scale(8);
            let gb_x1 = gb_x2 - gb_w;
            let gb_rc = RECT { left: gb_x1, top: edit_y, right: gb_x2, bottom: edit_y + edit_h };
            st.group_btn_rect = gb_rc;

            let gb_bg = if is_grouped { active_tab_bg } else { inactive_tab_bg };
            let gb_brush = CreateSolidBrush(gb_bg);
            FillRect(mem_dc, &gb_rc, gb_brush);
            DeleteObject(gb_brush);
            let gb_border = CreateSolidBrush(border_color);
            FrameRect(mem_dc, &gb_rc, gb_border);
            DeleteObject(gb_border);
            SetTextColor(mem_dc, if is_grouped { 0x00FFFFFF } else { text_dim });
            let group_text =
                String::from_utf16_lossy(&s.inspect_btn_group[..s.inspect_btn_group.len().saturating_sub(1)]);
            let gb_label = if is_grouped { format!("✔ {group_text}") } else { group_text };
            let gb_wstr: Vec<u16> = gb_label.encode_utf16().collect();
            let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
            GetTextExtentPoint32W(mem_dc, gb_wstr.as_ptr(), gb_wstr.len() as i32, &mut sz);
            let tx = gb_x1 + (gb_w - sz.cx) / 2;
            let ty = edit_y + (edit_h - sz.cy) / 2;
            TextOutW(mem_dc, tx, ty, gb_wstr.as_ptr(), gb_wstr.len() as i32);
        } else {
            st.group_btn_rect = zeroed();
        }

        if st.active_tab == InspectTab::Processes && !st.hung_windows.is_empty() {
            let gb_x1 = if st.group_btn_rect.left > 0 { st.group_btn_rect.left } else { edit_x };
            let b_x1 = tabs_end_x + st.scale(8);
            let b_x2 = (gb_x1 - st.scale(8)).min(b_x1 + st.scale(320));
            if b_x2 > b_x1 + st.scale(70) {
                if let Some(first_hung) = st.hung_windows.first() {
                    let pill_rc = RECT { left: b_x1, top: edit_y, right: b_x2, bottom: edit_y + edit_h };
                    let p_brush = CreateSolidBrush(0x001B1BD0);
                    FillRect(mem_dc, &pill_rc, p_brush);
                    DeleteObject(p_brush);

                    let p_border = CreateSolidBrush(0x003333FF);
                    FrameRect(mem_dc, &pill_rc, p_border);
                    DeleteObject(p_border);

                    SelectObject(mem_dc, font_body);
                    SetTextColor(mem_dc, 0x00FFFFFF);
                    let title_trunc = if first_hung.title.chars().count() > 16 {
                        let mut t: String = first_hung.title.chars().take(14).collect();
                        t.push('…');
                        t
                    } else {
                        first_hung.title.clone()
                    };
                    let label = if is_it {
                        format!("⚠️ {title_trunc} [Termina subito]")
                    } else {
                        format!("⚠️ {title_trunc} [Kill now]")
                    };
                    let wstr: Vec<u16> = label.encode_utf16().collect();
                    let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                    GetTextExtentPoint32W(mem_dc, wstr.as_ptr(), wstr.len() as i32, &mut sz);
                    let tx = b_x1 + (b_x2 - b_x1 - sz.cx).max(4) / 2;
                    let ty = edit_y + (edit_h - sz.cy) / 2;
                    TextOutW(mem_dc, tx, ty, wstr.as_ptr(), wstr.len() as i32);

                    st.hung_kill_rect = Some((pill_rc, first_hung.pid));
                }
            } else {
                st.hung_kill_rect = None;
            }
        } else {
            st.hung_kill_rect = None;
        }

        if st.active_tab == InspectTab::Settings {
            paint_settings_page(mem_dc, w, h, is_dark, &st.fonts, s, st.dpi);
        } else if st.active_tab == InspectTab::Info {
            paint_info_page(mem_dc, w, h, is_dark, st, s);
        } else {
            // 3. Header Colonne della tabella (y: top_h..top_h + hdr_h)
            let tbl_hdr_rc = RECT { left: 0, top: top_h, right: w, bottom: top_h + hdr_h };
            let th_brush = CreateSolidBrush(table_header_bg);
            FillRect(mem_dc, &tbl_hdr_rc, th_brush);
            DeleteObject(th_brush);

            let bounds = st.col_bounds(w);

            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);

            let draw_col = |dc: HDC, x1: i32, x2: i32, text: &[u16], col_idx: usize, align_right: bool| {
                if x2 <= x1 {
                    return;
                }
                let mut label = String::from_utf16_lossy(&text[..text.len().saturating_sub(1)]);
                if st.sort_col == col_idx {
                    label.push_str(if st.sort_asc { " ▲" } else { " ▼" });
                }
                let wstr: Vec<u16> = label.encode_utf16().collect();
                let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                GetTextExtentPoint32W(dc, wstr.as_ptr(), wstr.len() as i32, &mut sz);
                let tx = if align_right { x2 - sz.cx - 8 } else { x1 + 8 };
                let ty = top_h + (hdr_h - sz.cy) / 2;
                let clip_rc = RECT { left: x1 + 2, top: top_h, right: x2 - 2, bottom: top_h + hdr_h };
                ExtTextOutW(dc, tx, ty, ETO_CLIPPED, &clip_rc, wstr.as_ptr(), wstr.len() as u32, core::ptr::null());
            };

            match st.active_tab {
                InspectTab::Processes => {
                    let cols: [(&[u16], usize, bool); 7] = [
                        (s.inspect_col_name, 0, false),
                        (s.inspect_col_pid, 1, true),
                        (s.inspect_col_cpu, 2, true),
                        (s.inspect_col_ws, 3, true),
                        (s.inspect_col_priv, 4, true),
                        (s.inspect_col_threads, 5, true),
                        (s.inspect_col_assoc, 6, false),
                    ];
                    for (i, &(text, col_idx, align_right)) in cols.iter().enumerate() {
                        if let Some(&(x1, x2)) = bounds.get(i) {
                            draw_col(mem_dc, x1, x2, text, col_idx, align_right);
                        }
                    }
                }
                InspectTab::Sockets => {
                    let cols: [(&[u16], usize, bool); 6] = [
                        (s.inspect_col_name, 0, false),
                        (s.inspect_col_pid, 1, true),
                        (s.inspect_col_proto, 2, false),
                        (s.inspect_col_local, 3, false),
                        (s.inspect_col_remote, 4, false),
                        (s.inspect_col_state, 5, false),
                    ];
                    for (i, &(text, col_idx, align_right)) in cols.iter().enumerate() {
                        if let Some(&(x1, x2)) = bounds.get(i) {
                            draw_col(mem_dc, x1, x2, text, col_idx, align_right);
                        }
                    }
                }
                InspectTab::Services => {
                    let cols: [(&[u16], usize, bool); 5] = [
                        (s.inspect_col_name, 0, false),
                        (s.inspect_col_display, 1, false),
                        (s.inspect_col_state, 2, false),
                        (s.inspect_col_pid, 3, true),
                        (s.inspect_col_name, 4, false),
                    ];
                    for (i, &(text, col_idx, align_right)) in cols.iter().enumerate() {
                        if let Some(&(x1, x2)) = bounds.get(i) {
                            draw_col(mem_dc, x1, x2, text, col_idx, align_right);
                        }
                    }
                }
                InspectTab::Info | InspectTab::Settings => {}
            }

            // Tacche verticali separatori colonne nell'header
            let sep_brush = CreateSolidBrush(border_color);
            for &(x1, _) in bounds.iter().skip(1) {
                let sep_rc = RECT { left: x1 - 1, top: top_h + 4, right: x1, bottom: top_h + hdr_h - 4 };
                FillRect(mem_dc, &sep_rc, sep_brush);
            }
            DeleteObject(sep_brush);

            // Linea divisoria sotto l'header della tabella
            let div_brush = CreateSolidBrush(border_color);
            let div_rc = RECT { left: 0, top: top_h + hdr_h - 1, right: w, bottom: top_h + hdr_h };
            FillRect(mem_dc, &div_rc, div_brush);
            DeleteObject(div_brush);

            // 4. Righe della tabella (y: table_top..table_bottom)
            let table_top = top_h + hdr_h;
            let table_bottom = h - btm_h;
            let visible_rows = ((table_bottom - table_top) / row_h + 2) as usize;
            let start_idx = (st.scroll_y / row_h) as usize;
            let end_idx = (start_idx + visible_rows).min(st.row_count());

            for (row_visual_idx, idx) in (start_idx..end_idx).enumerate() {
                let row_y = table_top + (row_visual_idx as i32 * row_h) - (st.scroll_y % row_h);
                if row_y + row_h < table_top || row_y >= table_bottom {
                    continue;
                }

                let is_sel = st.selected_index == Some(idx);
                let is_hover = st.hover_row == Some(idx);
                let r_bg = if is_sel {
                    row_sel
                } else if is_hover {
                    row_hover
                } else if idx % 2 == 0 {
                    row_even
                } else {
                    row_odd
                };

                let row_rect = RECT { left: 0, top: row_y, right: w - st.scale(20), bottom: row_y + row_h };
                let r_brush = CreateSolidBrush(r_bg);
                FillRect(mem_dc, &row_rect, r_brush);
                DeleteObject(r_brush);

                let txt_c = if is_sel { 0x00FFFFFF } else { text_normal };
                SetTextColor(mem_dc, txt_c);

                let draw_cell = |dc: HDC,
                                 x1: i32,
                                 x2: i32,
                                 text: &str,
                                 align_right: bool,
                                 custom_color: Option<u32>| {
                    if x2 <= x1 {
                        return;
                    }
                    if let (Some(c), false) = (custom_color, is_sel) {
                        SetTextColor(dc, c);
                    }
                    let wstr: Vec<u16> = text.encode_utf16().collect();
                    let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                    GetTextExtentPoint32W(dc, wstr.as_ptr(), wstr.len() as i32, &mut sz);
                    let tx = if align_right { x2 - sz.cx - 8 } else { x1 + 8 };
                    let ty = row_y + (row_h - sz.cy) / 2;
                    let clip_rc = RECT { left: x1 + 2, top: row_y, right: x2 - 2, bottom: row_y + row_h };
                    ExtTextOutW(dc, tx, ty, ETO_CLIPPED, &clip_rc, wstr.as_ptr(), wstr.len() as u32, core::ptr::null());
                    SetTextColor(dc, txt_c);
                };

                match st.active_tab {
                    InspectTab::Processes => {
                        let item_opt = st.proc_items.get(idx);
                        match item_opt {
                            Some(ProcItem::Group {
                                name,
                                count,
                                cpu_percent,
                                ws_bytes,
                                priv_bytes,
                                threads,
                                services_summary,
                                sockets_count,
                                is_hung,
                                is_leak,
                                is_expanded,
                                ..
                            }) => {
                                let arrow = if *is_expanded { "▼ " } else { "▶ " };
                                let badge = if *is_hung {
                                    if is_it { " [⚠️ Non risponde]" } else { " [⚠️ Not responding]" }
                                } else if *is_leak {
                                    " [⚠️ Leak]"
                                } else {
                                    ""
                                };
                                let name_display = format!("{arrow}{name} ({count}){badge}");
                                let name_color = if *is_hung {
                                    Some(text_red)
                                } else if *is_leak {
                                    Some(text_yellow)
                                } else {
                                    None
                                };
                                SelectObject(mem_dc, font_title);
                                draw_cell(mem_dc, bounds[0].0, bounds[0].1, &name_display, false, name_color);

                                SelectObject(mem_dc, font_mono);
                                draw_cell(
                                    mem_dc,
                                    bounds[1].0,
                                    bounds[1].1,
                                    &format!("({count})"),
                                    true,
                                    Some(text_dim),
                                );

                                let cpu_color = if *cpu_percent > 10.0 { Some(text_yellow) } else { None };
                                draw_cell(
                                    mem_dc,
                                    bounds[2].0,
                                    bounds[2].1,
                                    &format!("{cpu_percent:.1}%"),
                                    true,
                                    cpu_color,
                                );
                                draw_cell(mem_dc, bounds[3].0, bounds[3].1, &format_bytes(*ws_bytes), true, None);
                                draw_cell(
                                    mem_dc,
                                    bounds[4].0,
                                    bounds[4].1,
                                    &format_bytes(*priv_bytes),
                                    true,
                                    Some(text_dim),
                                );
                                draw_cell(mem_dc, bounds[5].0, bounds[5].1, &threads.to_string(), true, Some(text_dim));

                                SelectObject(mem_dc, font_body);
                                let sock_word = if is_it || *sockets_count == 1 { "socket" } else { "sockets" };
                                let extra = if *sockets_count > 0 && !services_summary.is_empty() {
                                    format!("{services_summary} | {sockets_count} {sock_word}")
                                } else if *sockets_count > 0 {
                                    format!("{sockets_count} {sock_word}")
                                } else {
                                    services_summary.clone()
                                };
                                draw_cell(mem_dc, bounds[6].0, bounds[6].1, &extra, false, Some(text_cyan));
                            }
                            Some(ProcItem::Child { proc_idx, .. }) => {
                                if let Some(p) = st.procs.get(*proc_idx) {
                                    SelectObject(mem_dc, font_body);
                                    let hung_msg =
                                        if is_it { " [⚠️ Non risponde]" } else { " [⚠️ Not responding]" };
                                    let (name_display, name_color) = if p.is_hung {
                                        (format!("    └─ {}{hung_msg}", p.name), Some(text_red))
                                    } else if p.is_leak {
                                        (
                                            format!("    └─ {} [⚠️ Leak +{} MB]", p.name, p.mem_delta_mb),
                                            Some(text_yellow),
                                        )
                                    } else {
                                        (format!("    └─ {}", p.name), Some(text_dim))
                                    };
                                    draw_cell(mem_dc, bounds[0].0, bounds[0].1, &name_display, false, name_color);

                                    SelectObject(mem_dc, font_mono);
                                    draw_cell(
                                        mem_dc,
                                        bounds[1].0,
                                        bounds[1].1,
                                        &p.pid.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    let cpu_color = if p.cpu_percent > 10.0 { Some(text_yellow) } else { None };
                                    draw_cell(
                                        mem_dc,
                                        bounds[2].0,
                                        bounds[2].1,
                                        &format!("{:.1}%", p.cpu_percent),
                                        true,
                                        cpu_color,
                                    );
                                    draw_cell(mem_dc, bounds[3].0, bounds[3].1, &format_bytes(p.ws_bytes), true, None);
                                    draw_cell(
                                        mem_dc,
                                        bounds[4].0,
                                        bounds[4].1,
                                        &format_bytes(p.priv_bytes),
                                        true,
                                        Some(text_dim),
                                    );
                                    draw_cell(
                                        mem_dc,
                                        bounds[5].0,
                                        bounds[5].1,
                                        &p.threads.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    SelectObject(mem_dc, font_body);
                                    let sock_word = if is_it || p.sockets_count == 1 { "socket" } else { "sockets" };
                                    let extra = if p.sockets_count > 0 && !p.services_summary.is_empty() {
                                        format!("{} | {} {sock_word}", p.services_summary, p.sockets_count)
                                    } else if p.sockets_count > 0 {
                                        format!("{} {sock_word}", p.sockets_count)
                                    } else {
                                        p.services_summary.clone()
                                    };
                                    draw_cell(mem_dc, bounds[6].0, bounds[6].1, &extra, false, Some(text_cyan));
                                }
                            }
                            Some(ProcItem::Single { proc_idx }) => {
                                if let Some(p) = st.procs.get(*proc_idx) {
                                    SelectObject(mem_dc, font_body);
                                    let hung_msg =
                                        if is_it { " [⚠️ Non risponde]" } else { " [⚠️ Not responding]" };
                                    let (name_display, name_color) = if p.is_hung {
                                        (format!("{}{hung_msg}", p.name), Some(text_red))
                                    } else if p.is_leak {
                                        (format!("{} [⚠️ Leak +{} MB]", p.name, p.mem_delta_mb), Some(text_yellow))
                                    } else {
                                        (p.name.clone(), None)
                                    };
                                    draw_cell(mem_dc, bounds[0].0, bounds[0].1, &name_display, false, name_color);

                                    SelectObject(mem_dc, font_mono);
                                    draw_cell(
                                        mem_dc,
                                        bounds[1].0,
                                        bounds[1].1,
                                        &p.pid.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    let cpu_color = if p.cpu_percent > 10.0 { Some(text_yellow) } else { None };
                                    draw_cell(
                                        mem_dc,
                                        bounds[2].0,
                                        bounds[2].1,
                                        &format!("{:.1}%", p.cpu_percent),
                                        true,
                                        cpu_color,
                                    );
                                    draw_cell(mem_dc, bounds[3].0, bounds[3].1, &format_bytes(p.ws_bytes), true, None);
                                    draw_cell(
                                        mem_dc,
                                        bounds[4].0,
                                        bounds[4].1,
                                        &format_bytes(p.priv_bytes),
                                        true,
                                        Some(text_dim),
                                    );
                                    draw_cell(
                                        mem_dc,
                                        bounds[5].0,
                                        bounds[5].1,
                                        &p.threads.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    SelectObject(mem_dc, font_body);
                                    let sock_word = if is_it || p.sockets_count == 1 { "socket" } else { "sockets" };
                                    let extra = if p.sockets_count > 0 && !p.services_summary.is_empty() {
                                        format!("{} | {} {sock_word}", p.services_summary, p.sockets_count)
                                    } else if p.sockets_count > 0 {
                                        format!("{} {sock_word}", p.sockets_count)
                                    } else {
                                        p.services_summary.clone()
                                    };
                                    draw_cell(mem_dc, bounds[6].0, bounds[6].1, &extra, false, Some(text_cyan));
                                }
                            }
                            None => {}
                        }
                    }
                    InspectTab::Sockets => {
                        let item_opt = st.socket_items.get(idx);
                        match item_opt {
                            Some(SocketItem::Group {
                                proc_name,
                                pids,
                                count,
                                tcp_count,
                                udp_count,
                                is_expanded,
                                ..
                            }) => {
                                let arrow = if *is_expanded { "▼ " } else { "▶ " };
                                let name_display = format!("{arrow}{proc_name} ({count})");
                                SelectObject(mem_dc, font_title);
                                draw_cell(mem_dc, bounds[0].0, bounds[0].1, &name_display, false, None);

                                SelectObject(mem_dc, font_mono);
                                let pid_str = if pids.len() == 1 { pids[0].to_string() } else { format!("({count})") };
                                draw_cell(mem_dc, bounds[1].0, bounds[1].1, &pid_str, true, Some(text_dim));

                                let proto_str = format!("{tcp_count} TCP / {udp_count} UDP");
                                draw_cell(mem_dc, bounds[2].0, bounds[2].1, &proto_str, false, Some(text_cyan));
                                let conn_word = if is_it { "connessioni" } else { "connections" };
                                draw_cell(
                                    mem_dc,
                                    bounds[3].0,
                                    bounds[3].1,
                                    &format!("{count} {conn_word}"),
                                    false,
                                    Some(text_dim),
                                );
                                draw_cell(mem_dc, bounds[4].0, bounds[4].1, "—", false, Some(text_dim));
                                draw_cell(mem_dc, bounds[5].0, bounds[5].1, "—", false, Some(text_dim));
                            }
                            Some(SocketItem::Child { socket_idx }) => {
                                if let Some(s) = st.sockets.get(*socket_idx) {
                                    SelectObject(mem_dc, font_body);
                                    let name_display = format!("    └─ {}", s.proc_name);
                                    draw_cell(mem_dc, bounds[0].0, bounds[0].1, &name_display, false, Some(text_dim));

                                    SelectObject(mem_dc, font_mono);
                                    draw_cell(
                                        mem_dc,
                                        bounds[1].0,
                                        bounds[1].1,
                                        &s.pid.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    let proto_c = if s.proto == "TCP" { Some(text_cyan) } else { Some(text_dim) };
                                    draw_cell(mem_dc, bounds[2].0, bounds[2].1, s.proto, false, proto_c);
                                    draw_cell(mem_dc, bounds[3].0, bounds[3].1, &s.local_addr, false, None);
                                    let rem_display = if !s.scope_badge.is_empty()
                                        && s.remote_addr != "—"
                                        && s.remote_addr != "*"
                                        && s.remote_addr != "*:*"
                                    {
                                        format!("{} [{}]", s.remote_addr, s.scope_badge)
                                    } else {
                                        s.remote_addr.clone()
                                    };
                                    let rem_c = if s.is_public { Some(text_yellow) } else { None };
                                    draw_cell(mem_dc, bounds[4].0, bounds[4].1, &rem_display, false, rem_c);

                                    let state_c = if s.state == "ESTABLISHED" {
                                        Some(text_cyan)
                                    } else if s.state == "LISTENING" {
                                        Some(text_yellow)
                                    } else {
                                        Some(text_dim)
                                    };
                                    draw_cell(mem_dc, bounds[5].0, bounds[5].1, s.state, false, state_c);
                                }
                            }
                            Some(SocketItem::Single { socket_idx }) => {
                                if let Some(s) = st.sockets.get(*socket_idx) {
                                    SelectObject(mem_dc, font_body);
                                    draw_cell(mem_dc, bounds[0].0, bounds[0].1, &s.proc_name, false, None);

                                    SelectObject(mem_dc, font_mono);
                                    draw_cell(
                                        mem_dc,
                                        bounds[1].0,
                                        bounds[1].1,
                                        &s.pid.to_string(),
                                        true,
                                        Some(text_dim),
                                    );

                                    let proto_c = if s.proto == "TCP" { Some(text_cyan) } else { Some(text_dim) };
                                    draw_cell(mem_dc, bounds[2].0, bounds[2].1, s.proto, false, proto_c);
                                    draw_cell(mem_dc, bounds[3].0, bounds[3].1, &s.local_addr, false, None);
                                    let rem_display = if !s.scope_badge.is_empty()
                                        && s.remote_addr != "—"
                                        && s.remote_addr != "*"
                                        && s.remote_addr != "*:*"
                                    {
                                        format!("{} [{}]", s.remote_addr, s.scope_badge)
                                    } else {
                                        s.remote_addr.clone()
                                    };
                                    let rem_c = if s.is_public { Some(text_yellow) } else { None };
                                    draw_cell(mem_dc, bounds[4].0, bounds[4].1, &rem_display, false, rem_c);

                                    let state_c = if s.state == "ESTABLISHED" {
                                        Some(text_cyan)
                                    } else if s.state == "LISTENING" {
                                        Some(text_yellow)
                                    } else {
                                        Some(text_dim)
                                    };
                                    draw_cell(mem_dc, bounds[5].0, bounds[5].1, s.state, false, state_c);
                                }
                            }
                            None => {}
                        }
                    }
                    InspectTab::Services => {
                        let svc_opt = st.filtered_services.get(idx).and_then(|&s_idx| st.services.get(s_idx));
                        if let Some(s) = svc_opt {
                            SelectObject(mem_dc, font_title);
                            draw_cell(mem_dc, bounds[0].0, bounds[0].1, &s.name, false, None);

                            SelectObject(mem_dc, font_body);
                            draw_cell(mem_dc, bounds[1].0, bounds[1].1, &s.display_name, false, Some(text_dim));

                            let st_c = if s.is_running { Some(text_cyan) } else { Some(text_dim) };
                            draw_cell(mem_dc, bounds[2].0, bounds[2].1, &s.state_str, false, st_c);

                            SelectObject(mem_dc, font_mono);
                            draw_cell(mem_dc, bounds[3].0, bounds[3].1, &s.pid.to_string(), true, Some(text_dim));

                            SelectObject(mem_dc, font_body);
                            draw_cell(mem_dc, bounds[4].0, bounds[4].1, &s.proc_name, false, Some(text_dim));
                        }
                    }
                    InspectTab::Info | InspectTab::Settings => {}
                }
            }

            // 5. Barra di scorrimento a destra
            let max_s = st.max_scroll_y(h);
            if max_s > 0 {
                let sb_w = st.scale(12).max(10);
                let sb_x2 = w - st.scale(6);
                let sb_x1 = sb_x2 - sb_w;
                let viewport_top = table_top;
                let viewport_bottom = table_bottom;
                let viewport_h = (viewport_bottom - viewport_top).max(1);

                // Pista della scrollbar
                let track_rc = RECT { left: sb_x1, top: viewport_top, right: sb_x2, bottom: viewport_bottom };
                let track_bg = if is_dark { 0x00191919 } else { 0x00E8E8E8 };
                let track_brush = CreateSolidBrush(track_bg);
                FillRect(mem_dc, &track_rc, track_brush);
                DeleteObject(track_brush);

                // Cursore (thumb)
                let total_h = st.row_count() as i32 * row_h;
                let thumb_h = (((viewport_h as f32) / (total_h as f32)) * viewport_h as f32) as i32;
                let thumb_h = thumb_h.clamp(st.scale(24).max(20), viewport_h);
                let avail = (viewport_h - thumb_h).max(1);
                let thumb_y = viewport_top + ((st.scroll_y as f32 / max_s as f32) * avail as f32) as i32;

                let thumb_bg = if st.is_dragging_scrollbar {
                    if is_dark { 0x00707070 } else { 0x00787878 }
                } else if is_dark {
                    0x00404040
                } else {
                    0x00B0B0B0
                };
                let thumb_rc = RECT { left: sb_x1 + 1, top: thumb_y, right: sb_x2 - 1, bottom: thumb_y + thumb_h };
                let thumb_brush = CreateSolidBrush(thumb_bg);
                FillRect(mem_dc, &thumb_rc, thumb_brush);
                DeleteObject(thumb_brush);
            }
        }

        // 5. Barra in basso (y: h - btm_h..h)
        let btm_rc = RECT { left: 0, top: h - btm_h, right: w, bottom: h };
        let btm_brush = CreateSolidBrush(header_bg);
        FillRect(mem_dc, &btm_rc, btm_brush);
        DeleteObject(btm_brush);

        let div_b_brush = CreateSolidBrush(border_color);
        let div_b_rc = RECT { left: 0, top: h - btm_h, right: w, bottom: h - btm_h + 1 };
        FillRect(mem_dc, &div_b_rc, div_b_brush);
        DeleteObject(div_b_brush);

        let btn_h = st.scale(26).max(24);
        let btn_y1 = h - btm_h + (btm_h - btn_h) / 2;
        let btn_y2 = btn_y1 + btn_h;
        let mut cur_btn_x = st.scale(14);
        let btn_gap = st.scale(10);

        // Disegna bottoni d'azione
        let draw_btn = |dc: HDC, x1: i32, x2: i32, text: &[u16], active: bool| -> RECT {
            let btn_rc = RECT { left: x1, top: btn_y1, right: x2, bottom: btn_y2 };
            let btn_bg = if active { inactive_tab_bg } else { header_bg };
            let b_brush = CreateSolidBrush(btn_bg);
            FillRect(dc, &btn_rc, b_brush);
            DeleteObject(b_brush);
            let f_brush = CreateSolidBrush(border_color);
            FrameRect(dc, &btn_rc, f_brush);
            DeleteObject(f_brush);

            SetTextColor(dc, if active { text_normal } else { text_dim });
            let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
            let len = text.len().saturating_sub(1);
            GetTextExtentPoint32W(dc, text.as_ptr(), len as i32, &mut sz);
            let tx = x1 + (x2 - x1 - sz.cx) / 2;
            let ty = btn_y1 + (btn_h - sz.cy) / 2;
            TextOutW(dc, tx, ty, text.as_ptr(), len as i32);
            btn_rc
        };

        SelectObject(mem_dc, font_body);

        let has_sel = st.selected_index.is_some();
        match st.active_tab {
            InspectTab::Processes => {
                let (btn_text, active) = match st.selected_proc_item() {
                    Some(ProcItem::Group { count, .. }) => {
                        let term_group = if is_it { "Termina Gruppo" } else { "Terminate Group" };
                        (format!("{term_group} ({count})"), true)
                    }
                    Some(ProcItem::Child { .. }) | Some(ProcItem::Single { .. }) => {
                        let name =
                            String::from_utf16_lossy(&s.inspect_btn_kill[..s.inspect_btn_kill.len().saturating_sub(1)]);
                        (name, true)
                    }
                    None => {
                        let name =
                            String::from_utf16_lossy(&s.inspect_btn_kill[..s.inspect_btn_kill.len().saturating_sub(1)]);
                        (name, false)
                    }
                };
                let btn_wstr: Vec<u16> = btn_text.encode_utf16().chain(core::iter::once(0)).collect();
                let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                GetTextExtentPoint32W(mem_dc, btn_wstr.as_ptr(), btn_wstr.len().saturating_sub(1) as i32, &mut sz);
                let b1_w = (sz.cx + st.scale(28)).max(st.scale(140));
                st.bottom_btn_rects[0] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b1_w, &btn_wstr, active);
                cur_btn_x += b1_w + btn_gap;
            }
            InspectTab::Sockets => {
                let (btn_text, active) = match st.selected_socket_item() {
                    Some(SocketItem::Group { tcp_count, .. }) => {
                        let close_conn = if is_it { "Chiudi Connessioni" } else { "Close Connections" };
                        (format!("{close_conn} ({tcp_count})"), *tcp_count > 0)
                    }
                    Some(SocketItem::Child { socket_idx }) | Some(SocketItem::Single { socket_idx }) => {
                        let is_tcp = st.sockets.get(*socket_idx).map(|s| s.proto == "TCP").unwrap_or(false);
                        (
                            String::from_utf16_lossy(
                                &s.inspect_btn_close_conn[..s.inspect_btn_close_conn.len().saturating_sub(1)],
                            ),
                            is_tcp,
                        )
                    }
                    None => (
                        String::from_utf16_lossy(
                            &s.inspect_btn_close_conn[..s.inspect_btn_close_conn.len().saturating_sub(1)],
                        ),
                        false,
                    ),
                };
                let btn_wstr: Vec<u16> = btn_text.encode_utf16().chain(core::iter::once(0)).collect();
                let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                GetTextExtentPoint32W(mem_dc, btn_wstr.as_ptr(), btn_wstr.len().saturating_sub(1) as i32, &mut sz);
                let b1_w = (sz.cx + st.scale(28)).max(st.scale(140));
                st.bottom_btn_rects[0] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b1_w, &btn_wstr, active);
                cur_btn_x += b1_w + btn_gap;
            }
            InspectTab::Services => {
                let is_running = st
                    .selected_index
                    .and_then(|idx| st.filtered_services.get(idx))
                    .and_then(|&s_idx| st.services.get(s_idx))
                    .map(|s| s.is_running)
                    .unwrap_or(false);
                let btn_text = if is_running { s.inspect_btn_stop_svc } else { s.inspect_btn_start_svc };
                let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                GetTextExtentPoint32W(mem_dc, btn_text.as_ptr(), btn_text.len().saturating_sub(1) as i32, &mut sz);
                let b1_w = (sz.cx + st.scale(28)).max(st.scale(140));
                st.bottom_btn_rects[0] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b1_w, btn_text, has_sel);
                cur_btn_x += b1_w + btn_gap;
            }
            InspectTab::Info => {
                let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
                GetTextExtentPoint32W(
                    mem_dc,
                    s.inspect_btn_taskmgr.as_ptr(),
                    s.inspect_btn_taskmgr.len().saturating_sub(1) as i32,
                    &mut sz,
                );
                let b1_w = (sz.cx + st.scale(28)).max(st.scale(140));
                st.bottom_btn_rects[0] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b1_w, s.inspect_btn_taskmgr, true);
                cur_btn_x += b1_w + btn_gap;
            }
            InspectTab::Settings => {
                st.bottom_btn_rects[0] = zeroed();
            }
        }

        let mut sz2: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(
            mem_dc,
            s.inspect_btn_refresh.as_ptr(),
            s.inspect_btn_refresh.len().saturating_sub(1) as i32,
            &mut sz2,
        );
        let b2_w = (sz2.cx + st.scale(28)).max(st.scale(100));
        st.bottom_btn_rects[1] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b2_w, s.inspect_btn_refresh, true);
        cur_btn_x += b2_w + btn_gap;

        if !crate::sys::elevation::is_elevated() {
            let mut sz3: windows_sys::Win32::Foundation::SIZE = zeroed();
            GetTextExtentPoint32W(
                mem_dc,
                s.inspect_btn_restart_admin.as_ptr(),
                s.inspect_btn_restart_admin.len().saturating_sub(1) as i32,
                &mut sz3,
            );
            let b3_w = (sz3.cx + st.scale(28)).max(st.scale(180));
            st.bottom_btn_rects[2] = draw_btn(mem_dc, cur_btn_x, cur_btn_x + b3_w, s.inspect_btn_restart_admin, true);
        } else {
            st.bottom_btn_rects[2] = zeroed();
        }

        // Testo di stato a destra
        let (p_lbl, so_lbl, se_lbl) =
            if is_it { ("Processi", "Socket", "Servizi") } else { ("Processes", "Sockets", "Services") };
        let status_str = if let Some((msg, ts)) = &st.status_msg {
            if ts.elapsed().as_secs() < 3 {
                msg.clone()
            } else {
                format!(
                    "{p_lbl}: {}  |  {so_lbl}: {}  |  {se_lbl}: {}",
                    st.procs.len(),
                    st.sockets.len(),
                    st.services.len()
                )
            }
        } else {
            format!(
                "{p_lbl}: {}  |  {so_lbl}: {}  |  {se_lbl}: {}",
                st.procs.len(),
                st.sockets.len(),
                st.services.len()
            )
        };

        let status_wstr: Vec<u16> = status_str.encode_utf16().collect();
        SetTextColor(mem_dc, text_dim);
        let mut sz_stat: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(mem_dc, status_wstr.as_ptr(), status_wstr.len() as i32, &mut sz_stat);
        let tx = w - sz_stat.cx - st.scale(14);
        let ty = btn_y1 + (btn_h - sz_stat.cy) / 2;
        TextOutW(mem_dc, tx, ty, status_wstr.as_ptr(), status_wstr.len() as i32);

        // Cleanup
        SelectObject(mem_dc, old_font);
    });

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
}

unsafe fn create_gdi_font(dpi: u32, pt: i32, weight: i32, mono: bool) -> HFONT {
    let font_name = if mono { wide!("Consolas") } else { wide!("Segoe UI") };
    let height = -((pt * dpi as i32) / 72);
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        1, // DEFAULT_CHARSET
        0,
        0,
        5, // CLEARTYPE_QUALITY
        0,
        font_name.as_ptr(),
    )
}

struct SettingsLayout {
    pub dpi: u32,
    pub start_x: i32,
    pub start_y: i32,
    pub card_y: i32,
    pub card_w: i32,
    pub card_h: i32,
    pub btn_h: i32,
    pub row_h: i32,
    pub cx1: i32,
    pub cx2: i32,
    pub cx3: i32,
    pub b1_x1: i32,
    pub b1_x2: i32,
    pub b2_x1: i32,
    pub b2_x2: i32,
    pub b3_x1: i32,
    pub b3_x2: i32,
}

impl SettingsLayout {
    pub fn new(w: i32, h: i32, dpi: u32) -> Self {
        let scale = |v: i32| (v * dpi as i32 + 48) / 96;
        let start_x = scale(24);
        let start_y = scale(52);
        let card_y = start_y + scale(50);
        let gap = scale(14);
        let card_w = ((w - start_x * 2 - gap * 2) / 3).max(scale(220));
        let card_h = (h - card_y - scale(16)).max(scale(520));
        let btn_w = scale(84);
        let btn_h = scale(24).max(22);
        let row_h = scale(30).max(26);

        let cx1 = start_x;
        let cx2 = start_x + card_w + gap;
        let cx3 = start_x + (card_w + gap) * 2;

        let b1_x1 = cx1 + card_w - scale(14) - btn_w;
        let b1_x2 = cx1 + card_w - scale(14);

        let b2_x1 = cx2 + card_w - scale(14) - btn_w;
        let b2_x2 = cx2 + card_w - scale(14);

        let b3_x1 = cx3 + card_w - scale(14) - btn_w;
        let b3_x2 = cx3 + card_w - scale(14);

        Self {
            dpi,
            start_x,
            start_y,
            card_y,
            card_w,
            card_h,
            btn_h,
            row_h,
            cx1,
            cx2,
            cx3,
            b1_x1,
            b1_x2,
            b2_x1,
            b2_x2,
            b3_x1,
            b3_x2,
        }
    }

    #[inline]
    pub fn scale(&self, v: i32) -> i32 {
        (v * self.dpi as i32 + 48) / 96
    }

    #[inline]
    pub fn row_y(&self, idx: i32) -> i32 {
        self.card_y + self.scale(40) + idx * self.row_h
    }

    #[inline]
    pub fn card1_row_y(&self, row: i32) -> i32 {
        if row < 9 { self.row_y(row) } else { self.row_y(row) + self.scale(20) }
    }

    #[inline]
    pub fn card2_row_y(&self, row: i32) -> i32 {
        self.row_y(row)
    }

    #[inline]
    pub fn card3_row_y(&self, row: i32, is_elevated: bool) -> i32 {
        let elev_offset = if is_elevated { 0 } else { self.scale(26) };
        if row < 3 {
            self.row_y(row)
        } else if row < 6 {
            self.row_y(row) + elev_offset
        } else {
            self.row_y(row) + elev_offset + self.scale(24)
        }
    }
}

unsafe fn paint_settings_page(
    dc: HDC,
    w: i32,
    h: i32,
    is_dark: bool,
    fonts: &InspectFonts,
    s: &'static Strings,
    dpi: u32,
) {
    let Some((settings, autostart_reg, autostart_admin)) = crate::app::get_settings_snapshot() else {
        return;
    };
    let is_it = core::ptr::eq(s, &crate::strings::IT);
    let sl = SettingsLayout::new(w, h, dpi);

    let font_title_lg = fonts.title_lg;
    let font_sub = fonts.body;
    let font_card_title = fonts.title;
    let font_item_label = fonts.body;
    let font_btn = fonts.btn;

    let text_title = if is_dark { 0x00FFFFFF } else { 0x00000000 };
    let text_dim = if is_dark { 0x00888888 } else { 0x00666666 };
    let text_normal = if is_dark { 0x00E0E0E0 } else { 0x001A1A1A };
    let text_cyan = 0x00B0C94E; // BGR cyan
    let text_yellow = 0x00AADCD; // BGR yellow
    let border_color = if is_dark { 0x00333333 } else { 0x00CCCCCC };
    let card_bg = if is_dark { 0x00262626 } else { 0x00FAFAFA };
    let card_hdr_bg = if is_dark { 0x002D2D2D } else { 0x00EDEDED };
    let active_bg = 0x00D77800; // Accent Blue in BGR
    let inactive_btn_bg = if is_dark { 0x00333333 } else { 0x00D5D5D5 };

    let old_font = SelectObject(dc, font_title_lg);
    SetTextColor(dc, text_title);
    let title_wstr: Vec<u16> =
        (if is_it { "Impostazioni di nextm" } else { "nextm Settings" }).encode_utf16().collect();
    TextOutW(dc, sl.start_x, sl.start_y, title_wstr.as_ptr(), title_wstr.len() as i32);

    SelectObject(dc, font_sub);
    SetTextColor(dc, text_dim);
    let sub_wstr: Vec<u16> = (if is_it {
        "Configurazione metriche nella barra, metodi di calcolo, prestazioni e avvio automatico"
    } else {
        "Configure tray metrics, calculation methods, performance, and autostart"
    })
    .encode_utf16()
    .collect();
    TextOutW(dc, sl.start_x, sl.start_y + sl.scale(24), sub_wstr.as_ptr(), sub_wstr.len() as i32);

    let draw_card_frame = |cx: i32, cy: i32, cw: i32, ch: i32, title: &str| {
        let card_rc = RECT { left: cx, top: cy, right: cx + cw, bottom: cy + ch };
        let c_brush = CreateSolidBrush(card_bg);
        FillRect(dc, &card_rc, c_brush);
        DeleteObject(c_brush);

        let c_border = CreateSolidBrush(border_color);
        FrameRect(dc, &card_rc, c_border);
        DeleteObject(c_border);

        let hdr_rc = RECT { left: cx, top: cy, right: cx + cw, bottom: cy + sl.scale(34) };
        let h_brush = CreateSolidBrush(card_hdr_bg);
        FillRect(dc, &hdr_rc, h_brush);
        DeleteObject(h_brush);

        let div_rc = RECT { left: cx, top: cy + sl.scale(33), right: cx + cw, bottom: cy + sl.scale(34) };
        let div_brush = CreateSolidBrush(border_color);
        FillRect(dc, &div_rc, div_brush);
        DeleteObject(div_brush);

        SelectObject(dc, font_card_title);
        SetTextColor(dc, text_cyan);
        let tw: Vec<u16> = title.encode_utf16().collect();
        TextOutW(dc, cx + sl.scale(14), cy + sl.scale(9), tw.as_ptr(), tw.len() as i32);
    };

    let draw_toggle = |rx1: i32, rx2: i32, ry: i32, is_on: bool, label_on: &str, label_off: &str| {
        let btn_rc = RECT { left: rx1, top: ry, right: rx2, bottom: ry + sl.btn_h };
        let bg = if is_on { active_bg } else { inactive_btn_bg };
        let brush = CreateSolidBrush(bg);
        FillRect(dc, &btn_rc, brush);
        DeleteObject(brush);

        let border = CreateSolidBrush(border_color);
        FrameRect(dc, &btn_rc, border);
        DeleteObject(border);

        SelectObject(dc, font_btn);
        SetTextColor(dc, if is_on { 0x00FFFFFF } else { text_dim });
        let text = if is_on { label_on } else { label_off };
        let wstr: Vec<u16> = text.encode_utf16().collect();
        let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, wstr.as_ptr(), wstr.len() as i32, &mut sz);
        let tx = rx1 + (rx2 - rx1 - sz.cx) / 2;
        let ty = ry + (sl.btn_h - sz.cy) / 2;
        TextOutW(dc, tx, ty, wstr.as_ptr(), wstr.len() as i32);
    };

    let draw_button = |rx1: i32, rx2: i32, ry: i32, is_active: bool, text: &str, custom_color: Option<COLORREF>| {
        let btn_rc = RECT { left: rx1, top: ry, right: rx2, bottom: ry + sl.btn_h };
        let bg = if is_active { active_bg } else { inactive_btn_bg };
        let brush = CreateSolidBrush(bg);
        FillRect(dc, &btn_rc, brush);
        DeleteObject(brush);

        let border = CreateSolidBrush(border_color);
        FrameRect(dc, &btn_rc, border);
        DeleteObject(border);

        SelectObject(dc, font_btn);
        let txt_c = if is_active { 0x00FFFFFF } else { custom_color.unwrap_or(text_normal) };
        SetTextColor(dc, txt_c);
        let wstr: Vec<u16> = text.encode_utf16().collect();
        let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, wstr.as_ptr(), wstr.len() as i32, &mut sz);
        let tx = rx1 + (rx2 - rx1 - sz.cx) / 2;
        let ty = ry + (sl.btn_h - sz.cy) / 2;
        TextOutW(dc, tx, ty, wstr.as_ptr(), wstr.len() as i32);
    };

    let draw_stepper = |rx1: i32, rx2: i32, ry: i32, val_str: &str| {
        let btn_rc = RECT { left: rx1, top: ry, right: rx2, bottom: ry + sl.btn_h };
        let brush = CreateSolidBrush(inactive_btn_bg);
        FillRect(dc, &btn_rc, brush);
        DeleteObject(brush);

        let border = CreateSolidBrush(border_color);
        FrameRect(dc, &btn_rc, border);

        let step_btn_w = sl.scale(24);
        let div1 = RECT { left: rx1 + step_btn_w, top: ry, right: rx1 + step_btn_w + 1, bottom: ry + sl.btn_h };
        let div2 = RECT { left: rx2 - step_btn_w, top: ry, right: rx2 - step_btn_w + 1, bottom: ry + sl.btn_h };
        FillRect(dc, &div1, border);
        FillRect(dc, &div2, border);
        DeleteObject(border);

        SelectObject(dc, font_btn);
        SetTextColor(dc, text_normal);

        let minus_wstr: [u16; 1] = [b'-' as u16];
        let mut sz_m: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, minus_wstr.as_ptr(), 1, &mut sz_m);
        TextOutW(dc, rx1 + (step_btn_w - sz_m.cx) / 2, ry + (sl.btn_h - sz_m.cy) / 2, minus_wstr.as_ptr(), 1);

        let val_wstr: Vec<u16> = val_str.encode_utf16().collect();
        let mut sz_v: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, val_wstr.as_ptr(), val_wstr.len() as i32, &mut sz_v);
        let center_w = (rx2 - step_btn_w) - (rx1 + step_btn_w + 1);
        TextOutW(
            dc,
            rx1 + step_btn_w + 1 + (center_w - sz_v.cx) / 2,
            ry + (sl.btn_h - sz_v.cy) / 2,
            val_wstr.as_ptr(),
            val_wstr.len() as i32,
        );

        let plus_wstr: [u16; 1] = [b'+' as u16];
        let mut sz_p: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, plus_wstr.as_ptr(), 1, &mut sz_p);
        TextOutW(
            dc,
            rx2 - step_btn_w + (step_btn_w - sz_p.cx) / 2,
            ry + (sl.btn_h - sz_p.cy) / 2,
            plus_wstr.as_ptr(),
            1,
        );
    };

    let draw_label = |x: i32, y: i32, text: &str, is_dim: bool, custom_c: Option<COLORREF>| {
        SelectObject(dc, font_item_label);
        let c = custom_c.unwrap_or(if is_dim { text_dim } else { text_normal });
        SetTextColor(dc, c);
        let wstr: Vec<u16> = text.encode_utf16().collect();
        TextOutW(dc, x, y, wstr.as_ptr(), wstr.len() as i32);
    };

    let lbl_on = if is_it { "✔ ATTIVO" } else { "✔ ON" };
    let lbl_off = if is_it { "DISATTIVO" } else { "OFF" };

    // CARD 1: ICONE NELLA BARRA (TRAY)
    draw_card_frame(
        sl.cx1,
        sl.card_y,
        sl.card_w,
        sl.card_h,
        if is_it { "ICONE NELLA BARRA (TRAY)" } else { "TRAY ICONS" },
    );

    let r0 = sl.card1_row_y(0);
    draw_label(sl.cx1 + sl.scale(14), r0 + sl.scale(4), if is_it { "Icona CPU" } else { "CPU Icon" }, false, None);
    draw_toggle(sl.b1_x1, sl.b1_x2, r0, settings.is_icon_active(crate::settings::ICON_CPU), lbl_on, lbl_off);

    let r1 = sl.card1_row_y(1);
    draw_label(sl.cx1 + sl.scale(14), r1 + sl.scale(4), if is_it { "Icona RAM" } else { "RAM Icon" }, false, None);
    draw_toggle(sl.b1_x1, sl.b1_x2, r1, settings.is_icon_active(crate::settings::ICON_RAM), lbl_on, lbl_off);

    let r2 = sl.card1_row_y(2);
    draw_label(
        sl.cx1 + sl.scale(14),
        r2 + sl.scale(4),
        if is_it { "Icona Carico GPU %" } else { "GPU Usage % Icon" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r2, settings.is_icon_active(crate::settings::ICON_GPU), lbl_on, lbl_off);

    let r3 = sl.card1_row_y(3);
    draw_label(sl.cx1 + sl.scale(14), r3 + sl.scale(4), if is_it { "Icona Rete" } else { "Network Icon" }, false, None);
    draw_toggle(sl.b1_x1, sl.b1_x2, r3, settings.is_icon_active(crate::settings::ICON_NET), lbl_on, lbl_off);

    let r4 = sl.card1_row_y(4);
    draw_label(
        sl.cx1 + sl.scale(14),
        r4 + sl.scale(4),
        if is_it { "Temp. ACPI / CPU" } else { "ACPI / CPU Temp" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r4, settings.is_icon_active(crate::settings::ICON_TEMP_ACPI), lbl_on, lbl_off);

    let r5 = sl.card1_row_y(5);
    draw_label(
        sl.cx1 + sl.scale(14),
        r5 + sl.scale(4),
        if is_it { "Temp. Scheda GPU" } else { "GPU Temp" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r5, settings.is_icon_active(crate::settings::ICON_TEMP_GPU), lbl_on, lbl_off);

    let r6 = sl.card1_row_y(6);
    draw_label(
        sl.cx1 + sl.scale(14),
        r6 + sl.scale(4),
        if is_it { "Temp. Disco / SSD" } else { "Disk / SSD Temp" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r6, settings.is_icon_active(crate::settings::ICON_TEMP_DISK), lbl_on, lbl_off);

    let r7 = sl.card1_row_y(7);
    draw_label(
        sl.cx1 + sl.scale(14),
        r7 + sl.scale(4),
        if is_it { "Unità velocità rete" } else { "Network rate unit" },
        false,
        None,
    );
    let u1_x1 = sl.cx1 + sl.card_w - sl.scale(14) - sl.scale(130);
    let u1_x2 = u1_x1 + sl.scale(62);
    let u2_x1 = u1_x2 + sl.scale(6);
    let u2_x2 = sl.cx1 + sl.card_w - sl.scale(14);
    draw_button(u1_x1, u1_x2, r7, settings.net_bits, "Bit/s", None);
    draw_button(u2_x1, u2_x2, r7, !settings.net_bits, "Byte/s", None);

    let r8 = sl.card1_row_y(8);
    draw_label(
        sl.cx1 + sl.scale(14),
        r8,
        if is_it { "Stile icone barra tray" } else { "Tray icon style" },
        false,
        None,
    );
    let sym_half = (sl.card_w - sl.scale(28) - sl.scale(6)) / 2;
    let s1_x1 = sl.cx1 + sl.scale(14);
    let s1_x2 = s1_x1 + sym_half;
    let s2_x1 = s1_x2 + sl.scale(6);
    let s2_x2 = sl.cx1 + sl.card_w - sl.scale(14);
    let is_only_nums = settings.icon_symbols == 0;
    draw_button(
        s1_x1,
        s1_x2,
        r8 + sl.scale(20),
        is_only_nums,
        if is_it { "Solo numeri" } else { "Numbers only" },
        None,
    );
    draw_button(
        s2_x1,
        s2_x2,
        r8 + sl.scale(20),
        !is_only_nums,
        if is_it { "Icona + numero" } else { "Icon + number" },
        None,
    );

    let r9 = sl.card1_row_y(9);
    draw_label(
        sl.cx1 + sl.scale(14),
        r9 + sl.scale(4),
        if is_it { "Pausa a schermo off" } else { "Pause when display off" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r9, settings.pause_display, lbl_on, lbl_off);

    let r10 = sl.card1_row_y(10);
    draw_label(
        sl.cx1 + sl.scale(14),
        r10 + sl.scale(4),
        if is_it { "Rallenta con risparmio" } else { "Slow with battery saver" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r10, settings.slow_energy_saver, lbl_on, lbl_off);

    let r11 = sl.card1_row_y(11);
    draw_label(
        sl.cx1 + sl.scale(14),
        r11 + sl.scale(4),
        if is_it { "EcoQoS (Efficient Core)" } else { "EcoQoS (Efficient Cores)" },
        false,
        None,
    );
    draw_toggle(sl.b1_x1, sl.b1_x2, r11, settings.ecoqos, lbl_on, lbl_off);

    // CARD 2: RIQUADRO HOVER (POPUP)
    draw_card_frame(
        sl.cx2,
        sl.card_y,
        sl.card_w,
        sl.card_h,
        if is_it { "RIQUADRO HOVER (POPUP)" } else { "HOVER POPUP & PERFORMANCE" },
    );

    let c2_r0 = sl.card2_row_y(0);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r0 + sl.scale(4),
        if is_it { "Intervallo polling" } else { "Polling interval" },
        false,
        None,
    );
    let w3 = sl.scale(40);
    let iv1_x1 = sl.cx2 + sl.card_w - sl.scale(14) - (w3 * 3 + sl.scale(12));
    let iv1_x2 = iv1_x1 + w3;
    let iv2_x1 = iv1_x2 + sl.scale(6);
    let iv2_x2 = iv2_x1 + w3;
    let iv3_x1 = iv2_x2 + sl.scale(6);
    let iv3_x2 = sl.cx2 + sl.card_w - sl.scale(14);
    draw_button(iv1_x1, iv1_x2, c2_r0, settings.interval_ms == 1000, "1s", None);
    draw_button(iv2_x1, iv2_x2, c2_r0, settings.interval_ms == 2000, "2s", None);
    draw_button(iv3_x1, iv3_x2, c2_r0, settings.interval_ms >= 5000, "5s", None);

    let c2_r1 = sl.card2_row_y(1);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r1 + sl.scale(4),
        if is_it { "Metodo calcolo CPU" } else { "CPU calculation method" },
        false,
        None,
    );
    let w2 = sl.scale(62);
    let m1_x1 = sl.cx2 + sl.card_w - sl.scale(14) - (w2 * 2 + sl.scale(6));
    let m1_x2 = m1_x1 + w2;
    let m2_x1 = m1_x2 + sl.scale(6);
    let m2_x2 = sl.cx2 + sl.card_w - sl.scale(14);
    draw_button(m1_x1, m1_x2, c2_r1, settings.cpu_mode == crate::settings::CpuMode::Standard, "Standard", None);
    draw_button(
        m2_x1,
        m2_x2,
        c2_r1,
        settings.cpu_mode == crate::settings::CpuMode::Utility,
        if is_it { "Utilità" } else { "Utility" },
        None,
    );

    let c2_r2 = sl.card2_row_y(2);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r2 + sl.scale(4),
        if is_it { "Dettaglio core su hover" } else { "Per-core detail on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r2, settings.cpu_per_core_hover, lbl_on, lbl_off);

    let c2_r3 = sl.card2_row_y(3);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r3 + sl.scale(4),
        if is_it { "Spazio dischi su hover" } else { "Disk space on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r3, settings.disk_space_hover, lbl_on, lbl_off);

    let c2_r4 = sl.card2_row_y(4);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r4 + sl.scale(4),
        if is_it { "Mini grafici (30s) hover" } else { "Sparklines (30s) on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r4, settings.hover_sparklines, lbl_on, lbl_off);

    let c2_r5 = sl.card2_row_y(5);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r5 + sl.scale(4),
        if is_it { "Scheda GPU & VRAM hover" } else { "GPU & VRAM on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r5, settings.hover_gpu, lbl_on, lbl_off);

    let c2_r6 = sl.card2_row_y(6);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r6 + sl.scale(4),
        if is_it { "Top CPU su hover" } else { "Top CPU on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r6, settings.hover_top_cpu, lbl_on, lbl_off);

    let c2_r7 = sl.card2_row_y(7);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r7 + sl.scale(4),
        if is_it { "  Max processi CPU (1-25)" } else { "  Max CPU processes (1-25)" },
        true,
        None,
    );
    draw_stepper(sl.b2_x1, sl.b2_x2, c2_r7, &settings.hover_top_n_cpu.to_string());

    let c2_r8 = sl.card2_row_y(8);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r8 + sl.scale(4),
        if is_it { "Top RAM su hover" } else { "Top RAM on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r8, settings.hover_top_ram, lbl_on, lbl_off);

    let c2_r9 = sl.card2_row_y(9);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r9 + sl.scale(4),
        if is_it { "  Max processi RAM (1-25)" } else { "  Max RAM processes (1-25)" },
        true,
        None,
    );
    draw_stepper(sl.b2_x1, sl.b2_x2, c2_r9, &settings.hover_top_n_ram.to_string());

    let c2_r10 = sl.card2_row_y(10);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r10 + sl.scale(4),
        if is_it { "Top Rete / I/O su hover" } else { "Top Net / I/O on hover" },
        false,
        None,
    );
    draw_toggle(sl.b2_x1, sl.b2_x2, c2_r10, settings.hover_top_net, lbl_on, lbl_off);

    let c2_r11 = sl.card2_row_y(11);
    draw_label(
        sl.cx2 + sl.scale(14),
        c2_r11 + sl.scale(4),
        if is_it { "  Max processi Rete (1-25)" } else { "  Max Net processes (1-25)" },
        true,
        None,
    );
    draw_stepper(sl.b2_x1, sl.b2_x2, c2_r11, &settings.hover_top_n_net.to_string());

    // CARD 3: SISTEMA & AVVISI
    draw_card_frame(
        sl.cx3,
        sl.card_y,
        sl.card_w,
        sl.card_h,
        if is_it { "SISTEMA & AVVISI" } else { "SYSTEM & ALERTS" },
    );

    let is_elevated = crate::sys::elevation::is_elevated();

    let c3_r0 = sl.card3_row_y(0, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r0 + sl.scale(4),
        if is_it { "Avvio automatico (Run)" } else { "Autostart (Run)" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r0, autostart_reg, lbl_on, lbl_off);

    let c3_r1 = sl.card3_row_y(1, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r1 + sl.scale(4),
        if is_it { "Avvio Admin (Task)" } else { "Admin Autostart (Task)" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r1, autostart_admin, lbl_on, lbl_off);

    let c3_r2 = sl.card3_row_y(2, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r2 + sl.scale(4),
        if is_it { "Privilegi correnti" } else { "Current privileges" },
        false,
        None,
    );
    let (badge_text, badge_color) = if is_elevated {
        (if is_it { "● Amministratore" } else { "● Administrator" }, text_cyan)
    } else {
        (if is_it { "○ Utente Standard" } else { "○ Standard User" }, text_yellow)
    };
    draw_label(
        sl.cx3 + sl.card_w - sl.scale(14) - sl.scale(110),
        c3_r2 + sl.scale(4),
        badge_text,
        false,
        Some(badge_color),
    );

    if !is_elevated {
        draw_button(
            sl.cx3 + sl.scale(14),
            sl.cx3 + sl.card_w - sl.scale(14),
            c3_r2 + sl.scale(24),
            false,
            if is_it { "🛡️ Riavvia come Amministratore" } else { "🛡️ Restart as Administrator" },
            Some(text_yellow),
        );
    }

    let c3_r3 = sl.card3_row_y(3, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r3,
        if is_it { "Lingua interfaccia" } else { "Interface language" },
        false,
        None,
    );
    let lang_w = (sl.card_w - sl.scale(28) - sl.scale(12)) / 3;
    let l1_x1 = sl.cx3 + sl.scale(14);
    let l1_x2 = l1_x1 + lang_w;
    let l2_x1 = l1_x2 + sl.scale(6);
    let l2_x2 = l2_x1 + lang_w;
    let l3_x1 = l2_x2 + sl.scale(6);
    let l3_x2 = sl.cx3 + sl.card_w - sl.scale(14);
    draw_button(
        l1_x1,
        l1_x2,
        c3_r3 + sl.scale(20),
        settings.language == crate::settings::Language::Auto,
        "Auto (OS)",
        None,
    );
    draw_button(
        l2_x1,
        l2_x2,
        c3_r3 + sl.scale(20),
        settings.language == crate::settings::Language::Italian,
        "Italiano",
        None,
    );
    draw_button(
        l3_x1,
        l3_x2,
        c3_r3 + sl.scale(20),
        settings.language == crate::settings::Language::English,
        "English",
        None,
    );

    let c3_r4 = sl.card3_row_y(4, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r4 + sl.scale(24),
        if is_it { "AVVISI PROATTIVI (TRAY)" } else { "PROACTIVE ALERTS (TRAY)" },
        false,
        Some(text_cyan),
    );

    let c3_r5 = sl.card3_row_y(5, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r5 + sl.scale(4),
        if is_it { "Allarme carico CPU" } else { "CPU overload alert" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r5, settings.alert_cpu, lbl_on, lbl_off);

    let c3_r6 = sl.card3_row_y(6, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r6 + sl.scale(4),
        if is_it { "  Soglia critica CPU" } else { "  CPU critical threshold" },
        true,
        None,
    );
    draw_stepper(sl.b3_x1, sl.b3_x2, c3_r6, &format!("{}%", settings.alert_cpu_threshold));

    let c3_r7 = sl.card3_row_y(7, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r7 + sl.scale(4),
        if is_it { "Allarme memoria RAM" } else { "RAM memory alert" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r7, settings.alert_ram, lbl_on, lbl_off);

    let c3_r8 = sl.card3_row_y(8, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r8 + sl.scale(4),
        if is_it { "  Soglia critica RAM" } else { "  RAM critical threshold" },
        true,
        None,
    );
    draw_stepper(sl.b3_x1, sl.b3_x2, c3_r8, &format!("{}%", settings.alert_ram_threshold));

    let c3_r9 = sl.card3_row_y(9, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r9 + sl.scale(4),
        if is_it { "Allarme spazio disco" } else { "Low disk space alert" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r9, settings.alert_disk, lbl_on, lbl_off);

    let c3_r10 = sl.card3_row_y(10, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r10 + sl.scale(4),
        if is_it { "  Soglia disco pieno" } else { "  Full disk threshold" },
        true,
        None,
    );
    draw_stepper(sl.b3_x1, sl.b3_x2, c3_r10, &format!("{}%", settings.alert_disk_threshold));

    let c3_r11 = sl.card3_row_y(11, is_elevated);
    draw_button(
        sl.cx3 + sl.scale(14),
        sl.cx3 + sl.card_w - sl.scale(14),
        c3_r11,
        false,
        if is_it { "🔄 Controlla aggiornamenti..." } else { "🔄 Check for updates..." },
        None,
    );

    let c3_r12 = sl.card3_row_y(12, is_elevated);
    draw_label(
        sl.cx3 + sl.scale(14),
        c3_r12 + sl.scale(4),
        if is_it { "Registratore Blackbox (CSV)" } else { "Blackbox Recorder (CSV)" },
        false,
        None,
    );
    draw_toggle(sl.b3_x1, sl.b3_x2, c3_r12, settings.blackbox_recording, lbl_on, lbl_off);

    let c3_r13 = sl.card3_row_y(13, is_elevated);
    draw_button(
        sl.cx3 + sl.scale(14),
        sl.cx3 + sl.card_w - sl.scale(14),
        c3_r13,
        false,
        if is_it { "📁 Apri cartella Blackbox (CSV)" } else { "📁 Open Blackbox folder (CSV)" },
        None,
    );

    SelectObject(dc, old_font);
}

fn handle_settings_click(x: i32, y: i32, w: i32, h: i32, dpi: u32) -> bool {
    let Some((settings, _autostart_reg, _autostart_admin)) = crate::app::get_settings_snapshot() else {
        return false;
    };
    let sl = SettingsLayout::new(w, h, dpi);
    let is_elevated = crate::sys::elevation::is_elevated();

    // Card 1
    if (sl.b1_x1..sl.b1_x2).contains(&x) {
        let in_row = |r: i32| (sl.card1_row_y(r)..sl.card1_row_y(r) + sl.btn_h).contains(&y);
        if in_row(0) {
            crate::app::execute_command(crate::app::CMD_CPU_ICON);
            return true;
        } else if in_row(1) {
            crate::app::execute_command(crate::app::CMD_RAM_ICON);
            return true;
        } else if in_row(2) {
            crate::app::execute_command(crate::app::CMD_GPU_ICON);
            return true;
        } else if in_row(3) {
            crate::app::execute_command(crate::app::CMD_NET_ICON);
            return true;
        } else if in_row(4) {
            crate::app::execute_command(crate::app::CMD_TEMP_ACPI_ICON);
            return true;
        } else if in_row(5) {
            crate::app::execute_command(crate::app::CMD_TEMP_GPU_ICON);
            return true;
        } else if in_row(6) {
            crate::app::execute_command(crate::app::CMD_TEMP_DISK_ICON);
            return true;
        } else if in_row(9) {
            crate::app::execute_command(crate::app::CMD_PAUSE_DISPLAY);
            return true;
        } else if in_row(10) {
            crate::app::execute_command(crate::app::CMD_SLOW_SAVER);
            return true;
        } else if in_row(11) {
            crate::app::execute_command(crate::app::CMD_ECOQOS);
            return true;
        }
    }

    // Card 1 - Rete Bit/s o Byte/s
    let r7 = sl.card1_row_y(7);
    if (r7..r7 + sl.btn_h).contains(&y) {
        let u1_x1 = sl.cx1 + sl.card_w - sl.scale(14) - sl.scale(130);
        let u1_x2 = u1_x1 + sl.scale(62);
        let u2_x1 = u1_x2 + sl.scale(6);
        let u2_x2 = sl.cx1 + sl.card_w - sl.scale(14);
        if ((u1_x1..u1_x2).contains(&x) && !settings.net_bits) || ((u2_x1..u2_x2).contains(&x) && settings.net_bits) {
            crate::app::execute_command(crate::app::CMD_NET_BITS);
            return true;
        }
    }

    // Card 1 - Stile icone: Solo numeri vs Icona + numero
    let r8_btn_y = sl.card1_row_y(8) + sl.scale(20);
    if (r8_btn_y..r8_btn_y + sl.btn_h).contains(&y) {
        let sym_half = (sl.card_w - sl.scale(28) - sl.scale(6)) / 2;
        let s1_x1 = sl.cx1 + sl.scale(14);
        let s1_x2 = s1_x1 + sym_half;
        let s2_x1 = s1_x2 + sl.scale(6);
        let s2_x2 = sl.cx1 + sl.card_w - sl.scale(14);
        if (s1_x1..s1_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_ICON_SYMBOLS_OFF);
            return true;
        } else if (s2_x1..s2_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_ICON_SYMBOLS_ON);
            return true;
        }
    }

    // Card 2 - Intervallo polling
    let c2_r0 = sl.card2_row_y(0);
    if (c2_r0..c2_r0 + sl.btn_h).contains(&y) {
        let w3 = sl.scale(40);
        let iv1_x1 = sl.cx2 + sl.card_w - sl.scale(14) - (w3 * 3 + sl.scale(12));
        let iv1_x2 = iv1_x1 + w3;
        let iv2_x1 = iv1_x2 + sl.scale(6);
        let iv2_x2 = iv2_x1 + w3;
        let iv3_x1 = iv2_x2 + sl.scale(6);
        let iv3_x2 = sl.cx2 + sl.card_w - sl.scale(14);
        if (iv1_x1..iv1_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_INTERVAL_1S);
            return true;
        } else if (iv2_x1..iv2_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_INTERVAL_2S);
            return true;
        } else if (iv3_x1..iv3_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_INTERVAL_5S);
            return true;
        }
    }

    // Card 2 - Metodo CPU
    let c2_r1 = sl.card2_row_y(1);
    if (c2_r1..c2_r1 + sl.btn_h).contains(&y) {
        let w2 = sl.scale(62);
        let m1_x1 = sl.cx2 + sl.card_w - sl.scale(14) - (w2 * 2 + sl.scale(6));
        let m1_x2 = m1_x1 + w2;
        let m2_x1 = m1_x2 + sl.scale(6);
        let m2_x2 = sl.cx2 + sl.card_w - sl.scale(14);
        if ((m1_x1..m1_x2).contains(&x) && settings.cpu_mode != crate::settings::CpuMode::Standard)
            || ((m2_x1..m2_x2).contains(&x) && settings.cpu_mode != crate::settings::CpuMode::Utility)
        {
            crate::app::execute_command(crate::app::CMD_CPU_UTILITY);
            return true;
        }
    }

    // Card 2 - Toggles e Steppers
    if (sl.b2_x1..sl.b2_x2).contains(&x) {
        let in_row = |r: i32| (sl.card2_row_y(r)..sl.card2_row_y(r) + sl.btn_h).contains(&y);
        let step_w = sl.scale(24);
        let is_minus = (sl.b2_x1..sl.b2_x1 + step_w).contains(&x);
        let is_plus = (sl.b2_x2 - step_w..sl.b2_x2).contains(&x);

        if in_row(2) {
            crate::app::execute_command(crate::app::CMD_CPU_PER_CORE);
            return true;
        } else if in_row(3) {
            crate::app::execute_command(crate::app::CMD_DISK_SPACE_HOVER);
            return true;
        } else if in_row(4) {
            crate::app::execute_command(crate::app::CMD_HOVER_SPARKLINES);
            return true;
        } else if in_row(5) {
            crate::app::execute_command(crate::app::CMD_HOVER_GPU);
            return true;
        } else if in_row(6) {
            crate::app::execute_command(crate::app::CMD_HOVER_TOP_CPU);
            return true;
        } else if in_row(7) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_TOP_CPU_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_TOP_CPU_INC);
                return true;
            }
        } else if in_row(8) {
            crate::app::execute_command(crate::app::CMD_HOVER_TOP_RAM);
            return true;
        } else if in_row(9) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_TOP_RAM_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_TOP_RAM_INC);
                return true;
            }
        } else if in_row(10) {
            crate::app::execute_command(crate::app::CMD_HOVER_TOP_NET);
            return true;
        } else if in_row(11) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_TOP_NET_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_TOP_NET_INC);
                return true;
            }
        }
    }

    // Card 3
    if (sl.b3_x1..sl.b3_x2).contains(&x) {
        let in_row = |r: i32| (sl.card3_row_y(r, is_elevated)..sl.card3_row_y(r, is_elevated) + sl.btn_h).contains(&y);
        let step_w = sl.scale(24);
        let is_minus = (sl.b3_x1..sl.b3_x1 + step_w).contains(&x);
        let is_plus = (sl.b3_x2 - step_w..sl.b3_x2).contains(&x);

        if in_row(0) {
            crate::app::execute_command(crate::app::CMD_AUTOSTART);
            return true;
        } else if in_row(1) {
            crate::app::execute_command(crate::app::CMD_AUTOSTART_ADMIN);
            return true;
        } else if in_row(5) {
            crate::app::execute_command(crate::app::CMD_ALERT_CPU_TOGGLE);
            return true;
        } else if in_row(6) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_ALERT_CPU_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_ALERT_CPU_INC);
                return true;
            }
        } else if in_row(7) {
            crate::app::execute_command(crate::app::CMD_ALERT_RAM_TOGGLE);
            return true;
        } else if in_row(8) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_ALERT_RAM_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_ALERT_RAM_INC);
                return true;
            }
        } else if in_row(9) {
            crate::app::execute_command(crate::app::CMD_ALERT_DISK_TOGGLE);
            return true;
        } else if in_row(10) {
            if is_minus {
                crate::app::execute_command(crate::app::CMD_ALERT_DISK_DEC);
                return true;
            } else if is_plus {
                crate::app::execute_command(crate::app::CMD_ALERT_DISK_INC);
                return true;
            }
        } else if in_row(12) {
            crate::app::execute_command(crate::app::CMD_BLACKBOX_TOGGLE);
            return true;
        }
    }

    // Card 3 full-width action buttons
    let act3_x1 = sl.cx3 + sl.scale(14);
    let act3_x2 = sl.cx3 + sl.card_w - sl.scale(14);
    if (act3_x1..act3_x2).contains(&x) {
        if !is_elevated {
            let adm_y = sl.card3_row_y(2, is_elevated) + sl.scale(24);
            if (adm_y..adm_y + sl.btn_h).contains(&y) {
                crate::app::execute_command(crate::app::CMD_RESTART_ADMIN);
                return true;
            }
        }

        let c3_r11 = sl.card3_row_y(11, is_elevated);
        if (c3_r11..c3_r11 + sl.btn_h).contains(&y) {
            crate::app::execute_command(crate::app::CMD_CHECK_UPDATES);
            return true;
        }

        let c3_r13 = sl.card3_row_y(13, is_elevated);
        if (c3_r13..c3_r13 + sl.btn_h).contains(&y) {
            crate::app::execute_command(crate::app::CMD_BLACKBOX_OPEN_DIR);
            return true;
        }
    }

    // Card 3 - Lingua interfaccia
    let c3_r3_btn_y = sl.card3_row_y(3, is_elevated) + sl.scale(20);
    if (c3_r3_btn_y..c3_r3_btn_y + sl.btn_h).contains(&y) {
        let lang_w = (sl.card_w - sl.scale(28) - sl.scale(12)) / 3;
        let l1_x1 = sl.cx3 + sl.scale(14);
        let l1_x2 = l1_x1 + lang_w;
        let l2_x1 = l1_x2 + sl.scale(6);
        let l2_x2 = l2_x1 + lang_w;
        let l3_x1 = l2_x2 + sl.scale(6);
        let l3_x2 = sl.cx3 + sl.card_w - sl.scale(14);

        if (l1_x1..l1_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_LANG_AUTO);
            return true;
        } else if (l2_x1..l2_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_LANG_IT);
            return true;
        } else if (l3_x1..l3_x2).contains(&x) {
            crate::app::execute_command(crate::app::CMD_LANG_EN);
            return true;
        }
    }

    false
}

struct InfoLayout {
    pub dpi: u32,
    pub start_x: i32,
    pub start_y: i32,
    pub icon_size: i32,
    pub text_x: i32,
    pub title_y: i32,
    pub sub_y: i32,
    pub meta_y: i32,
    pub badge_y: i32,
    pub btn_w: i32,
    pub btn_h: i32,
    pub btn_x1: i32,
    pub btn_x2: i32,
    pub btn_y: i32,
    pub status_y: i32,
    pub dl_y: i32,
    pub card_y: i32,
    pub card_w: i32,
    pub card_h: i32,
    pub card1_x: i32,
    pub card2_x: i32,
    pub card3_x: i32,
    pub card_hdr_h: i32,
    pub item_step: i32,
    pub val_offset: i32,
}

impl InfoLayout {
    pub fn new(w: i32, h: i32, dpi: u32) -> Self {
        let scale = |v: i32| (v * dpi as i32 + 48) / 96;
        let start_x = scale(32);
        let start_y = scale(56);
        let icon_size = scale(80).clamp(64, 96);
        let text_x = start_x + icon_size + scale(20);

        let title_y = start_y;
        let sub_y = title_y + scale(34);
        let meta_y = sub_y + scale(24);
        let badge_y = meta_y + scale(24);

        let btn_w = scale(210).max(180);
        let btn_h = scale(28).max(24);
        let btn_x2 = w - start_x;
        let btn_x1 = btn_x2 - btn_w;
        let btn_y = start_y + scale(4);
        let status_y = btn_y + btn_h + scale(6);
        let dl_y = status_y + scale(24);

        let btm_h = (44 * dpi as i32 + 48) / 96;
        let card_y = (badge_y + scale(34)).max(start_y + icon_size + scale(20));
        let card_gap = scale(16);
        let card_w = ((w - start_x * 2 - card_gap * 2) / 3).max(scale(200));
        let card_h = (h - btm_h - scale(16) - card_y).max(scale(240));

        let card1_x = start_x;
        let card2_x = start_x + card_w + card_gap;
        let card3_x = start_x + (card_w + card_gap) * 2;

        let card_hdr_h = scale(36).max(30);
        let item_step = scale(46).max(40);
        let val_offset = scale(18).max(16);

        Self {
            dpi,
            start_x,
            start_y,
            icon_size,
            text_x,
            title_y,
            sub_y,
            meta_y,
            badge_y,
            btn_w,
            btn_h,
            btn_x1,
            btn_x2,
            btn_y,
            status_y,
            dl_y,
            card_y,
            card_w,
            card_h,
            card1_x,
            card2_x,
            card3_x,
            card_hdr_h,
            item_step,
            val_offset,
        }
    }

    #[inline]
    pub fn scale(&self, v: i32) -> i32 {
        (v * self.dpi as i32 + 48) / 96
    }
}

unsafe fn paint_info_page(dc: HDC, w: i32, h: i32, is_dark: bool, st: &mut InspectState, _s: &'static Strings) {
    let il = InfoLayout::new(w, h, st.dpi);
    let is_it = st.is_italian();

    let border_color = if is_dark { 0x003A3A3A } else { 0x00D0D0D0 };
    let active_tab_bg = if is_dark { 0x002B2B2B } else { 0x000078D7 };
    let inactive_tab_bg = if is_dark { 0x00202020 } else { 0x00F0F0F0 };
    let text_normal = if is_dark { 0x00EDEDED } else { 0x001A1A1A };
    let text_dim = if is_dark { 0x00888888 } else { 0x00666666 };
    let text_cyan = if is_dark { 0x00D77800 } else { 0x008A4500 };
    let text_yellow = if is_dark { 0x0000D7FF } else { 0x000080B0 };

    // 1. Icona applicazione
    if !st.icon_96.is_null() {
        DrawIconEx(dc, il.start_x, il.start_y, st.icon_96 as _, il.icon_size, il.icon_size, 0, null_mut(), DI_NORMAL);
    }

    // 2. Titolo HERO "nextm"
    SelectObject(dc, st.fonts.hero);
    SetTextColor(dc, if is_dark { 0x00FFFFFF } else { 0x00000000 });
    let title = wide!("nextm");
    TextOutW(dc, il.text_x, il.title_y, title.as_ptr(), 5);

    // 3. Sottotitolo
    SelectObject(dc, st.fonts.info_sub);
    SetTextColor(dc, text_dim);
    let sub_text = if is_it {
        "Monitor di sistema leggerissimo per Windows 11"
    } else {
        "Ultra-lightweight system monitor for Windows 11"
    };
    let sub_wstr: Vec<u16> = sub_text.encode_utf16().collect();
    TextOutW(dc, il.text_x, il.sub_y, sub_wstr.as_ptr(), sub_wstr.len() as i32);

    // 4. Meta line (versione, autore con link, github con link)
    SelectObject(dc, st.fonts.body);
    let line_y = il.meta_y;
    let mut cur_x = il.text_x;

    SetTextColor(dc, text_dim);
    let prefix =
        format!("v{} • x86_64 • {}", env!("CARGO_PKG_VERSION"), if is_it { "Sviluppato da " } else { "Developed by " });
    let p_wstr: Vec<u16> = prefix.encode_utf16().collect();
    let mut sz: windows_sys::Win32::Foundation::SIZE = zeroed();
    GetTextExtentPoint32W(dc, p_wstr.as_ptr(), p_wstr.len() as i32, &mut sz);
    TextOutW(dc, cur_x, line_y, p_wstr.as_ptr(), p_wstr.len() as i32);
    cur_x += sz.cx;

    SetTextColor(dc, text_cyan);
    let author = "Lorenzo Pompili";
    let a_wstr: Vec<u16> = author.encode_utf16().collect();
    GetTextExtentPoint32W(dc, a_wstr.as_ptr(), a_wstr.len() as i32, &mut sz);
    TextOutW(dc, cur_x, line_y, a_wstr.as_ptr(), a_wstr.len() as i32);
    st.info_author_rect = RECT { left: cur_x, top: line_y, right: cur_x + sz.cx, bottom: line_y + sz.cy };
    cur_x += sz.cx;

    SetTextColor(dc, text_dim);
    let sep = " • GitHub: ";
    let s_wstr: Vec<u16> = sep.encode_utf16().collect();
    GetTextExtentPoint32W(dc, s_wstr.as_ptr(), s_wstr.len() as i32, &mut sz);
    TextOutW(dc, cur_x, line_y, s_wstr.as_ptr(), s_wstr.len() as i32);
    cur_x += sz.cx;

    SetTextColor(dc, text_cyan);
    let repo = "lorenzopompili/nextm";
    let r_wstr: Vec<u16> = repo.encode_utf16().collect();
    GetTextExtentPoint32W(dc, r_wstr.as_ptr(), r_wstr.len() as i32, &mut sz);
    TextOutW(dc, cur_x, line_y, r_wstr.as_ptr(), r_wstr.len() as i32);
    st.info_repo_rect = RECT { left: cur_x, top: line_y, right: cur_x + sz.cx, bottom: line_y + sz.cy };

    // 5. Badge privilegi
    let is_elevated = crate::sys::elevation::is_elevated();
    SelectObject(dc, st.fonts.badge);
    let (status_badge, badge_color) = if is_elevated {
        (
            if is_it {
                "● Esecuzione con privilegi di Amministratore"
            } else {
                "● Running with Administrator privileges"
            },
            text_cyan,
        )
    } else {
        (
            if is_it {
                "○ Esecuzione Utente Standard (alcune funzioni richiedono Admin)"
            } else {
                "○ Standard User mode (some features require Admin)"
            },
            text_yellow,
        )
    };
    SetTextColor(dc, badge_color);
    let badge_wstr: Vec<u16> = status_badge.encode_utf16().collect();
    TextOutW(dc, il.text_x, il.badge_y, badge_wstr.as_ptr(), badge_wstr.len() as i32);

    // 6. Pulsante Controllo Aggiornamenti
    let update_state = crate::sys::update::get_update_state();
    let is_checking = update_state == crate::sys::update::UpdateState::Checking;
    let btn_bg = if is_checking { active_tab_bg } else { inactive_tab_bg };
    let b_brush = CreateSolidBrush(btn_bg);
    let btn_rc = RECT { left: il.btn_x1, top: il.btn_y, right: il.btn_x2, bottom: il.btn_y + il.btn_h };
    FillRect(dc, &btn_rc, b_brush);
    DeleteObject(b_brush);

    let b_border = CreateSolidBrush(border_color);
    FrameRect(dc, &btn_rc, b_border);
    DeleteObject(b_border);

    st.info_update_btn_rect = btn_rc;

    SelectObject(dc, st.fonts.btn);
    SetTextColor(dc, if is_checking { 0x00FFFFFF } else { text_normal });
    let btn_label = if is_checking {
        if is_it { "⏳ Verifica in corso…" } else { "⏳ Checking…" }
    } else {
        if is_it { "🔄 Controlla aggiornamenti" } else { "🔄 Check for updates" }
    };
    let bl_wstr: Vec<u16> = btn_label.encode_utf16().collect();
    let mut bsz: windows_sys::Win32::Foundation::SIZE = zeroed();
    GetTextExtentPoint32W(dc, bl_wstr.as_ptr(), bl_wstr.len() as i32, &mut bsz);
    let tx = il.btn_x1 + (il.btn_w - bsz.cx) / 2;
    let ty = il.btn_y + (il.btn_h - bsz.cy) / 2;
    TextOutW(dc, tx, ty, bl_wstr.as_ptr(), bl_wstr.len() as i32);

    // Stato dell'aggiornamento
    SelectObject(dc, st.fonts.badge);
    let (status_text, status_color, has_download_btn) = match &update_state {
        crate::sys::update::UpdateState::Idle => (
            (if is_it { "Verifica su GitHub Releases" } else { "Checks GitHub Releases" }).to_string(),
            text_dim,
            false,
        ),
        crate::sys::update::UpdateState::Checking => (
            (if is_it { "Connessione ad api.github.com..." } else { "Connecting to api.github.com..." }).to_string(),
            text_yellow,
            false,
        ),
        crate::sys::update::UpdateState::UpToDate { version } => (
            if is_it { format!("✔ Versione aggiornata ({version})") } else { format!("✔ Up to date ({version})") },
            text_cyan,
            false,
        ),
        crate::sys::update::UpdateState::NewVersion { version, .. } => (
            if is_it { format!("★ Nuova versione {version}!") } else { format!("★ New version {version}!") },
            0x0020D000,
            true,
        ),
        crate::sys::update::UpdateState::Error(err) => {
            (if is_it { format!("✖ Errore: {err}") } else { format!("✖ Error: {err}") }, 0x005050FF, false)
        }
    };
    SetTextColor(dc, status_color);
    let st_wstr: Vec<u16> = status_text.encode_utf16().collect();
    let mut st_sz: windows_sys::Win32::Foundation::SIZE = zeroed();
    GetTextExtentPoint32W(dc, st_wstr.as_ptr(), st_wstr.len() as i32, &mut st_sz);
    let st_tx = il.btn_x1 + (il.btn_w - st_sz.cx) / 2;
    TextOutW(dc, st_tx, il.status_y, st_wstr.as_ptr(), st_wstr.len() as i32);

    if has_download_btn {
        let dl_rc = RECT { left: il.btn_x1, top: il.dl_y, right: il.btn_x2, bottom: il.dl_y + il.btn_h };
        let dl_brush = CreateSolidBrush(active_tab_bg);
        FillRect(dc, &dl_rc, dl_brush);
        DeleteObject(dl_brush);

        let dl_border = CreateSolidBrush(border_color);
        FrameRect(dc, &dl_rc, dl_border);
        DeleteObject(dl_border);

        st.info_download_btn_rect = dl_rc;

        SelectObject(dc, st.fonts.btn);
        SetTextColor(dc, 0x00FFFFFF);
        let dl_label = if is_it { "⬇ Scarica aggiornamento" } else { "⬇ Download update" };
        let dl_wstr: Vec<u16> = dl_label.encode_utf16().collect();
        let mut dlsz: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, dl_wstr.as_ptr(), dl_wstr.len() as i32, &mut dlsz);
        let dltx = il.btn_x1 + (il.btn_w - dlsz.cx) / 2;
        let dlty = il.dl_y + (il.btn_h - dlsz.cy) / 2;
        TextOutW(dc, dltx, dlty, dl_wstr.as_ptr(), dl_wstr.len() as i32);
    } else {
        st.info_download_btn_rect = zeroed();
    }

    // 7. Tre Card informative
    let draw_card = |dc: HDC, cx: i32, cy: i32, cw: i32, ch: i32, title: &str, items: &[(&str, &str)]| {
        let card_rc = RECT { left: cx, top: cy, right: cx + cw, bottom: cy + ch };
        let c_bg = if is_dark { 0x00262626 } else { 0x00FAFAFA };
        let c_brush = CreateSolidBrush(c_bg);
        FillRect(dc, &card_rc, c_brush);
        DeleteObject(c_brush);

        let c_border = CreateSolidBrush(border_color);
        FrameRect(dc, &card_rc, c_border);
        DeleteObject(c_border);

        let hdr_rc = RECT { left: cx, top: cy, right: cx + cw, bottom: cy + il.card_hdr_h };
        let h_bg = if is_dark { 0x002D2D2D } else { 0x00EDEDED };
        let h_brush = CreateSolidBrush(h_bg);
        FillRect(dc, &hdr_rc, h_brush);
        DeleteObject(h_brush);

        let div_brush = CreateSolidBrush(border_color);
        let div_rc = RECT { left: cx, top: cy + il.card_hdr_h - 1, right: cx + cw, bottom: cy + il.card_hdr_h };
        FillRect(dc, &div_rc, div_brush);
        DeleteObject(div_brush);

        SelectObject(dc, st.fonts.title);
        SetTextColor(dc, text_cyan);
        let tw: Vec<u16> = title.encode_utf16().collect();
        let mut tsz: windows_sys::Win32::Foundation::SIZE = zeroed();
        GetTextExtentPoint32W(dc, tw.as_ptr(), tw.len() as i32, &mut tsz);
        let title_pad_x = il.scale(14);
        let title_y = cy + (il.card_hdr_h - tsz.cy) / 2;
        TextOutW(dc, cx + title_pad_x, title_y, tw.as_ptr(), tw.len() as i32);

        SelectObject(dc, st.fonts.body);
        let mut iy = cy + il.card_hdr_h + il.scale(10);
        for &(label, val) in items {
            SetTextColor(dc, text_dim);
            let lw: Vec<u16> = label.encode_utf16().collect();
            TextOutW(dc, cx + title_pad_x, iy, lw.as_ptr(), lw.len() as i32);

            SetTextColor(dc, text_normal);
            let vw: Vec<u16> = val.encode_utf16().collect();
            TextOutW(dc, cx + title_pad_x, iy + il.val_offset, vw.as_ptr(), vw.len() as i32);

            iy += il.item_step;
        }
    };

    let procs_count_str = format!("{} {}", st.procs.len(), if is_it { "attivi" } else { "active" });
    let sock_count_str = format!("{} {}", st.sockets.len(), if is_it { "attivi" } else { "active" });
    let svc_count_str = format!("{} {}", st.services.len(), if is_it { "configurati" } else { "configured" });

    draw_card(
        dc,
        il.card1_x,
        il.card_y,
        il.card_w,
        il.card_h,
        if is_it { "STATO SISTEMA" } else { "SYSTEM STATUS" },
        &[
            (if is_it { "Processi monitorati" } else { "Monitored processes" }, &procs_count_str),
            (if is_it { "Socket di rete" } else { "Network sockets" }, &sock_count_str),
            (if is_it { "Servizi Windows" } else { "Windows services" }, &svc_count_str),
            (if is_it { "Campionamento" } else { "Sampling" }, if is_it { "1.0s continuo" } else { "1.0s continuous" }),
        ],
    );

    draw_card(
        dc,
        il.card2_x,
        il.card_y,
        il.card_w,
        il.card_h,
        if is_it { "SICUREZZA & KERNEL" } else { "SECURITY & KERNEL" },
        &[
            (if is_it { "Autore & Sviluppo" } else { "Author & Development" }, "Lorenzo Pompili"),
            (if is_it { "API Kernel" } else { "Kernel APIs" }, "NtQuerySystemInformation"),
            (if is_it { "Protezioni PE" } else { "PE Protections" }, "DEP, CFG, ASLR 64-bit"),
            (
                if is_it { "Librerie runtime" } else { "Runtime libraries" },
                if is_it { "0 DLL esterne (Win32 pura)" } else { "0 external DLLs (pure Win32)" },
            ),
        ],
    );

    let card2_author_y = il.card_y + il.card_hdr_h + il.scale(10);
    st.info_card2_author_rect = RECT {
        left: il.card2_x + il.scale(14),
        top: card2_author_y,
        right: il.card2_x + il.card_w - il.scale(14),
        bottom: card2_author_y + il.item_step,
    };

    draw_card(
        dc,
        il.card3_x,
        il.card_y,
        il.card_w,
        il.card_h,
        if is_it { "COMANDI RAPIDI" } else { "KEYBOARD SHORTCUTS" },
        &[
            (
                if is_it { "Aggiorna dati" } else { "Refresh data" },
                if is_it { "F5 / Bottone Aggiorna" } else { "F5 / Refresh Button" },
            ),
            (
                if is_it { "Espandi / Comprimi" } else { "Expand / Collapse" },
                if is_it { "Spazio / Doppio Clic" } else { "Space / Double Click" },
            ),
            (
                if is_it { "Navigazione ad albero" } else { "Tree navigation" },
                if is_it { "Frecce Su/Giù/Sinistra/Destra" } else { "Arrows Up/Down/Left/Right" },
            ),
            (
                if is_it { "Chiusura immediata" } else { "Immediate termination" },
                if is_it { "Canc / Del su riga selezionata" } else { "Del on selected row" },
            ),
        ],
    );
}

fn handle_info_click(hwnd: HWND, x: i32, y: i32) -> bool {
    let (author_rc, repo_rc, btn_rc, dl_rc, card2_rc) = STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|st| {
                (
                    st.info_author_rect,
                    st.info_repo_rect,
                    st.info_update_btn_rect,
                    st.info_download_btn_rect,
                    st.info_card2_author_rect,
                )
            })
            .unwrap_or_default()
    });

    // 1. Link email autore nell'intestazione
    if (author_rc.left..=author_rc.right).contains(&x) && (author_rc.top..=author_rc.bottom).contains(&y) {
        let mailto = wide!("mailto:lorenzo.pompili@gmail.com");
        crate::sys::shell::open_with_explorer(mailto);
        return true;
    }

    // 2. Link repository GitHub nell'intestazione
    if (repo_rc.left..=repo_rc.right).contains(&x) && (repo_rc.top..=repo_rc.bottom).contains(&y) {
        let github = wide!("https://github.com/lorenzopompili/nextm");
        crate::sys::shell::open_with_explorer(github);
        return true;
    }

    // 3. Click su Autore & Sviluppo nella Card 2
    if card2_rc.right > 0
        && (card2_rc.left..=card2_rc.right).contains(&x)
        && (card2_rc.top..=card2_rc.bottom).contains(&y)
    {
        let mailto = wide!("mailto:lorenzo.pompili@gmail.com");
        crate::sys::shell::open_with_explorer(mailto);
        return true;
    }

    // 4. Click sul pulsante "Controlla aggiornamenti"
    if (btn_rc.left..=btn_rc.right).contains(&x) && (btn_rc.top..=btn_rc.bottom).contains(&y) {
        crate::sys::update::check_for_updates_async(Some(hwnd));
        return true;
    }

    // 5. Click sul pulsante "Scarica aggiornamento" (se presente)
    if dl_rc.right > 0 && (dl_rc.left..=dl_rc.right).contains(&x) && (dl_rc.top..=dl_rc.bottom).contains(&y) {
        let update_state = crate::sys::update::get_update_state();
        if let crate::sys::update::UpdateState::NewVersion { url, .. } = update_state {
            let url_wstr: Vec<u16> = url.encode_utf16().chain(core::iter::once(0)).collect();
            crate::sys::shell::open_with_explorer(&url_wstr);
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_window_creates_successfully() {
        let mut win = InspectWindow::new(&crate::strings::IT, 96).unwrap();
        assert!(!win.is_visible());
        win.show(true);
        assert!(win.is_visible());
        win.hide();
        assert!(!win.is_visible());
    }

    #[test]
    fn selection_tracks_entry_identity_across_sorting_and_changes() {
        let mut st = InspectState {
            hwnd: null_mut(),
            search_edit: null_mut(),
            engine: InspectEngine::new(),
            active_tab: InspectTab::Processes,
            procs: vec![
                ProcessRow {
                    pid: 100,
                    name: "alpha.exe".into(),
                    cpu_percent: 10.0,
                    ws_bytes: 100,
                    priv_bytes: 100,
                    threads: 1,
                    services_summary: String::new(),
                    sockets_count: 0,
                    is_hung: false,
                    is_leak: false,
                    mem_delta_mb: 0,
                },
                ProcessRow {
                    pid: 200,
                    name: "beta.exe".into(),
                    cpu_percent: 50.0,
                    ws_bytes: 200,
                    priv_bytes: 200,
                    threads: 2,
                    services_summary: String::new(),
                    sockets_count: 0,
                    is_hung: false,
                    is_leak: false,
                    mem_delta_mb: 0,
                },
                ProcessRow {
                    pid: 300,
                    name: "gamma.exe".into(),
                    cpu_percent: 5.0,
                    ws_bytes: 300,
                    priv_bytes: 300,
                    threads: 3,
                    services_summary: String::new(),
                    sockets_count: 0,
                    is_hung: false,
                    is_leak: false,
                    mem_delta_mb: 0,
                },
            ],
            proc_items: Vec::new(),
            group_procs: false,
            expanded_groups: BTreeSet::new(),
            sockets: Vec::new(),
            socket_items: Vec::new(),
            group_sockets: false,
            expanded_socket_groups: BTreeSet::new(),
            services: Vec::new(),
            filtered_services: Vec::new(),
            hung_windows: Vec::new(),
            hung_kill_rect: None,
            proc_mem_history: BTreeMap::new(),
            filter_text: String::new(),
            selected_index: None,
            selected_key: None,
            scroll_y: 0,
            hover_row: None,
            sort_col: 2,     // CPU %
            sort_asc: false, // Descending: beta (50%), alpha (10%), gamma (5%)
            status_msg: None,
            is_dark: true,
            dpi: 96,
            fonts: unsafe { InspectFonts::new(96) },
            s: &crate::strings::IT,
            col_widths_procs: [220, 80, 80, 110, 110, 70],
            col_widths_sockets: [200, 70, 60, 180, 180],
            col_widths_services: [200, 280, 120, 100],
            hover_col_sep: None,
            resizing_col: None,
            tab_rects: unsafe { [zeroed(); 5] },
            group_btn_rect: unsafe { zeroed() },
            bottom_btn_rects: unsafe { [zeroed(); 3] },
            is_dragging_scrollbar: false,
            scrollbar_drag_start_y: 0,
            scrollbar_drag_start_scroll: 0,
            last_mouse_x: 0,
            last_mouse_y: 0,
            info_author_rect: unsafe { zeroed() },
            info_repo_rect: unsafe { zeroed() },
            info_update_btn_rect: unsafe { zeroed() },
            info_download_btn_rect: unsafe { zeroed() },
            info_card2_author_rect: unsafe { zeroed() },
            edit_brush_dark: null_mut(),
            edit_brush_light: null_mut(),
            icon_96: null_mut(),
        };

        st.apply_sorting();
        st.apply_filter();

        // Inizialmente: indice 0 è beta.exe (50%), indice 1 è alpha.exe (10%), indice 2 è gamma.exe (5%)
        assert_eq!(st.proc_items.len(), 3);
        match &st.proc_items[0] {
            ProcItem::Single { proc_idx } => assert_eq!(st.procs[*proc_idx].pid, 200),
            _ => panic!("atteso ProcItem::Single"),
        }

        // Seleziona beta.exe all'indice 0
        st.set_selected_index(Some(0));
        assert_eq!(st.selected_key, Some(SelectedKey::ProcessPid(200)));

        // Ordina per Nome crescente: alpha (0), beta (1), gamma (2)
        st.sort_col = 0;
        st.sort_asc = true;
        st.apply_sorting();
        st.apply_filter();

        // beta.exe si è spostato all'indice 1! La selezione lo ha seguito!
        assert_eq!(st.selected_index, Some(1));
        match &st.proc_items[1] {
            ProcItem::Single { proc_idx } => assert_eq!(st.procs[*proc_idx].pid, 200),
            _ => panic!("atteso ProcItem::Single"),
        }

        // Modifica la CPU di beta a 1.0% e ordina per CPU decrescente: alpha (10%), gamma (5%), beta (1%)
        for p in &mut st.procs {
            if p.pid == 200 {
                p.cpu_percent = 1.0;
            }
        }
        st.sort_col = 2;
        st.sort_asc = false;
        st.apply_sorting();
        st.apply_filter();

        // beta.exe si è spostato all'indice 2!
        assert_eq!(st.selected_index, Some(2));
        match &st.proc_items[2] {
            ProcItem::Single { proc_idx } => assert_eq!(st.procs[*proc_idx].pid, 200),
            _ => panic!("atteso ProcItem::Single"),
        }

        // Se beta.exe termina ed esce dal processo:
        st.procs.retain(|p| p.pid != 200);
        st.apply_sorting();
        st.apply_filter();
        assert_eq!(st.selected_index, None);
        assert_eq!(st.selected_key, None);
    }
}
