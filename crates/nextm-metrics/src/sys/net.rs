//! Rete: lettura da Windows con iphlpapi caricata su richiesta (M2).
//!
//! iphlpapi si carica da System32 quando la metrica viene attivata e si scarica quando
//! `NetSampler` esce di scena. Le funzioni si risolvono con `GetProcAddress`: niente import
//! statici, che `cargo xtask check-imports` vieta.
//!
//! - **Elenco** (operazione rara): `GetIfTable2Ex(MibIfTableNormalWithoutStatistics)`, che non
//!   legge i contatori, più le route predefinite (`GetIpForwardTable2`) con la metrica
//!   dell'interfaccia (`GetIpInterfaceEntry`), per l'ordine che `net::select` usa in modalità
//!   Auto.
//! - **A ogni tick**: `GetIfEntry2` al livello Normal (il livello Raw dà 0 sulla Wi-Fi
//!   disconnessa) sulle interfacce contate, con una sola `MIB_IF_ROW2` riusata. Poi
//!   `GetIfEntry2Ex(MibIfEntryNormalWithoutStatistics)`, più economica, sulle schede fisiche che
//!   all'ultimo elenco non erano Up: così una Wi-Fi che si connette o un cavo inserito si vedono
//!   al tick seguente (§5.4), senza thread di notifica.
//!
//! Quando una lettura dice che l'elenco non è più valido (scheda sparita, lettura fallita,
//! passaggio fra Up e non Up) `needs_refresh` diventa vero: l'app rilegge l'elenco e rifà la
//! scelta con `net::select`.

use core::ffi::c_void;
use core::mem::transmute;
use core::ptr::null_mut;

use windows_sys::Win32::Foundation::{NO_ERROR, WIN32_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    IF_TYPE_ETHERNET_CSMACD, MIB_IF_ENTRY_LEVEL, MIB_IF_ROW2, MIB_IF_TABLE_LEVEL, MIB_IF_TABLE2, MIB_IPFORWARD_ROW2,
    MIB_IPFORWARD_TABLE2, MIB_IPINTERFACE_ROW, MibIfEntryNormalWithoutStatistics, MibIfTableNormalWithoutStatistics,
};
use windows_sys::Win32::NetworkManagement::Ndis::{IfOperStatusNotPresent, IfOperStatusUp, NET_LUID_LH};
use windows_sys::Win32::Networking::WinSock::{ADDRESS_FAMILY, AF_INET, AF_INET6, AF_UNSPEC};

use crate::net::{IfInfo, counts_in_sum, decode_flags};
use crate::sys::dll::{Library, RawProc};

// Firme di netioapi.h (le stesse che windows-sys dichiara con un import statico).
type GetIfTable2ExFn = unsafe extern "system" fn(MIB_IF_TABLE_LEVEL, *mut *mut MIB_IF_TABLE2) -> WIN32_ERROR;
type FreeMibTableFn = unsafe extern "system" fn(*const c_void);
type GetIfEntry2Fn = unsafe extern "system" fn(*mut MIB_IF_ROW2) -> WIN32_ERROR;
type GetIfEntry2ExFn = unsafe extern "system" fn(MIB_IF_ENTRY_LEVEL, *mut MIB_IF_ROW2) -> WIN32_ERROR;
type GetIpForwardTable2Fn = unsafe extern "system" fn(ADDRESS_FAMILY, *mut *mut MIB_IPFORWARD_TABLE2) -> WIN32_ERROR;
type GetIpInterfaceEntryFn = unsafe extern "system" fn(*mut MIB_IPINTERFACE_ROW) -> WIN32_ERROR;

