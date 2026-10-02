//! Ispezione delle porte e connessioni TCP e UDP (stile TCPView).
//!
//! Carica `iphlpapi.dll` dinamicamente da System32 (zero import statici), interroga le tabelle
//! estese TCP e UDP sia per IPv4 che per IPv6 e associa ogni socket al rispettivo PID proprietario.

use core::ffi::c_void;
use core::mem::size_of;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    UDP_TABLE_OWNER_PID,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use crate::inspect::{IpAddrKind, SocketInfo, SocketProtocol, TcpState};
use crate::sys::dll::Library;

const MIB_TCP_STATE_DELETE_TCB: u32 = 12;

type FnGetExtendedTcpTable = unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32;

type FnGetExtendedUdpTable = unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32;

type FnSetTcpEntry = unsafe extern "system" fn(*const MibTcpRow) -> u32;

#[repr(C)]
struct MibTcpRow {
    dw_state: u32,
    dw_local_addr: u32,
    dw_local_port: u32,
    dw_remote_addr: u32,
    dw_remote_port: u32,
}

/// Scanner delle tabelle di rete TCP e UDP.
pub struct SocketScanner {
    _dll: Library,
    get_tcp: FnGetExtendedTcpTable,
    get_udp: FnGetExtendedUdpTable,
    set_tcp: Option<FnSetTcpEntry>,
    buffer: Vec<u8>,
}

impl SocketScanner {
    /// Inizializza lo scanner caricando `iphlpapi.dll` da System32.
    pub fn new() -> Option<SocketScanner> {
        let dll = Library::load("iphlpapi.dll")?;
        let get_tcp_raw = dll.proc(b"GetExtendedTcpTable\0")?;
        let get_udp_raw = dll.proc(b"GetExtendedUdpTable\0")?;
        let set_tcp_raw = dll.proc(b"SetTcpEntry\0");

        // SAFETY: puntatori validi ottenuti da iphlpapi.dll.
        let get_tcp: FnGetExtendedTcpTable = unsafe { core::mem::transmute(get_tcp_raw) };
        let get_udp: FnGetExtendedUdpTable = unsafe { core::mem::transmute(get_udp_raw) };
        let set_tcp: Option<FnSetTcpEntry> = set_tcp_raw.map(|p| unsafe { core::mem::transmute(p) });

        Some(SocketScanner { _dll: dll, get_tcp, get_udp, set_tcp, buffer: Vec::with_capacity(64 * 1024) })
    }

    /// Esegue la scansione completa di tutti i socket (TCP e UDP, IPv4 e IPv6).
    /// I risultati vengono accumulati in `out`.
    pub fn scan(&mut self, out: &mut Vec<SocketInfo>) -> bool {
        out.clear();
        self.scan_tcp_v4(out);
        self.scan_tcp_v6(out);
        self.scan_udp_v4(out);
        self.scan_udp_v6(out);
        true
    }

