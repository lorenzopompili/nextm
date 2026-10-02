//! Core saturo: logica pura (spec §5.2, ricerca `docs/research/2026-09-29-03-core-saturo.md`).
//!
//! Un thread che occupa un core intero vale solo 1/N_LP della CPU globale (il 5% con 20 CPU
//! logiche) e, poiché migra fra le CPU logiche decine di volte al secondo, non ne tiene al 100%
//! nessuna. Il segnale affidabile è la CPU del singolo thread, espressa in core:
//!
//! - u_i = Δ(KernelTime + UserTime) / Δt sulla finestra fra due scansioni. La chiave del thread è
//!   (PID, TID, CreateTime); il PID 0 (Idle) è escluso.
//! - Un thread nato dentro la finestra conta zero prima della nascita: tutto il suo tempo CPU
//!   cade nella finestra, divisa per la sua durata intera. Niente stima CPU/età, che per un thread
//!   appena nato varrebbe circa 1,0 e farebbe scattare lo stato.
//! - s_i = media di u_i pesata sulla durata delle finestre più recenti che coprono almeno 3 s,
//!   con lo zero prima della nascita.
//! - Un thread entra nello stato saturo con s_i ≥ 0,85 core ed esce con s_i < 0,70 (isteresi per
//!   thread); k è il numero dei thread saturi.
//! - "CPU piena", derivata da V dal chiamante, ha la precedenza e sospende la valutazione.
//!
//! Memoria: per ogni thread vivo una voce da 36 byte (chiave, ultimo tempo CPU, u delle ultime 7
//! finestre, stato) più l'indice hash (4 byte per posizione, al massimo metà pieno) e una piccola
//! tabella dei processi. I thread spariti escono dalla mappa a ogni aggiornamento; dopo il
//! riscaldamento `update` non alloca: le strutture crescono solo se crescono i thread.

use core::mem::size_of;

/// Chiave di un thread: PID, TID e CreateTime (unità da 100 ns). Il CreateTime distingue un TID
/// che Windows riusa dopo la fine del thread precedente.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadKey {
    pub pid: u32,
    pub tid: u32,
    pub create: u64,
}

/// Una lettura di un thread in una scansione: tempo CPU cumulativo (kernel + user), in unità da
/// 100 ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadRead {
    pub key: ThreadKey,
    pub cpu: u64,
}

/// Stato della CPU per il core saturo, in ordine di priorità: `Full` > `Saturated` > `Normal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SatState {
    /// Nessun collo di bottiglia.
    Normal,
    /// k thread (1..=255) occupano ciascuno un core intero.
    Saturated(u8),
    /// CPU piena: ha la precedenza sul core saturo.
    Full,
}

/// Esito di un aggiornamento.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SatReport {
    pub state: SatState,
    /// (PID, core × 100) del thread più carico, sulle finestre della media.
    pub top_thread: Option<(u32, u16)>,
    /// (PID, core × 100) del processo più carico (somma dei suoi thread), sulle stesse finestre.
    pub top_process: Option<(u32, u32)>,
}

/// La media copre le finestre più recenti fino ad almeno 3 s (unità da 100 ns).
const SPAN: u64 = 30_000_000;
/// Tolleranza sulla copertura per il ritardo del timer: tre scansioni "ogni secondo" danno
/// finestre da 0,98–1,02 s, e senza tolleranza servirebbe una quarta finestra (1 s di latenza).
const SPAN_SLACK: u64 = 1_500_000;
/// Una finestra più breve di 0,5 s non produce un campione: con Δt di poche decine di ms un
/// thread al 100% si legge fra il 60% e il 129% (ricerca, B-F09). L'aggiornamento si ignora e il
/// tempo passa alla finestra successiva.
const MIN_WINDOW: u64 = 5_000_000;
/// Una finestra più lunga di 15 s (tre volte il periodo più lungo, 5 s) vuol dire sospensione o
/// processo congelato: si riparte da una nuova base.
const MAX_WINDOW: u64 = 150_000_000;
/// Finestre ricordate: con finestre di almeno 0,5 s ne bastano 6 per coprire 3 s.
const RING: usize = 7;
/// Unità di u per finestra: 1/10.000 di core.
const SCALE: u64 = 10_000;
/// Ingresso nello stato saturo: s_i ≥ 0,85 core.
const ENTER: u64 = 8_500;
/// Uscita dallo stato saturo: s_i < 0,70 core.
const EXIT: u64 = 7_000;
/// Oltre 1,25 core per un solo thread la lettura è anomala (un thread gira su una CPU logica alla
/// volta e la contabilità a tick sbaglia di pochi punti): si scarta.
const MAX_RATE: u64 = 12_500;
/// Tetto dei thread seguiti (≈2,4 MB di voci più 0,5 MB di indice). Oltre, i thread in più si
/// ignorano e nessun thread sconosciuto viene preso per nuovo (vedi `complete`).
const MAX_THREADS: usize = 1 << 16;
/// Dimensione minima dell'indice e della tabella dei processi (potenze di due).
const MIN_TABLE: usize = 64;
/// Posizione libera nell'indice.
const EMPTY: u32 = u32::MAX;

/// Stato del thread: saturo (con isteresi).
const SATURATED: u8 = 1;
/// Visto nell'aggiornamento in corso; chi non lo è esce dalla mappa.
const SEEN: u8 = 2;

/// Un thread vivo. La chiave è spezzata in campi da 4 byte, così la voce non richiede
/// l'allineamento a 8 byte e sta in 36 byte invece di 40.
#[derive(Clone, Copy)]
struct Slot {
    pid: u32,
    tid: u32,
    create_lo: u32,
    create_hi: u32,
    /// Ultimi 32 bit del tempo CPU cumulativo alla scansione precedente. Fra due scansioni un
    /// thread consuma al massimo `MAX_WINDOW` × 1,25 (meno di 19 s), molto meno dei 429 s che
    /// fanno girare il contatore, quindi la differenza modulo 2^32 è esatta.
    cpu: u32,
    /// u per finestra, in 1/10.000 di core, nella posizione dell'anello globale delle finestre.
    rate: [u16; RING],
    flags: u8,
}

