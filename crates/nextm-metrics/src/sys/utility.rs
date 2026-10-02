//! CPU come Task Manager (Utilità) via PerfLib V2 con advapi32 caricata su richiesta (M2).
//!
//! Fonte: counterset "Processor Information" `{b4fc721a-0378-476f-89ba-a5a79f810b36}`, istanza
//! `_Total`, che copre tutte le CPU logiche di tutti i gruppi. Metadati letti con
//! `PerfQueryCounterSetRegistrationInfo` sulla macchina di sviluppo (Windows 11 25H2):
//! - contatore 26, "% Processor Utility": `PERF_AVERAGE_BULK` (0x40020500), 64 bit, base 27;
//! - contatore 27, senza nome: `PERF_AVERAGE_BASE` (0x40030402), **a 32 bit**.
//!
//! Utilità = Δc26 / Δc27, già in percentuale e senza scala. È la formula di PDH, che riporta lo
//! stesso tipo e usa c26 e c27 come valori grezzi: nello stesso istante lo scarto medio è di 0,02
//! punti (massimo 0,05). Supera 100 perché pesa il tempo di lavoro con la frequenza effettiva
//! rispetto a quella nominale.
//!
//! La base è un orologio comune a tutte le istanze e non dipende dal numero di CPU: avanza di
//! 625 000 unità al secondo (1,6 µs). A 32 bit ricomincia da zero ogni 6872 s, circa 1,9 ore: dopo
//! 35 ore di uptime il valore letto era già ripartito 18 volte. Il campionatore la estende a 64 bit.
//!
//! La query restituisce i contatori nell'ordine in cui sono stati aggiunti (verificato con
//! `PerfQueryCounterInfo`). Un'istanza assente non fa fallire `PerfAddCounters`: si scopre alla
//! lettura, con un blocco `PERF_ERROR_RETURN`.
//!
//! Costi misurati sulla macchina di sviluppo (20 CPU logiche, con altri processi attivi):
//! - attivazione: 12–19 ms di CPU. `LoadLibraryExW` costa 1,8 ms e carica anche sechost, msvcrt
//!   e rpcrt4; il primo `PerfAddCounters` costa 10–14 ms perché inizializza PerfLib per tutto il
//!   processo (una seconda query, anche su un altro counterset, costa 0,03 ms finché advapi32
//!   resta caricata);
//! - memoria: +530 KiB di working set privato (circa 165 per le DLL, 240–390 per PerfLib). Dopo
//!   il `Drop` ne restano circa 190 e rpcrt4 resta caricata; una seconda attivazione ne aggiunge
//!   circa 20;
//! - lettura: 0,3–0,9 ms di parete e 0,1–0,3 ms di CPU del thread. Il thread passa da tutti i
//!   core: 16–23 cambi di contesto per lettura, contro gli 8–10 di `GetSystemTimes`.

use core::mem::{offset_of, size_of};
use core::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_COUNTER_DATA, PERF_COUNTER_HEADER, PERF_COUNTER_IDENTIFIER, PERF_DATA_HEADER, PERF_SINGLE_COUNTER,
};
use windows_sys::core::{GUID, PCWSTR};

use crate::sys::dll::{Library, RawProc};

/// Counterset "Processor Information".
const PROCESSOR_INFORMATION: GUID = GUID::from_u128(0xb4fc721a_0378_476f_89ba_a5a79f810b36);
/// "% Processor Utility": il numeratore.
const UTILITY_ID: u32 = 26;
/// La sua base: il denominatore.
const UTILITY_BASE_ID: u32 = 27;
/// L'istanza si sceglie per nome: l'identificatore numerico resta un jolly.
const ANY_INSTANCE_ID: u32 = u32::MAX;
/// "_Total" in UTF-16 con il terminatore, completato a 8 caratteri.
const TOTAL: [u16; 8] = {
    let name = b"_Total";
    let mut out = [0u16; 8];
    let mut i = 0;
    while i < name.len() {
        out[i] = name[i] as u16;
        i += 1;
    }
    out
};

