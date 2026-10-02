//! Rilevamento privilegi, abilitazione SeDebugPrivilege, riavvio elevato (UAC)
//! e gestione avvio automatico con privilegi elevati tramite Utilità di Pianificazione (schtasks).

#![allow(clippy::upper_case_acronyms)]

use core::mem::zeroed;
use core::ptr::null_mut;
use nextm_metrics::sys::dll::Library;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND};
use windows_sys::Win32::System::Threading::{CreateProcessW, GetCurrentProcess, PROCESS_INFORMATION, STARTUPINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_HIDE, SW_SHOWNORMAL};

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct SHELLEXECUTEINFOW {
    pub cbSize: u32,
    pub fMask: u32,
    pub hwnd: HWND,
    pub lpVerb: *const u16,
    pub lpFile: *const u16,
    pub lpParameters: *const u16,
    pub lpDirectory: *const u16,
    pub nShow: i32,
    pub hInstApp: HINSTANCE,
    pub lpIDList: *mut core::ffi::c_void,
    pub lpClass: *const u16,
    pub hkeyClass: *mut core::ffi::c_void,
    pub dwHotKey: u32,
    pub hIconOrMonitor: HANDLE,
    pub hProcess: HANDLE,
}

pub const SEE_MASK_NOCLOSEPROCESS: u32 = 0x00000040;

windows_link::link!("shell32.dll" "system" fn ShellExecuteExW(pExecInfo: *mut SHELLEXECUTEINFOW) -> i32);

