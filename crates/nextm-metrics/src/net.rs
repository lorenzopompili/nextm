//! Rete: logica pura (M2).
//!
//! - Quali interfacce leggere nelle tre modalità (spec §5.4) e con quale etichetta.
//! - Le schede da offrire nel sotto-menu Rete (§4.5).
//! - Velocità dai contatori cumulativi, con le regole della nuova base (§5.4, "Algoritmo").
//! - Testi della velocità: 3 cifre significative per tooltip e pannello (§4.2, §5.4) e forma
//!   compatta di al massimo 3 caratteri per l'icona (§4.1), senza allocazioni né `core::fmt`.
//!
//! I dati arrivano da `sys::net::NetSampler`; qui non c'è nessuna chiamata di sistema.

// Tipi di interfaccia (IANA ifType) e mezzo fisico NDIS usati dalle regole. Sono definiti qui
// perché questo modulo è logica pura e si compila anche senza windows-sys.
const IF_TYPE_PPP: u32 = 23;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_PROP_VIRTUAL: u32 = 53;
const IF_TYPE_TUNNEL: u32 = 131;
/// `NdisPhysicalMediumBluetooth`: il tethering Bluetooth (PAN).
const MEDIUM_BLUETOOTH: u32 = 10;

/// Voci al massimo nel sotto-menu delle schede.
const MENU_MAX: usize = 10;
/// Lunghezza massima di un nome nell'etichetta del tooltip (§4.2), puntini compresi.
const LABEL_NAME_MAX: usize = 15;

/// Modalità di conteggio della rete (spec §5.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetMode {
    /// Tutte le schede fisiche Up (default).
    Sum,
    /// La scheda fisica Up con la route predefinita di metrica minore (la "principale").
    Auto,
    /// Una sola interfaccia, identificata dal suo NET_LUID (stabile fra i riavvii).
    Specific(u64),
}

/// Un'interfaccia dell'elenco di Windows (`GetIfTable2Ex`), con i soli dati che servono alle regole.
///
/// **Ordine dell'elenco.** `NetSampler::interfaces` mette prima le interfacce con una route
/// predefinita, dalla metrica effettiva minore (metrica della route più quella dell'interfaccia;
/// IPv4 prima di IPv6; a parità, prima l'Ethernet), poi tutte le altre nell'ordine di Windows.
/// La modalità Auto di [`select`] prende la prima scheda fisica Up con la route predefinita: un
/// elenco costruito a mano (per esempio nei test) deve rispettare lo stesso ordine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfInfo {
    /// NET_LUID: identità stabile fra i riavvii (l'indice dell'interfaccia non lo è).
    pub luid: u64,
    /// IfType IANA: 6 Ethernet, 71 Wi-Fi, 24 loopback, 131 tunnel, 53 virtuale, 23 PPP, ...
    pub if_type: u32,
    /// `NDIS_PHYSICAL_MEDIUM`: 14 802.3, 9 Wi-Fi, 10 Bluetooth, 0 non specificato, ...
    pub medium: u32,
    /// OperStatus Up.
    pub up: bool,
    /// OperStatus NotPresent: il dispositivo è scollegato e la riga è un fantasma.
    pub not_present: bool,
    /// HardwareInterface, dichiarato dal driver (vedi [`decode_flags`]): la scheda è fisica.
    pub hardware: bool,
    /// FilterInterface: una riga di un filtro NDIS, con gli stessi contatori della scheda sotto.
    pub filter: bool,
    /// EndPointInterface: un dispositivo "endpoint" collegato al PC.
    pub endpoint: bool,
    /// Ha almeno una route 0.0.0.0/0 o ::/0. Windows la conserva anche sulla Wi-Fi disconnessa:
    /// da sola non dice che la scheda è in uso.
    pub default_route: bool,
    /// Nome della connessione ("Ethernet 7"), rinominabile dall'utente.
    pub alias: String,
    /// Descrizione del driver ("Realtek Gaming USB 2.5GbE Family Controller").
    pub description: String,
}

/// Decodifica il byte `InterfaceAndOperStatusFlags` di `MIB_IF_ROW2`:
/// restituisce (HardwareInterface, FilterInterface, EndPointInterface).
///
/// Bit 0 HardwareInterface, bit 1 FilterInterface, bit 7 EndPointInterface; gli altri
/// (ConnectorPresent, NotAuthenticated, NotMediaConnected, Paused, LowPower) non servono alle regole.
pub fn decode_flags(bits: u8) -> (bool, bool, bool) {
    (bits & 0x01 != 0, bits & 0x02 != 0, bits & 0x80 != 0)
}

/// Tipi esclusi dalla somma anche se il driver li dichiara hardware: loopback, tunnel,
/// virtuale proprietario (WireGuard, OpenVPN, TAP) e PPP.
fn excluded_type(if_type: u32) -> bool {
    matches!(if_type, IF_TYPE_SOFTWARE_LOOPBACK | IF_TYPE_TUNNEL | IF_TYPE_PROP_VIRTUAL | IF_TYPE_PPP)
}

/// Regola della modalità somma (§5.4): `(Hardware && !Filter && !EndPoint) || (!Filter &&
/// medium Bluetooth && Up)`, esclusi i tipi 24, 131, 53 e 23 e le righe NotPresent.
///
/// Vale anche per una scheda fisica Down (per esempio la Wi-Fi disconnessa): è candidata e
/// conta appena torna Up. [`select`] legge solo quelle Up. Il Bluetooth PAN (virtuale per il
/// driver) conta solo da Up: così il tethering Bluetooth non risulta a 0. I filtri NDIS
/// replicano i contatori della scheda sottostante e non contano mai: sommarli moltiplicherebbe
/// il traffico (6 volte sulla macchina di sviluppo).
pub fn counts_in_sum(i: &IfInfo) -> bool {
    if i.filter || i.not_present || excluded_type(i.if_type) {
        return false;
    }
    (i.hardware && !i.endpoint) || (i.medium == MEDIUM_BLUETOOTH && i.up)
}

/// Le interfacce da leggere a ogni tick e l'etichetta per il tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// NET_LUID delle interfacce contate, nell'ordine dell'elenco (la principale per prima).
    pub luids: Vec<u64>,
    /// Nomi delle interfacce contate: "Ethernet 7", "Ethernet 7 + Wi-Fi", "Ethernet 7 + 2" (con
    /// più di due schede), "WireGuard". Ogni nome è troncato a 15 caratteri con "…". Vuota se
    /// non c'è nessuna interfaccia da leggere. Il prefisso della modalità ("somma: ",
    /// "principale: ", "solo ") è compito dell'app, che conosce la lingua.
    pub label: String,
    /// La scelta richiesta non è disponibile e si conta la somma delle schede fisiche; il
    /// tooltip lo deve dire. In modalità Specific la scheda scelta non è presente (NotPresent o
    /// assente dall'elenco); in modalità Auto nessuna scheda fisica Up ha una route predefinita.
    pub fallback: bool,
}