const _: () = assert!(size_of::<Slot>() <= 36);

impl Slot {
    fn new(key: ThreadKey, cpu: u64) -> Slot {
        Slot {
            pid: key.pid,
            tid: key.tid,
            create_lo: key.create as u32,
            create_hi: (key.create >> 32) as u32,
            cpu: cpu as u32,
            rate: [0; RING],
            flags: 0,
        }
    }

    fn key(&self) -> ThreadKey {
        ThreadKey {
            pid: self.pid,
            tid: self.tid,
            create: (u64::from(self.create_hi) << 32) | u64::from(self.create_lo),
        }
    }

    fn matches(&self, key: ThreadKey) -> bool {
        self.pid == key.pid
            && self.tid == key.tid
            && self.create_lo == key.create as u32
            && self.create_hi == (key.create >> 32) as u32
    }
}

/// Le finestre della media: le più recenti che coprono almeno 3 s, oppure tutte se non bastano.
struct Span {
    /// Posizione nell'anello e durata (100 ns) di ciascuna finestra, dalla più recente.
    windows: [(usize, u64); RING],
    len: usize,
    /// Durata totale (100 ns).
    total: u64,
    /// Le finestre coprono almeno 3 s (a meno della tolleranza): si può valutare lo stato.
    covered: bool,
}

impl Span {
    /// Σ u × durata sulle finestre: la media pesata vale `weighted / total`, in 1/10.000 di core.
    fn weighted(&self, rate: &[u16; RING]) -> u64 {
        self.windows[..self.len].iter().map(|&(i, d)| u64::from(rate[i]) * d).sum()
    }
}

/// Rileva i thread che saturano un core. Vedi la documentazione del modulo.
pub struct SaturationDetector {
    logical: u32,
    /// La mappa dei thread vivi, nell'ordine in cui la scansione li ha incontrati la prima volta.
    slots: Vec<Slot>,
    /// Indice hash da chiave a posizione in `slots` (indirizzamento aperto, sondaggio lineare).
    index: Vec<u32>,
    /// L'indice rispecchia `slots`; dopo una rimozione si ricostruisce al primo bisogno.
    index_valid: bool,
    /// Durate (100 ns) delle ultime finestre: l'anello è comune a tutti i thread.
    dur: [u32; RING],
    /// Posizione della finestra più recente nell'anello.
    head: usize,
    /// Finestre valide nell'anello.
    filled: usize,
    /// Orologio della scansione precedente; `None`: la prossima fa da base.
    last_wall: Option<u64>,
    /// La scansione precedente ha registrato tutti i thread, quindi un thread sconosciuto è nato
    /// dopo. Falso solo oltre `MAX_THREADS`.
    complete: bool,
    /// Somme per processo (PID, Σ pesata), riusate a ogni aggiornamento; PID 0 = libero.
    procs: Vec<(u32, u64)>,
    /// Ultimo esito, restituito quando la finestra è troppo breve.
    last_k: u8,
    last_top_thread: Option<(u32, u16)>,
    last_top_process: Option<(u32, u32)>,
}

impl SaturationDetector {
    /// `logical` è il numero di CPU logiche (N_LP, tutti i gruppi di processori). Non alloca:
    /// le strutture nascono al primo aggiornamento.
    pub fn new(logical: u32) -> SaturationDetector {
        SaturationDetector {
            logical: logical.max(1),
            slots: Vec::new(),
            index: Vec::new(),
            index_valid: false,
            dur: [0; RING],
            head: 0,
            filled: 0,
            last_wall: None,
            complete: true,
            procs: Vec::new(),
            last_k: 0,
            last_top_thread: None,
            last_top_process: None,
        }
    }

    /// Dimentica thread, finestre e stati: il prossimo aggiornamento fa da base (dopo scansioni
    /// saltate, sospensione o errore). Tiene la memoria già allocata.
    pub fn reset(&mut self) {
        self.slots.clear();
        self.index_valid = false;
        self.clear_windows();
        self.last_wall = None;
        self.complete = true;
        self.last_k = 0;
        self.last_top_thread = None;
        self.last_top_process = None;
    }

    /// Soglia di attivazione, in punti di CPU globale: sotto questa percentuale nessun thread può
    /// arrivare a 0,85 core, quindi la scansione si salta. Vale 0,8 / N_LP arrotondato per
    /// eccesso: 4 con 20 CPU logiche.
    pub fn gate_percent(&self) -> u8 {
        80u32.div_ceil(self.logical) as u8
    }