/// Restituisce `true` se il processo corrente è in esecuzione con privilegi elevati (Amministratore).
pub fn is_elevated() -> bool {
    let Some(advapi) = Library::load("advapi32.dll") else { return false };
    let Some(open_process_token) = advapi.proc(b"OpenProcessToken\0") else { return false };
    let Some(get_token_information) = advapi.proc(b"GetTokenInformation\0") else { return false };

    type FnOpenProcessToken = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
    type FnGetTokenInformation = unsafe extern "system" fn(HANDLE, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;

    let open_process_token: FnOpenProcessToken = unsafe { core::mem::transmute(open_process_token) };
    let get_token_information: FnGetTokenInformation = unsafe { core::mem::transmute(get_token_information) };

    let mut token: HANDLE = null_mut();
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_ELEVATION: u32 = 20;

    #[repr(C)]
    struct TokenElevation {
        token_is_elevated: u32,
    }

    unsafe {
        if open_process_token(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TokenElevation = zeroed();
        let mut ret_len = 0u32;
        let res = get_token_information(
            token,
            TOKEN_ELEVATION,
            (&raw mut elevation).cast(),
            size_of::<TokenElevation>() as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        res != 0 && elevation.token_is_elevated != 0
    }
}

/// Abilita `SeDebugPrivilege` nel token del processo corrente se è elevato.
/// Consente di terminare qualsiasi processo (`PROCESS_TERMINATE`) e controllare socket/servizi.
pub fn enable_debug_privilege() -> bool {
    let Some(advapi) = Library::load("advapi32.dll") else { return false };
    let Some(open_process_token) = advapi.proc(b"OpenProcessToken\0") else { return false };
    let Some(lookup_privilege_value_w) = advapi.proc(b"LookupPrivilegeValueW\0") else { return false };
    let Some(adjust_token_privileges) = advapi.proc(b"AdjustTokenPrivileges\0") else { return false };

    type FnOpenProcessToken = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
    type FnLookupPrivilegeValueW = unsafe extern "system" fn(*const u16, *const u16, *mut LUID) -> i32;
    type FnAdjustTokenPrivileges =
        unsafe extern "system" fn(HANDLE, i32, *const TOKEN_PRIVILEGES, u32, *mut core::ffi::c_void, *mut u32) -> i32;

    let open_process_token: FnOpenProcessToken = unsafe { core::mem::transmute(open_process_token) };
    let lookup_privilege_value_w: FnLookupPrivilegeValueW = unsafe { core::mem::transmute(lookup_privilege_value_w) };
    let adjust_token_privileges: FnAdjustTokenPrivileges = unsafe { core::mem::transmute(adjust_token_privileges) };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LUID {
        low_part: u32,
        high_part: i32,
    }

    #[repr(C)]
    struct LUID_AND_ATTRIBUTES {
        luid: LUID,
        attributes: u32,
    }

    #[repr(C)]
    struct TOKEN_PRIVILEGES {
        privilege_count: u32,
        privileges: [LUID_AND_ATTRIBUTES; 1],
    }

    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_QUERY: u32 = 0x0008;
    const SE_PRIVILEGE_ENABLED: u32 = 0x00000002;

    let mut token: HANDLE = null_mut();
    unsafe {
        if open_process_token(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
            return false;
        }

        let se_debug_name: Vec<u16> = "SeDebugPrivilege".encode_utf16().chain(core::iter::once(0)).collect();
        let mut luid: LUID = zeroed();
        if lookup_privilege_value_w(core::ptr::null(), se_debug_name.as_ptr(), &mut luid) == 0 {
            CloseHandle(token);
            return false;
        }

        let tp = TOKEN_PRIVILEGES {
            privilege_count: 1,
            privileges: [LUID_AND_ATTRIBUTES { luid, attributes: SE_PRIVILEGE_ENABLED }],
        };

        let res = adjust_token_privileges(token, 0, &tp, 0, null_mut(), null_mut());
        CloseHandle(token);
        res != 0
    }
}

/// Riavvia `nextm` con elevazione UAC (`runas`). Se l'utente accetta il popup UAC,
/// la nuova istanza parte come amministratore e l'istanza corrente viene chiusa.
pub fn restart_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe_w: Vec<u16> = exe.to_string_lossy().encode_utf16().chain(core::iter::once(0)).collect();
    let verb_w: Vec<u16> = "runas".encode_utf16().chain(core::iter::once(0)).collect();
    let params_w: Vec<u16> = "--admin-restart".encode_utf16().chain(core::iter::once(0)).collect();
    let dir = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let dir_w: Vec<u16> = dir.encode_utf16().chain(core::iter::once(0)).collect();

    let hwnd = unsafe { GetForegroundWindow() };

    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.hwnd = hwnd;
    info.lpVerb = verb_w.as_ptr();
    info.lpFile = exe_w.as_ptr();
    info.lpParameters = params_w.as_ptr();
    info.lpDirectory = if dir.is_empty() { core::ptr::null() } else { dir_w.as_ptr() };
    info.nShow = SW_SHOWNORMAL;

    let res = unsafe { ShellExecuteExW(&mut info) };
    if res == 0 {
        // L'utente ha annullato il prompt UAC o l'avvio è fallito: non chiudere il programma!
        return false;
    }

    if !info.hProcess.is_null() {
        unsafe { CloseHandle(info.hProcess) };
    }

    // Rilascia immediatamente il mutex dell'istanza corrente per permettere
    // alla nuova istanza elevata di acquisirlo istantaneamente senza ritardi.
    crate::sys::single::release_current();

    // Termina l'istanza corrente non elevata.
    std::process::exit(0);
}

/// Controlla se l'attività pianificata `nextm` esiste nell'Utilità di Pianificazione.
pub fn is_task_scheduler_enabled() -> bool {
    let Ok(code) = run_silent_cmd("schtasks.exe /query /tn \"nextm\"") else {
        return false;
    };
    code == 0
}

/// Esegue schtasks con elevazione UAC se il processo corrente non è elevato.
fn run_schtasks_elevated(args: &str) -> bool {
    let sys_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let exe = format!("{sys_dir}\\System32\\schtasks.exe");
    let exe_w: Vec<u16> = exe.encode_utf16().chain(core::iter::once(0)).collect();
    let verb_w: Vec<u16> = "runas".encode_utf16().chain(core::iter::once(0)).collect();
    let params_w: Vec<u16> = args.encode_utf16().chain(core::iter::once(0)).collect();

    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.hwnd = unsafe { GetForegroundWindow() };
    info.lpVerb = verb_w.as_ptr();
    info.lpFile = exe_w.as_ptr();
    info.lpParameters = params_w.as_ptr();
    info.nShow = SW_HIDE;

    let res = unsafe { ShellExecuteExW(&mut info) };
    if res == 0 {
        return false;
    }
    if !info.hProcess.is_null() {
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(info.hProcess, 10000);
            let mut code = 0u32;
            windows_sys::Win32::System::Threading::GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
            code == 0
        }
    } else {
        true
    }
}

/// Crea o rimuove l'attività pianificata per avviare `nextm` all'accesso come amministratore
/// con il flag `/rl highest`, senza mostrare popup UAC all'accensione del PC.
pub fn set_task_scheduler_enabled(enable: bool) -> bool {
    let elevated = is_elevated();
    if enable {
        let Ok(exe) = std::env::current_exe() else { return false };
        let exe_str = exe.to_string_lossy();
        let args = format!("/create /tn \"nextm\" /tr \"'{exe_str}'\" /sc onlogon /rl highest /f");
        if elevated {
            let cmd = format!("schtasks.exe {args}");
            match run_silent_cmd(&cmd) {
                Ok(code) => code == 0,
                Err(_) => false,
            }
        } else {
            run_schtasks_elevated(&args)
        }
    } else {
        let args = "/delete /tn \"nextm\" /f";
        if elevated {
            let cmd = format!("schtasks.exe {args}");
            match run_silent_cmd(&cmd) {
                Ok(code) => code == 0,
                Err(_) => false,
            }
        } else {
            run_schtasks_elevated(args)
        }
    }
}

fn run_silent_cmd(cmd: &str) -> Result<u32, u32> {
    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(core::iter::once(0)).collect();
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    unsafe {
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = zeroed();

        if CreateProcessW(
            core::ptr::null(),
            cmd_w.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            core::ptr::null(),
            core::ptr::null(),
            &si,
            &mut pi,
        ) == 0
        {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }

        windows_sys::Win32::System::Threading::WaitForSingleObject(pi.hProcess, 5000);
        let mut exit_code = 0u32;
        windows_sys::Win32::System::Threading::GetExitCodeProcess(pi.hProcess, &mut exit_code);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(exit_code)
    }
}