/// Legge l'elenco delle interfacce e i contatori di rete.
pub struct NetSampler {
    get_if_table2_ex: GetIfTable2ExFn,
    free_mib_table: FreeMibTableFn,
    get_if_entry2: GetIfEntry2Fn,
    /// Facoltative: senza, niente controllo economico delle schede non Up e niente metrica
    /// dell'interfaccia (resta quella della route). Esistono da Windows 10 1703.
    get_if_entry2_ex: Option<GetIfEntry2ExFn>,
    get_ip_forward_table2: GetIpForwardTable2Fn,
    get_ip_interface_entry: Option<GetIpInterfaceEntryFn>,
    /// La riga riusata per ogni lettura (1352 byte su x64): nessuna allocazione per tick.
    row: MIB_IF_ROW2,
    /// Stato Up di ogni interfaccia all'ultimo elenco: una lettura diversa lo rende vecchio.
    listed: Vec<(u64, bool)>,
    /// Schede fisiche non Up all'ultimo elenco (escluse le NotPresent, che tornano con
    /// `WM_DEVICECHANGE`): si controlla a ogni tick se sono passate a Up.
    watch: Vec<u64>,
    refresh: bool,
    /// Ultimo campo: la DLL si scarica dopo il resto. Le funzioni sopra valgono finché è caricata.
    _lib: Library,
}

impl NetSampler {
    /// Carica iphlpapi da System32 e risolve le funzioni. `None` se la DLL o una funzione
    /// indispensabile manca: la voce di menu resta grigia.
    pub fn new() -> Option<NetSampler> {
        let lib = Library::load("iphlpapi.dll")?;
        let p = lib.proc(b"GetIfTable2Ex\0")?;
        // SAFETY: firma di GetIfTable2Ex in netioapi.h; valida finché `lib` è caricata (campo `_lib`).
        let get_if_table2_ex = unsafe { transmute::<RawProc, GetIfTable2ExFn>(p) };
        let p = lib.proc(b"FreeMibTable\0")?;
        // SAFETY: firma di FreeMibTable in netioapi.h; valida finché `lib` è caricata.
        let free_mib_table = unsafe { transmute::<RawProc, FreeMibTableFn>(p) };
        let p = lib.proc(b"GetIfEntry2\0")?;
        // SAFETY: firma di GetIfEntry2 in netioapi.h; valida finché `lib` è caricata.
        let get_if_entry2 = unsafe { transmute::<RawProc, GetIfEntry2Fn>(p) };
        let p = lib.proc(b"GetIpForwardTable2\0")?;
        // SAFETY: firma di GetIpForwardTable2 in netioapi.h; valida finché `lib` è caricata.
        let get_ip_forward_table2 = unsafe { transmute::<RawProc, GetIpForwardTable2Fn>(p) };
        let get_if_entry2_ex = lib.proc(b"GetIfEntry2Ex\0").map(|p| {
            // SAFETY: firma di GetIfEntry2Ex in netioapi.h; valida finché `lib` è caricata.
            unsafe { transmute::<RawProc, GetIfEntry2ExFn>(p) }
        });
        let get_ip_interface_entry = lib.proc(b"GetIpInterfaceEntry\0").map(|p| {
            // SAFETY: firma di GetIpInterfaceEntry in netioapi.h; valida finché `lib` è caricata.
            unsafe { transmute::<RawProc, GetIpInterfaceEntryFn>(p) }
        });
        Some(NetSampler {
            get_if_table2_ex,
            free_mib_table,
            get_if_entry2,
            get_if_entry2_ex,
            get_ip_forward_table2,
            get_ip_interface_entry,
            row: MIB_IF_ROW2::default(),
            listed: Vec::new(),
            watch: Vec::new(),
            refresh: false,
            _lib: lib,
        })
    }

    /// Elenco delle interfacce (operazione rara: avvio, ripresa, `WM_DEVICECHANGE`,
    /// `needs_refresh`, ogni 60 s). Ordinato come chiede `net::IfInfo`: prima le interfacce con
    /// una route predefinita, dalla metrica effettiva minore, poi le altre nell'ordine di Windows.
    /// Vuoto se la lettura fallisce; in quel caso `needs_refresh` resta vero.
    pub fn interfaces(&mut self) -> Vec<IfInfo> {
        let mut list = self.read_interface_table();
        let routes = self.default_routes();
        for i in &mut list {
            i.default_route = routes.iter().any(|r| r.luid == i.luid);
        }
        // Ordinamento stabile: a parità di rango resta l'ordine di Windows.
        list.sort_by_key(|i| route_rank(&routes, i));
        self.listed.clear();
        self.listed.extend(list.iter().map(|i| (i.luid, i.up)));
        self.watch.clear();
        self.watch.extend(list.iter().filter(|i| !i.up && counts_in_sum(i)).map(|i| i.luid));
        self.refresh = list.is_empty();
        list
    }

