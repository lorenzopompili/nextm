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

/// Dettaglio dello stato energetico e thermal/power throttling del processore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuThrottleStatus {
    pub is_throttled: bool,
    pub current_mhz: u32,
    pub max_mhz: u32,
    pub mhz_limit: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
struct ProcessorPowerInformation {
    number: u32,
    max_mhz: u32,
    current_mhz: u32,
    mhz_limit: u32,
    max_idle_state: u32,
    current_idle_state: u32,
}

type FnCallNtPowerInformation =
    unsafe extern "system" fn(u32, *const core::ffi::c_void, u32, *mut core::ffi::c_void, u32) -> i32;

static FN_CALL_NT_POWER_INFO: std::sync::OnceLock<Option<FnCallNtPowerInformation>> = std::sync::OnceLock::new();

fn get_call_nt_power_info() -> Option<FnCallNtPowerInformation> {
    *FN_CALL_NT_POWER_INFO.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
        let h_powrprof = unsafe {
            LoadLibraryExW(wide!("powrprof.dll").as_ptr(), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
        };
        if h_powrprof.is_null() {
            return None;
        }
        let proc = unsafe { GetProcAddress(h_powrprof, c"CallNtPowerInformation".as_ptr().cast()) }?;
        let call_pwr: FnCallNtPowerInformation = unsafe { core::mem::transmute(proc) };
        Some(call_pwr)
    })
}

/// Rileva lo stato di thermal o power throttling della CPU interrogando `CallNtPowerInformation`.
pub fn check_cpu_throttling() -> Option<CpuThrottleStatus> {
    use windows_sys::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount};

    let call_pwr = get_call_nt_power_info()?;
    let count = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1) as usize;

    let mut stack_buf = [ProcessorPowerInformation::default(); 64];
    let mut heap_buf: Vec<ProcessorPowerInformation>;
    let target_slice: &mut [ProcessorPowerInformation] = if count <= stack_buf.len() {
        &mut stack_buf[..count]
    } else {
        heap_buf = vec![ProcessorPowerInformation::default(); count];
        &mut heap_buf[..]
    };

    let buf_size = (count * size_of::<ProcessorPowerInformation>()) as u32;
    let status = unsafe { call_pwr(11, core::ptr::null(), 0, target_slice.as_mut_ptr().cast(), buf_size) };
    if status != 0 {
        return None;
    }

    let mut sum_cur = 0u64;
    let mut sum_max = 0u64;
    let mut sum_limit = 0u64;
    let mut any_throttled = false;

    for info in target_slice.iter() {
        sum_cur += u64::from(info.current_mhz);
        sum_max += u64::from(info.max_mhz);
        sum_limit += u64::from(info.mhz_limit);
        if (info.max_mhz > 0 && info.mhz_limit < info.max_mhz)
            || (info.max_mhz > 0 && info.current_mhz < (info.max_mhz * 85 / 100))
        {
            any_throttled = true;
        }
    }

    let avg_cur = (sum_cur / count as u64) as u32;
    let avg_max = (sum_max / count as u64) as u32;
    let avg_limit = (sum_limit / count as u64) as u32;

    Some(CpuThrottleStatus {
        is_throttled: any_throttled,
        current_mhz: avg_cur,
        max_mhz: avg_max,
        mhz_limit: avg_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_throttling_check() {
        if let Some(st) = check_cpu_throttling() {
            eprintln!("CPU Power/Throttle: {:?}", st);
            assert!(st.max_mhz > 0, "max_mhz deve essere maggiore di 0");
        }
    }
}
