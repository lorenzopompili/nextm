//! Risparmio energetico: EcoQoS e notifiche su schermo e risparmio energia.

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification, UnregisterPowerSettingNotification,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
    SetProcessInformation,
};
use windows_sys::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_WINDOW_HANDLE;
use windows_sys::core::GUID;

pub use windows_sys::Win32::System::SystemServices::{GUID_POWER_SAVING_STATUS, GUID_SESSION_DISPLAY_STATUS};

/// Stato del risparmio energia di Windows 11 (winnt.h, SDK 10.0.26100: sì, è davvero questo GUID).
pub const GUID_ENERGY_SAVER_STATUS: GUID =
    GUID { data1: 0x550E_8400, data2: 0xE29B, data3: 0x41D4, data4: [0xA7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00] };

/// Attiva o disattiva EcoQoS: Windows esegue il processo sui core efficienti a bassa frequenza.
pub fn set_ecoqos(on: bool) -> bool {
    let mask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: mask,
        StateMask: if on { mask } else { 0 },
    };
    // SAFETY: struttura locale valida della dimensione dichiarata.
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const state).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

/// Registrazione a una notifica di alimentazione, annullata automaticamente.
pub struct PowerNotify(HPOWERNOTIFY);

impl PowerNotify {
    /// Chiede a Windows di inviare `WM_POWERBROADCAST` a `hwnd` quando `guid` cambia.
    /// Il valore corrente di solito arriva subito dopo la registrazione.
    pub fn register(hwnd: HWND, guid: &GUID) -> Option<PowerNotify> {
        // SAFETY: finestra valida, GUID valido per la durata della chiamata.
        let h = unsafe { RegisterPowerSettingNotification(hwnd, guid, DEVICE_NOTIFY_WINDOW_HANDLE) };
        (h != 0).then_some(PowerNotify(h))
    }
}

impl Drop for PowerNotify {
    fn drop(&mut self) {
        // SAFETY: registrazione fatta da noi, annullata una sola volta.
        unsafe { UnregisterPowerSettingNotification(self.0) };
    }
}

/// Uguaglianza tra GUID (windows-sys non la fornisce).
pub fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// Legge GUID e primo DWORD di un `PBT_POWERSETTINGCHANGE`.
///
/// # Safety
/// `lparam` deve essere quello di un `WM_POWERBROADCAST` con `wParam == PBT_POWERSETTINGCHANGE`.
pub unsafe fn setting_from_lparam(lparam: LPARAM) -> Option<(GUID, u32)> {
    let p = lparam as *const POWERBROADCAST_SETTING;
    if p.is_null() {
        return None;
    }
    // SAFETY: garantito dal chiamante; i dati seguono l'intestazione e sono almeno 4 byte
    // per le impostazioni che usiamo (tutte DWORD).
    unsafe {
        let s = &*p;
        if s.DataLength < 4 {
            return None;
        }
        let data = core::ptr::read_unaligned(s.Data.as_ptr().cast::<u32>());
        Some((s.PowerSetting, data))
    }
}