    /// Aggiorna con le letture di una scansione.
    ///
    /// - `wall_100ns`: orologio monotono in unità da 100 ns (lo stesso di `CpuSample::wall`).
    /// - `threads`: tutte le letture della scansione, in qualunque ordine (lo stesso ordine della
    ///   scansione precedente rende la ricerca quasi sempre diretta).
    /// - `full`: lo stato "CPU piena" derivato da V (entra a V ≥ 90, esce a V < 80): se è vero lo
    ///   stato è `Full` e la valutazione dei thread è sospesa.
    ///
    /// Il primo aggiornamento, quello dopo `reset` e quello dopo un intervallo di oltre 15 s fanno
    /// solo da base. Un aggiornamento a meno di 0,5 s dal precedente si ignora e restituisce
    /// l'esito precedente. Lo stato del thread si valuta quando le finestre coprono almeno 3 s;
    /// thread e processo più carichi si danno già dalla prima finestra.
    pub fn update(&mut self, wall_100ns: u64, threads: &[ThreadRead], full: bool) -> SatReport {
        let dt = match self.last_wall.map(|prev| wall_100ns.checked_sub(prev)) {
            Some(Some(dt)) if dt <= MAX_WINDOW => dt,
            // Prima scansione, orologio tornato indietro o intervallo troppo lungo: nuova base.
            _ => {
                self.rebase(wall_100ns, threads);
                return self.remember(0, None, None, full);
            }
        };
        if dt < MIN_WINDOW {
            return SatReport {
                state: state_of(full, self.last_k),
                top_thread: self.last_top_thread,
                top_process: self.last_top_process,
            };
        }
        self.last_wall = Some(wall_100ns);
        self.head = (self.head + 1) % RING;
        self.dur[self.head] = dt as u32; // dt ≤ MAX_WINDOW < 2^32
        self.filled = (self.filled + 1).min(RING);
        let span = self.span();
        self.prepare_procs(threads);

        let head = self.head;
        let complete = self.complete;
        let n_old = self.slots.len();
        let mut cursor = 0usize;
        let mut overflow = false;
        let mut k = 0u32;
        let mut top_thread: Option<(u32, u64)> = None;
        let mut top_process: Option<(u32, u64)> = None;
        let (mut run_pid, mut run_sum) = (0u32, 0u64);

        for t in threads {
            let key = t.key;
            if key.pid == 0 {
                continue;
            }
            let p = match self.locate(key, cursor, n_old) {
                Some(p) => {
                    let slot = &mut self.slots[p];
                    if slot.flags & SEEN != 0 {
                        continue; // chiave ripetuta nella stessa scansione
                    }
                    if p < n_old {
                        cursor = p + 1;
                    }
                    let delta = u64::from((t.cpu as u32).wrapping_sub(slot.cpu));
                    slot.cpu = t.cpu as u32;
                    slot.flags |= SEEN;
                    match rate(delta, dt) {
                        Some(r) => slot.rate[head] = r,
                        // Contatore anomalo (tornato indietro o impossibile): si riparte da zero.
                        None => {
                            slot.rate = [0; RING];
                            slot.flags &= !SATURATED;
                        }
                    }
                    p
                }
                None => {
                    if self.slots.len() >= MAX_THREADS {
                        overflow = true;
                        continue;
                    }
                    let mut slot = Slot::new(key, t.cpu);
                    slot.flags = SEEN;
                    // Nato dopo la scansione precedente: prima della nascita conta zero, quindi
                    // tutto il suo tempo CPU cade in questa finestra. Se la scansione precedente
                    // non aveva registrato tutti i thread non è detto che sia nuovo: parte da zero.
                    if complete && let Some(r) = rate(t.cpu, dt) {
                        slot.rate[head] = r;
                    }
                    self.push(slot)
                }
            };

            let slot = &mut self.slots[p];
            let w = span.weighted(&slot.rate);
            if full {
                // "CPU piena" ha la precedenza: i thread si valutano solo fuori da "piena".
                slot.flags &= !SATURATED;
            } else if span.covered {
                let threshold = if slot.flags & SATURATED != 0 { EXIT } else { ENTER };
                if w >= threshold * span.total {
                    slot.flags |= SATURATED;
                } else {
                    slot.flags &= !SATURATED;
                }
            }
            if slot.flags & SATURATED != 0 {
                k += 1;
            }
            if top_thread.is_none_or(|(_, best)| w > best) {
                top_thread = Some((key.pid, w));
            }
            if key.pid != run_pid {
                flush_run(&mut self.procs, run_pid, run_sum, &mut top_process);
                (run_pid, run_sum) = (key.pid, 0);
            }
            run_sum += w;
        }
        flush_run(&mut self.procs, run_pid, run_sum, &mut top_process);

        // I thread non visti sono finiti: escono dalla mappa, le posizioni si compattano e
        // l'indice si ricostruisce al primo bisogno.
        let before = self.slots.len();
        self.slots.retain_mut(|s| {
            let seen = s.flags & SEEN != 0;
            s.flags &= !SEEN;
            seen
        });
        if self.slots.len() != before {
            self.index_valid = false;
        }
        self.complete = !overflow;

        let top_thread = top_thread.map(|(pid, w)| (pid, core100(w, span.total).min(u64::from(u16::MAX)) as u16));
        let top_process = top_process.map(|(pid, w)| (pid, core100(w, span.total).min(u64::from(u32::MAX)) as u32));
        self.remember(k.min(255) as u8, top_thread, top_process, full)
    }

    /// Nuova base: la mappa riparte dalle letture date, senza finestre né stati.
    fn rebase(&mut self, wall: u64, threads: &[ThreadRead]) {
        self.slots.clear();
        self.index_valid = false;
        self.clear_windows();
        self.last_wall = Some(wall);
        let n = threads.len().min(MAX_THREADS);
        if self.slots.capacity() < n {
            self.slots.reserve_exact(n + n / 8 + MIN_TABLE);
        }
        let mut overflow = false;
        for t in threads.iter().filter(|t| t.key.pid != 0) {
            if self.slots.len() >= MAX_THREADS {
                overflow = true;
                break;
            }
            self.slots.push(Slot::new(t.key, t.cpu));
        }
        self.complete = !overflow;
    }

    fn clear_windows(&mut self) {
        self.dur = [0; RING];
        self.head = 0;
        self.filled = 0;
    }

    /// Salva l'esito per gli aggiornamenti troppo ravvicinati e lo restituisce.
    fn remember(
        &mut self,
        k: u8,
        top_thread: Option<(u32, u16)>,
        top_process: Option<(u32, u32)>,
        full: bool,
    ) -> SatReport {
        self.last_k = k;
        self.last_top_thread = top_thread;
        self.last_top_process = top_process;
        SatReport { state: state_of(full, k), top_thread, top_process }
    }