/// Sceglie le interfacce da leggere secondo la modalità (spec §5.4).
///
/// - `Sum`: tutte le interfacce Up per cui vale [`counts_in_sum`].
/// - `Auto`: la prima scheda dell'elenco Up, con la route predefinita e per cui vale
///   [`counts_in_sum`]; l'elenco è ordinato per metrica (vedi [`IfInfo`]), quindi è quella di
///   metrica minore. Con una VPN full-tunnel la route migliore è sulla VPN, che non è fisica:
///   vince la scheda fisica sottostante. Se nessuna scheda fisica ha la route, somma con
///   `fallback`.
/// - `Specific`: l'interfaccia scelta se è presente, anche Down (conta 0) o virtuale; se è
///   NotPresent o manca dall'elenco, somma con `fallback`. La scelta non si cancella: quando
///   la scheda torna, la stessa chiamata la riprende.
pub fn select(list: &[IfInfo], mode: NetMode) -> Selection {
    let one = match mode {
        NetMode::Sum => return sum_selection(list, false),
        NetMode::Auto => list.iter().find(|i| i.up && i.default_route && counts_in_sum(i)),
        NetMode::Specific(luid) => list.iter().find(|i| i.luid == luid && !i.not_present),
    };
    match one {
        Some(i) => {
            let mut label = String::new();
            push_short_name(&mut label, &i.alias);
            Selection { luids: vec![i.luid], label, fallback: false }
        }
        None => sum_selection(list, true),
    }
}

fn sum_selection(list: &[IfInfo], fallback: bool) -> Selection {
    let counted = || list.iter().filter(|i| i.up && counts_in_sum(i));
    let luids: Vec<u64> = counted().map(|i| i.luid).collect();
    let mut label = String::new();
    let mut names = counted();
    if let Some(first) = names.next() {
        push_short_name(&mut label, &first.alias);
    }
    match luids.len() {
        0 | 1 => {}
        2 => {
            if let Some(second) = names.next() {
                label.push_str(" + ");
                push_short_name(&mut label, &second.alias);
            }
        }
        n => {
            label.push_str(" + ");
            push_decimal(&mut label, (n - 1) as u64);
        }
    }
    Selection { luids, label, fallback }
}

/// Aggiunge `name`, troncato a `LABEL_NAME_MAX` caratteri con "…" se è più lungo.
fn push_short_name(out: &mut String, name: &str) {
    if name.chars().count() <= LABEL_NAME_MAX {
        out.push_str(name);
    } else {
        out.extend(name.chars().take(LABEL_NAME_MAX - 1));
        out.push('\u{2026}');
    }
}

fn push_decimal(out: &mut String, v: u64) {
    let mut digits = [0u8; 20];
    for &d in decimal_digits(v, &mut digits) {
        out.push(char::from(d));
    }
}