/// Un'istantanea dei due contatori cumulativi dell'Utilità, entrambi monotoni a 64 bit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UtilitySample {
    /// "% Processor Utility" cumulativo: il numeratore.
    pub utility: u64,
    /// La sua base cumulativa, già estesa a 64 bit: il denominatore.
    pub base: u64,
}

/// Utilità fra due istantanee, in centesimi di punto: 100 × Δutility / Δbase, arrotondata.
///
/// Non è limitata a 10000: con il turbo supera il 100%. Restituisce `None` se la base non è
/// avanzata o se un contatore è tornato indietro: il chiamante riparte da una nuova base.
pub fn utility_bp(prev: UtilitySample, cur: UtilitySample) -> Option<u32> {
    let d_utility = cur.utility.checked_sub(prev.utility)?;
    let d_base = cur.base.checked_sub(prev.base)?;
    if d_base == 0 {
        return None;
    }
    let (num, den) = (u128::from(d_utility) * 100, u128::from(d_base));
    Some(u32::try_from((num + den / 2) / den).unwrap_or(u32::MAX))
}

/// Valore grezzo di un contatore, come lo restituisce la query: 4 o 8 byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Raw {
    U32(u32),
    U64(u64),
}

/// Rende monotono a 64 bit un contatore che la query restituisce a 32 bit.
///
/// Si accumula la differenza dal valore precedente, letta come intero con segno: il ritorno a
/// zero diventa un passo avanti, mentre un contatore che torna davvero indietro resta un passo
/// indietro, che `utility_bp` scarta. Vale finché fra due letture il contatore avanza meno di
/// 2^31: per la base dell'Utilità, circa 57 minuti.
#[derive(Clone, Copy, Debug, Default)]
struct Widened {
    last: Option<u32>,
    total: u64,
}

