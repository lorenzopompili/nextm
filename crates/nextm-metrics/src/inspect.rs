//! Modelli dati per l'ispezione unificata di rete, servizi e processi.

use core::fmt;

/// Protocollo di trasporto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketProtocol {
    Tcp,
    Udp,
}

impl SocketProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// Stato di una connessione TCP (come riportato da MIB_TCPROW).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listening,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
    Unknown(u32),
}

impl TcpState {
    pub fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::Closed,
            2 => Self::Listening,
            3 => Self::SynSent,
            4 => Self::SynReceived,
            5 => Self::Established,
            6 => Self::FinWait1,
            7 => Self::FinWait2,
            8 => Self::CloseWait,
            9 => Self::Closing,
            10 => Self::LastAck,
            11 => Self::TimeWait,
            12 => Self::DeleteTcb,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "CLOSED",
            Self::Listening => "LISTENING",
            Self::SynSent => "SYN_SENT",
            Self::SynReceived => "SYN_RCVD",
            Self::Established => "ESTABLISHED",
            Self::FinWait1 => "FIN_WAIT1",
            Self::FinWait2 => "FIN_WAIT2",
            Self::CloseWait => "CLOSE_WAIT",
            Self::Closing => "CLOSING",
            Self::LastAck => "LAST_ACK",
            Self::TimeWait => "TIME_WAIT",
            Self::DeleteTcb => "DELETE_TCB",
            Self::Unknown(_) => "UNKNOWN",
        }
    }
}

/// Indirizzo IP (IPv4 o IPv6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddrKind {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl fmt::Display for IpAddrKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(b) => write!(f, "{}.{}.{}.{}", b[0], b[1], b[2], b[3]),
            Self::V6(b) => {
                if *self == Self::V6([0; 16]) {
                    write!(f, "::")
                } else if *self == Self::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]) {
                    write!(f, "::1")
                } else {
                    let s = [
                        u16::from_be_bytes([b[0], b[1]]),
                        u16::from_be_bytes([b[2], b[3]]),
                        u16::from_be_bytes([b[4], b[5]]),
                        u16::from_be_bytes([b[6], b[7]]),
                        u16::from_be_bytes([b[8], b[9]]),
                        u16::from_be_bytes([b[10], b[11]]),
                        u16::from_be_bytes([b[12], b[13]]),
                        u16::from_be_bytes([b[14], b[15]]),
                    ];
                    write!(f, "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}", s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7])
                }
            }
        }
    }
}

/// Informazioni su una porta o connessione socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketInfo {
    pub protocol: SocketProtocol,
    pub local_addr: IpAddrKind,
    pub local_port: u16,
    pub remote_addr: Option<IpAddrKind>,
    pub remote_port: Option<u16>,
    pub state: Option<TcpState>,
    pub pid: u32,
}

/// Stato di un servizio Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown(u32),
}

impl ServiceState {
    pub fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::StartPending => "Starting",
            Self::StopPending => "Stopping",
            Self::Running => "Running",
            Self::ContinuePending => "Resuming",
            Self::PausePending => "Pausing",
            Self::Paused => "Paused",
            Self::Unknown(_) => "Unknown",
        }
    }
}

/// Informazioni su un servizio Windows ospitato da un processo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub state: ServiceState,
    pub pid: u32,
}

/// Informazioni su un processo Windows rilevato dal kernel.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub name: String,
    pub threads: u32,
    pub working_set_bytes: u64,
    pub private_bytes: u64,
    pub cpu_time_100ns: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_state_conversion() {
        assert_eq!(TcpState::from_u32(2), TcpState::Listening);
        assert_eq!(TcpState::Listening.as_str(), "LISTENING");
        assert_eq!(TcpState::from_u32(5), TcpState::Established);
        assert_eq!(TcpState::Established.as_str(), "ESTABLISHED");
        assert_eq!(TcpState::from_u32(999), TcpState::Unknown(999));
    }

    #[test]
    fn ip_addr_display() {
        let v4 = IpAddrKind::V4([127, 0, 0, 1]);
        assert_eq!(format!("{v4}"), "127.0.0.1");

        let v6_any = IpAddrKind::V6([0; 16]);
        assert_eq!(format!("{v6_any}"), "::");

        let v6_loopback = IpAddrKind::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(format!("{v6_loopback}"), "::1");
    }

    #[test]
    fn service_state_conversion() {
        assert_eq!(ServiceState::from_u32(4), ServiceState::Running);
        assert_eq!(ServiceState::Running.as_str(), "Running");
        assert_eq!(ServiceState::from_u32(1), ServiceState::Stopped);
        assert_eq!(ServiceState::Stopped.as_str(), "Stopped");
    }
}