    /// Finestre della media, dalla più recente, fino a coprire almeno 3 s.
    fn span(&self) -> Span {
        let mut windows = [(0, 0); RING];
        let (mut len, mut total, mut covered) = (0, 0, false);
        for (w, out) in windows.iter_mut().enumerate().take(self.filled) {
            let i = (self.head + RING - w) % RING;
            let d = u64::from(self.dur[i]);
            *out = (i, d);
            len = w + 1;
            total += d;
            if total + SPAN_SLACK >= SPAN {
                covered = true;
                break;
            }
        }
        Span { windows, len, total, covered }
    }

    /// Prepara la tabella dei processi: almeno il doppio delle sequenze di PID (una per processo
    /// quando i thread di un processo sono contigui, come nella scansione), svuotata.
    fn prepare_procs(&mut self, threads: &[ThreadRead]) {
        let mut runs = 0usize;
        let mut last = 0u32;
        for t in threads {
            if t.key.pid != 0 && t.key.pid != last {
                runs += 1;
                last = t.key.pid;
            }
        }
        let want = (runs * 2).next_power_of_two().max(MIN_TABLE);
        if self.procs.len() < want {
            self.procs = vec![(0, 0); want];
        } else {
            self.procs.fill((0, 0));
        }
    }

    /// Posizione del thread nella mappa: prima alla posizione che segue l'ultimo trovato (la
    /// scansione elenca i thread sempre nello stesso ordine), poi nell'indice.
    fn locate(&mut self, key: ThreadKey, cursor: usize, n_old: usize) -> Option<usize> {
        if cursor < n_old && self.slots.get(cursor).is_some_and(|s| s.matches(key)) {
            return Some(cursor);
        }
        if !self.index_valid {
            self.rebuild_index();
        }
        let mask = self.index.len().checked_sub(1)?;
        let mut b = hash_key(key) as usize & mask;
        for _ in 0..self.index.len() {
            let i = *self.index.get(b)?;
            if i == EMPTY {
                return None;
            }
            if self.slots.get(i as usize).is_some_and(|s| s.matches(key)) {
                return Some(i as usize);
            }
            b = (b + 1) & mask;
        }
        None
    }

    /// Aggiunge un thread alla mappa e, se l'indice è valido, all'indice.
    fn push(&mut self, slot: Slot) -> usize {
        if self.slots.len() == self.slots.capacity() {
            // Solo se i thread crescono oltre il riscaldamento: un ottavo in più.
            self.slots.reserve_exact(self.slots.len() / 8 + MIN_TABLE);
        }
        let p = self.slots.len();
        self.slots.push(slot);
        if self.index_valid {
            if self.slots.len() * 2 > self.index.len() {
                self.rebuild_index();
            } else {
                self.insert_index(p);
            }
        }
        p
    }

    /// Ricostruisce l'indice, riempito al massimo per metà: cresce solo se crescono i thread.
    fn rebuild_index(&mut self) {
        let want = (self.slots.len() * 2).next_power_of_two().max(MIN_TABLE);
        if self.index.len() < want {
            self.index = vec![EMPTY; want];
        } else {
            self.index.fill(EMPTY);
        }
        for p in 0..self.slots.len() {
            self.insert_index(p);
        }
        self.index_valid = true;
    }

    fn insert_index(&mut self, p: usize) {
        let (Some(slot), Some(mask)) = (self.slots.get(p), self.index.len().checked_sub(1)) else { return };
        let mut b = hash_key(slot.key()) as usize & mask;
        for _ in 0..self.index.len() {
            if let Some(e) = self.index.get_mut(b)
                && *e == EMPTY
            {
                *e = p as u32; // p < MAX_THREADS
                return;
            }
            b = (b + 1) & mask;
        }
    }
}

/// Stato da mostrare: "CPU piena" ha la precedenza sul core saturo.
fn state_of(full: bool, k: u8) -> SatState {
    if full {
        SatState::Full
    } else if k > 0 {
        SatState::Saturated(k)
    } else {
        SatState::Normal
    }
}

/// u su una finestra, in 1/10.000 di core, arrotondato; `None` se la lettura è anomala.
fn rate(delta: u64, dt: u64) -> Option<u16> {
    if delta == 0 {
        return Some(0);
    }
    let r = delta.checked_mul(SCALE)?.checked_add(dt / 2)?.checked_div(dt)?;
    (r <= MAX_RATE).then_some(r as u16)
}

/// Da Σ u × durata (1/10.000 di core × 100 ns) a core × 100, arrotondato.
fn core100(weighted: u64, total: u64) -> u64 {
    let unit = total * (SCALE / 100);
    (weighted + unit / 2).checked_div(unit).unwrap_or(0)
}

/// Chiude la sequenza di thread di un processo: la somma va nella tabella dei processi, e il
/// processo diventa il più carico se supera il migliore. I totali crescono soltanto, quindi il
/// massimo alla fine è quello vero anche se i thread di un processo non sono contigui.
fn flush_run(procs: &mut [(u32, u64)], pid: u32, sum: u64, best: &mut Option<(u32, u64)>) {
    if pid == 0 {
        return;
    }
    let total = add_process(procs, pid, sum);
    if best.is_none_or(|(_, b)| total > b) {
        *best = Some((pid, total));
    }
}

/// Somma `w` al processo `pid` (indirizzamento aperto, PID 0 = libero) e restituisce il totale.
fn add_process(procs: &mut [(u32, u64)], pid: u32, w: u64) -> u64 {
    let mask = procs.len().wrapping_sub(1);
    let mut b = mix(u64::from(pid)) as usize & mask;
    for _ in 0..procs.len() {
        let Some(e) = procs.get_mut(b) else { break };
        if e.0 == pid {
            e.1 += w;
            return e.1;
        }
        if e.0 == 0 {
            *e = (pid, w);
            return w;
        }
        b = (b + 1) & mask;
    }
    w
}