/// Le cifre decimali di `v` (ASCII), scritte in fondo a `buf`.
fn decimal_digits(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
    // u64::MAX ha 20 cifre: `start` non scende mai sotto zero.
    let mut start = buf.len();
    loop {
        start -= 1;
        buf[start] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    &buf[start..]
}

/// Voci del sotto-menu "Rete" per la modalità scheda specifica (§4.5, §5.4): (NET_LUID, alias).
///
/// - Prima le schede hardware, anche Down (non filtri, non endpoint, non NotPresent, non dei
///   tipi esclusi dalla somma); poi le interfacce virtuali o VPN Up (vEthernet, WireGuard, ...).
///   Dentro ogni gruppo resta l'ordine dell'elenco, quindi la principale viene per prima.
/// - Esclusi filtri, loopback, righe NotPresent, WAN Miniport e pseudo-interfacce: i tunnel di
///   sistema (tipo 131: Teredo, 6to4, IP-HTTPS, WAN Miniport SSTP/IKEv2/L2TP/PPTP), le porte
///   dello switch virtuale di Hyper-V ("vSwitch (...)", senza traffico IP proprio) e l'adattatore
///   del debugger del kernel. Le descrizioni dei driver restano in inglese anche su Windows in
///   italiano (verificato sulla macchina di sviluppo).
/// - Al massimo 10 voci. `chosen` compare sempre, anche se esclusa dalle regole o oltre la
///   decima voce (prende il posto dell'ultima). Se manca del tutto dall'elenco compare con il
///   nome vuoto: l'app la mostra con un testo proprio (per esempio "scheda non presente").
pub fn menu_candidates(list: &[IfInfo], chosen: Option<u64>) -> Vec<(u64, String)> {
    let is_chosen = |i: &IfInfo| chosen == Some(i.luid);
    let hardware = list.iter().filter(|i| menu_hardware(i));
    let virtual_up = list.iter().filter(|i| !menu_hardware(i) && menu_virtual(i));
    let other_chosen = list.iter().filter(|i| is_chosen(i) && !menu_hardware(i) && !menu_virtual(i));
    let mut out: Vec<(u64, String)> =
        hardware.chain(virtual_up).chain(other_chosen).map(|i| (i.luid, i.alias.clone())).collect();
    if let Some(c) = chosen {
        match out.iter().position(|(luid, _)| *luid == c) {
            Some(p) if p >= MENU_MAX => {
                let entry = out.swap_remove(p);
                out.truncate(MENU_MAX - 1);
                out.push(entry);
            }
            Some(_) => {}
            None => {
                out.truncate(MENU_MAX - 1);
                out.push((c, String::new()));
            }
        }
    }
    out.truncate(MENU_MAX);
    out
}

fn menu_hardware(i: &IfInfo) -> bool {
    i.hardware && !i.filter && !i.endpoint && !i.not_present && !excluded_type(i.if_type)
}

fn menu_virtual(i: &IfInfo) -> bool {
    i.up && !i.filter
        && !i.not_present
        && i.if_type != IF_TYPE_SOFTWARE_LOOPBACK
        && i.if_type != IF_TYPE_TUNNEL
        && !is_pseudo_interface(&i.description)
}

/// Interfacce di servizio che non sono una connessione da osservare.
fn is_pseudo_interface(description: &str) -> bool {
    let d = description.as_bytes();
    starts_with_ignore_case(d, b"WAN Miniport")
        || starts_with_ignore_case(d, b"Microsoft Kernel Debug")
        || contains_ignore_case(d, b"Virtual Switch Extension Adapter")
}

fn starts_with_ignore_case(text: &[u8], prefix: &[u8]) -> bool {
    text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn contains_ignore_case(text: &[u8], needle: &[u8]) -> bool {
    text.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// Velocità in byte al secondo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    /// Ricevuti (download).
    pub down: u64,
    /// Inviati (upload).
    pub up: u64,
}

/// Un secondo in unità da 100 ns.
const SECOND_100NS: u64 = 10_000_000;

/// Velocità dai contatori cumulativi di una o più interfacce.
///
/// Delta per interfaccia, poi somma: un'interfaccia che compare o sparisce non produce picchi
/// finti. Il campione si scarta e si riparte da una nuova base (`update` restituisce `None`):
/// - al primo campione e dopo [`RateMeter::reset`];
/// - se l'insieme delle interfacce cambia (un luid nuovo o sparito);
/// - se un contatore cala: è un reset, non un giro (i contatori sono a 64 bit);
/// - se l'intervallo è zero o l'orologio torna indietro;
/// - se l'intervallo supera 3 volte il precedente (sospensione non notificata, processo
///   congelato dal Modern Standby). Come riferimento vale almeno 1 s, il periodo base più breve:
///   così il primo tick anticipato dell'app (300 ms dopo l'avvio) non fa scartare il seguente,
///   e dopo un cambio di periodo (1 s → 5 s) si perde al massimo un campione.
///
/// La media degli ultimi 2 campioni (§5.4) è compito del chiamante: va azzerata quando `update`
/// restituisce `None`. Nessuna allocazione per campione finché il numero di interfacce non
/// cresce: il buffer della base si riusa.
#[derive(Clone, Debug)]
pub struct RateMeter {
    /// Contatori della base (luid, ricevuti, inviati).
    base: Vec<(u64, u64, u64)>,
    /// Istante della base in unità da 100 ns; `None` finché non c'è una base.
    base_wall: Option<u64>,
    /// Durata dell'intervallo precedente, anche se scartato: segue i cambi di periodo.
    last_interval: Option<u64>,
}

impl RateMeter {
    pub fn new() -> RateMeter {
        RateMeter { base: Vec::with_capacity(4), base_wall: None, last_interval: None }
    }

    /// Dimentica base e intervallo (ripresa, schermo acceso, cambio di selezione).
    pub fn reset(&mut self) {
        self.base.clear();
        self.base_wall = None;
        self.last_interval = None;
    }

    /// Aggiunge un campione: `counters` sono i contatori cumulativi (luid, byte ricevuti, byte
    /// inviati) di ogni interfaccia letta, `wall_100ns` un orologio monotono in unità da 100 ns
    /// (`QueryPerformanceCounter`). Restituisce la velocità dall'ultimo campione, oppure `None`
    /// quando il campione fa da nuova base. Il campione diventa comunque la base seguente.
    pub fn update(&mut self, counters: &[(u64, u64, u64)], wall_100ns: u64) -> Option<Rates> {
        let rates = self.rates_since_base(counters, wall_100ns);
        self.base.clear();
        self.base.extend_from_slice(counters);
        self.base_wall = Some(wall_100ns);
        rates
    }

    fn rates_since_base(&mut self, counters: &[(u64, u64, u64)], wall: u64) -> Option<Rates> {
        let dt = wall.checked_sub(self.base_wall?)?;
        let reference = self.last_interval.replace(dt);
        if dt == 0 || reference.is_some_and(|r| dt > r.max(SECOND_100NS).saturating_mul(3)) {
            return None;
        }
        if counters.len() != self.base.len() {
            return None;
        }
        let (mut down, mut up) = (0u128, 0u128);
        for &(luid, rx, tx) in counters {
            let &(_, rx0, tx0) = self.base.iter().find(|b| b.0 == luid)?;
            down += u128::from(rx.checked_sub(rx0)?);
            up += u128::from(tx.checked_sub(tx0)?);
        }
        // Anche nell'altro verso: un luid ripetuto non deve nasconderne uno sparito.
        if self.base.iter().any(|b| !counters.iter().any(|c| c.0 == b.0)) {
            return None;
        }
        Some(Rates { down: per_second(down, dt), up: per_second(up, dt) })
    }
}

impl Default for RateMeter {
    fn default() -> RateMeter {
        RateMeter::new()
    }
}

/// Byte in `dt_100ns` → byte al secondo, arrotondati.
fn per_second(bytes: u128, dt_100ns: u64) -> u64 {
    let dt = u128::from(dt_100ns.max(1));
    u64::try_from((bytes * u128::from(SECOND_100NS) + dt / 2) / dt).unwrap_or(u64::MAX)
}

/// Unità della velocità mostrata: byte/s (default) o bit/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Bytes,
    Bits,
}

const RATE_TEXT_CAP: usize = 16;

/// Testo breve ASCII in un buffer fisso: nessuna allocazione.
#[derive(Clone, Copy)]
pub struct RateText {
    buf: [u8; RATE_TEXT_CAP],
    len: u8,
}

impl RateText {
    const fn new() -> RateText {
        RateText { buf: [0; RATE_TEXT_CAP], len: 0 }
    }

    /// Il testo, per esempio "9,87 MB/s" o ".1M".
    pub fn as_str(&self) -> &str {
        let bytes = self.buf.get(..usize::from(self.len)).unwrap_or_default();
        core::str::from_utf8(bytes).unwrap_or_default()
    }

    /// Aggiunge un byte ASCII; oltre la capacità non scrive (non succede con i testi di questo modulo).
    fn push(&mut self, b: u8) {
        if let Some(slot) = self.buf.get_mut(usize::from(self.len)) {
            *slot = b;
            self.len += 1;
        }
    }

    fn push_bytes(&mut self, s: &[u8]) {
        for &b in s {
            self.push(b);
        }
    }

    fn push_u64(&mut self, v: u64) {
        let mut digits = [0u8; 20];
        self.push_bytes(decimal_digits(v, &mut digits));
    }
}

impl PartialEq for RateText {
    fn eq(&self, other: &RateText) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for RateText {}

impl core::fmt::Debug for RateText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// Unità decimali, dalla più piccola: si arriva fino a exa, così anche `u64::MAX` resta a 3 cifre.
const BYTE_UNITS: [&[u8]; 6] = [b"kB/s", b"MB/s", b"GB/s", b"TB/s", b"PB/s", b"EB/s"];
const BIT_UNITS: [&[u8]; 6] = [b"kbps", b"Mbps", b"Gbps", b"Tbps", b"Pbps", b"Ebps"];

/// La velocità nell'unità scelta: byte/s oppure bit/s (8 bit per byte, senza overflow).
fn in_units(bytes_per_s: u64, units: Units) -> u128 {
    match units {
        Units::Bytes => u128::from(bytes_per_s),
        Units::Bits => u128::from(bytes_per_s) * 8,
    }
}

/// Testo per tooltip e pannello: 3 cifre significative in base 1000 ("9,87 MB/s", "98,7 kB/s",
/// "987 kB/s"; in bit "kbps", "Mbps", "Gbps").
///
/// - Si passa all'unità successiva da 999,5 in su: 999,5 kB/s si scrive "1,00 MB/s".
/// - Arrotondamento a metà in su: 9,995 diventa "10,0" e 99,95 diventa "100".
/// - L'unità più piccola è kB/s (kbps): sotto 1 kB/s si scrivono due decimali ("0,50 kB/s");
///   si scrive "0 kB/s" solo quando il valore arrotondato è zero (meno di 5 B/s).
/// - `comma` sceglie il separatore decimale: virgola in italiano, punto in inglese (§4.7).
/// - Il campo numerico è lungo al massimo 4 caratteri, così l'app può allinearlo a destra.
pub fn format_rate(bytes_per_s: u64, units: Units, comma: bool) -> RateText {
    let names = match units {
        Units::Bytes => &BYTE_UNITS,
        Units::Bits => &BIT_UNITS,
    };
    let x = in_units(bytes_per_s, units);
    // Scala dell'unità (1000 per kB/s). Si sale finché il valore, arrotondato a 3 cifre,
    // arriverebbe a 1000: x / scala ≥ 999,5, cioè 2·x ≥ 1999·scala.
    let mut unit = 0;
    let mut scale: u128 = 1_000;
    while unit + 1 < names.len() && x * 2 >= 1_999 * scale {
        unit += 1;
        scale *= 1_000;
    }
    // Decimali: 2 sotto 9,995, 1 sotto 99,95, altrimenti nessuno.
    let (decimals, pow) = if x * 200 < 1_999 * scale {
        (2, 100)
    } else if x * 20 < 1_999 * scale {
        (1, 10)
    } else {
        (0, 1)
    };
    // Valore in centesimi, decimi o unità, arrotondato a metà in su.
    let n = u64::try_from((x * pow * 2 + scale) / (scale * 2)).unwrap_or(u64::MAX);
    let mut t = RateText::new();
    if n == 0 {
        t.push(b'0');
    } else {
        let pow = pow as u64;
        t.push_u64(n / pow);
        if decimals > 0 {
            t.push(if comma { b',' } else { b'.' });
            let frac = n % pow;
            if decimals == 2 && frac < 10 {
                t.push(b'0');
            }
            t.push_u64(frac);
        }
    }
    t.push(b' ');
    t.push_bytes(names.get(unit).copied().unwrap_or_default());
    t
}

/// Testo per l'icona della rete, al massimo 3 caratteri (§4.1): 1–2 cifre più l'unità K, M o G.
///
/// - Sotto 1 kB/s: "0". Poi "1K"–"99K".
/// - Da 100 a 999 kB/s: ".1M"–".9M"; da 1 MB/s "1M"–"99M"; da 100 a 999 MB/s ".1G"–".9G";
///   da 1 GB/s "1G"–"99G".
/// - Si tronca (non si arrotonda): il numero è sempre un valore minimo garantito. Oltre 99 G il
///   font dell'icona non ha altre unità e il testo resta "99G": il valore esatto è nel tooltip.
/// - In bit/s le stesse regole, con le lettere minuscole k, m e g.
/// - Il punto è sempre ".", anche in italiano, come nella spec.
pub fn compact_rate(bytes_per_s: u64, units: Units) -> RateText {
    let letters: &[u8; 3] = match units {
        Units::Bytes => b"KMG",
        Units::Bits => b"kmg",
    };
    let x = in_units(bytes_per_s, units);
    let mut t = RateText::new();
    if x < 1_000 {
        t.push(b'0');
        return t;
    }
    let mut scale: u128 = 1_000;
    for (k, &letter) in letters.iter().enumerate() {
        let whole = x / scale;
        if whole < 100 {
            t.push_u64(whole as u64);
            t.push(letter);
            return t;
        }
        if let Some(&next) = letters.get(k + 1) {
            // Decimi dell'unità successiva: 100 kB/s = 0,1 MB/s.
            let tenths = x / (scale * 100);
            if tenths < 10 {
                t.push(b'.');
                t.push_u64(tenths as u64);
                t.push(next);
                return t;
            }
        }
        scale *= 1_000;
    }
    t.push_bytes(b"99");
    t.push(letters[2]);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- elenchi sintetici ----

    const ETH7: u64 = 0x0006_0000_0100_0000;
    const WIFI: u64 = 0x0047_0000_0200_0000;
    const ETH5: u64 = 0x0006_0000_0300_0000;
    const WG: u64 = 0x0035_0000_0400_0000;
    const VETH: u64 = 0x0006_0000_0500_0000;
    const VETH_WSL: u64 = 0x0006_0000_0600_0000;
    const VSWITCH: u64 = 0x0006_0000_0700_0000;
    const WAN_IP: u64 = 0x0006_0000_0800_0000;
    const WAN_SSTP: u64 = 0x0083_0000_0900_0000;
    const LOOPBACK: u64 = 0x0018_0000_0A00_0000;
    const TEREDO: u64 = 0x0083_0000_0B00_0000;
    const BT_PAN: u64 = 0x0006_0000_0C00_0000;
    const KDNET: u64 = 0x0006_0000_0D00_0000;
    const ETH7_WFP: u64 = 0x0006_0000_0E00_0000;
    const ETH7_QOS: u64 = 0x0006_0000_0F00_0000;
    const WIFI_NATIVE: u64 = 0x0006_0000_1000_0000;

    fn iface(luid: u64, alias: &str, if_type: u32, medium: u32, flags: u8, up: bool) -> IfInfo {
        let (hardware, filter, endpoint) = decode_flags(flags);
        IfInfo {
            luid,
            if_type,
            medium,
            up,
            not_present: false,
            hardware,
            filter,
            endpoint,
            default_route: false,
            alias: alias.to_string(),
            description: String::new(),
        }
    }

    fn described(mut i: IfInfo, description: &str) -> IfInfo {
        i.description = description.to_string();
        i
    }

    fn with_route(mut i: IfInfo) -> IfInfo {
        i.default_route = true;
        i
    }

    fn missing(mut i: IfInfo) -> IfInfo {
        i.up = false;
        i.not_present = true;
        i
    }

    /// Come la macchina di sviluppo, nell'ordine di `NetSampler::interfaces`: prima le route
    /// predefinite per metrica (Ethernet 7: 0 + 20, Wi-Fi disconnessa: 0 + 55), poi le altre.
    fn dev_machine() -> Vec<IfInfo> {
        vec![
            with_route(iface(ETH7, "Ethernet 7", 6, 14, 0x05, true)),
            with_route(iface(WIFI, "Wi-Fi", 71, 9, 0x15, false)),
            // Filtri NDIS sopra le schede: stessi contatori della miniport.
            iface(ETH7_WFP, "Ethernet 7-WFP Native MAC Layer LightWeight Filter-0000", 6, 14, 0x02, true),
            iface(ETH7_QOS, "Ethernet 7-QoS Packet Scheduler-0000", 6, 14, 0x03, true),
            iface(WIFI_NATIVE, "Wi-Fi-Native WiFi Filter Driver-0000", 71, 9, 0x12, false),
            missing(iface(ETH5, "Ethernet 5", 6, 14, 0x05, false)),
            iface(WG, "k8splayzone", 53, 0, 0x00, true),
            described(iface(VETH, "vEthernet (Default Switch)", 6, 0, 0x00, true), "Hyper-V Virtual Ethernet Adapter"),
            described(
                iface(VETH_WSL, "vEthernet (WSL (Hyper-V firewall))", 6, 0, 0x00, true),
                "Hyper-V Virtual Ethernet Adapter #2",
            ),
            described(
                iface(VSWITCH, "vSwitch (Default Switch)", 6, 0, 0x00, true),
                "Hyper-V Virtual Switch Extension Adapter",
            ),
            described(iface(WAN_IP, "Connessione alla rete locale (LAN)* 6", 6, 0, 0x00, true), "WAN Miniport (IP)"),
            described(
                iface(WAN_SSTP, "Connessione alla rete locale (LAN)* 1", 131, 0, 0x00, false),
                "WAN Miniport (SSTP)",
            ),
            iface(LOOPBACK, "Loopback Pseudo-Interface 1", 24, 0, 0x00, true),
            missing(iface(TEREDO, "Teredo Tunneling Pseudo-Interface", 131, 0, 0x00, false)),
            described(
                iface(BT_PAN, "Connessione di rete Bluetooth", 6, 10, 0x00, false),
                "Bluetooth Device (Personal Area Network)",
            ),
            described(
                missing(iface(KDNET, "Ethernet (debugger del kernel)", 6, 14, 0x00, false)),
                "Microsoft Kernel Debug Network Adapter",
            ),
        ]
    }

    fn find(list: &mut [IfInfo], luid: u64) -> &mut IfInfo {
        list.iter_mut().find(|i| i.luid == luid).unwrap()
    }

    // ---- flag e regola della somma ----

    #[test]
    fn decodes_flags_seen_on_the_dev_machine() {
        assert_eq!(decode_flags(0x05), (true, false, false), "Ethernet");
        assert_eq!(decode_flags(0x15), (true, false, false), "Wi-Fi disconnessa");
        assert_eq!(decode_flags(0x02), (false, true, false), "filtro");
        assert_eq!(decode_flags(0x12), (false, true, false), "filtro su scheda disconnessa");
        assert_eq!(decode_flags(0x00), (false, false, false), "WireGuard");
        assert_eq!(decode_flags(0x80), (false, false, true), "endpoint");
        assert_eq!(decode_flags(0xFF), (true, true, true));
        // Gli altri bit (ConnectorPresent, NotMediaConnected, Paused, LowPower...) non contano.
        assert_eq!(decode_flags(0x7C), (false, false, false));
    }

    #[test]
    fn sum_rule_on_the_dev_machine() {
        let list = dev_machine();
        let counted: Vec<u64> = list.iter().filter(|i| counts_in_sum(i)).map(|i| i.luid).collect();
        // La Wi-Fi Down è candidata (conterà quando torna Up); nessun filtro, VPN, vEthernet,
        // vSwitch, WAN Miniport, loopback, tunnel, NotPresent o Bluetooth Down.
        assert_eq!(counted, [ETH7, WIFI]);
    }

    #[test]
    fn filters_never_count_even_when_flagged_hardware() {
        let f = iface(1, "filtro", 6, 14, 0x03, true);
        assert!(!counts_in_sum(&f));
        let f = iface(1, "filtro Bluetooth", 6, 10, 0x02, true);
        assert!(!counts_in_sum(&f));
    }

    #[test]
    fn bluetooth_pan_counts_only_when_up() {
        let mut bt = iface(1, "Connessione di rete Bluetooth", 6, 10, 0x00, false);
        assert!(!counts_in_sum(&bt));
        bt.up = true;
        assert!(counts_in_sum(&bt));
    }

    #[test]
    fn excluded_types_do_not_count_even_if_hardware() {
        for t in [24, 131, 53, 23] {
            assert!(!counts_in_sum(&iface(1, "x", t, 14, 0x01, true)), "tipo {t}");
        }
        assert!(counts_in_sum(&iface(1, "x", 6, 14, 0x01, true)));
        assert!(counts_in_sum(&iface(1, "x", 71, 9, 0x01, true)));
        // Modem WWAN (MBIM dichiara NCF_PHYSICAL) e tethering USB (RNDIS/NCM): hardware.
        assert!(counts_in_sum(&iface(1, "Cellulare", 243, 8, 0x01, true)));
    }

    #[test]
    fn endpoints_and_not_present_do_not_count() {
        assert!(!counts_in_sum(&iface(1, "x", 6, 14, 0x81, true)));
        assert!(!counts_in_sum(&missing(iface(1, "x", 6, 14, 0x05, true))));
    }

    // ---- select ----

    #[test]
    fn sum_counts_only_up_physical_adapters_not_their_filters() {
        let sel = select(&dev_machine(), NetMode::Sum);
        assert_eq!(sel, Selection { luids: vec![ETH7], label: "Ethernet 7".to_string(), fallback: false });
    }

    #[test]
    fn sum_label_names_two_adapters_and_counts_the_rest() {
        let mut list = dev_machine();
        find(&mut list, WIFI).up = true;
        let sel = select(&list, NetMode::Sum);
        assert_eq!(sel.luids, [ETH7, WIFI]);
        assert_eq!(sel.label, "Ethernet 7 + Wi-Fi");
        find(&mut list, BT_PAN).up = true;
        let sel = select(&list, NetMode::Sum);
        assert_eq!(sel.luids, [ETH7, WIFI, BT_PAN]);
        assert_eq!(sel.label, "Ethernet 7 + 2");
        assert!(!sel.fallback);
    }

    #[test]
    fn sum_with_nothing_up_is_empty() {
        let mut list = dev_machine();
        find(&mut list, ETH7).up = false;
        let sel = select(&list, NetMode::Sum);
        assert_eq!(sel, Selection { luids: vec![], label: String::new(), fallback: false });
    }

    #[test]
    fn long_names_are_truncated_in_the_label() {
        let sel = select(&dev_machine(), NetMode::Specific(VETH_WSL));
        assert_eq!(sel.label, "vEthernet (WSL…");
        assert_eq!(sel.label.chars().count(), 15);
        // Esattamente 15 caratteri: nessun troncamento.
        let list = vec![iface(1, "ABCDEFGHIJKLMNO", 6, 14, 0x05, true)];
        assert_eq!(select(&list, NetMode::Sum).label, "ABCDEFGHIJKLMNO");
    }

    #[test]
    fn auto_takes_the_physical_adapter_with_the_best_default_route() {
        let sel = select(&dev_machine(), NetMode::Auto);
        assert_eq!(sel, Selection { luids: vec![ETH7], label: "Ethernet 7".to_string(), fallback: false });
    }

    #[test]
    fn auto_ignores_adapters_that_are_down_even_with_a_route() {
        // Cavo staccato: resta la Wi-Fi, se è connessa; la sua route c'era già.
        let mut list = dev_machine();
        find(&mut list, ETH7).up = false;
        find(&mut list, WIFI).up = true;
        assert_eq!(select(&list, NetMode::Auto).luids, [WIFI]);
    }

    #[test]
    fn auto_with_full_tunnel_vpn_takes_the_physical_adapter_underneath() {
        // WireGuard full-tunnel: la sua 0.0.0.0/0 ha la metrica migliore (0 + 5) ed è in cima.
        let mut list = dev_machine();
        let pos = list.iter().position(|i| i.luid == WG).unwrap();
        let mut wg = list.remove(pos);
        wg.default_route = true;
        list.insert(0, wg);
        let sel = select(&list, NetMode::Auto);
        assert_eq!(sel.luids, [ETH7]);
        assert_eq!(sel.label, "Ethernet 7");
        assert!(!sel.fallback);
    }

    #[test]
    fn auto_prefers_the_first_adapter_in_metric_order() {
        // Ethernet e Wi-Fi connesse insieme: vince la metrica minore, cioè la prima dell'elenco.
        let mut list = dev_machine();
        find(&mut list, WIFI).up = true;
        assert_eq!(select(&list, NetMode::Auto).luids, [ETH7]);
        list.swap(0, 1);
        assert_eq!(select(&list, NetMode::Auto).luids, [WIFI]);
    }

    #[test]
    fn auto_without_physical_default_route_falls_back_to_the_sum() {
        // OpenVPN 2.x senza def1: la route fisica sparisce, resta solo quella del tunnel.
        let mut list = dev_machine();
        for i in &mut list {
            i.default_route = i.luid == WG;
        }
        let sel = select(&list, NetMode::Auto);
        assert_eq!(sel, Selection { luids: vec![ETH7], label: "Ethernet 7".to_string(), fallback: true });
    }

    #[test]
    fn auto_accepts_bluetooth_tethering_as_the_main_adapter() {
        let mut list = dev_machine();
        find(&mut list, ETH7).up = false;
        let bt = find(&mut list, BT_PAN);
        bt.up = true;
        bt.default_route = true;
        assert_eq!(select(&list, NetMode::Auto).luids, [BT_PAN]);
    }

    #[test]
    fn specific_reads_only_the_chosen_interface() {
        let sel = select(&dev_machine(), NetMode::Specific(WG));
        assert_eq!(sel, Selection { luids: vec![WG], label: "k8splayzone".to_string(), fallback: false });
        // Anche Down: la scelta resta, e conta 0.
        let sel = select(&dev_machine(), NetMode::Specific(WIFI));
        assert_eq!(sel, Selection { luids: vec![WIFI], label: "Wi-Fi".to_string(), fallback: false });
    }

    #[test]
    fn specific_not_present_falls_back_to_the_sum() {
        let sel = select(&dev_machine(), NetMode::Specific(ETH5));
        assert_eq!(sel, Selection { luids: vec![ETH7], label: "Ethernet 7".to_string(), fallback: true });
    }

    #[test]
    fn specific_gone_from_the_list_falls_back_and_comes_back() {
        // Scheda USB scollegata: il LUID sparisce dall'elenco (GetIfEntry2 dà ERROR_FILE_NOT_FOUND).
        let mut list = dev_machine();
        list.retain(|i| i.luid != WIFI);
        let sel = select(&list, NetMode::Specific(WIFI));
        assert_eq!(sel.luids, [ETH7]);
        assert!(sel.fallback);
        // Quando torna, la stessa scelta la riprende.
        let sel = select(&dev_machine(), NetMode::Specific(WIFI));
        assert_eq!(sel.luids, [WIFI]);
        assert!(!sel.fallback);
    }

    // ---- menu ----

    fn luids(menu: &[(u64, String)]) -> Vec<u64> {
        menu.iter().map(|(l, _)| *l).collect()
    }

    #[test]
    fn menu_lists_hardware_first_then_up_virtual_interfaces() {
        let menu = menu_candidates(&dev_machine(), None);
        assert_eq!(luids(&menu), [ETH7, WIFI, WG, VETH, VETH_WSL]);
        assert_eq!(menu[0].1, "Ethernet 7");
        // Niente filtri, loopback, NotPresent, WAN Miniport, vSwitch, tunnel, debugger del kernel.
    }

    #[test]
    fn menu_lists_up_bluetooth_and_vpn_but_not_down_ones() {
        let mut list = dev_machine();
        find(&mut list, BT_PAN).up = true;
        find(&mut list, WG).up = false;
        assert_eq!(luids(&menu_candidates(&list, None)), [ETH7, WIFI, VETH, VETH_WSL, BT_PAN]);
    }

    #[test]
    fn menu_always_shows_the_chosen_interface() {
        // NotPresent: esclusa dalle regole, ma è la scelta.
        let menu = menu_candidates(&dev_machine(), Some(ETH5));
        assert_eq!(luids(&menu), [ETH7, WIFI, WG, VETH, VETH_WSL, ETH5]);
        assert_eq!(menu[5].1, "Ethernet 5");
        // Assente dall'elenco: nome vuoto, l'app scrive "scheda non presente".
        let menu = menu_candidates(&dev_machine(), Some(0xDEAD));
        assert_eq!(menu.last(), Some(&(0xDEAD, String::new())));
        // Già fra le voci: nessun duplicato.
        let menu = menu_candidates(&dev_machine(), Some(WG));
        assert_eq!(luids(&menu), [ETH7, WIFI, WG, VETH, VETH_WSL]);
    }

    #[test]
    fn menu_has_at_most_ten_entries_and_keeps_the_choice() {
        let mut list = dev_machine();
        for k in 0..12u64 {
            list.push(iface(0x100 + k, "vEthernet (rete interna)", 6, 0, 0x00, true));
        }
        let menu = menu_candidates(&list, None);
        assert_eq!(menu.len(), 10);
        assert_eq!(luids(&menu)[..2], [ETH7, WIFI]);
        // La scelta oltre la decima voce prende il posto dell'ultima.
        let menu = menu_candidates(&list, Some(0x100 + 11));
        assert_eq!(menu.len(), 10);
        assert_eq!(menu.last().map(|m| m.0), Some(0x100 + 11));
        let menu = menu_candidates(&list, Some(0xDEAD));
        assert_eq!(menu.len(), 10);
        assert_eq!(menu.last().map(|m| m.0), Some(0xDEAD));
    }

    #[test]
    fn pseudo_interfaces_are_recognized_ignoring_case() {
        assert!(is_pseudo_interface("WAN Miniport (Network Monitor)"));
        assert!(is_pseudo_interface("wan miniport (IPv6)"));
        assert!(is_pseudo_interface("Hyper-V Virtual Switch Extension Adapter #2"));
        assert!(is_pseudo_interface("Microsoft Kernel Debug Network Adapter"));
        assert!(!is_pseudo_interface("Hyper-V Virtual Ethernet Adapter"));
        assert!(!is_pseudo_interface("WireGuard Tunnel"));
        assert!(!is_pseudo_interface(""));
    }

    // ---- RateMeter ----

    const S: u64 = SECOND_100NS;

    #[test]
    fn first_sample_is_a_base_then_rates_per_second() {
        let mut m = RateMeter::new();
        assert_eq!(m.update(&[(ETH7, 1_000, 500)], 5 * S), None);
        assert_eq!(m.update(&[(ETH7, 3_000, 1_500)], 6 * S), Some(Rates { down: 2_000, up: 1_000 }));
        // Mezzo secondo: la velocità raddoppia.
        assert_eq!(m.update(&[(ETH7, 4_000, 1_500)], 6 * S + S / 2), Some(Rates { down: 2_000, up: 0 }));
    }

    #[test]
    fn deltas_are_summed_over_adapters_in_any_order() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 100, 10), (WIFI, 1_000, 100)], 0);
        let r = m.update(&[(WIFI, 1_600, 150), (ETH7, 500, 30)], S);
        assert_eq!(r, Some(Rates { down: 1_000, up: 70 }));
    }

    #[test]
    fn reset_starts_from_a_new_base() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        m.reset();
        assert_eq!(m.update(&[(ETH7, 5_000, 5_000)], S), None);
        assert_eq!(m.update(&[(ETH7, 6_000, 5_500)], 2 * S), Some(Rates { down: 1_000, up: 500 }));
    }

    #[test]
    fn a_counter_going_down_is_a_reset_not_a_wrap() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 10_000, 10_000), (WIFI, 50, 50)], 0);
        // La Wi-Fi si riconnette e azzera i contatori: niente picco da 2^64.
        assert_eq!(m.update(&[(ETH7, 11_000, 10_500), (WIFI, 0, 60)], S), None);
        assert_eq!(m.update(&[(ETH7, 12_000, 11_000), (WIFI, 100, 60)], 2 * S), Some(Rates { down: 1_100, up: 500 }));
        // Basta una sola direzione.
        assert_eq!(m.update(&[(ETH7, 13_000, 10_000), (WIFI, 100, 60)], 3 * S), None);
    }

    #[test]
    fn a_long_interval_starts_from_a_new_base() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        assert!(m.update(&[(ETH7, 1_000, 0)], S).is_some());
        // Sospensione non notificata: 3 volte il periodo non basta, oltre sì.
        assert!(m.update(&[(ETH7, 2_000, 0)], 4 * S).is_some());
        assert_eq!(m.update(&[(ETH7, 3_000, 0)], 14 * S), None);
        // L'intervallo seguente si confronta con quello lungo: nessun secondo scarto.
        assert_eq!(m.update(&[(ETH7, 4_000, 0)], 15 * S), Some(Rates { down: 1_000, up: 0 }));
    }

    #[test]
    fn a_period_change_loses_at_most_one_sample() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        m.update(&[(ETH7, 1_000, 0)], S);
        // Da 1 s a 5 s: il primo intervallo lungo si scarta, il secondo no.
        assert_eq!(m.update(&[(ETH7, 6_000, 0)], 6 * S), None);
        assert_eq!(m.update(&[(ETH7, 11_000, 0)], 11 * S), Some(Rates { down: 1_000, up: 0 }));
    }

    #[test]
    fn the_early_first_tick_does_not_discard_the_next_sample() {
        // L'app legge la base all'avvio, il primo tick dopo 300 ms, poi ogni secondo.
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        assert!(m.update(&[(ETH7, 300, 0)], 3 * S / 10).is_some());
        assert_eq!(m.update(&[(ETH7, 1_300, 0)], 13 * S / 10), Some(Rates { down: 1_000, up: 0 }));
    }

    #[test]
    fn zero_or_backwards_intervals_give_no_sample() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 10 * S);
        assert_eq!(m.update(&[(ETH7, 100, 0)], 10 * S), None);
        assert_eq!(m.update(&[(ETH7, 200, 0)], 9 * S), None);
        assert_eq!(m.update(&[(ETH7, 300, 0)], 10 * S), Some(Rates { down: 100, up: 0 }));
    }

    #[test]
    fn a_new_or_vanished_adapter_starts_from_a_new_base() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        assert_eq!(m.update(&[(ETH7, 100, 0), (WIFI, 1 << 40, 0)], S), None, "nuova");
        assert!(m.update(&[(ETH7, 200, 0), (WIFI, 1 << 40, 0)], 2 * S).is_some());
        assert_eq!(m.update(&[(ETH7, 300, 0)], 3 * S), None, "sparita");
        assert_eq!(m.update(&[(WIFI, 1 << 40, 0)], 4 * S), None, "sostituita");
        assert_eq!(m.update(&[(ETH7, 400, 0), (WIFI, 1 << 40, 0)], 5 * S), None, "nuova");
        // Stessa lunghezza, ma un luid ripetuto al posto di uno sparito.
        assert_eq!(m.update(&[(WIFI, 1 << 40, 0), (WIFI, 1 << 40, 0)], 6 * S), None, "ripetuta");
    }

    #[test]
    fn no_interfaces_means_zero_traffic() {
        let mut m = RateMeter::new();
        assert_eq!(m.update(&[], 0), None);
        assert_eq!(m.update(&[], S), Some(Rates { down: 0, up: 0 }));
    }

    #[test]
    fn rates_are_rounded_and_never_overflow() {
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0)], 0);
        assert_eq!(m.update(&[(ETH7, 1, 2)], 3 * S), Some(Rates { down: 0, up: 1 }));
        let mut m = RateMeter::new();
        m.update(&[(ETH7, 0, 0), (WIFI, 0, 0)], 0);
        let r = m.update(&[(ETH7, u64::MAX, u64::MAX), (WIFI, u64::MAX, 0)], 1).unwrap();
        assert_eq!(r, Rates { down: u64::MAX, up: u64::MAX });
    }

    // ---- testi ----

    fn rate(bytes: u64) -> String {
        format_rate(bytes, Units::Bytes, true).as_str().to_string()
    }

    fn compact(bytes: u64) -> String {
        compact_rate(bytes, Units::Bytes).as_str().to_string()
    }

    #[test]
    fn three_significant_digits() {
        assert_eq!(rate(9_870_000), "9,87 MB/s");
        assert_eq!(rate(98_700), "98,7 kB/s");
        assert_eq!(rate(987_000), "987 kB/s");
        assert_eq!(rate(2_100_000), "2,10 MB/s");
        assert_eq!(rate(1_234_567_890), "1,23 GB/s");
    }

    #[test]
    fn zero_and_values_below_one_kilobyte() {
        assert_eq!(rate(0), "0 kB/s");
        assert_eq!(rate(4), "0 kB/s");
        assert_eq!(rate(5), "0,01 kB/s");
        assert_eq!(rate(500), "0,50 kB/s");
        assert_eq!(rate(994), "0,99 kB/s");
        assert_eq!(rate(995), "1,00 kB/s");
        assert_eq!(rate(999), "1,00 kB/s");
        assert_eq!(rate(1_000), "1,00 kB/s");
    }

    #[test]
    fn rounding_edges_inside_a_unit() {
        assert_eq!(rate(9_994), "9,99 kB/s");
        assert_eq!(rate(9_995), "10,0 kB/s");
        assert_eq!(rate(99_949), "99,9 kB/s");
        assert_eq!(rate(99_950), "100 kB/s");
        assert_eq!(rate(999_000), "999 kB/s");
        assert_eq!(rate(999_499), "999 kB/s");
    }

    #[test]
    fn unit_changes_from_999_5() {
        assert_eq!(rate(999_500), "1,00 MB/s");
        assert_eq!(rate(1_000_000), "1,00 MB/s");
        assert_eq!(rate(999_499_999), "999 MB/s");
        assert_eq!(rate(999_500_000), "1,00 GB/s");
        assert_eq!(rate(9_995_000_000), "10,0 GB/s");
        assert_eq!(rate(999_500_000_000), "1,00 TB/s");
    }

    #[test]
    fn decimal_point_or_comma() {
        assert_eq!(format_rate(9_870_000, Units::Bytes, false).as_str(), "9.87 MB/s");
        assert_eq!(format_rate(98_700, Units::Bytes, false).as_str(), "98.7 kB/s");
        assert_eq!(format_rate(987_000, Units::Bytes, false).as_str(), "987 kB/s");
        assert_eq!(format_rate(5, Units::Bytes, false).as_str(), "0.01 kB/s");
    }

    #[test]
    fn bits_per_second() {
        let bits = |bytes| format_rate(bytes, Units::Bits, true).as_str().to_string();
        assert_eq!(bits(0), "0 kbps");
        assert_eq!(bits(125), "1,00 kbps");
        assert_eq!(bits(62), "0,50 kbps");
        assert_eq!(bits(1_250_000), "10,0 Mbps");
        // 124 937 500 B/s sono esattamente 999,5 Mbps: si passa ai Gbps.
        assert_eq!(bits(124_937_499), "999 Mbps");
        assert_eq!(bits(124_937_500), "1,00 Gbps");
        assert_eq!(bits(125_000_000), "1,00 Gbps");
        assert_eq!(format_rate(12_345_678, Units::Bits, false).as_str(), "98.8 Mbps");
    }

    #[test]
    fn huge_values_stay_three_digits() {
        assert_eq!(rate(u64::MAX), "18,4 EB/s");
        assert_eq!(format_rate(u64::MAX, Units::Bits, true).as_str(), "148 Ebps");
    }

    #[test]
    fn numeric_field_is_at_most_four_characters() {
        let mut v: u64 = 1;
        while v < u64::MAX / 3 {
            for units in [Units::Bytes, Units::Bits] {
                for x in [v, v + v / 2, v * 2 - 1] {
                    let t = format_rate(x, units, true);
                    let number = t.as_str().split(' ').next().unwrap();
                    assert!(number.len() <= 4, "{x} -> {}", t.as_str());
                    assert!(t.as_str().len() <= 9, "{x} -> {}", t.as_str());
                }
            }
            v = v * 3 + 1;
        }
    }

    #[test]
    fn compact_below_one_kilobyte_is_zero() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "0");
        assert_eq!(compact(1_000), "1K");
    }

    #[test]
    fn compact_truncates_and_switches_to_tenths() {
        assert_eq!(compact(1_999), "1K");
        assert_eq!(compact(9_999), "9K");
        assert_eq!(compact(12_345), "12K");
        assert_eq!(compact(99_999), "99K");
        assert_eq!(compact(100_000), ".1M");
        assert_eq!(compact(199_999), ".1M");
        assert_eq!(compact(999_999), ".9M");
        assert_eq!(compact(1_000_000), "1M");
        assert_eq!(compact(2_100_000), "2M");
        assert_eq!(compact(12_345_678), "12M");
        assert_eq!(compact(99_999_999), "99M");
        assert_eq!(compact(100_000_000), ".1G");
        assert_eq!(compact(999_999_999), ".9G");
        assert_eq!(compact(1_000_000_000), "1G");
        assert_eq!(compact(99_999_999_999), "99G");
    }

    #[test]
    fn compact_saturates_beyond_the_icon_font() {
        assert_eq!(compact(100_000_000_000), "99G");
        assert_eq!(compact(u64::MAX), "99G");
        assert_eq!(compact_rate(u64::MAX, Units::Bits).as_str(), "99g");
    }

    #[test]
    fn compact_bits_use_lowercase_units() {
        let bits = |bytes| compact_rate(bytes, Units::Bits).as_str().to_string();
        assert_eq!(bits(124), "0");
        assert_eq!(bits(125), "1k");
        assert_eq!(bits(12_500), ".1m");
        assert_eq!(bits(1_250_000), "10m");
        assert_eq!(bits(12_500_000), ".1g");
        assert_eq!(bits(125_000_000), "1g");
        assert_eq!(bits(12_375_000_000), "99g");
    }

    #[test]
    fn compact_is_never_longer_than_three_characters() {
        let mut v: u64 = 1;
        while v < u64::MAX / 3 {
            for units in [Units::Bytes, Units::Bits] {
                for x in [v, v + v / 2, v * 2 - 1] {
                    let t = compact_rate(x, units);
                    assert!((1..=3).contains(&t.as_str().len()), "{x} -> {}", t.as_str());
                }
            }
            v = v * 3 + 1;
        }
    }

    #[test]
    fn rate_text_compares_by_content() {
        assert_eq!(compact(1_500), compact(1_999));
        assert_eq!(compact_rate(1_500, Units::Bytes), compact_rate(1_999, Units::Bytes));
        assert_ne!(compact_rate(1_500, Units::Bytes), compact_rate(2_000, Units::Bytes));
        assert_eq!(format!("{:?}", compact_rate(100_000, Units::Bytes)), "\".1M\"");
    }
}
