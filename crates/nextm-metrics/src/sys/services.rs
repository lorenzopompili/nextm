//! Ispezione e controllo dei servizi Windows (SCM) via advapi32.dll.
//!
//! Carica dinamicamente advapi32.dll da System32, interroga il Service Control Manager
//! per ottenere tutti i servizi e associarli al relativo PID del processo che li ospita.

use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::null;

use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::System::Services::{
    ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
    SC_STATUS_TYPE, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATE_ALL, SERVICE_STATUS,
    SERVICE_STOP, SERVICE_WIN32,
};

use crate::inspect::{ServiceInfo, ServiceState};
use crate::sys::dll::Library;

type ScHandle = *mut c_void;

type FnOpenSCManagerW = unsafe extern "system" fn(*const u16, *const u16, u32) -> ScHandle;

type FnCloseServiceHandle = unsafe extern "system" fn(ScHandle) -> i32;

type FnEnumServicesStatusExW = unsafe extern "system" fn(
    ScHandle,
    SC_STATUS_TYPE,
    u32,
    u32,
    *mut u8,
    u32,
    *mut u32,
    *mut u32,
    *mut u32,
    *const u16,
) -> i32;

type FnOpenServiceW = unsafe extern "system" fn(ScHandle, *const u16, u32) -> ScHandle;

type FnControlService = unsafe extern "system" fn(ScHandle, u32, *mut SERVICE_STATUS) -> i32;

type FnStartServiceW = unsafe extern "system" fn(ScHandle, u32, *const *const u16) -> i32;

/// Scanner e gestore dei servizi Windows.
pub struct ServiceScanner {
    _dll: Library,
    open_scm: FnOpenSCManagerW,
    close_scm: FnCloseServiceHandle,
    enum_services: FnEnumServicesStatusExW,
    open_service: Option<FnOpenServiceW>,
    control_service: Option<FnControlService>,
    start_service: Option<FnStartServiceW>,
    buffer: Vec<u8>,
}

impl ServiceScanner {
    /// Inizializza lo scanner caricando `advapi32.dll` da System32.
    pub fn new() -> Option<ServiceScanner> {
        let dll = Library::load("advapi32.dll")?;
        let open_scm_raw = dll.proc(b"OpenSCManagerW\0")?;
        let close_scm_raw = dll.proc(b"CloseServiceHandle\0")?;
        let enum_services_raw = dll.proc(b"EnumServicesStatusExW\0")?;

        let open_service_raw = dll.proc(b"OpenServiceW\0");
        let control_service_raw = dll.proc(b"ControlService\0");
        let start_service_raw = dll.proc(b"StartServiceW\0");

        // SAFETY: puntatori validi esportati da advapi32.dll.
        let open_scm: FnOpenSCManagerW = unsafe { core::mem::transmute(open_scm_raw) };
        let close_scm: FnCloseServiceHandle = unsafe { core::mem::transmute(close_scm_raw) };
        let enum_services: FnEnumServicesStatusExW = unsafe { core::mem::transmute(enum_services_raw) };

        let open_service: Option<FnOpenServiceW> = open_service_raw.map(|p| unsafe { core::mem::transmute(p) });
        let control_service: Option<FnControlService> = control_service_raw.map(|p| unsafe { core::mem::transmute(p) });
        let start_service: Option<FnStartServiceW> = start_service_raw.map(|p| unsafe { core::mem::transmute(p) });

        Some(ServiceScanner {
            _dll: dll,
            open_scm,
            close_scm,
            enum_services,
            open_service,
            control_service,
            start_service,
            buffer: Vec::with_capacity(128 * 1024),
        })
    }

    /// Esegue la scansione di tutti i servizi Windows e li aggiunge a `out`.
    pub fn scan(&mut self, out: &mut Vec<ServiceInfo>) -> bool {
        out.clear();

        // SAFETY: parametri nulli per la macchina locale e il database predefinito.
        let scm = unsafe { (self.open_scm)(null(), null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) };
        if scm.is_null() {
            return false;
        }

        let mut bytes_needed: u32 = 0;
        let mut services_returned: u32 = 0;
        let mut resume_handle: u32 = 0;

        let ret = unsafe {
            (self.enum_services)(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                self.buffer.as_mut_ptr(),
                self.buffer.len() as u32,
                &mut bytes_needed,
                &mut services_returned,
                &mut resume_handle,
                null(),
            )
        };

        if ret == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if err == ERROR_MORE_DATA || bytes_needed > self.buffer.len() as u32 {
                self.buffer.resize(bytes_needed as usize + 4096, 0);
                bytes_needed = 0;
                services_returned = 0;
                resume_handle = 0;

                let ret2 = unsafe {
                    (self.enum_services)(
                        scm,
                        SC_ENUM_PROCESS_INFO,
                        SERVICE_WIN32,
                        SERVICE_STATE_ALL,
                        self.buffer.as_mut_ptr(),
                        self.buffer.len() as u32,
                        &mut bytes_needed,
                        &mut services_returned,
                        &mut resume_handle,
                        null(),
                    )
                };

                if ret2 == 0 {
                    unsafe { (self.close_scm)(scm) };
                    return false;
                }
            } else {
                unsafe { (self.close_scm)(scm) };
                return false;
            }
        }