impl Widened {
    fn update(&mut self, raw: Raw) -> u64 {
        match raw {
            Raw::U64(value) => {
                self.last = None;
                self.total = value;
            }
            Raw::U32(value) => {
                self.total = match self.last {
                    Some(last) => self.total.wrapping_add_signed(i64::from(value.wrapping_sub(last) as i32)),
                    None => u64::from(value),
                };
                self.last = Some(value);
            }
        }
        self.total
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

/// Estrae i due contatori, nell'ordine in cui sono stati aggiunti, dal blocco restituito da
/// `PerfQueryCounterData`: un `PERF_DATA_HEADER` seguito da un blocco per contatore.
///
/// `None` se il blocco non è coerente o se un contatore riporta un errore.
fn parse_pair(bytes: &[u8]) -> Option<(Raw, Raw)> {
    let total = read_u32(bytes, offset_of!(PERF_DATA_HEADER, dwTotalSize))? as usize;
    let count = read_u32(bytes, offset_of!(PERF_DATA_HEADER, dwNumCounters))?;
    let bytes = bytes.get(..total)?;
    if count != 2 {
        return None;
    }
    let mut at = size_of::<PERF_DATA_HEADER>();
    let first = single_counter(bytes, &mut at)?;
    let second = single_counter(bytes, &mut at)?;
    Some((first, second))
}

/// Legge il blocco `PERF_SINGLE_COUNTER` che inizia in `*at` e sposta `*at` sul successivo.
///
/// Il blocco è un `PERF_COUNTER_HEADER`, poi un `PERF_COUNTER_DATA`, poi il valore.
fn single_counter(bytes: &[u8], at: &mut usize) -> Option<Raw> {
    let start = *at;
    let status = read_u32(bytes, start + offset_of!(PERF_COUNTER_HEADER, dwStatus))?;
    let kind = read_u32(bytes, start + offset_of!(PERF_COUNTER_HEADER, dwType))?;
    let size = read_u32(bytes, start + offset_of!(PERF_COUNTER_HEADER, dwSize))? as usize;
    let end = start.checked_add(size)?;
    let block = bytes.get(start..end)?;
    // Un contatore senza dati (per esempio istanza assente) arriva come PERF_ERROR_RETURN.
    if status != ERROR_SUCCESS || kind != PERF_SINGLE_COUNTER as u32 {
        return None;
    }
    let data = size_of::<PERF_COUNTER_HEADER>();
    let value = data + size_of::<PERF_COUNTER_DATA>();
    let raw = match read_u32(block, data + offset_of!(PERF_COUNTER_DATA, dwDataSize))? {
        4 => Raw::U32(read_u32(block, value)?),
        8 => Raw::U64(read_u64(block, value)?),
        _ => return None,
    };
    *at = end;
    Some(raw)
}

type PerfOpenQueryHandleFn = unsafe extern "system" fn(machine: PCWSTR, query: *mut HANDLE) -> u32;
type PerfAddCountersFn =
    unsafe extern "system" fn(query: HANDLE, counters: *mut PERF_COUNTER_IDENTIFIER, size: u32) -> u32;
type PerfQueryCounterDataFn =
    unsafe extern "system" fn(query: HANDLE, block: *mut PERF_DATA_HEADER, size: u32, actual: *mut u32) -> u32;
type PerfCloseQueryHandleFn = unsafe extern "system" fn(query: HANDLE) -> u32;

/// Un contatore da aggiungere alla query: l'identificatore seguito dal nome dell'istanza, in un
/// blocco lungo un multiplo di 8 byte, come chiede `PerfAddCounters`.
#[repr(C, align(8))]
struct CounterSpec {
    id: PERF_COUNTER_IDENTIFIER,
    instance: [u16; 8],
}

const _: () = assert!(size_of::<CounterSpec>().is_multiple_of(8));

impl CounterSpec {
    fn total(counter: u32) -> CounterSpec {
        CounterSpec {
            id: PERF_COUNTER_IDENTIFIER {
                CounterSetGuid: PROCESSOR_INFORMATION,
                Status: 0,
                Size: size_of::<CounterSpec>() as u32,
                CounterId: counter,
                InstanceId: ANY_INSTANCE_ID,
                Index: 0,
                Reserved: 0,
            },
            instance: TOTAL,
        }
    }
}

/// Buffer della risposta, allineato come `PERF_DATA_HEADER`. La risposta con due contatori
/// occupa 112 byte (48 di intestazione e 32 per contatore); se un giorno 256 non bastassero, la
/// lettura di prova fallirebbe e `new` restituirebbe `None`.
#[repr(C, align(8))]
struct QueryBuffer([u8; 256]);

/// Legge istantanee `UtilitySample` dalla query PerfLib V2 aperta da `new`.
///
/// Tiene caricata advapi32 (con sechost, msvcrt e rpcrt4) finché vive; la query si chiude nel
/// `Drop`, prima che il campo `_lib` scarichi la DLL.
pub struct UtilitySampler {
    query: HANDLE,
    query_data: PerfQueryCounterDataFn,
    close: PerfCloseQueryHandleFn,
    buf: QueryBuffer,
    utility: Widened,
    base: Widened,
    _lib: Library,
}

impl UtilitySampler {
    /// Carica advapi32 da System32, apre la query sull'istanza `_Total` e fa una lettura di prova.
    /// `None` se la DLL, le funzioni, il counterset o i dati non sono disponibili.
    pub fn new() -> Option<UtilitySampler> {
        let lib = Library::load("advapi32.dll")?;
        let open = lib.proc(b"PerfOpenQueryHandle\0")?;
        let add = lib.proc(b"PerfAddCounters\0")?;
        let query_data = lib.proc(b"PerfQueryCounterData\0")?;
        let close = lib.proc(b"PerfCloseQueryHandle\0")?;
        // SAFETY: funzioni esportate da advapi32 con le firme di perflib.h, le stesse dichiarate da
        // windows-sys; i puntatori a funzione hanno la stessa dimensione qualunque sia la firma.
        let (open, add, query_data, close) = unsafe {
            (
                core::mem::transmute::<RawProc, PerfOpenQueryHandleFn>(open),
                core::mem::transmute::<RawProc, PerfAddCountersFn>(add),
                core::mem::transmute::<RawProc, PerfQueryCounterDataFn>(query_data),
                core::mem::transmute::<RawProc, PerfCloseQueryHandleFn>(close),
            )
        };
        let mut query: HANDLE = null_mut();
        // SAFETY: macchina locale (NULL); `query` è una variabile locale.
        if unsafe { open(null(), &mut query) } != ERROR_SUCCESS || query.is_null() {
            return None;
        }
        // Da qui in poi, se qualcosa fallisce, il Drop chiude la query.
        let mut sampler = UtilitySampler {
            query,
            query_data,
            close,
            buf: QueryBuffer([0; 256]),
            utility: Widened::default(),
            base: Widened::default(),
            _lib: lib,
        };
        let mut specs = [CounterSpec::total(UTILITY_ID), CounterSpec::total(UTILITY_BASE_ID)];
        // SAFETY: `specs` sono blocchi contigui PERF_COUNTER_IDENTIFIER + nome, ciascuno lungo `Size`
        // byte, `size_of_val(&specs)` in tutto; la funzione aggiorna i campi di stato dei blocchi
        // senza uscire dal buffer.
        let added = unsafe { add(sampler.query, specs.as_mut_ptr().cast(), size_of_val(&specs) as u32) };
        // Un contatore rifiutato ha un errore in `Status` anche se la funzione restituisce 0.
        if added != ERROR_SUCCESS || specs.iter().any(|s| s.id.Status != ERROR_SUCCESS) {
            return None;
        }
        sampler.read()?;
        Some(sampler)
    }

    /// Un'istantanea; `None` se la lettura fallisce o se i dati non hanno il formato atteso.
    pub fn read(&mut self) -> Option<UtilitySample> {
        let mut actual = 0u32;
        // SAFETY: query aperta da `new` e non ancora chiusa; il buffer, allineato a 8, ha
        // `size_of_val` byte scrivibili; `actual` è una variabile locale.
        let status = unsafe {
            (self.query_data)(
                self.query,
                self.buf.0.as_mut_ptr().cast::<PERF_DATA_HEADER>(),
                size_of_val(&self.buf.0) as u32,
                &mut actual,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let (utility, base) = parse_pair(self.buf.0.get(..actual as usize)?)?;
        Some(UtilitySample { utility: self.utility.update(utility), base: self.base.update(base) })
    }
}

impl Drop for UtilitySampler {
    fn drop(&mut self) {
        // SAFETY: query aperta da `new` e chiusa una sola volta; advapi32 resta caricata fino a
        // quando `_lib` esce di scena, cioè dopo questa funzione.
        unsafe { (self.close)(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(utility: u64, base: u64) -> UtilitySample {
        UtilitySample { utility, base }
    }

    #[test]
    fn utility_is_the_ratio_in_basis_points() {
        // Δutility/Δbase è già in percentuale: 42,5 -> 4250 centesimi di punto.
        assert_eq!(utility_bp(s(0, 0), s(425, 10)), Some(4_250));
        assert_eq!(utility_bp(s(1_000, 500), s(1_000, 900)), Some(0));
        // Numeri come quelli misurati: 48 462 010 / 625 966 = 77,42%.
        assert_eq!(utility_bp(s(3_257_081_670_366, 1_446_119_387), s(3_257_130_132_376, 1_446_745_353)), Some(7_742));
    }

    #[test]
    fn utility_can_exceed_one_hundred_percent() {
        assert_eq!(utility_bp(s(0, 0), s(150, 1)), Some(15_000));
        assert_eq!(utility_bp(s(10, 10), s(2_510, 20)), Some(25_000));
    }

    #[test]
    fn rounds_half_up() {
        // 100/3 = 33,333...% -> 3333; 200/3 = 66,666...% -> 6667.
        assert_eq!(utility_bp(s(0, 0), s(100, 3)), Some(3_333));
        assert_eq!(utility_bp(s(0, 0), s(200, 3)), Some(6_667));
    }

    #[test]
    fn stopped_base_gives_none() {
        assert_eq!(utility_bp(s(5, 7), s(5, 7)), None);
        assert_eq!(utility_bp(s(5, 7), s(90, 7)), None);
    }

    #[test]
    fn counters_going_backwards_give_none() {
        assert_eq!(utility_bp(s(10, 0), s(5, 100)), None);
        assert_eq!(utility_bp(s(0, 100), s(5, 50)), None);
    }

    #[test]
    fn huge_values_do_not_overflow_and_saturate() {
        // Rapporto 1 = 1%.
        assert_eq!(utility_bp(s(0, 0), s(u64::MAX, u64::MAX)), Some(100));
        assert_eq!(utility_bp(s(0, 0), s(u64::MAX, 1)), Some(u32::MAX));
    }

    #[test]
    fn widened_32_bit_counter_crosses_zero() {
        let mut w = Widened::default();
        assert_eq!(w.update(Raw::U32(u32::MAX - 9)), u64::from(u32::MAX - 9));
        // Ritorno a zero: 10 fino a 2^32, poi 5.
        assert_eq!(w.update(Raw::U32(5)), u64::from(u32::MAX - 9) + 15);
        assert_eq!(w.update(Raw::U32(1_005)), u64::from(u32::MAX - 9) + 1_015);
    }

    #[test]
    fn widened_32_bit_counter_going_backwards_stays_backwards() {
        let mut w = Widened::default();
        let a = w.update(Raw::U32(1_000));
        let b = w.update(Raw::U32(900));
        assert_eq!(b, a - 100);
        assert_eq!(utility_bp(s(0, a), s(10, b)), None);
    }

    #[test]
    fn widened_64_bit_counter_is_passed_through() {
        let mut w = Widened::default();
        assert_eq!(w.update(Raw::U64(7)), 7);
        assert_eq!(w.update(Raw::U64(3)), 3);
        assert_eq!(w.update(Raw::U64(u64::MAX)), u64::MAX);
    }

    /// Scrive un blocco come quelli di `PerfQueryCounterData`, nel layout misurato.
    fn block(counters: &[(u32, u32, Raw)]) -> Vec<u8> {
        let mut out = vec![0u8; size_of::<PERF_DATA_HEADER>()];
        for &(status, kind, raw) in counters {
            let mut b = Vec::new();
            b.extend_from_slice(&status.to_ne_bytes());
            b.extend_from_slice(&kind.to_ne_bytes());
            b.extend_from_slice(&0u32.to_ne_bytes()); // dwSize, scritto sotto
            b.extend_from_slice(&0u32.to_ne_bytes());
            if kind == PERF_SINGLE_COUNTER as u32 {
                let value = match raw {
                    Raw::U32(v) => v.to_ne_bytes().to_vec(),
                    Raw::U64(v) => v.to_ne_bytes().to_vec(),
                };
                b.extend_from_slice(&(value.len() as u32).to_ne_bytes());
                b.extend_from_slice(&16u32.to_ne_bytes());
                b.extend_from_slice(&value);
                b.resize(32, 0);
            }
            let len = b.len() as u32;
            b[8..12].copy_from_slice(&len.to_ne_bytes());
            out.extend_from_slice(&b);
        }
        let total = out.len() as u32;
        out[0..4].copy_from_slice(&total.to_ne_bytes());
        out[4..8].copy_from_slice(&(counters.len() as u32).to_ne_bytes());
        out
    }

    const SINGLE: u32 = PERF_SINGLE_COUNTER as u32;

    #[test]
    fn parses_the_measured_layout() {
        let b = block(&[(0, SINGLE, Raw::U64(3_257_081_685_577)), (0, SINGLE, Raw::U32(1_446_119_652))]);
        assert_eq!(b.len(), 112);
        assert_eq!(parse_pair(&b), Some((Raw::U64(3_257_081_685_577), Raw::U32(1_446_119_652))));
    }

    #[test]
    fn parses_a_real_buffer() {
        // Risposta reale di PerfQueryCounterData (istanza _Total, contatori 26, 27 e 0), ridotta ai
        // primi due: dwTotalSize e dwNumCounters sono corretti a mano, il resto è com'era.
        let hex = "70000000020000006fa6e15c25010000\
                   00a40f7d3850dd018096980000000000\
                   ea07090002001d00110020001700f302\
                   00000000010000002000000000000000\
                   0800000010000000d64ce853f6020000\
                   00000000010000002000000000000000\
                   0400000010000000c46e195600000000";
        let b: Vec<u8> =
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("esadecimale")).collect();
        assert_eq!(parse_pair(&b), Some((Raw::U64(0x02f6_53e8_4cd6), Raw::U32(0x5619_6ec4))));
    }

    #[test]
    fn counter_with_error_gives_none() {
        // Istanza assente: blocco PERF_ERROR_RETURN (tipo 0) con stato 232, lungo 16 byte.
        let b = block(&[(232, 0, Raw::U64(0)), (0, SINGLE, Raw::U32(1))]);
        assert_eq!(parse_pair(&b), None);
        let b = block(&[(0, SINGLE, Raw::U64(1)), (5, SINGLE, Raw::U32(1))]);
        assert_eq!(parse_pair(&b), None);
    }

    #[test]
    fn inconsistent_blocks_give_none() {
        let good = block(&[(0, SINGLE, Raw::U64(1)), (0, SINGLE, Raw::U32(2))]);
        // Troncato.
        assert_eq!(parse_pair(&good[..good.len() - 1]), None);
        assert_eq!(parse_pair(&good[..10]), None);
        assert_eq!(parse_pair(&[]), None);
        // Numero di contatori diverso da 2.
        let one = block(&[(0, SINGLE, Raw::U64(1))]);
        assert_eq!(parse_pair(&one), None);
        // dwTotalSize oltre la fine del buffer.
        let mut long = good.clone();
        long[0..4].copy_from_slice(&1_000u32.to_ne_bytes());
        assert_eq!(parse_pair(&long), None);
        // Dimensione del dato non valida.
        let mut odd = good.clone();
        odd[64..68].copy_from_slice(&3u32.to_ne_bytes());
        assert_eq!(parse_pair(&odd), None);
        // dwSize del primo blocco oltre la fine.
        let mut wide = good;
        wide[56..60].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(parse_pair(&wide), None);
    }

    #[test]
    fn total_instance_name_is_utf16_with_terminator() {
        let expected: Vec<u16> = "_Total".encode_utf16().chain([0, 0]).collect();
        assert_eq!(TOTAL.as_slice(), expected.as_slice());
        assert_eq!(size_of::<CounterSpec>(), 56);
    }

    #[test]
    fn live_two_reads_give_a_plausible_utility() {
        let mut sampler = UtilitySampler::new().expect("counterset Processor Information");
        let a = sampler.read().expect("prima lettura");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let b = sampler.read().expect("seconda lettura");
        let bp = utility_bp(a, b).expect("in 500 ms la base avanza");
        assert!(bp <= 30_000, "utilità {bp} centesimi di punto");
    }

    #[test]
    fn live_open_and_close_repeatedly() {
        for _ in 0..3 {
            let mut sampler = UtilitySampler::new().expect("counterset Processor Information");
            assert!(sampler.read().is_some());
        }
    }
}