fn hash_key(key: ThreadKey) -> u64 {
    mix(((u64::from(key.pid) << 32) | u64::from(key.tid)) ^ mix(key.create))
}

/// Rimescolamento finale di MurmurHash3: bit alti e bassi dipendono da tutto l'ingresso.
fn mix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// Nomi leggibili dei processi di sistema che possono saturare un core (spec §4.2). Il confronto
/// è sul nome intero dell'immagine, senza distinzione di maiuscole.
const FRIENDLY: &[(&str, &str)] = &[
    ("vmmemWSL", "WSL"),
    ("vmmem", "VM"),
    ("MsMpEng.exe", "Defender"),
    ("MpDefenderCoreService.exe", "Defender"),
    ("NisSrv.exe", "Defender"),
    ("System", "Sistema"),
    ("TiWorker.exe", "Windows Update"),
    ("TrustedInstaller.exe", "Windows Update"),
    ("MoUsoCoreWorker.exe", "Windows Update"),
    ("WmiPrvSE.exe", "WMI"),
];

/// Nome da mostrare per un'immagine: quello leggibile per i processi di sistema noti
/// ("vmmemWSL" → "WSL", "MsMpEng.exe" → "Defender", "System" → "Sistema"), altrimenti il nome
/// senza ".exe".
pub fn friendly_name(image: &str) -> &str {
    if let Some(&(_, friendly)) = FRIENDLY.iter().find(|(name, _)| image.eq_ignore_ascii_case(name)) {
        return friendly;
    }
    match image.len().checked_sub(4).and_then(|i| Some((image.get(..i)?, image.get(i..)?))) {
        Some((stem, ext)) if !stem.is_empty() && ext.eq_ignore_ascii_case(".exe") => stem,
        _ => image,
    }
}

#[cfg(test)]
impl SaturationDetector {
    /// Thread nella mappa.
    pub(crate) fn tracked(&self) -> usize {
        self.slots.len()
    }