        let count = services_returned as usize;
        let entry_size = size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
        let buf_start = self.buffer.as_ptr() as usize;
        let buf_end = buf_start + self.buffer.len();

        for i in 0..count {
            let offset = i * entry_size;
            if offset + entry_size > self.buffer.len() {
                break;
            }

            let entry = unsafe { &*self.buffer.as_ptr().add(offset).cast::<ENUM_SERVICE_STATUS_PROCESSW>() };
            let pid = entry.ServiceStatusProcess.dwProcessId;
            let state = ServiceState::from_u32(entry.ServiceStatusProcess.dwCurrentState);

            let name = if !entry.lpServiceName.is_null()
                && (entry.lpServiceName as usize) >= buf_start
                && (entry.lpServiceName as usize) < buf_end
            {
                unsafe { wide_ptr_to_string(entry.lpServiceName, buf_end) }
            } else {
                String::new()
            };

            let display_name = if !entry.lpDisplayName.is_null()
                && (entry.lpDisplayName as usize) >= buf_start
                && (entry.lpDisplayName as usize) < buf_end
            {
                unsafe { wide_ptr_to_string(entry.lpDisplayName, buf_end) }
            } else {
                String::new()
            };

            out.push(ServiceInfo { name, display_name, state, pid });
        }

        unsafe { (self.close_scm)(scm) };
        true
    }

    /// Arresta un servizio dato il nome (es. "wuauserv"). Richiede privilegi idonei.
    pub fn stop_service(&self, service_name: &str) -> bool {
        let (Some(open_svc), Some(ctrl_svc)) = (self.open_service, self.control_service) else {
            return false;
        };

        let scm = unsafe { (self.open_scm)(null(), null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return false;
        }

        let wide_name: Vec<u16> = service_name.encode_utf16().chain(core::iter::once(0)).collect();
        let svc = unsafe { open_svc(scm, wide_name.as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS) };
        if svc.is_null() {
            unsafe { (self.close_scm)(scm) };
            return false;
        }

        let mut status: SERVICE_STATUS = unsafe { core::mem::zeroed() };
        let ret = unsafe { ctrl_svc(svc, SERVICE_CONTROL_STOP, &mut status) };

        unsafe {
            (self.close_scm)(svc);
            (self.close_scm)(scm);
        }

        ret != 0
    }

    /// Avvia un servizio dato il nome. Richiede privilegi idonei.
    pub fn start_service(&self, service_name: &str) -> bool {
        let (Some(open_svc), Some(start_svc)) = (self.open_service, self.start_service) else {
            return false;
        };

        let scm = unsafe { (self.open_scm)(null(), null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return false;
        }

        let wide_name: Vec<u16> = service_name.encode_utf16().chain(core::iter::once(0)).collect();
        let svc = unsafe { open_svc(scm, wide_name.as_ptr(), SERVICE_START | SERVICE_QUERY_STATUS) };
        if svc.is_null() {
            unsafe { (self.close_scm)(scm) };
            return false;
        }

        let ret = unsafe { start_svc(svc, 0, null()) };

        unsafe {
            (self.close_scm)(svc);
            (self.close_scm)(scm);
        }

        ret != 0
    }
}

unsafe fn wide_ptr_to_string(ptr: *const u16, max_addr: usize) -> String {
    let mut len = 0;
    unsafe {
        while (ptr.add(len) as usize) < max_addr && *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = core::slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_live_system_services() {
        let mut scanner = ServiceScanner::new().expect("service scanner initialization");
        let mut services = Vec::new();
        let ok = scanner.scan(&mut services);
        assert!(ok);
        assert!(!services.is_empty(), "ci devono essere servizi registrati su Windows");

        // Ci devono essere servizi in stato Running
        let has_running = services.iter().any(|s| s.state == ServiceState::Running);
        assert!(has_running, "almeno un servizio attivo su Windows");

        // Servizi noti tipici di Windows (es. RpcSs o EventLog o Dnscache)
        let has_known = services.iter().any(|s| s.name == "RpcSs" || s.name == "EventLog" || s.name == "Dnscache");
        assert!(has_known, "servizi di sistema standard trovati");
    }
}