    /// Contatori cumulativi (luid, byte ricevuti, byte inviati) delle interfacce `luids`, in
    /// `out` (svuotato e riusato). `GetIfEntry2` al livello Normal, una riga riusata: nessuna
    /// allocazione finché `out` ha già la capacità.
    ///
    /// Una lettura che fallisce (`ERROR_FILE_NOT_FOUND`: il LUID non esiste più) salta quella
    /// interfaccia e rende vero `needs_refresh`; lo stesso succede se un'interfaccia letta, o una
    /// scheda fisica sorvegliata, è passata fra Up e non Up dall'ultimo elenco. Restituisce `true`
    /// se tutte le interfacce sono state lette. Un'interfaccia saltata manca da `out`, quindi
    /// `RateMeter` riparte comunque da una nuova base.
    pub fn counters(&mut self, luids: &[u64], out: &mut Vec<(u64, u64, u64)>) -> bool {
        out.clear();
        out.reserve(luids.len());
        let get = self.get_if_entry2;
        let mut complete = true;
        for &luid in luids {
            let read = read_row(&mut self.row, luid, |row| {
                // SAFETY: `row` è una MIB_IF_ROW2 valida e scrivibile con InterfaceLuid impostato.
                unsafe { get(row) }
            });
            if !read {
                complete = false;
                self.refresh = true;
                continue;
            }
            let up = self.row.OperStatus == IfOperStatusUp;
            if self.listed.iter().any(|&(l, was_up)| l == luid && was_up != up) {
                self.refresh = true;
            }
            out.push((luid, self.row.InOctets, self.row.OutOctets));
        }
        self.check_watched();
        complete
    }

    /// `true` se l'elenco va riletto con [`NetSampler::interfaces`] (e la scelta rifatta).
    pub fn needs_refresh(&self) -> bool {
        self.refresh
    }

    /// Le schede fisiche non Up all'ultimo elenco: basta una lettura senza statistiche.
    fn check_watched(&mut self) {
        let Some(get_ex) = self.get_if_entry2_ex else { return };
        let NetSampler { row, watch, refresh, .. } = self;
        for &luid in watch.iter() {
            let read = read_row(row, luid, |row| {
                // SAFETY: `row` è una MIB_IF_ROW2 valida e scrivibile con InterfaceLuid impostato.
                unsafe { get_ex(MibIfEntryNormalWithoutStatistics, row) }
            });
            // Passata a Up, oppure sparita: in entrambi i casi l'elenco è vecchio.
            if !read || row.OperStatus == IfOperStatusUp {
                *refresh = true;
            }
        }
    }

    fn read_interface_table(&self) -> Vec<IfInfo> {
        let mut table: *mut MIB_IF_TABLE2 = null_mut();
        // SAFETY: `table` riceve una tabella allocata da iphlpapi, liberata da `MibTable`.
        let err = unsafe { (self.get_if_table2_ex)(MibIfTableNormalWithoutStatistics, &mut table) };
        if err != NO_ERROR || table.is_null() {
            return Vec::new();
        }
        let _free = MibTable { ptr: table.cast(), free: self.free_mib_table };
        // SAFETY: la chiamata è riuscita: `table` punta a NumEntries righe contigue (il campo
        // `Table` è un vettore di lunghezza variabile), valide finché `_free` non la libera.
        let rows = unsafe {
            core::slice::from_raw_parts((&raw const (*table).Table).cast::<MIB_IF_ROW2>(), (*table).NumEntries as usize)
        };
        rows.iter().map(info_from_row).collect()
    }

