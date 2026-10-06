//! Finestra popup ricca per l'hover sulle icone della tray ("riquadro").
//!
//! Mostra tutte le metriche attive in una scheda moderna di Windows 11:
//! - CPU, saturazione e dettaglio per-core in colonne verticali pulite
//! - RAM (percentuale e GB)
//! - Rete (download e upload con interfaccia)
//! - Temperature (CPU ACPI, GPU con modello, Disco con etichetta)
//!
//! Non prende mai il focus (`WS_EX_NOACTIVATE`), angoli arrotondati e tema scuro/chiaro DWM,
//! doppio buffer GDI senza sfarfallio.

#![allow(unsafe_op_in_unsafe_fn)]

use core::mem::zeroed;
use core::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{FreeLibrary, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FillRect, FrameRect, GetMonitorInfoW, GetTextExtentPoint32W, HDC, HFONT, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, TextOutW, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect, HWND_TOPMOST, IsWindowVisible,
    MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowPos, ShowWindow,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::wide::WBuf;
use nextm_metrics::saturation::SatState;

pub const HOVER_CLASS: &[u16] = wide!("nextm-hover");

#[derive(Clone, Debug)]
pub struct HoverSnapshot {
    pub is_dark: bool,
    pub is_it: bool,
    pub dpi: u32,

    pub cpu_active: bool,
    pub cpu_percent: Option<u8>,
    pub cpu_utility: Option<u8>,
    pub sat_state: SatState,
    pub sat_top_proc: Option<(String, u32)>,
    pub cpu_cores: Option<Vec<u8>>,

    pub ram_active: bool,
    pub ram_percent: Option<u8>,
    pub ram_used_x10: Option<u32>,
    pub ram_total_x10: Option<u32>,

    pub net_active: bool,
    pub net_label: Option<String>,
    pub net_down: Option<String>,
    pub net_up: Option<String>,

    pub temp_acpi_active: bool,
    pub temp_acpi_c: Option<i16>,
    pub temp_acpi_fixed: bool,

    pub temp_gpu_active: bool,
    pub temp_gpu_c: Option<i16>,
    pub temp_gpu_name: Option<String>,

    pub temp_disk_active: bool,
    pub temp_disk_c: Option<i16>,
    pub temp_disk_label: Option<String>,

    pub disk_space_active: bool,
    pub mounted_disks: Vec<nextm_metrics::disk::MountedDisk>,

    pub top_cpu_procs: Vec<(String, String)>,
    pub top_ram_procs: Vec<(String, String)>,
    pub top_net_procs: Vec<(String, String)>,

    pub cpu_sparkline: Vec<u8>,
    pub ram_sparkline: Vec<u8>,
    pub net_sparkline: Vec<u8>,
    pub net_spark_min: Option<String>,
    pub net_spark_avg: Option<String>,
    pub net_spark_max: Option<String>,

    pub gpu_active: bool,
    pub gpu_name: Option<String>,
    pub gpu_3d_pct: Option<u8>,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,

    pub cpu_throttled: bool,
    pub cpu_mhz: Option<u32>,
}

impl Default for HoverSnapshot {
    fn default() -> Self {
        HoverSnapshot {
            is_dark: true,
            is_it: true,
            dpi: 96,
            cpu_active: true,
            cpu_percent: None,
            cpu_utility: None,
            sat_state: SatState::Normal,
            sat_top_proc: None,
            cpu_cores: None,
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
            temp_gpu_name: None,
            temp_disk_active: false,
            temp_disk_c: None,
            temp_disk_label: None,
            disk_space_active: true,
            mounted_disks: Vec::new(),
            top_cpu_procs: Vec::new(),
            top_ram_procs: Vec::new(),
            top_net_procs: Vec::new(),
            cpu_sparkline: Vec::new(),
            ram_sparkline: Vec::new(),
            net_sparkline: Vec::new(),
            net_spark_min: None,
            net_spark_avg: None,
            net_spark_max: None,
            gpu_active: false,
            gpu_name: None,
            gpu_3d_pct: None,
            vram_used_mb: None,
            vram_total_mb: None,
            cpu_throttled: false,
            cpu_mhz: None,
        }
    }
}

struct HoverFonts {
    dpi: u32,
    title: HFONT,
    body: HFONT,
    mono: HFONT,
}

impl HoverFonts {
    unsafe fn new(dpi: u32) -> Self {
        Self {
            dpi,
            title: create_gdi_font(dpi, 9, 600, false),
            body: create_gdi_font(dpi, 9, 400, false),
            mono: create_gdi_font(dpi, 9, 400, true),
        }
    }

    unsafe fn destroy(&mut self) {
        if !self.title.is_null() {
            DeleteObject(self.title);
            self.title = core::ptr::null_mut();
        }
        if !self.body.is_null() {
            DeleteObject(self.body);
            self.body = core::ptr::null_mut();
        }
        if !self.mono.is_null() {
            DeleteObject(self.mono);
            self.mono = core::ptr::null_mut();
        }
    }
}

impl Drop for HoverFonts {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}

use std::cell::RefCell;

thread_local! {
    static CURRENT_DATA: RefCell<Option<HoverSnapshot>> = const { RefCell::new(None) };
    static HOVER_FONTS: RefCell<Option<HoverFonts>> = const { RefCell::new(None) };
}

unsafe extern "system" fn hover_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_null() {
                CURRENT_DATA.with(|cell| {
                    if let Some(data) = cell.borrow().as_ref() {
                        paint_hover(hwnd, hdc, data);
                    }
                });
                EndPaint(hwnd, &ps);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct HoverWindow {
    hwnd: HWND,
    width: i32,
    height: i32,
    pos_x: i32,
    pos_y: i32,
    anchor_x: i32,
    anchor_y: i32,
    is_dark: Option<bool>,
}

impl HoverWindow {
    pub fn new(parent: HWND) -> Option<Self> {
        unsafe {
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSEXW = zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(hover_wndproc);
            wc.hInstance = instance;
            wc.lpszClassName = HOVER_CLASS.as_ptr();
            let _ = RegisterClassExW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                HOVER_CLASS.as_ptr(),
                wide!("nextm-hover").as_ptr(),
                WS_POPUP,
                0,
                0,
                300,
                200,
                parent,
                null_mut(),
                instance,
                null(),
            );

            if hwnd.is_null() {
                return None;
            }

            Some(HoverWindow {
                hwnd,
                width: 300,
                height: 200,
                pos_x: 0,
                pos_y: 0,
                anchor_x: 0,
                anchor_y: 0,
                is_dark: None,
            })
        }
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) != 0 }
    }

    pub fn hide(&mut self) {
        if self.is_visible() {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
            CURRENT_DATA.with(|cell| {
                *cell.borrow_mut() = None;
            });
        }
    }

    pub fn contains_point(&self, pt: POINT, tolerance: i32) -> bool {
        let mut rc: RECT = unsafe { zeroed() };
        unsafe {
            GetWindowRect(self.hwnd, &mut rc);
        }
        pt.x >= rc.left - tolerance
            && pt.x <= rc.right + tolerance
            && pt.y >= rc.top - tolerance
            && pt.y <= rc.bottom + tolerance
    }

    pub fn anchor_distance(&self, pt: POINT) -> i32 {
        let dx = (pt.x - self.anchor_x).abs();
        let dy = (pt.y - self.anchor_y).abs();
        dx.max(dy)
    }

    pub fn show_or_update(&mut self, data: HoverSnapshot, x: i32, y: i32) {
        let was_visible = self.is_visible();
        let anchor_moved = (x - self.anchor_x).abs() > 30 || (y - self.anchor_y).abs() > 30;
        self.anchor_x = x;
        self.anchor_y = y;

        let is_dark = data.is_dark;
        let (req_w, req_h) = calculate_dimensions(&data);
        let size_changed = self.width != req_w || self.height != req_h;
        self.width = req_w;
        self.height = req_h;

        CURRENT_DATA.with(|cell| {
            *cell.borrow_mut() = Some(data);
        });

        unsafe {
            if self.is_dark != Some(is_dark) {
                apply_dwm_styling(self.hwnd, is_dark);
                self.is_dark = Some(is_dark);
            }
            if !was_visible || anchor_moved || size_changed {
                let (pos_x, pos_y) = calculate_position(x, y, req_w, req_h);
                self.pos_x = pos_x;
                self.pos_y = pos_y;
                SetWindowPos(self.hwnd, HWND_TOPMOST, pos_x, pos_y, req_w, req_h, SWP_NOACTIVATE);
            }

            if !was_visible {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            InvalidateRect(self.hwnd, null(), 0);
            UpdateWindow(self.hwnd);
        }
    }

    pub fn refresh_if_visible(&mut self, data: HoverSnapshot) {
        if self.is_visible() {
            let is_dark = data.is_dark;
            let (req_w, req_h) = calculate_dimensions(&data);
            let size_changed = self.width != req_w || self.height != req_h;
            self.width = req_w;
            self.height = req_h;

            CURRENT_DATA.with(|cell| {
                *cell.borrow_mut() = Some(data);
            });
            unsafe {
                if self.is_dark != Some(is_dark) {
                    apply_dwm_styling(self.hwnd, is_dark);
                    self.is_dark = Some(is_dark);
                }
                if size_changed {
                    let (pos_x, pos_y) = calculate_position(self.anchor_x, self.anchor_y, req_w, req_h);
                    self.pos_x = pos_x;
                    self.pos_y = pos_y;
                    SetWindowPos(self.hwnd, HWND_TOPMOST, pos_x, pos_y, req_w, req_h, SWP_NOACTIVATE);
                }
                InvalidateRect(self.hwnd, null(), 0);
                UpdateWindow(self.hwnd);
            }
        }
    }
}

