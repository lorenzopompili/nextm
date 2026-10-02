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

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
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
    anchor_x: i32,
    anchor_y: i32,
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

            Some(HoverWindow { hwnd, width: 300, height: 200, anchor_x: 0, anchor_y: 0 })
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
        self.anchor_x = x;
        self.anchor_y = y;

        let is_dark = data.is_dark;
        let (req_w, req_h) = calculate_dimensions(&data);
        self.width = req_w;
        self.height = req_h;

        CURRENT_DATA.with(|cell| {
            *cell.borrow_mut() = Some(data);
        });

        unsafe {
            apply_dwm_styling(self.hwnd, is_dark);

            // Calcola la posizione sopra la tray / cursore, assicurandosi che stia nello schermo
            let (pos_x, pos_y) = calculate_position(x, y, req_w, req_h);

            SetWindowPos(self.hwnd, HWND_TOPMOST, pos_x, pos_y, req_w, req_h, SWP_NOACTIVATE);

            InvalidateRect(self.hwnd, null(), 0);
            UpdateWindow(self.hwnd);
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn refresh_if_visible(&mut self, data: HoverSnapshot) {
        if self.is_visible() {
            CURRENT_DATA.with(|cell| {
                *cell.borrow_mut() = Some(data);
            });
            unsafe {
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
}

fn calculate_dimensions(data: &HoverSnapshot) -> (i32, i32) {
    let scale = data.dpi as f32 / 96.0;
    let base_w = if data.cpu_cores.as_ref().is_some_and(|c| c.len() > 16) { 340.0 } else { 300.0 };
    let width = (base_w * scale) as i32;

    let pad_y = (12.0 * scale) as i32;
    let line_h = (16.0 * scale) as i32;
    let group_gap = (12.0 * scale) as i32;
    let mut h = pad_y * 2;
    let mut has_prev_group = false;

    if data.cpu_active {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h;
        if data.sat_top_proc.is_some() {
            h += (14.0 * scale) as i32;
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
    }

    if data.net_active {
        if has_prev_group {
            h += group_gap;
        }
        has_prev_group = true;

        h += line_h; // Title: Rete (Ethernet)
        h += line_h; // Download
        h += line_h; // Upload
    }

    let has_any_temp = data.temp_acpi_active || data.temp_gpu_active || data.temp_disk_active;
    if has_any_temp {
        if has_prev_group {
            h += group_gap;
        }
        // has_prev_group = true;

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

    (width, h.max((50.0 * scale) as i32))
}

unsafe fn calculate_position(cursor_x: i32, cursor_y: i32, width: i32, height: i32) -> (i32, i32) {
    let pt = POINT { x: cursor_x, y: cursor_y };
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hmon, &mut mi);
    let work = mi.rcWork;

    let mut x = cursor_x - width / 2;
    let mut y = cursor_y - height - 12;

    if y < work.top + 8 {
        y = cursor_y + 24;
    }

    x = x.clamp(work.left + 8, work.right - width - 8);
    y = y.clamp(work.top + 8, work.bottom - height - 8);

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
    }

    // 4. Temperature
    let has_any_temp = data.temp_acpi_active || data.temp_gpu_active || data.temp_disk_active;
    if has_any_temp {
        if has_prev_group {
            cur_y += group_gap;
        }
        // has_prev_group = true;

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

    let _ = cur_y;

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
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