    /// Le route 0.0.0.0/0 e ::/0, con la metrica effettiva (route + interfaccia, §5.4).
    fn default_routes(&self) -> Vec<DefaultRoute> {
        let mut out = Vec::new();
        let mut table: *mut MIB_IPFORWARD_TABLE2 = null_mut();
        // SAFETY: `table` riceve una tabella allocata da iphlpapi, liberata da `MibTable`.
        let err = unsafe { (self.get_ip_forward_table2)(AF_UNSPEC, &mut table) };
        if err != NO_ERROR || table.is_null() {
            return out;
        }
        let _free = MibTable { ptr: table.cast(), free: self.free_mib_table };
        // SAFETY: come sopra: NumEntries righe contigue, valide finché `_free` non la libera.
        let rows = unsafe {
            core::slice::from_raw_parts(
                (&raw const (*table).Table).cast::<MIB_IPFORWARD_ROW2>(),
                (*table).NumEntries as usize,
            )
        };
        for r in rows.iter().filter(|r| r.DestinationPrefix.PrefixLength == 0) {
            // SAFETY: `si_family` sta all'inizio di ogni variante di SOCKADDR_INET.
            let family = unsafe { r.DestinationPrefix.Prefix.si_family };
            let family_rank = match family {
                AF_INET => 0,
                AF_INET6 => 1,
                _ => continue,
            };
            // SAFETY: il LUID è un intero a 64 bit: ogni configurazione di bit è valida.
            let luid = unsafe { r.InterfaceLuid.Value };
            let metric = u64::from(r.Metric) + self.interface_metric(luid, family);
            out.push(DefaultRoute { luid, family_rank, metric });
        }
        out
    }

    /// Metrica dell'interfaccia per una famiglia (automatica o impostata); 0 se non si legge.
    fn interface_metric(&self, luid: u64, family: ADDRESS_FAMILY) -> u64 {
        let Some(get) = self.get_ip_interface_entry else { return 0 };
        let mut row =
            MIB_IPINTERFACE_ROW { Family: family, InterfaceLuid: NET_LUID_LH { Value: luid }, ..Default::default() };
        // SAFETY: riga valida e scrivibile, con famiglia e LUID impostati come chiede l'API.
        let err = unsafe { get(&mut row) };
        if err == NO_ERROR { u64::from(row.Metric) } else { 0 }
    }
}

/// Azzera `row`, imposta il LUID e chiama `get`; `false` se la chiamata fallisce.
fn read_row(row: &mut MIB_IF_ROW2, luid: u64, get: impl FnOnce(*mut MIB_IF_ROW2) -> WIN32_ERROR) -> bool {
    *row = MIB_IF_ROW2 { InterfaceLuid: NET_LUID_LH { Value: luid }, ..Default::default() };
    get(row) == NO_ERROR
}

fn info_from_row(r: &MIB_IF_ROW2) -> IfInfo {
    let (hardware, filter, endpoint) = decode_flags(r.InterfaceAndOperStatusFlags._bitfield);
    IfInfo {
        // SAFETY: il LUID è un intero a 64 bit: ogni configurazione di bit è valida.
        luid: unsafe { r.InterfaceLuid.Value },
        if_type: r.Type,
        medium: u32::try_from(r.PhysicalMediumType).unwrap_or(0),
        up: r.OperStatus == IfOperStatusUp,
        not_present: r.OperStatus == IfOperStatusNotPresent,
        hardware,
        filter,
        endpoint,
        default_route: false,
        alias: wide_to_string(&r.Alias),
        description: wide_to_string(&r.Description),
    }
}

/// Da un campo UTF-16 terminato da zero (o pieno) a `String`.
fn wide_to_string(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(w.get(..n).unwrap_or_default())
}

/// Una route predefinita: interfaccia, famiglia (0 = IPv4, 1 = IPv6) e metrica effettiva.
struct DefaultRoute {
    luid: u64,
    family_rank: u8,
    metric: u64,
}

