//! Motore unificato di ispezione: correla Processi, Socket (TCP/UDP) e Servizi Windows.

use std::collections::BTreeMap;

use crate::inspect::{ProcessRecord, ServiceInfo, SocketInfo};
use crate::sys::processes::ProcessScanner;
use crate::sys::services::ServiceScanner;
use crate::sys::sockets::SocketScanner;

/// Processo unificato con relativi servizi ospitati e porte/connessioni di rete attive.
#[derive(Debug, Clone)]
pub struct UnifiedProcess {
    pub pid: u32,
    pub name: String,
    pub threads: u32,
    pub working_set_bytes: u64,
    pub private_bytes: u64,
    pub cpu_percent: f32,
    pub services: Vec<ServiceInfo>,
    pub sockets: Vec<SocketInfo>,
}

/// Istantanea completa del sistema per la finestra unificata di ispezione.
#[derive(Debug, Clone)]
pub struct InspectSnapshot {
    pub processes: Vec<UnifiedProcess>,
    pub orphaned_sockets: Vec<SocketInfo>,
    pub total_sockets: usize,
    pub total_services: usize,
}

/// Motore unificato che orchestra la raccolta e correlazione dei dati.
pub struct InspectEngine {
    proc_scanner: ProcessScanner,
    socket_scanner: Option<SocketScanner>,
    service_scanner: Option<ServiceScanner>,
    prev_cpu: BTreeMap<u32, u64>,
    next_prev_cpu: BTreeMap<u32, u64>,
    last_tick_ms: u64,
    num_logical_cpus: u32,
    scratch_procs: Vec<ProcessRecord>,
    scratch_sockets: Vec<SocketInfo>,
    scratch_services: Vec<ServiceInfo>,
    pid_to_idx: BTreeMap<u32, usize>,
}

impl Default for InspectEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl InspectEngine {
    /// Crea una nuova istanza del motore di ispezione.
    pub fn new() -> InspectEngine {
        let num_logical_cpus = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        }
        .max(1);

        InspectEngine {
            proc_scanner: ProcessScanner::new(),
            socket_scanner: SocketScanner::new(),
            service_scanner: ServiceScanner::new(),
            prev_cpu: BTreeMap::new(),
            next_prev_cpu: BTreeMap::new(),
            last_tick_ms: 0,
            num_logical_cpus,
            scratch_procs: Vec::with_capacity(512),
            scratch_sockets: Vec::with_capacity(1024),
            scratch_services: Vec::with_capacity(512),
            pid_to_idx: BTreeMap::new(),
        }
    }

    /// Esegue la scansione completa e correla tutti i dati.
    pub fn scan(&mut self) -> InspectSnapshot {
        let now_ms = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        let delta_ms =
            if self.last_tick_ms > 0 && now_ms > self.last_tick_ms { now_ms - self.last_tick_ms } else { 1000 };
        self.last_tick_ms = now_ms;

        // 1. Scansione processi
        self.proc_scanner.scan(&mut self.scratch_procs);

        // 2. Scansione socket
        if let Some(s) = &mut self.socket_scanner {
            s.scan(&mut self.scratch_sockets);
        } else {
            self.scratch_sockets.clear();
        }

        // 3. Scansione servizi
        if let Some(s) = &mut self.service_scanner {
            s.scan(&mut self.scratch_services);
        } else {
            self.scratch_services.clear();
        }

        let total_sockets = self.scratch_sockets.len();
        let total_services = self.scratch_services.len();

        // 4. Calcola CPU % e indicizza i processi
        let mut unified_procs: Vec<UnifiedProcess> = Vec::with_capacity(self.scratch_procs.len());
        self.next_prev_cpu.clear();
        self.pid_to_idx.clear();

        let delta_time_100ns = (delta_ms as f32) * 10_000.0 * (self.num_logical_cpus as f32);

        for (idx, p) in self.scratch_procs.iter().enumerate() {
            self.next_prev_cpu.insert(p.pid, p.cpu_time_100ns);
            self.pid_to_idx.insert(p.pid, idx);

            let cpu_pct = if let Some(&old_cpu) = self.prev_cpu.get(&p.pid) {
                let cpu_delta = p.cpu_time_100ns.saturating_sub(old_cpu) as f32;
                if delta_time_100ns > 0.0 { ((cpu_delta / delta_time_100ns) * 100.0).clamp(0.0, 100.0) } else { 0.0 }
            } else {
                0.0
            };

            unified_procs.push(UnifiedProcess {
                pid: p.pid,
                name: p.name.clone(),
                threads: p.threads,
                working_set_bytes: p.working_set_bytes,
                private_bytes: p.private_bytes,
                cpu_percent: cpu_pct,
                services: Vec::new(),
                sockets: Vec::new(),
            });
        }
        std::mem::swap(&mut self.prev_cpu, &mut self.next_prev_cpu);

        // 5. Correlazione Servizi -> Processo (O(1))
        for svc in self.scratch_services.drain(..) {
            if let Some(&idx) = self.pid_to_idx.get(&svc.pid) {
                unified_procs[idx].services.push(svc);
            }
        }

        // 6. Correlazione Socket -> Processo (O(1))
        let mut orphaned_sockets: Vec<SocketInfo> = Vec::new();
        for sock in self.scratch_sockets.drain(..) {
            if let Some(&idx) = self.pid_to_idx.get(&sock.pid) {
                unified_procs[idx].sockets.push(sock);
            } else {
                orphaned_sockets.push(sock);
            }
        }

        InspectSnapshot { processes: unified_procs, orphaned_sockets, total_sockets, total_services }
    }

    /// Termina il processo specificato.
    pub fn kill_process(&self, pid: u32) -> bool {
        ProcessScanner::kill_process(pid)
    }

    /// Arresta il servizio specificato.
    pub fn stop_service(&self, name: &str) -> bool {
        self.service_scanner.as_ref().is_some_and(|s| s.stop_service(name))
    }

    /// Avvia il servizio specificato.
    pub fn start_service(&self, name: &str) -> bool {
        self.service_scanner.as_ref().is_some_and(|s| s.start_service(name))
    }

    /// Chiude una connessione TCP IPv4 attiva.
    pub fn close_tcp_v4(&self, local_ip: [u8; 4], local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> bool {
        self.socket_scanner.as_ref().is_some_and(|s| s.close_tcp_v4(local_ip, local_port, remote_ip, remote_port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_engine_scans_and_correlates() {
        let mut engine = InspectEngine::new();
        let snapshot = engine.scan();

        assert!(!snapshot.processes.is_empty(), "ci devono essere processi attivi");
        assert!(snapshot.total_sockets > 0, "ci devono essere socket attivi");
        assert!(snapshot.total_services > 0, "ci devono essere servizi attivi");

        // Almeno un processo svchost.exe deve avere servizi correlati
        let svchost_with_services = snapshot
            .processes
            .iter()
            .filter(|p| p.name.to_lowercase().contains("svchost"))
            .any(|p| !p.services.is_empty());
        assert!(svchost_with_services, "svchost deve avere servizi correlati");

        // Almeno un processo deve avere socket di rete correlati
        let proc_with_sockets = snapshot.processes.iter().any(|p| !p.sockets.is_empty());
        assert!(proc_with_sockets, "almeno un processo deve avere socket correlati");
    }
}