impl Drop for HoverWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe {
                DestroyWindow(self.hwnd);
            }
        }
        CURRENT_DATA.with(|cell| {
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

const MAX_HOVER_TOP_PROCS: usize = 5;

fn top_procs_row_count(len: usize) -> usize {
    if len == 0 {
        0
    } else if len <= MAX_HOVER_TOP_PROCS {
        len
    } else {
        MAX_HOVER_TOP_PROCS + 1
    }
}

fn calculate_dimensions(data: &HoverSnapshot) -> (i32, i32) {
    let scale = data.dpi as f32 / 96.0;
    let base_w = if data.disk_space_active && !data.mounted_disks.is_empty() {
        480.0
    } else if data.cpu_cores.as_ref().is_some_and(|c| c.len() > 16) {
        360.0
    } else if !data.top_cpu_procs.is_empty()
        || !data.top_ram_procs.is_empty()
        || !data.top_net_procs.is_empty()
        || (data.gpu_active && data.gpu_name.is_some())
    {
        340.0
    } else {
        300.0
    };
    let width = (base_w * scale) as i32;

    let pad_y = (12.0 * scale) as i32;
    let line_h = (16.0 * scale) as i32;
    let group_gap = (12.0 * scale) as i32;
    let spark_h = (30.0 * scale) as i32;
    let mut h = pad_y * 2;
    let mut has_prev_group = false;

    if data.cpu_active {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        if data.cpu_throttled {
            h += line_h;
        }
        if data.sat_top_proc.is_some() {
            h += (14.0 * scale) as i32;
        }
        if !data.cpu_sparkline.is_empty() {
            h += spark_h + line_h + (4.0 * scale) as i32;
        }
        if let Some(cores) = &data.cpu_cores {
            let n = cores.len();
            if n > 0 {
                let cols = if n <= 16 {
                    2
                } else if n <= 24 {
                    3
                } else {
                    4
                };
                let rows = n.div_ceil(cols);
                h += rows as i32 * (14.0 * scale) as i32 + (4.0 * scale) as i32;
            }
        }
        if !data.top_cpu_procs.is_empty() {
            h += line_h;
            h += top_procs_row_count(data.top_cpu_procs.len()) as i32 * line_h;
        }
    } else if !data.top_cpu_procs.is_empty() {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        h += top_procs_row_count(data.top_cpu_procs.len()) as i32 * line_h;
    }

    if data.ram_active {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        if data.ram_used_x10.is_some() && data.ram_total_x10.is_some() {
            h += line_h;
        }
        if !data.ram_sparkline.is_empty() {
            h += spark_h + line_h + (4.0 * scale) as i32;
        }
        if !data.top_ram_procs.is_empty() {
            h += line_h;
            h += top_procs_row_count(data.top_ram_procs.len()) as i32 * line_h;
        }
    } else if !data.top_ram_procs.is_empty() {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        h += top_procs_row_count(data.top_ram_procs.len()) as i32 * line_h;
    }

    if data.net_active {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h; // Title: Rete (Ethernet)
        h += line_h; // Download
        h += line_h; // Upload
        if !data.net_sparkline.is_empty() {
            h += spark_h + line_h + (4.0 * scale) as i32;
        }
        if !data.top_net_procs.is_empty() {
            h += line_h;
            h += top_procs_row_count(data.top_net_procs.len()) as i32 * line_h;
        }
    } else if !data.top_net_procs.is_empty() {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        h += top_procs_row_count(data.top_net_procs.len()) as i32 * line_h;
    }

    let has_gpu =
        data.gpu_active && (data.gpu_name.is_some() || data.gpu_3d_pct.is_some() || data.vram_used_mb.is_some());
    if has_gpu {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        if data.gpu_3d_pct.is_some() {
            h += line_h;
        }
        if data.vram_used_mb.is_some() && data.vram_total_mb.is_some() {
            h += line_h;
        }
    }

    let has_any_temp = data.temp_acpi_active || data.temp_gpu_active || data.temp_disk_active;
    if has_any_temp {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h; // Title: Temperature
        if data.temp_acpi_active {
            h += line_h;
        }
        if data.temp_gpu_active {
            h += line_h;
        }
        if data.temp_disk_active {
            h += line_h;
        }
    }

    let has_disks = data.disk_space_active && !data.mounted_disks.is_empty();
    if has_disks {
        if has_prev_group {
            h += group_gap;
        }
        // has_prev_group = true;

        h += line_h; // Title: Dischi / Drives
        h += data.mounted_disks.len() as i32 * line_h;
    }

    (width, h.max((50.0 * scale) as i32))
}

unsafe fn calculate_position(cursor_x: i32, cursor_y: i32, width: i32, height: i32) -> (i32, i32) {
    let pt = POINT { x: cursor_x, y: cursor_y };
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hmon, &mut mi);
    let work = mi.rcWork;

    let mon_mid_y = (work.top + work.bottom) / 2;
    let mut y = if cursor_y >= mon_mid_y { cursor_y - height - 12 } else { cursor_y + 24 };

    let mut x = cursor_x - width / 2;

    let min_x = work.left + 8;
    let max_x = (work.right - width - 8).max(min_x);
    x = x.clamp(min_x, max_x);

    let min_y = work.top + 8;
    let max_y = (work.bottom - height - 8).max(min_y);
    y = y.clamp(min_y, max_y);

    (x, y)
}

unsafe fn paint_hover(hwnd: HWND, hdc: HDC, data: &HoverSnapshot) {
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    let is_dark = data.is_dark;
    let bg_color = if is_dark { 0x00202020 } else { 0x00F8F8F8 };
    let border_color = if is_dark { 0x003C3C3C } else { 0x00D0D0D0 };
    let text_bright = if is_dark { 0x00FFFFFF } else { 0x001A1A1A };
    let text_normal = if is_dark { 0x00D0D0D0 } else { 0x00333333 };
    let text_dim = if is_dark { 0x00909090 } else { 0x00777777 };
    let text_amber = 0x0000B9FF; // RGB(255, 185, 0) in BGR format
    let text_red = 0x003333FF; // RGB(255, 51, 51) in BGR format

    let bg_brush = CreateSolidBrush(bg_color);
    FillRect(mem_dc, &rc, bg_brush);
    DeleteObject(bg_brush);

    let border_brush = CreateSolidBrush(border_color);
    FrameRect(mem_dc, &rc, border_brush);
    DeleteObject(border_brush);

    SetBkMode(mem_dc, TRANSPARENT as i32);

    let dpi = data.dpi;
    let (font_title, font_body, font_mono) = HOVER_FONTS.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.as_ref().is_none_or(|f| f.dpi != dpi) {
            *opt = Some(HoverFonts::new(dpi));
        }
        match opt.as_ref() {
            Some(f) => (f.title, f.body, f.mono),
            None => (core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut()),
        }
    });

    let scale = dpi as f32 / 96.0;
    let pad_x = (14.0 * scale) as i32;
    let pad_y = (12.0 * scale) as i32;
    let line_h = (16.0 * scale) as i32;
    let group_gap = (12.0 * scale) as i32;
    let sub_pad = pad_x + (8.0 * scale) as i32;
    let mut cur_y = pad_y;
    let mut has_prev_group = false;

    // 1. CPU
    if data.cpu_active {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let mut buf = WBuf::<64>::new();
        buf.push_str("CPU ");
        if let Some(util) = data.cpu_utility {
            buf.push_u32(u32::from(util));
            buf.push_str("% (utilità)");
        } else if let Some(p) = data.cpu_percent {
            buf.push_u32(u32::from(p));
            buf.push(b'%' as u16);
        } else {
            buf.push_str("—");
        }
        TextOutW(mem_dc, pad_x, cur_y, buf.as_slice().as_ptr(), buf.as_slice().len() as i32);

        // Saturation status
        let mut sz: SIZE = zeroed();
        GetTextExtentPoint32W(mem_dc, buf.as_slice().as_ptr(), buf.as_slice().len() as i32, &mut sz);
        let sat_x = pad_x + sz.cx;

        match data.sat_state {
            SatState::Normal => {}
            SatState::Full => {
                SetTextColor(mem_dc, text_red);
                let full_str = if data.is_it { wide!(" · piena") } else { wide!(" · full") };
                TextOutW(mem_dc, sat_x, cur_y, full_str.as_ptr(), (full_str.len().saturating_sub(1)) as i32);
            }
            SatState::Saturated(k) => {
                SetTextColor(mem_dc, text_amber);
                let mut sat_buf = WBuf::<32>::new();
                sat_buf.push_str(" · ");
                sat_buf.push_u32(u32::from(k));
                sat_buf.push_str(if k == 1 {
                    if data.is_it { " saturo" } else { " saturated" }
                } else {
                    if data.is_it { " saturi" } else { " saturated" }
                });
                TextOutW(mem_dc, sat_x, cur_y, sat_buf.as_slice().as_ptr(), sat_buf.as_slice().len() as i32);
            }
        }

        cur_y += line_h;

        let spark_h = (30.0 * scale) as i32;
        if !data.cpu_sparkline.is_empty() {
            let spark_rc = RECT { left: sub_pad, top: cur_y, right: w - pad_x, bottom: cur_y + spark_h };
            unsafe {
                draw_sparkline(
                    mem_dc,
                    &spark_rc,
                    &data.cpu_sparkline,
                    if is_dark { 0x00FFCD60 } else { 0x00D47800 },
                    if is_dark { 0x004B3214 } else { 0x00F5E1D0 },
                    if is_dark { 0x00181818 } else { 0x00EFEFEF },
                );
            }
            cur_y += spark_h + (2.0 * scale) as i32;

            let min = data.cpu_sparkline.iter().copied().min().unwrap_or(0);
            let max = data.cpu_sparkline.iter().copied().max().unwrap_or(0);
            let avg =
                (data.cpu_sparkline.iter().map(|&v| v as u32).sum::<u32>() / data.cpu_sparkline.len() as u32) as u8;
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let mut s_buf = WBuf::<64>::new();
            s_buf.push_str("min: ");
            s_buf.push_u32(u32::from(min));
            s_buf.push_str("% · ");
            s_buf.push_str(if data.is_it { "med: " } else { "avg: " });
            s_buf.push_u32(u32::from(avg));
            s_buf.push_str("% · max: ");
            s_buf.push_u32(u32::from(max));
            s_buf.push_str("%");
            TextOutW(mem_dc, sub_pad, cur_y, s_buf.as_slice().as_ptr(), s_buf.as_slice().len() as i32);
            cur_y += line_h + (2.0 * scale) as i32;
        }

        // Top process if saturated
        if let Some((name, core100)) = &data.sat_top_proc {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_amber);
            let mut top_buf = WBuf::<64>::new();
            top_buf.push_str("▲ ");
            top_buf.push_str(name);
            top_buf.push_str(": ");
            top_buf.push_u32(core100 / 100);
            top_buf.push(if data.is_it { b',' as u16 } else { b'.' as u16 });
            top_buf.push_u32((core100 % 100) / 10);
            top_buf.push_str(if data.is_it { " core" } else { " cores" });
            TextOutW(mem_dc, sub_pad, cur_y, top_buf.as_slice().as_ptr(), top_buf.as_slice().len() as i32);
            cur_y += (14.0 * scale) as i32;
        }

        // Per-core vertical grid
        if let Some(cores) = &data.cpu_cores {
            let n = cores.len();
            if n > 0 {
                SelectObject(mem_dc, font_mono);
                let cols = if n <= 16 {
                    2
                } else if n <= 24 {
                    3
                } else {
                    4
                };
                let rows = n.div_ceil(cols);
                let avail_w = w - pad_x * 2;
                let col_w = avail_w / cols as i32;
                let core_line_h = (14.0 * scale) as i32;

                for c in 0..cols {
                    for r in 0..rows {
                        let idx = c * rows + r;
                        if idx < n {
                            let pct = cores[idx];
                            let cell_x = pad_x + c as i32 * col_w;
                            let cell_y = cur_y + r as i32 * core_line_h;

                            if pct >= 90 {
                                SetTextColor(mem_dc, text_amber);
                            } else if pct >= 50 {
                                SetTextColor(mem_dc, text_normal);
                            } else {
                                SetTextColor(mem_dc, text_dim);
                            }

                            let mut core_buf = WBuf::<16>::new();
                            core_buf.push(b'C' as u16);
                            if idx < 10 {
                                core_buf.push(b'0' as u16);
                            }
                            core_buf.push_u32(idx as u32);
                            core_buf.push_str(": ");
                            if pct < 10 {
                                core_buf.push(b' ' as u16);
                            }
                            core_buf.push_u32(u32::from(pct));
                            core_buf.push(b'%' as u16);

                            TextOutW(
                                mem_dc,
                                cell_x,
                                cell_y,
                                core_buf.as_slice().as_ptr(),
                                core_buf.as_slice().len() as i32,
                            );
                        }
                    }
                }
                cur_y += rows as i32 * core_line_h + (4.0 * scale) as i32;
            }
        }

        if data.cpu_throttled {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_amber);
            let mut th_buf = WBuf::<64>::new();
            th_buf.push_str(if data.is_it { "⚠️ Throttling attivo" } else { "⚠️ Throttling active" });
            if let Some(mhz) = data.cpu_mhz {
                th_buf.push_str(" (");
                th_buf.push_u32(mhz);
                th_buf.push_str(" MHz)");
            }
            TextOutW(mem_dc, sub_pad, cur_y, th_buf.as_slice().as_ptr(), th_buf.as_slice().len() as i32);
            cur_y += line_h;
        }

        if !data.top_cpu_procs.is_empty() {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let top_hdr = wide!("Top CPU:");
            TextOutW(mem_dc, sub_pad, cur_y, top_hdr.as_ptr(), (top_hdr.len().saturating_sub(1)) as i32);
            cur_y += line_h;

            let proc_pad = sub_pad + (8.0 * scale) as i32;
            let display_n = data.top_cpu_procs.len().min(MAX_HOVER_TOP_PROCS);
            for (name, val) in &data.top_cpu_procs[..display_n] {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_normal);
                let mut p_buf = WBuf::<128>::new();
                p_buf.push_str(name);
                p_buf.push_str(": ");
                p_buf.push_str(val);
                TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
            if data.top_cpu_procs.len() > MAX_HOVER_TOP_PROCS {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_dim);
                let mut more_buf = WBuf::<64>::new();
                more_buf.push_str("... (+");
                more_buf.push_u32((data.top_cpu_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
                more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
                TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
        }
    } else if !data.top_cpu_procs.is_empty() {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let top_title = wide!("Top CPU");
        TextOutW(mem_dc, pad_x, cur_y, top_title.as_ptr(), (top_title.len().saturating_sub(1)) as i32);
        cur_y += line_h;

        let proc_pad = sub_pad;
        let display_n = data.top_cpu_procs.len().min(MAX_HOVER_TOP_PROCS);
        for (name, val) in &data.top_cpu_procs[..display_n] {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_normal);
            let mut p_buf = WBuf::<128>::new();
            p_buf.push_str(name);
            p_buf.push_str(": ");
            p_buf.push_str(val);
            TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
        if data.top_cpu_procs.len() > MAX_HOVER_TOP_PROCS {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let mut more_buf = WBuf::<64>::new();
            more_buf.push_str("... (+");
            more_buf.push_u32((data.top_cpu_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
            more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
            TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    // 2. RAM
    if data.ram_active {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let mut ram_buf = WBuf::<64>::new();
        ram_buf.push_str("RAM ");
        if let Some(p) = data.ram_percent {
            ram_buf.push_u32(u32::from(p));
            buf_push_percent(&mut ram_buf);
        } else {
            ram_buf.push_str("—");
        }
        TextOutW(mem_dc, pad_x, cur_y, ram_buf.as_slice().as_ptr(), ram_buf.as_slice().len() as i32);
        cur_y += line_h;

        if let (Some(used), Some(total)) = (data.ram_used_x10, data.ram_total_x10) {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_normal);
            let mut mem_buf = WBuf::<64>::new();
            mem_buf.push_str(if data.is_it { "Memoria: " } else { "Memory: " });
            push_gib_x10(&mut mem_buf, used, data.is_it);
            mem_buf.push_str(" / ");
            push_gib_x10(&mut mem_buf, total, data.is_it);
            mem_buf.push_str(" GB");
            TextOutW(mem_dc, sub_pad, cur_y, mem_buf.as_slice().as_ptr(), mem_buf.as_slice().len() as i32);
            cur_y += line_h;
        }

        let spark_h = (30.0 * scale) as i32;
        if !data.ram_sparkline.is_empty() {
            let spark_rc = RECT { left: sub_pad, top: cur_y, right: w - pad_x, bottom: cur_y + spark_h };
            unsafe {
                draw_sparkline(
                    mem_dc,
                    &spark_rc,
                    &data.ram_sparkline,
                    if is_dark { 0x00DC64B4 } else { 0x00B43C8C },
                    if is_dark { 0x0037192D } else { 0x00FFE6F5 },
                    if is_dark { 0x00181818 } else { 0x00EEEEEE },
                );
            }
            cur_y += spark_h + (2.0 * scale) as i32;

            let min = data.ram_sparkline.iter().copied().min().unwrap_or(0);
            let max = data.ram_sparkline.iter().copied().max().unwrap_or(0);
            let avg =
                (data.ram_sparkline.iter().map(|&v| v as u32).sum::<u32>() / data.ram_sparkline.len() as u32) as u8;
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let mut s_buf = WBuf::<64>::new();
            s_buf.push_str("min: ");
            s_buf.push_u32(u32::from(min));
            s_buf.push_str("% · ");
            s_buf.push_str(if data.is_it { "med: " } else { "avg: " });
            s_buf.push_u32(u32::from(avg));
            s_buf.push_str("% · max: ");
            s_buf.push_u32(u32::from(max));
            s_buf.push_str("%");
            TextOutW(mem_dc, sub_pad, cur_y, s_buf.as_slice().as_ptr(), s_buf.as_slice().len() as i32);
            cur_y += line_h + (2.0 * scale) as i32;
        }

        if !data.top_ram_procs.is_empty() {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let top_hdr = wide!("Top RAM:");
            TextOutW(mem_dc, sub_pad, cur_y, top_hdr.as_ptr(), (top_hdr.len().saturating_sub(1)) as i32);
            cur_y += line_h;

            let proc_pad = sub_pad + (8.0 * scale) as i32;
            let display_n = data.top_ram_procs.len().min(MAX_HOVER_TOP_PROCS);
            for (name, val) in &data.top_ram_procs[..display_n] {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_normal);
                let mut p_buf = WBuf::<128>::new();
                p_buf.push_str(name);
                p_buf.push_str(": ");
                p_buf.push_str(val);
                TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
            if data.top_ram_procs.len() > MAX_HOVER_TOP_PROCS {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_dim);
                let mut more_buf = WBuf::<64>::new();
                more_buf.push_str("... (+");
                more_buf.push_u32((data.top_ram_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
                more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
                TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
        }
    } else if !data.top_ram_procs.is_empty() {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let top_title = wide!("Top RAM");
        TextOutW(mem_dc, pad_x, cur_y, top_title.as_ptr(), (top_title.len().saturating_sub(1)) as i32);
        cur_y += line_h;

        let proc_pad = sub_pad;
        let display_n = data.top_ram_procs.len().min(MAX_HOVER_TOP_PROCS);
        for (name, val) in &data.top_ram_procs[..display_n] {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_normal);
            let mut p_buf = WBuf::<128>::new();
            p_buf.push_str(name);
            p_buf.push_str(": ");
            p_buf.push_str(val);
            TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
        if data.top_ram_procs.len() > MAX_HOVER_TOP_PROCS {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let mut more_buf = WBuf::<64>::new();
            more_buf.push_str("... (+");
            more_buf.push_u32((data.top_ram_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
            more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
            TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    // 3. Rete
    if data.net_active {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let mut net_buf = WBuf::<64>::new();
        net_buf.push_str(if data.is_it { "Rete" } else { "Network" });
        if let Some(lbl) = &data.net_label {
            net_buf.push_str(" (");
            net_buf.push_str(lbl);
            net_buf.push(b')' as u16);
        }
        TextOutW(mem_dc, pad_x, cur_y, net_buf.as_slice().as_ptr(), net_buf.as_slice().len() as i32);
        cur_y += line_h;

        SelectObject(mem_dc, font_body);
        SetTextColor(mem_dc, text_normal);

        let mut down_buf = WBuf::<64>::new();
        down_buf.push_str("\u{2193} Download: ");
        if let Some(down) = &data.net_down {
            down_buf.push_str(down);
        } else {
            down_buf.push_str("—");
        }
        TextOutW(mem_dc, sub_pad, cur_y, down_buf.as_slice().as_ptr(), down_buf.as_slice().len() as i32);
        cur_y += line_h;

        let mut up_buf = WBuf::<64>::new();
        up_buf.push_str("\u{2191} Upload:   ");
        if let Some(up) = &data.net_up {
            up_buf.push_str(up);
        } else {
            up_buf.push_str("—");
        }
        TextOutW(mem_dc, sub_pad, cur_y, up_buf.as_slice().as_ptr(), up_buf.as_slice().len() as i32);
        cur_y += line_h;

        let spark_h = (30.0 * scale) as i32;
        if !data.net_sparkline.is_empty() {
            let spark_rc = RECT { left: sub_pad, top: cur_y, right: w - pad_x, bottom: cur_y + spark_h };
            unsafe {
                draw_sparkline(
                    mem_dc,
                    &spark_rc,
                    &data.net_sparkline,
                    if is_dark { 0x0028A0F0 } else { 0x001478D2 },
                    if is_dark { 0x000A283C } else { 0x00DCF0FF },
                    if is_dark { 0x00181818 } else { 0x00EEEEEE },
                );
            }
            cur_y += spark_h + (2.0 * scale) as i32;

            if let (Some(min_s), Some(avg_s), Some(max_s)) =
                (&data.net_spark_min, &data.net_spark_avg, &data.net_spark_max)
            {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_dim);
                let mut s_buf = WBuf::<128>::new();
                s_buf.push_str("min: ");
                s_buf.push_str(min_s);
                s_buf.push_str(" · ");
                s_buf.push_str(if data.is_it { "med: " } else { "avg: " });
                s_buf.push_str(avg_s);
                s_buf.push_str(" · max: ");
                s_buf.push_str(max_s);
                TextOutW(mem_dc, sub_pad, cur_y, s_buf.as_slice().as_ptr(), s_buf.as_slice().len() as i32);
                cur_y += line_h + (2.0 * scale) as i32;
            }
        }

        if !data.top_net_procs.is_empty() {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let top_hdr = wide!("Top I/O Rete:");
            TextOutW(mem_dc, sub_pad, cur_y, top_hdr.as_ptr(), (top_hdr.len().saturating_sub(1)) as i32);
            cur_y += line_h;

            let proc_pad = sub_pad + (8.0 * scale) as i32;
            let display_n = data.top_net_procs.len().min(MAX_HOVER_TOP_PROCS);
            for (name, val) in &data.top_net_procs[..display_n] {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_normal);
                let mut p_buf = WBuf::<128>::new();
                p_buf.push_str(name);
                p_buf.push_str(": ");
                p_buf.push_str(val);
                TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
            if data.top_net_procs.len() > MAX_HOVER_TOP_PROCS {
                SelectObject(mem_dc, font_body);
                SetTextColor(mem_dc, text_dim);
                let mut more_buf = WBuf::<64>::new();
                more_buf.push_str("... (+");
                more_buf.push_u32((data.top_net_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
                more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
                TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
                cur_y += line_h;
            }
        }
    } else if !data.top_net_procs.is_empty() {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let top_title = if data.is_it { wide!("Top I/O Rete") } else { wide!("Top Net I/O") };
        TextOutW(mem_dc, pad_x, cur_y, top_title.as_ptr(), (top_title.len().saturating_sub(1)) as i32);
        cur_y += line_h;

        let proc_pad = sub_pad;
        let display_n = data.top_net_procs.len().min(MAX_HOVER_TOP_PROCS);
        for (name, val) in &data.top_net_procs[..display_n] {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_normal);
            let mut p_buf = WBuf::<128>::new();
            p_buf.push_str(name);
            p_buf.push_str(": ");
            p_buf.push_str(val);
            TextOutW(mem_dc, proc_pad, cur_y, p_buf.as_slice().as_ptr(), p_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
        if data.top_net_procs.len() > MAX_HOVER_TOP_PROCS {
            SelectObject(mem_dc, font_body);
            SetTextColor(mem_dc, text_dim);
            let mut more_buf = WBuf::<64>::new();
            more_buf.push_str("... (+");
            more_buf.push_u32((data.top_net_procs.len() - MAX_HOVER_TOP_PROCS) as u32);
            more_buf.push_str(if data.is_it { " altri)" } else { " more)" });
            TextOutW(mem_dc, proc_pad, cur_y, more_buf.as_slice().as_ptr(), more_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    // GPU & VRAM
    let has_gpu =
        data.gpu_active && (data.gpu_name.is_some() || data.gpu_3d_pct.is_some() || data.vram_used_mb.is_some());
    if has_gpu {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let mut gpu_buf = WBuf::<128>::new();
        gpu_buf.push_str("GPU");
        if let Some(name) = &data.gpu_name {
            gpu_buf.push_str(" (");
            let max_len = 24;
            let short_name = if name.len() > max_len {
                let mut end = max_len;
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                &name[..end]
            } else {
                name.as_str()
            };
            gpu_buf.push_str(short_name);
            gpu_buf.push(b')' as u16);
        }
        TextOutW(mem_dc, pad_x, cur_y, gpu_buf.as_slice().as_ptr(), gpu_buf.as_slice().len() as i32);
        cur_y += line_h;

        SelectObject(mem_dc, font_body);
        SetTextColor(mem_dc, text_normal);

        if let Some(pct) = data.gpu_3d_pct {
            let mut load_buf = WBuf::<64>::new();
            load_buf.push_str("3D Engine: ");
            load_buf.push_u32(u32::from(pct));
            buf_push_percent(&mut load_buf);
            TextOutW(mem_dc, sub_pad, cur_y, load_buf.as_slice().as_ptr(), load_buf.as_slice().len() as i32);
            cur_y += line_h;
        }

        if let (Some(used), Some(total)) = (data.vram_used_mb, data.vram_total_mb) {
            let mut vram_buf = WBuf::<64>::new();
            vram_buf.push_str("VRAM: ");
            let used_x10 = ((used * 10) / 1024) as u32;
            let total_x10 = ((total * 10) / 1024) as u32;
            push_gib_x10(&mut vram_buf, used_x10, data.is_it);
            vram_buf.push_str(" / ");
            push_gib_x10(&mut vram_buf, total_x10, data.is_it);
            vram_buf.push_str(" GB");
            if let Some(div) = (used * 100).checked_div(total) {
                let pct = div as u32;
                vram_buf.push_str(" (");
                vram_buf.push_u32(pct);
                vram_buf.push_str("%)");
            }
            TextOutW(mem_dc, sub_pad, cur_y, vram_buf.as_slice().as_ptr(), vram_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    // 4. Temperature
    let has_any_temp = data.temp_acpi_active || data.temp_gpu_active || data.temp_disk_active;
    if has_any_temp {
        if has_prev_group {
            cur_y += group_gap;
        }
        has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let temp_title = if data.is_it { wide!("Temperature") } else { wide!("Temperatures") };
        TextOutW(mem_dc, pad_x, cur_y, temp_title.as_ptr(), (temp_title.len().saturating_sub(1)) as i32);
        cur_y += line_h;

        SelectObject(mem_dc, font_body);
        SetTextColor(mem_dc, text_normal);

        if data.temp_acpi_active {
            let mut temp_buf = WBuf::<64>::new();
            temp_buf.push_str("CPU: ");
            if let Some(t) = data.temp_acpi_c {
                temp_buf.push_u32(t.max(0) as u32);
                temp_buf.push(0x00B0);
                if data.temp_acpi_fixed {
                    temp_buf.push_str(if data.is_it { " (fisso)" } else { " (fixed)" });
                }
            } else {
                temp_buf.push_str("—");
            }
            TextOutW(mem_dc, sub_pad, cur_y, temp_buf.as_slice().as_ptr(), temp_buf.as_slice().len() as i32);
            cur_y += line_h;
        }

        if data.temp_gpu_active {
            let mut temp_buf = WBuf::<128>::new();
            temp_buf.push_str("GPU: ");
            if let Some(t) = data.temp_gpu_c {
                temp_buf.push_u32(t.max(0) as u32);
                temp_buf.push(0x00B0);
            } else {
                temp_buf.push_str("—");
            }
            if let Some(name) = &data.temp_gpu_name {
                temp_buf.push_str(" (");
                let max_len = 28;
                let short_name = if name.len() > max_len {
                    let mut end = max_len;
                    while !name.is_char_boundary(end) {
                        end -= 1;
                    }
                    &name[..end]
                } else {
                    name.as_str()
                };
                temp_buf.push_str(short_name);
                temp_buf.push(b')' as u16);
            }
            TextOutW(mem_dc, sub_pad, cur_y, temp_buf.as_slice().as_ptr(), temp_buf.as_slice().len() as i32);
            cur_y += line_h;
        }

        if data.temp_disk_active {
            let mut temp_buf = WBuf::<128>::new();
            let lbl = data.temp_disk_label.as_deref().unwrap_or(if data.is_it { "Disco" } else { "Disk" });
            let max_lbl = 26;
            let short_lbl = if lbl.len() > max_lbl {
                let mut end = max_lbl;
                while !lbl.is_char_boundary(end) {
                    end -= 1;
                }
                &lbl[..end]
            } else {
                lbl
            };
            temp_buf.push_str(short_lbl);
            temp_buf.push_str(": ");
            if let Some(t) = data.temp_disk_c {
                temp_buf.push_u32(t.max(0) as u32);
                temp_buf.push(0x00B0);
            } else {
                temp_buf.push_str("—");
            }
            TextOutW(mem_dc, sub_pad, cur_y, temp_buf.as_slice().as_ptr(), temp_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    // 5. Dischi montati (spazio occupato e rimanente)
    let has_disks = data.disk_space_active && !data.mounted_disks.is_empty();
    if has_disks {
        if has_prev_group {
            cur_y += group_gap;
        }
        // has_prev_group = true;

        SelectObject(mem_dc, font_title);
        SetTextColor(mem_dc, text_bright);
        let disk_title = if data.is_it { wide!("Dischi") } else { wide!("Drives") };
        TextOutW(mem_dc, pad_x, cur_y, disk_title.as_ptr(), (disk_title.len().saturating_sub(1)) as i32);
        cur_y += line_h;

        SelectObject(mem_dc, font_body);

        for disk in &data.mounted_disks {
            let used = nextm_metrics::disk::format_disk_size(disk.used_bytes(), data.is_it);
            let free = nextm_metrics::disk::format_disk_size(disk.free_bytes, data.is_it);
            let pct = disk.used_percent();

            if pct >= 90 {
                SetTextColor(mem_dc, text_amber);
            } else {
                SetTextColor(mem_dc, text_normal);
            }

            let mut d_buf = WBuf::<256>::new();
            d_buf.push(disk.letter as u16);
            d_buf.push_str(": ");
            d_buf.push_str(&used);
            d_buf.push_str(if data.is_it { " occupati · " } else { " used · " });
            d_buf.push_str(&free);
            d_buf.push_str(if data.is_it { " liberi (" } else { " free (" });
            d_buf.push_u32(u32::from(pct));
            d_buf.push_str("%)");

            d_buf.push_str(" [↓ ");
            d_buf.push_str(&nextm_metrics::disk::format_disk_speed(disk.read_bps));
            d_buf.push_str(" ↑ ");
            d_buf.push_str(&nextm_metrics::disk::format_disk_speed(disk.write_bps));
            d_buf.push(b']' as u16);

            TextOutW(mem_dc, sub_pad, cur_y, d_buf.as_slice().as_ptr(), d_buf.as_slice().len() as i32);
            cur_y += line_h;
        }
    }

    let _ = cur_y;

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
}

unsafe fn draw_sparkline(hdc: HDC, rc: &RECT, data: &[u8], stroke_color: u32, fill_color: u32, bg_color: u32) {
    if rc.bottom <= rc.top || rc.right <= rc.left || data.is_empty() {
        return;
    }
    let bg_brush = CreateSolidBrush(bg_color);
    FillRect(hdc, rc, bg_brush);
    DeleteObject(bg_brush);

    // Linea guida tratteggiata al 50%
    let mid_y = rc.top + (rc.bottom - rc.top) / 2;
    let grid_pen = windows_sys::Win32::Graphics::Gdi::CreatePen(
        windows_sys::Win32::Graphics::Gdi::PS_DOT,
        1,
        if bg_color < 0x00808080 { 0x002A2A2A } else { 0x00D0D0D0 },
    );
    let old_pen = SelectObject(hdc, grid_pen);
    windows_sys::Win32::Graphics::Gdi::MoveToEx(hdc, rc.left, mid_y, core::ptr::null_mut());
    windows_sys::Win32::Graphics::Gdi::LineTo(hdc, rc.right, mid_y);
    SelectObject(hdc, old_pen);
    DeleteObject(grid_pen);

    // Bordo sottile del grafico
    let border_brush = CreateSolidBrush(if bg_color < 0x00808080 { 0x00333333 } else { 0x00CCCCCC });
    FrameRect(hdc, rc, border_brush);
    DeleteObject(border_brush);

    let w = (rc.right - rc.left).max(1) as f32;
    let h = (rc.bottom - rc.top - 2).max(1) as f32;
    let n = data.len();
    let step = if n > 1 { w / (n - 1) as f32 } else { w };

    let count = n.min(60);
    let mut pts = [POINT { x: 0, y: 0 }; 64];

    for (i, &val) in data.iter().take(count).enumerate() {
        let x = rc.left + (i as f32 * step) as i32;
        let clamped = (val as f32).clamp(0.0, 100.0);
        let y = (rc.bottom - 1) - ((clamped / 100.0) * h) as i32;
        pts[i] = POINT { x, y: y.clamp(rc.top + 1, rc.bottom - 1) };
    }

    if count >= 2 {
        let mut poly = [POINT { x: 0, y: 0 }; 66];
        poly[0] = POINT { x: pts[0].x, y: rc.bottom - 1 };
        poly[1..=count].copy_from_slice(&pts[..count]);
        poly[count + 1] = POINT { x: pts[count - 1].x, y: rc.bottom - 1 };

        let fill_brush = CreateSolidBrush(fill_color);
        let null_pen = windows_sys::Win32::Graphics::Gdi::GetStockObject(windows_sys::Win32::Graphics::Gdi::NULL_PEN);
        let old_brush = SelectObject(hdc, fill_brush);
        let old_pen = SelectObject(hdc, null_pen);
        windows_sys::Win32::Graphics::Gdi::Polygon(hdc, poly.as_ptr(), (count + 2) as i32);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(fill_brush);
    }

    let pen =
        windows_sys::Win32::Graphics::Gdi::CreatePen(windows_sys::Win32::Graphics::Gdi::PS_SOLID, 1, stroke_color);
    let old_pen = SelectObject(hdc, pen);
    windows_sys::Win32::Graphics::Gdi::Polyline(hdc, pts.as_ptr(), count as i32);
    SelectObject(hdc, old_pen);
    DeleteObject(pen);
}

fn buf_push_percent(buf: &mut WBuf<64>) {
    buf.push(b'%' as u16);
}

fn push_gib_x10(buf: &mut WBuf<64>, val: u32, is_it: bool) {
    buf.push_u32(val / 10);
    buf.push(if is_it { b',' as u16 } else { b'.' as u16 });
    buf.push_u32(val % 10);
}

unsafe fn create_gdi_font(dpi: u32, pt_size: i32, weight: i32, monospace: bool) -> HFONT {
    let font_name = if monospace { wide!("Consolas") } else { wide!("Segoe UI") };
    let height = -((pt_size * dpi as i32) / 72);
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

#[cfg(test)]
mod tests {
    use super::*;
    use nextm_metrics::disk::MountedDisk;

    #[test]
    fn calculate_dimensions_includes_mounted_disks() {
        let mut snapshot = HoverSnapshot::default();
        let (w_no_disks, h_no_disks) = calculate_dimensions(&snapshot);

        snapshot.mounted_disks =
            vec![MountedDisk::new('C', 1_000_000_000, 200_000_000), MountedDisk::new('D', 2_000_000_000, 500_000_000)];
        let (w_with_disks, h_with_disks) = calculate_dimensions(&snapshot);

        // Larghezza base aumenta a 320px
        assert!(w_with_disks >= w_no_disks);
        // Altezza include titolo + 2 dischi + gap
        assert!(h_with_disks > h_no_disks);

        // Se disabilitato da impostazioni, l'altezza torna identica
        snapshot.disk_space_active = false;
        let (w_disabled, h_disabled) = calculate_dimensions(&snapshot);
        assert_eq!(w_disabled, w_no_disks);
        assert_eq!(h_disabled, h_no_disks);
    }

    #[test]
    fn calculate_dimensions_includes_top_cpu_and_ram_procs() {
        let mut snapshot = HoverSnapshot::default();
        let (w_base, h_base) = calculate_dimensions(&snapshot);

        snapshot.top_cpu_procs =
            vec![("chrome.exe (5)".into(), "15,2%".into()), ("rust-analyzer.exe".into(), "8,4%".into())];
        let (w_cpu, h_cpu) = calculate_dimensions(&snapshot);
        assert!(w_cpu >= w_base);
        assert!(h_cpu > h_base);

        snapshot.top_ram_procs =
            vec![("chrome.exe (5)".into(), "2,4 GB".into()), ("rust-analyzer.exe".into(), "1,2 GB".into())];
        let (_w_both, h_both) = calculate_dimensions(&snapshot);
        assert!(h_both > h_cpu);
    }
}