/// Rango di un'interfaccia nell'elenco: prima quelle con una route predefinita, per famiglia
/// (IPv4 se c'è, altrimenti IPv6), metrica effettiva e poi Ethernet; tutte le altre dopo, alla pari.
fn route_rank(routes: &[DefaultRoute], i: &IfInfo) -> (u8, u8, u64, u8) {
    let not_ethernet = u8::from(i.if_type != IF_TYPE_ETHERNET_CSMACD);
    routes
        .iter()
        .filter(|r| r.luid == i.luid)
        .map(|r| (0, r.family_rank, r.metric, not_ethernet))
        .min()
        .unwrap_or((1, 0, 0, 0))
}

/// Tabella allocata da iphlpapi, liberata con `FreeMibTable` quando esce di scena.
struct MibTable {
    ptr: *mut c_void,
    free: FreeMibTableFn,
}

impl Drop for MibTable {
    fn drop(&mut self) {
        // SAFETY: tabella restituita da GetIfTable2Ex o GetIpForwardTable2, liberata una sola volta.
        unsafe { (self.free)(self.ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{NetMode, RateMeter, Rates, select};
    use std::time::{Duration, Instant};

    /// Orologio monotono in unità da 100 ns, come `QueryPerformanceCounter` nell'app.
    fn wall_100ns(origin: Instant) -> u64 {
        (origin.elapsed().as_nanos() / 100) as u64
    }

    #[test]
    fn loads_iphlpapi_and_lists_interfaces() {
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        assert!(!list.is_empty());
        assert!(!s.needs_refresh());
        // Il loopback c'è sempre, e i LUID sono unici.
        assert!(list.iter().any(|i| i.if_type == 24 && i.up), "loopback");
        for (k, a) in list.iter().enumerate() {
            assert!(list[k + 1..].iter().all(|b| b.luid != a.luid), "LUID ripetuto: {}", a.alias);
            assert!(!a.alias.is_empty() || !a.description.is_empty());
        }
        // Le interfacce con la route predefinita vengono per prime.
        let routed = list.iter().take_while(|i| i.default_route).count();
        assert!(list[routed..].iter().all(|i| !i.default_route));
    }

    #[test]
    fn sum_contains_the_active_ethernet() {
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        let sel = select(&list, NetMode::Sum);
        for luid in &sel.luids {
            let i = list.iter().find(|i| i.luid == *luid).expect("LUID dell'elenco");
            assert!(i.up && !i.filter && counts_in_sum(i), "{i:?}");
        }
        // La scheda fisica che porta il traffico (Up, con la route predefinita) è nella somma;
        // sulla macchina di sviluppo è l'Ethernet 7 (IfType 6).
        for i in list.iter().filter(|i| i.up && i.hardware && !i.filter && !i.endpoint) {
            if i.default_route || i.if_type == 6 {
                assert!(sel.luids.contains(&i.luid), "{} manca dalla somma {:?}", i.alias, sel.label);
            }
        }
        assert!(!sel.fallback);
    }

    #[test]
    fn two_reads_one_second_apart_give_plausible_rates() {
        let origin = Instant::now();
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        let sel = select(&list, NetMode::Sum);
        let mut meter = RateMeter::new();
        let mut out = Vec::new();
        assert!(s.counters(&sel.luids, &mut out));
        assert_eq!(out.len(), sel.luids.len());
        assert_eq!(meter.update(&out, wall_100ns(origin)), None);
        std::thread::sleep(Duration::from_secs(1));
        assert!(s.counters(&sel.luids, &mut out));
        let r = meter.update(&out, wall_100ns(origin)).expect("secondo campione");
        // Plausibile: meno di 100 Gbit/s in ogni direzione.
        assert!(r.down < 12_500_000_000 && r.up < 12_500_000_000, "{r:?}");
    }

    #[test]
    fn unknown_luid_is_skipped_and_asks_for_a_new_list() {
        let mut s = NetSampler::new().expect("iphlpapi");
        s.interfaces();
        assert!(!s.needs_refresh());
        // IfType 6 con un NetLuidIndex che non esiste: GetIfEntry2 dà ERROR_FILE_NOT_FOUND.
        let ghost = (6u64 << 48) | (0xFF_FFFE << 24);
        let mut out = vec![(1, 2, 3)];
        assert!(!s.counters(&[ghost], &mut out));
        assert!(out.is_empty());
        assert!(s.needs_refresh());
        // Un nuovo elenco la rimette a posto.
        s.interfaces();
        assert!(!s.needs_refresh());
    }

    #[test]
    fn counters_reuse_the_output_buffer() {
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        let sel = select(&list, NetMode::Sum);
        let mut out = Vec::with_capacity(8);
        let before = out.as_ptr();
        for _ in 0..3 {
            s.counters(&sel.luids, &mut out);
        }
        assert_eq!(out.as_ptr(), before, "nessuna nuova allocazione");
    }

    // ---- misure e diagnostica: `cargo test -p nextm-metrics --lib sys::net -- --ignored --nocapture` ----

    #[test]
    #[ignore = "diagnostica: stampa l'elenco delle interfacce"]
    fn print_interfaces() {
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        println!("{} interfacce", list.len());
        for i in &list {
            println!(
                "{:016X} tipo {:3} mezzo {:2} {} hw {:5} filtro {:5} endpoint {:5} route {:5} somma {:5} | {} | {}",
                i.luid,
                i.if_type,
                i.medium,
                if i.up {
                    "Up        "
                } else if i.not_present {
                    "NotPresent"
                } else {
                    "Down      "
                },
                i.hardware,
                i.filter,
                i.endpoint,
                i.default_route,
                counts_in_sum(i),
                i.alias,
                i.description
            );
        }
        for mode in [NetMode::Sum, NetMode::Auto] {
            println!("{mode:?}: {:?}", select(&list, mode));
        }
        println!("Menu: {:?}", crate::net::menu_candidates(&list, None));
        println!("Sorvegliate: {:?}", s.watch);
    }

    fn thread_cycles() -> u64 {
        use windows_sys::Win32::System::Threading::GetCurrentThread;
        use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
        let mut c = 0u64;
        // SAFETY: pseudo-handle del thread corrente e puntatore a una variabile locale.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
        c
    }

    fn set_ecoqos(on: bool) {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
            SetProcessInformation,
        };
        let mask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: mask,
            StateMask: if on { mask } else { 0 },
        };
        // SAFETY: struttura locale valida della dimensione dichiarata.
        let ok = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
        assert_ne!(ok, 0);
    }

    fn module_loaded(name: &str) -> bool {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: nome terminato da zero; nessun riferimento al modulo viene preso.
        !unsafe { GetModuleHandleW(w.as_ptr()) }.is_null()
    }

    /// Mediana, minimo e massimo.
    fn stats(v: &mut [f64]) -> (f64, f64, f64) {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[0], v[v.len() - 1])
    }