    /// Tenta di terminare/chiudere forzatamente una connessione TCP IPv4 attiva.
    pub fn close_tcp_v4(&self, local_ip: [u8; 4], local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> bool {
        let Some(set_tcp) = self.set_tcp else {
            return false;
        };

        let row = MibTcpRow {
            dw_state: MIB_TCP_STATE_DELETE_TCB,
            dw_local_addr: u32::from_ne_bytes(local_ip),
            dw_local_port: u32::from(local_port.to_be()),
            dw_remote_addr: u32::from_ne_bytes(remote_ip),
            dw_remote_port: u32::from(remote_port.to_be()),
        };

        // SAFETY: puntatore valido a MibTcpRow.
        let ret = unsafe { set_tcp(&row) };
        ret == NO_ERROR
    }

    fn ensure_buffer(&mut self, needed: usize) {
        if self.buffer.len() < needed {
            self.buffer.resize(needed, 0);
        }
    }

    fn scan_tcp_v4(&mut self, out: &mut Vec<SocketInfo>) {
        let mut size: u32 = self.buffer.len() as u32;
        let mut ret = unsafe {
            (self.get_tcp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET as u32, TCP_TABLE_OWNER_PID_ALL, 0)
        };

        if ret == ERROR_INSUFFICIENT_BUFFER {
            self.ensure_buffer(size as usize);
            ret = unsafe {
                (self.get_tcp)(
                    self.buffer.as_mut_ptr().cast(),
                    &mut size,
                    1,
                    AF_INET as u32,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
        }

        if ret != NO_ERROR || (size as usize) < size_of::<u32>() {
            return;
        }

        let num_entries = unsafe { *self.buffer.as_ptr().cast::<u32>() } as usize;
        let entries_start = size_of::<u32>();
        let row_size = size_of::<MIB_TCPROW_OWNER_PID>();

        if entries_start + num_entries.saturating_mul(row_size) > size as usize {
            return;
        }

        for i in 0..num_entries {
            let offset = entries_start + i * row_size;
            let row = unsafe { &*self.buffer.as_ptr().add(offset).cast::<MIB_TCPROW_OWNER_PID>() };

            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);
            let remote_port = u16::from_be((row.dwRemotePort & 0xFFFF) as u16);
            let state = TcpState::from_u32(row.dwState);

            out.push(SocketInfo {
                protocol: SocketProtocol::Tcp,
                local_addr: IpAddrKind::V4(row.dwLocalAddr.to_ne_bytes()),
                local_port,
                remote_addr: if remote_port > 0 || state != TcpState::Listening {
                    Some(IpAddrKind::V4(row.dwRemoteAddr.to_ne_bytes()))
                } else {
                    None
                },
                remote_port: if remote_port > 0 || state != TcpState::Listening { Some(remote_port) } else { None },
                state: Some(state),
                pid: row.dwOwningPid,
            });
        }
    }

    fn scan_tcp_v6(&mut self, out: &mut Vec<SocketInfo>) {
        let mut size: u32 = self.buffer.len() as u32;
        let mut ret = unsafe {
            (self.get_tcp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET6 as u32, TCP_TABLE_OWNER_PID_ALL, 0)
        };

        if ret == ERROR_INSUFFICIENT_BUFFER {
            self.ensure_buffer(size as usize);
            ret = unsafe {
                (self.get_tcp)(
                    self.buffer.as_mut_ptr().cast(),
                    &mut size,
                    1,
                    AF_INET6 as u32,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
        }

        if ret != NO_ERROR || (size as usize) < size_of::<u32>() {
            return;
        }

        let num_entries = unsafe { *self.buffer.as_ptr().cast::<u32>() } as usize;
        let entries_start = size_of::<u32>();
        let row_size = size_of::<MIB_TCP6ROW_OWNER_PID>();

        if entries_start + num_entries.saturating_mul(row_size) > size as usize {
            return;
        }

        for i in 0..num_entries {
            let offset = entries_start + i * row_size;
            let row = unsafe { &*self.buffer.as_ptr().add(offset).cast::<MIB_TCP6ROW_OWNER_PID>() };

            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);
            let remote_port = u16::from_be((row.dwRemotePort & 0xFFFF) as u16);
            let state = TcpState::from_u32(row.dwState);

            out.push(SocketInfo {
                protocol: SocketProtocol::Tcp,
                local_addr: IpAddrKind::V6(row.ucLocalAddr),
                local_port,
                remote_addr: if remote_port > 0 || state != TcpState::Listening {
                    Some(IpAddrKind::V6(row.ucRemoteAddr))
                } else {
                    None
                },
                remote_port: if remote_port > 0 || state != TcpState::Listening { Some(remote_port) } else { None },
                state: Some(state),
                pid: row.dwOwningPid,
            });
        }
    }

    fn scan_udp_v4(&mut self, out: &mut Vec<SocketInfo>) {
        let mut size: u32 = self.buffer.len() as u32;
        let mut ret = unsafe {
            (self.get_udp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET as u32, UDP_TABLE_OWNER_PID, 0)
        };

        if ret == ERROR_INSUFFICIENT_BUFFER {
            self.ensure_buffer(size as usize);
            ret = unsafe {
                (self.get_udp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET as u32, UDP_TABLE_OWNER_PID, 0)
            };
        }

        if ret != NO_ERROR || (size as usize) < size_of::<u32>() {
            return;
        }

        let num_entries = unsafe { *self.buffer.as_ptr().cast::<u32>() } as usize;
        let entries_start = size_of::<u32>();
        let row_size = size_of::<MIB_UDPROW_OWNER_PID>();

        if entries_start + num_entries.saturating_mul(row_size) > size as usize {
            return;
        }

        for i in 0..num_entries {
            let offset = entries_start + i * row_size;
            let row = unsafe { &*self.buffer.as_ptr().add(offset).cast::<MIB_UDPROW_OWNER_PID>() };

            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);

            out.push(SocketInfo {
                protocol: SocketProtocol::Udp,
                local_addr: IpAddrKind::V4(row.dwLocalAddr.to_ne_bytes()),
                local_port,
                remote_addr: None,
                remote_port: None,
                state: None,
                pid: row.dwOwningPid,
            });
        }
    }

    fn scan_udp_v6(&mut self, out: &mut Vec<SocketInfo>) {
        let mut size: u32 = self.buffer.len() as u32;
        let mut ret = unsafe {
            (self.get_udp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET6 as u32, UDP_TABLE_OWNER_PID, 0)
        };

        if ret == ERROR_INSUFFICIENT_BUFFER {
            self.ensure_buffer(size as usize);
            ret = unsafe {
                (self.get_udp)(self.buffer.as_mut_ptr().cast(), &mut size, 1, AF_INET6 as u32, UDP_TABLE_OWNER_PID, 0)
            };
        }

        if ret != NO_ERROR || (size as usize) < size_of::<u32>() {
            return;
        }

        let num_entries = unsafe { *self.buffer.as_ptr().cast::<u32>() } as usize;
        let entries_start = size_of::<u32>();
        let row_size = size_of::<MIB_UDP6ROW_OWNER_PID>();

        if entries_start + num_entries.saturating_mul(row_size) > size as usize {
            return;
        }

        for i in 0..num_entries {
            let offset = entries_start + i * row_size;
            let row = unsafe { &*self.buffer.as_ptr().add(offset).cast::<MIB_UDP6ROW_OWNER_PID>() };

            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);

            out.push(SocketInfo {
                protocol: SocketProtocol::Udp,
                local_addr: IpAddrKind::V6(row.ucLocalAddr),
                local_port,
                remote_addr: None,
                remote_port: None,
                state: None,
                pid: row.dwOwningPid,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_live_system_sockets() {
        let mut scanner = SocketScanner::new().expect("scanner initialization");
        let mut sockets = Vec::new();
        let ok = scanner.scan(&mut sockets);
        assert!(ok);
        assert!(!sockets.is_empty(), "ci deve essere almeno un socket attivo su Windows");

        // Verifica che ci sia almeno un socket TCP e uno UDP
        let has_tcp = sockets.iter().any(|s| s.protocol == SocketProtocol::Tcp);
        let has_udp = sockets.iter().any(|s| s.protocol == SocketProtocol::Udp);
        assert!(has_tcp, "almeno un socket TCP presente nel sistema");
        assert!(has_udp, "almeno un socket UDP presente nel sistema");

        // Tutti i socket devono avere PID valido o System/Idle (0 o > 0)
        for s in &sockets {
            assert!(s.local_port > 0);
        }
    }
}