    /// Memoria allocata da mappa, indice e tabella dei processi, in byte.
    pub(crate) fn memory_bytes(&self) -> usize {
        self.slots.capacity() * size_of::<Slot>()
            + self.index.capacity() * size_of::<u32>()
            + self.procs.capacity() * size_of::<(u32, u64)>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un secondo in unità da 100 ns.
    const S: u64 = 10_000_000;

    fn key(pid: u32, tid: u32) -> ThreadKey {
        ThreadKey { pid, tid, create: 0x01DC_0000_0000_0000 + u64::from(tid) * 1_000 }
    }

    /// Simula una macchina: thread vivi con il loro tempo CPU cumulativo.
    struct Sim {
        det: SaturationDetector,
        wall: u64,
        threads: Vec<ThreadRead>,
    }

    impl Sim {
        fn new(keys: &[ThreadKey]) -> Sim {
            let threads = keys.iter().map(|&key| ThreadRead { key, cpu: 12_345_678 }).collect();
            Sim { det: SaturationDetector::new(20), wall: 1_000 * S, threads }
        }

        /// La scansione che fa da base.
        fn base(&mut self) -> SatReport {
            self.det.update(self.wall, &self.threads, false)
        }

        /// Avanza di `dt`; ogni thread in `loads` consuma la frazione indicata (per mille di
        /// core), gli altri restano fermi. Poi una scansione.
        fn step(&mut self, dt: u64, loads: &[(ThreadKey, u64)], full: bool) -> SatReport {
            self.advance(dt, loads);
            self.det.update(self.wall, &self.threads, full)
        }

        fn advance(&mut self, dt: u64, loads: &[(ThreadKey, u64)]) {
            self.wall += dt;
            for &(k, permille) in loads {
                let t = self.threads.iter_mut().find(|t| t.key == k).expect("thread vivo");
                t.cpu += dt * permille / 1_000;
            }
        }

        /// Un thread nato `age` prima della prossima scansione, al `permille` da allora.
        fn spawn(&mut self, key: ThreadKey, age: u64, permille: u64) {
            self.threads.push(ThreadRead { key, cpu: age * permille / 1_000 });
        }

        fn kill(&mut self, key: ThreadKey) {
            self.threads.retain(|t| t.key != key);
        }
    }

    fn state(r: SatReport) -> SatState {
        r.state
    }

    #[test]
    fn single_saturated_thread_needs_three_seconds() {
        let (a, b, c) = (key(100, 4), key(100, 8), key(200, 12));
        let mut sim = Sim::new(&[a, b, c]);
        let r = sim.base();
        assert_eq!(r, SatReport { state: SatState::Normal, top_thread: None, top_process: None });
        // Le prime due finestre coprono solo 2 s: il valore c'è già, lo stato no.
        let r = sim.step(S, &[(a, 1_000)], false);
        assert_eq!(r.state, SatState::Normal);
        assert_eq!(r.top_thread, Some((100, 100)));
        assert_eq!(state(sim.step(S, &[(a, 1_000)], false)), SatState::Normal);
        let r = sim.step(S, &[(a, 1_000)], false);
        assert_eq!(r.state, SatState::Saturated(1));
        assert_eq!(r.top_thread, Some((100, 100)));
        assert_eq!(r.top_process, Some((100, 100)));
        assert_eq!(state(sim.step(S, &[(a, 1_000)], false)), SatState::Saturated(1));
    }

    #[test]
    fn windows_of_one_two_and_five_seconds() {
        // Finestre da 1 s: servono 3 finestre; da 2 s: 2 (4 s); da 5 s: 1.
        for (dt, expected) in [(S, 3), (2 * S, 2), (5 * S, 1)] {
            let a = key(100, 4);
            let mut sim = Sim::new(&[a, key(300, 8)]);
            sim.base();
            let mut hit = None;
            for n in 1..=6 {
                if sim.step(dt, &[(a, 1_000)], false).state == SatState::Saturated(1) {
                    hit = Some(n);
                    break;
                }
            }
            assert_eq!(hit, Some(expected), "finestre da {} s", dt / S);
        }
    }

    #[test]
    fn timer_jitter_does_not_add_a_window() {
        // Tre finestre da 0,96 s coprono 2,88 s: bastano, grazie alla tolleranza di 150 ms.
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        let dt = S * 96 / 100;
        assert_eq!(sim.step(dt, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(dt, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(dt, &[(a, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn migrating_thread_keeps_its_key() {
        // Il thread cambia posizione nell'elenco a ogni scansione (come se cambiasse CPU logica o
        // la scansione cambiasse ordine): la chiave è la stessa e la mappa non cresce.
        let a = key(100, 4);
        let others: Vec<ThreadKey> = (1..=20).map(|i| key(100 + i, 100 + i * 4)).collect();
        let mut all = vec![a];
        all.extend(&others);
        let mut sim = Sim::new(&all);
        sim.base();
        let mut last = SatState::Normal;
        for n in 0..5 {
            sim.threads.rotate_left(7);
            last = sim.step(S, &[(a, 980)], false).state;
            assert_eq!(sim.det.tracked(), 21);
            if n < 2 {
                assert_eq!(last, SatState::Normal);
            }
        }
        assert_eq!(last, SatState::Saturated(1));
    }

    #[test]
    fn two_saturated_threads() {
        let (a, b, c) = (key(100, 4), key(200, 8), key(200, 12));
        let mut sim = Sim::new(&[a, b, c]);
        sim.base();
        let loads = [(a, 1_000), (b, 990), (c, 400)];
        sim.step(S, &loads, false);
        sim.step(S, &loads, false);
        let r = sim.step(S, &loads, false);
        assert_eq!(r.state, SatState::Saturated(2));
        assert_eq!(r.top_thread, Some((100, 100)));
        // Il processo 200 somma i suoi due thread: 0,99 + 0,40 core.
        assert_eq!(r.top_process, Some((200, 139)));
    }

    #[test]
    fn full_cpu_takes_precedence() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a, key(100, 8)]);
        sim.base();
        sim.step(S, &[(a, 1_000)], false);
        sim.step(S, &[(a, 1_000)], false);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
        // CPU piena: lo stato è Full e i thread non si valutano; il più carico si dà lo stesso
        // (0,8 + 1 + 1) / 3 = 0,93 core.
        let r = sim.step(S, &[(a, 800)], true);
        assert_eq!(r.state, SatState::Full);
        assert_eq!(r.top_thread, Some((100, 93)));
        for _ in 0..2 {
            assert_eq!(sim.step(S, &[(a, 800)], true).state, SatState::Full);
        }
        // Fuori da "piena" il thread riparte da non saturo: a 0,80 core (fra uscita e ingresso)
        // non rientra, mentre con la valutazione continua l'isteresi lo avrebbe tenuto saturo.
        for _ in 0..3 {
            assert_eq!(sim.step(S, &[(a, 800)], false).state, SatState::Normal);
        }
        // A 0,90 core rientra quando la media arriva a 0,85: (0,9 + 0,9 + 0,8) / 3 = 0,867.
        assert_eq!(sim.step(S, &[(a, 900)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 900)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn full_cpu_with_all_threads_busy_is_never_saturated() {
        // CPU piena: 20 thread a 0,94–0,959 core. Senza la precedenza di "piena" sarebbero 20
        // core saturi.
        let keys: Vec<ThreadKey> = (1..=20).map(|i| key(500, i * 4)).collect();
        let loads: Vec<(ThreadKey, u64)> = keys.iter().zip(940..).map(|(&k, p)| (k, p)).collect();
        let mut sim = Sim::new(&keys);
        sim.base();
        for _ in 0..6 {
            let r = sim.step(S, &loads, true);
            assert_eq!(r.state, SatState::Full);
            // Σ 0,940..=0,959 = 18,99 core.
            assert_eq!(r.top_process, Some((500, 1_899)));
            assert_eq!(r.top_thread, Some((500, 96)));
        }
    }

    #[test]
    fn new_thread_does_not_trigger_immediately() {
        let a = key(100, 4);
        let n = key(100, 8);
        let mut sim = Sim::new(&[a]);
        sim.base();
        sim.step(S, &[], false);
        sim.step(S, &[], false);
        // Nato 0,5 s prima della scansione, al 100% da allora: CPU/età varrebbe 1,0, ma prima
        // della nascita conta zero, quindi u = 0,5 sulla finestra da 1 s.
        sim.spawn(n, S / 2, 1_000);
        let r = sim.step(S, &[], false);
        assert_eq!(r.state, SatState::Normal);
        assert_eq!(r.top_thread, Some((100, 17))); // 0,5 s di CPU su 3 s
        // Poi sempre al 100%: s = (1 + 0,5 + 0) / 3, poi (1 + 1 + 0,5) / 3 = 0,83 < 0,85,
        // infine 1,0.
        assert_eq!(sim.step(S, &[(n, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(n, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(n, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn thread_born_just_before_the_scan_with_long_windows() {
        // Finestre da 5 s: un thread nato 1 s prima della scansione e sempre al 100% vale 0,2 core
        // nella prima finestra, non 1,0.
        let n = key(100, 8);
        let mut sim = Sim::new(&[key(100, 4)]);
        sim.base();
        sim.spawn(n, S, 1_000);
        let r = sim.step(5 * S, &[], false);
        assert_eq!(r.state, SatState::Normal);
        assert_eq!(r.top_thread, Some((100, 20)));
        assert_eq!(sim.step(5 * S, &[(n, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn exit_hysteresis() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        for _ in 0..3 {
            sim.step(S, &[(a, 1_000)], false);
        }
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
        // A 0,75 core resta saturo: s scende a 0,917, 0,833, 0,75, tutti ≥ 0,70.
        for _ in 0..3 {
            assert_eq!(sim.step(S, &[(a, 750)], false).state, SatState::Saturated(1));
        }
        // s = (0,75 + 0,75 + 0,60) / 3 = 0,70: non è sotto 0,70, resta saturo.
        assert_eq!(sim.step(S, &[(a, 600)], false).state, SatState::Saturated(1));
        // s = (0,75 + 0,60 + 0,60) / 3 = 0,65: esce.
        assert_eq!(sim.step(S, &[(a, 600)], false).state, SatState::Normal);
        // Senza essere già saturo, 0,80 non basta per entrare.
        for _ in 0..5 {
            assert_eq!(sim.step(S, &[(a, 800)], false).state, SatState::Normal);
        }
    }

    #[test]
    fn time_weighted_mean() {
        // 2 s a 1,0 core poi 1 s a 0,6: la media pesata è 0,867 (entra); quella semplice delle
        // due finestre sarebbe 0,8.
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        assert_eq!(sim.step(2 * S, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 600)], false).state, SatState::Saturated(1));
        // 1 s a 1,0 core poi 2 s a 0,7: pesata 0,8 (non entra); semplice 0,85.
        let mut sim = Sim::new(&[a]);
        sim.base();
        sim.step(S, &[(a, 1_000)], false);
        assert_eq!(sim.step(2 * S, &[(a, 700)], false).state, SatState::Normal);
    }

    #[test]
    fn vanished_thread_leaves_the_map() {
        let (a, b, c) = (key(100, 4), key(200, 8), key(300, 12));
        let mut sim = Sim::new(&[a, b, c]);
        sim.base();
        let loads = [(a, 1_000), (b, 1_000)];
        for _ in 0..2 {
            sim.step(S, &loads, false);
        }
        assert_eq!(sim.step(S, &loads, false).state, SatState::Saturated(2));
        assert_eq!(sim.det.tracked(), 3);
        sim.kill(a);
        let r = sim.step(S, &[(b, 1_000)], false);
        assert_eq!(r.state, SatState::Saturated(1));
        assert_eq!(r.top_thread.map(|t| t.0), Some(200));
        assert_eq!(sim.det.tracked(), 2);
        sim.kill(b);
        assert_eq!(sim.step(S, &[], false).state, SatState::Normal);
        assert_eq!(sim.det.tracked(), 1);
    }

    #[test]
    fn pid_zero_is_excluded() {
        let idle = key(0, 0);
        let a = key(100, 4);
        let mut sim = Sim::new(&[idle, a]);
        sim.base();
        for _ in 0..4 {
            let r = sim.step(S, &[(idle, 1_000), (a, 100)], false);
            assert_eq!(r.state, SatState::Normal);
            assert_eq!(r.top_thread.map(|t| t.0), Some(100));
            assert_eq!(r.top_process.map(|p| p.0), Some(100));
        }
        assert_eq!(sim.det.tracked(), 1);
    }

    #[test]
    fn reused_tid_is_a_new_thread() {
        // Stesso PID e TID, CreateTime diverso: è un altro thread. Il vecchio aveva un'ora di
        // CPU; il nuovo, nato nella finestra, ne ha 0,2 s.
        let old = ThreadKey { pid: 100, tid: 8, create: 1 };
        let new = ThreadKey { pid: 100, tid: 8, create: 2 };
        let mut sim = Sim::new(&[old]);
        sim.threads[0].cpu = 3_600 * S;
        sim.base();
        sim.step(S, &[(old, 500)], false);
        sim.kill(old);
        sim.spawn(new, S / 5, 1_000);
        let r = sim.step(S, &[], false);
        assert_eq!(r.state, SatState::Normal);
        assert_eq!(sim.det.tracked(), 1);
        // Resta solo il thread nuovo: 0,2 s di CPU su 2 s di finestre.
        assert_eq!(r.top_thread, Some((100, 10)));
    }

    #[test]
    fn counter_going_backwards_is_discarded() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.threads[0].cpu = 100 * S;
        sim.base();
        sim.step(S, &[(a, 1_000)], false);
        sim.threads[0].cpu = 10 * S; // contatore tornato indietro
        assert_eq!(sim.step(S, &[], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn impossible_rate_is_discarded() {
        // Più di 1,25 core per un solo thread non è una lettura vera.
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        for _ in 0..5 {
            assert_eq!(sim.step(S, &[(a, 2_000)], false).state, SatState::Normal);
        }
    }

    #[test]
    fn short_window_is_ignored() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        let first = sim.step(S, &[(a, 1_000)], false);
        // 0,2 s dopo: nessuna nuova finestra, stesso esito (con "piena" che ha la precedenza).
        let r = sim.step(S / 5, &[(a, 1_000)], false);
        assert_eq!(r, first);
        assert_eq!(sim.step(0, &[], true).state, SatState::Full);
        // La finestra successiva dura 1,2 s: con quella da 1 s fanno 2,2 s, non ancora 3.
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn long_gap_starts_a_new_base() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        for _ in 0..3 {
            sim.step(S, &[(a, 1_000)], false);
        }
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
        // 20 s senza scansioni (sospensione): nuova base, niente stato né valori.
        let r = sim.step(20 * S, &[(a, 1_000)], false);
        assert_eq!(r, SatReport { state: SatState::Normal, top_thread: None, top_process: None });
        sim.step(S, &[(a, 1_000)], false);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Normal);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
        // Orologio all'indietro: nuova base.
        let r = sim.det.update(sim.wall - S, &sim.threads, false);
        assert_eq!(r.top_thread, None);
    }

    #[test]
    fn reset_starts_a_new_base() {
        let a = key(100, 4);
        let mut sim = Sim::new(&[a]);
        sim.base();
        for _ in 0..3 {
            sim.step(S, &[(a, 1_000)], false);
        }
        sim.det.reset();
        assert_eq!(sim.det.tracked(), 0);
        let r = sim.step(S, &[(a, 1_000)], false);
        assert_eq!(r, SatReport { state: SatState::Normal, top_thread: None, top_process: None });
        assert_eq!(sim.det.tracked(), 1);
        sim.step(S, &[(a, 1_000)], false);
        sim.step(S, &[(a, 1_000)], false);
        assert_eq!(sim.step(S, &[(a, 1_000)], false).state, SatState::Saturated(1));
    }

    #[test]
    fn top_process_sums_threads_in_any_order() {
        // Tre thread del processo 100 a 0,3 core, sparsi fra quelli del 200 (0,5 core).
        let (a1, a2, a3) = (key(100, 4), key(100, 8), key(100, 12));
        let b = key(200, 16);
        let mut sim = Sim::new(&[a1, b, a2, key(300, 20), a3]);
        sim.base();
        let r = sim.step(S, &[(a1, 300), (a2, 300), (a3, 300), (b, 500)], false);
        assert_eq!(r.top_process, Some((100, 90)));
        assert_eq!(r.top_thread, Some((200, 50)));
    }

    #[test]
    fn saturated_count_is_capped() {
        let keys: Vec<ThreadKey> = (1..=300).map(|i| key(100 + i / 10, i * 4)).collect();
        let loads: Vec<(ThreadKey, u64)> = keys.iter().map(|&k| (k, 1_000)).collect();
        let mut sim = Sim::new(&keys);
        sim.base();
        for _ in 0..2 {
            sim.step(S, &loads, false);
        }
        assert_eq!(sim.step(S, &loads, false).state, SatState::Saturated(255));
    }

    #[test]
    fn overflow_never_mistakes_old_threads_for_new() {
        // Oltre il tetto i thread in più non si seguono; quando entrano non si sa quanta CPU
        // abbiano consumato prima, quindi partono da zero invece di sembrare nati ora.
        let keys: Vec<ThreadKey> = (0..MAX_THREADS as u32 + 5).map(|i| key(1 + i / 16, 4 + i * 4)).collect();
        let mut sim = Sim::new(&keys);
        let extra: Vec<ThreadKey> = keys[MAX_THREADS..].to_vec();
        for t in &mut sim.threads[MAX_THREADS..] {
            t.cpu = 5_000 * S;
        }
        sim.base();
        assert_eq!(sim.det.tracked(), MAX_THREADS);
        assert_eq!(sim.step(S, &[], false).state, SatState::Normal);
        for k in &keys[..10] {
            sim.kill(*k);
        }
        let r = sim.step(S, &[], false);
        assert_eq!(r.state, SatState::Normal);
        let r = sim.step(S, &[], false);
        assert_eq!(r.state, SatState::Normal);
        assert_eq!(r.top_thread.map(|t| t.1), Some(0));
        assert_eq!(sim.det.tracked(), MAX_THREADS - 5);
        // Da qui i thread entrati si seguono normalmente.
        let loads = [(extra[0], 1_000)];
        sim.step(S, &loads, false);
        sim.step(S, &loads, false);
        assert_eq!(sim.step(S, &loads, false).state, SatState::Saturated(1));
    }

    #[test]
    fn memory_is_reused_after_warm_up() {
        // 2.000 thread in 100 processi; a ogni scansione ne finiscono 20 e ne nascono 20.
        let mut next_tid = 4u32;
        let mut keys = Vec::new();
        for _ in 0..2_000 {
            keys.push(key(1 + next_tid % 100, next_tid));
            next_tid += 4;
        }
        let mut sim = Sim::new(&keys);
        sim.base();
        let mut churn = |sim: &mut Sim| {
            for _ in 0..20 {
                let old = sim.threads[0].key;
                sim.kill(old);
                sim.spawn(key(1 + next_tid % 100, next_tid), S / 2, 100);
                next_tid += 4;
            }
        };
        for _ in 0..3 {
            churn(&mut sim);
            sim.step(S, &[], false);
        }
        let warm = sim.det.memory_bytes();
        for _ in 0..50 {
            churn(&mut sim);
            sim.step(S, &[], false);
            assert_eq!(sim.det.tracked(), 2_000);
            assert_eq!(sim.det.memory_bytes(), warm);
        }
    }

    #[test]
    fn gate_is_point_eight_core_rounded_up() {
        let gate = |n| SaturationDetector::new(n).gate_percent();
        assert_eq!(gate(20), 4);
        assert_eq!(gate(16), 5);
        assert_eq!(gate(12), 7);
        assert_eq!(gate(8), 10);
        assert_eq!(gate(3), 27);
        assert_eq!(gate(1), 80);
        assert_eq!(gate(0), 80);
        assert_eq!(gate(64), 2);
        assert_eq!(gate(80), 1);
        assert_eq!(gate(1_024), 1);
    }

    #[test]
    fn friendly_names() {
        assert_eq!(friendly_name("vmmemWSL"), "WSL");
        assert_eq!(friendly_name("vmmem"), "VM");
        assert_eq!(friendly_name("MsMpEng.exe"), "Defender");
        assert_eq!(friendly_name("msmpeng.EXE"), "Defender");
        assert_eq!(friendly_name("System"), "Sistema");
        assert_eq!(friendly_name("TiWorker.exe"), "Windows Update");
        assert_eq!(friendly_name("pwsh.exe"), "pwsh");
        assert_eq!(friendly_name("Code.EXE"), "Code");
        assert_eq!(friendly_name("Registry"), "Registry");
        assert_eq!(friendly_name("system.exe"), "system");
        assert_eq!(friendly_name("città.exe"), "città");
        assert_eq!(friendly_name(".exe"), ".exe");
        assert_eq!(friendly_name("é"), "é");
        assert_eq!(friendly_name(""), "");
    }

    #[test]
    fn slot_is_compact() {
        assert_eq!(size_of::<Slot>(), 36);
    }
}