    #[test]
    #[ignore = "misura: costo di counters() con la selezione Somma"]
    fn measure_counters_cost() {
        let dlls = ["iphlpapi.dll", "nsi.dll", "winnsi.dll", "dhcpcsvc.dll", "dhcpcsvc6.dll", "ws2_32.dll"];
        let loaded = |when: &str| {
            let names: Vec<&str> = dlls.iter().copied().filter(|d| module_loaded(d)).collect();
            println!("moduli {when}: {names:?}");
        };
        loaded("prima");
        let mut s = NetSampler::new().expect("iphlpapi");
        loaded("con NetSampler");
        let t = Instant::now();
        let list = s.interfaces();
        println!("interfaces() a freddo: {:.0} µs, {} righe", t.elapsed().as_secs_f64() * 1e6, list.len());
        let mut warm = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            s.interfaces();
            warm.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let (med, min, max) = stats(&mut warm);
        println!("interfaces() a caldo: mediana {med:.0} µs (min {min:.0}, max {max:.0})");
        let sel = select(&list, NetMode::Sum);
        println!("Somma: {} = {:?}; sorvegliate non Up: {:?}", sel.label, sel.luids, s.watch);
        let mut out = Vec::with_capacity(8);

        for ecoqos in [false, true] {
            set_ecoqos(ecoqos);
            let label = if ecoqos { "con EcoQoS" } else { "senza EcoQoS" };
            // A caldo: chiamate una dopo l'altra.
            let n: u32 = 2_000;
            let (t, c) = (Instant::now(), thread_cycles());
            for _ in 0..n {
                s.counters(&sel.luids, &mut out);
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
            let cycles = (thread_cycles() - c) / u64::from(n);
            println!("counters() a caldo, {label}: {us:.1} µs, {cycles} cicli per chiamata");
            // Come nell'app: una chiamata al secondo, con le cache fredde.
            let (mut us, mut cy) = (Vec::new(), Vec::new());
            for _ in 0..12 {
                std::thread::sleep(Duration::from_secs(1));
                let (t, c) = (Instant::now(), thread_cycles());
                s.counters(&sel.luids, &mut out);
                us.push(t.elapsed().as_secs_f64() * 1e6);
                cy.push((thread_cycles() - c) as f64);
            }
            let (med, min, max) = stats(&mut us);
            let (cmed, cmin, cmax) = stats(&mut cy);
            println!(
                "counters() a 1 Hz, {label}: mediana {med:.0} µs (min {min:.0}, max {max:.0}); \
                 cicli mediana {cmed:.0} (min {cmin:.0}, max {cmax:.0})"
            );
        }
        set_ecoqos(false);
        assert!(!s.needs_refresh());
        drop(s);
        loaded("dopo il drop");
    }

    /// Contatori "Network Interface" (in italiano "Interfaccia di rete") letti con PDH: valori
    /// grezzi cumulativi di ogni istanza, per il confronto con la somma.
    struct Pdh {
        query: *mut c_void,
        rx: *mut c_void,
        tx: *mut c_void,
    }

    impl Pdh {
        fn open() -> Pdh {
            use windows_sys::Win32::System::Performance::{PdhAddEnglishCounterW, PdhOpenQueryW};
            let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
            let (mut query, mut rx, mut tx) = (null_mut(), null_mut(), null_mut());
            // SAFETY: puntatori a variabili locali e stringhe terminate da zero.
            unsafe {
                assert_eq!(PdhOpenQueryW(core::ptr::null(), 0, &mut query), 0);
                let path = wide("\\Network Interface(*)\\Bytes Received/sec");
                assert_eq!(PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut rx), 0);
                let path = wide("\\Network Interface(*)\\Bytes Sent/sec");
                assert_eq!(PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut tx), 0);
            }
            Pdh { query, rx, tx }
        }

        /// (istanza, ricevuti, inviati) di ogni istanza.
        fn read(&self) -> Vec<(String, u64, u64)> {
            use windows_sys::Win32::System::Performance::PdhCollectQueryData;
            // SAFETY: query aperta da `open`.
            assert_eq!(unsafe { PdhCollectQueryData(self.query) }, 0);
            let rx = raw_values(self.rx);
            let tx = raw_values(self.tx);
            rx.into_iter()
                .map(|(name, r)| {
                    let t = tx.iter().find(|(n, _)| *n == name).map_or(0, |x| x.1);
                    (name, r, t)
                })
                .collect()
        }
    }

    impl Drop for Pdh {
        fn drop(&mut self) {
            use windows_sys::Win32::System::Performance::PdhCloseQuery;
            // SAFETY: query aperta da `open`, chiusa una sola volta.
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    fn raw_values(counter: *mut c_void) -> Vec<(String, u64)> {
        use windows_sys::Win32::System::Performance::{PDH_MORE_DATA, PDH_RAW_COUNTER_ITEM_W, PdhGetRawCounterArrayW};
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: prima chiamata per la dimensione del buffer.
        let status = unsafe { PdhGetRawCounterArrayW(counter, &mut size, &mut count, null_mut()) };
        assert_eq!(status, PDH_MORE_DATA);
        // Buffer allineato a 8 byte per le PDH_RAW_COUNTER_ITEM_W (i nomi seguono nello stesso buffer).
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: il buffer ha almeno `size` byte scrivibili ed è allineato.
        let status = unsafe { PdhGetRawCounterArrayW(counter, &mut size, &mut count, buf.as_mut_ptr().cast()) };
        assert_eq!(status, 0);
        // SAFETY: PDH ha scritto `count` elementi all'inizio del buffer.
        let items =
            unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<PDH_RAW_COUNTER_ITEM_W>(), count as usize) };
        items
            .iter()
            .map(|it| {
                // SAFETY: `szName` punta a una stringa terminata da zero dentro `buf`.
                let len = (0..).take_while(|&k| unsafe { *it.szName.add(k) } != 0).count();
                // SAFETY: `len` unità valide, lette sopra.
                let name = String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(it.szName, len) });
                (name, it.RawValue.FirstValue as u64)
            })
            .collect()
    }

    #[test]
    #[ignore = "misura: confronto con i contatori di Windows, da eseguire durante un download"]
    fn compare_sum_with_windows_counters() {
        let seconds: u32 = std::env::var("NEXTM_NET_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(15);
        let origin = Instant::now();
        let mut s = NetSampler::new().expect("iphlpapi");
        let list = s.interfaces();
        let sel = select(&list, NetMode::Sum);
        let pdh = Pdh::open();
        let first = pdh.read();
        println!("Somma: {} {:?}", sel.label, sel.luids);
        println!("Istanze di \"Interfaccia di rete\": {:?}", first.iter().map(|x| &x.0).collect::<Vec<_>>());
        let sum_pdh = |v: &[(String, u64, u64)]| v.iter().fold((0u64, 0u64), |a, x| (a.0 + x.1, a.1 + x.2));
        let sum_ours = |v: &[(u64, u64, u64)]| v.iter().fold((0u64, 0u64), |a, x| (a.0 + x.1, a.1 + x.2));
        let mut meter = RateMeter::new();
        let mut out = Vec::new();
        s.counters(&sel.luids, &mut out);
        meter.update(&out, wall_100ns(origin));
        let (p0, o0, t0) = (sum_pdh(&first), sum_ours(&out), Instant::now());
        let (mut p_prev, mut t_prev) = (p0, t0);
        let mut rates: Vec<Rates> = Vec::new();
        println!("   s | PDH ↓ B/s     | somma ↓ B/s   | PDH ↑ B/s     | somma ↑ B/s");
        for k in 1..=seconds {
            std::thread::sleep(Duration::from_secs(1));
            // Le due letture a pochi µs di distanza: stessa finestra.
            let p = sum_pdh(&pdh.read());
            s.counters(&sel.luids, &mut out);
            let now = Instant::now();
            let r = meter.update(&out, wall_100ns(origin)).expect("campione");
            let dt = now.duration_since(t_prev).as_secs_f64();
            println!(
                "{k:4} | {:13.0} | {:13} | {:13.0} | {:13}",
                (p.0 - p_prev.0) as f64 / dt,
                r.down,
                (p.1 - p_prev.1) as f64 / dt,
                r.up
            );
            rates.push(r);
            (p_prev, t_prev) = (p, now);
        }
        let o1 = sum_ours(&out);
        let (d_pdh, d_ours) = ((p_prev.0 - p0.0, p_prev.1 - p0.1), (o1.0 - o0.0, o1.1 - o0.1));
        println!(
            "Totale ricevuti: PDH {} B, somma {} B, rapporto {:.4}",
            d_pdh.0,
            d_ours.0,
            d_ours.0 as f64 / d_pdh.0 as f64
        );
        println!(
            "Totale inviati:  PDH {} B, somma {} B, rapporto {:.4}",
            d_pdh.1,
            d_ours.1,
            d_ours.1 as f64 / d_pdh.1 as f64
        );
        println!("Finestra: {:.3} s", t_prev.duration_since(t0).as_secs_f64());
    }
}
