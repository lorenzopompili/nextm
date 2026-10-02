//! Lettura della temperatura della zona termica ACPI tramite PerfLib V2 (advapi32).
//!
//! Non richiede privilegi di amministratore quando eseguito nella sessione interattiva.
//! Carica advapi32.dll solo all'attivazione (sys/dll.rs).
//! Riconosce i modelli verificati (es. Lenovo 83HN) dove TZ01 è la CPU dall'EC (spec §5.5).

use crate::sys::dll::Library;
use crate::temp::ThermalZone;

type FnPerfOpenQueryHandle = unsafe extern "system" fn(*const u16, *mut isize) -> u32;
type FnPerfCloseQueryHandle = unsafe extern "system" fn(isize) -> u32;
type FnPerfAddCounters = unsafe extern "system" fn(isize, *mut u8, u32) -> u32;
type FnPerfQueryCounterData = unsafe extern "system" fn(isize, *mut u8, u32, *mut u32) -> u32;

windows_link::link!("api-ms-win-core-registry-l1-1-0.dll" "system" fn RegGetValueW(
    hkey: isize,
    lpsubkey: *const u16,
    lpvalue: *const u16,
    dwflags: u32,
    pdwtype: *mut u32,
    pvdata: *mut core::ffi::c_void,
    pcbdata: *mut u32,
) -> u32);

const HKEY_LOCAL_MACHINE: isize = -2147483646; // 0x80000002
const RRF_RT_REG_SZ: u32 = 0x00000002;

/// Campionatore della zona termica ACPI via PerfLib V2.
pub struct AcpiSampler {
    _lib: Library,
    close_query: FnPerfCloseQueryHandle,
    query_counter_data: FnPerfQueryCounterData,
    h_query: isize,
    is_cpu_ec: bool,
    last_temp: Option<i16>,
    fixed_ticks: u32,
    buf: Vec<u8>,
}

impl AcpiSampler {
    /// Inizializza il campionatore, aprendo la query e aggiungendo il counterset "Thermal Zone Information".
    pub fn new() -> Option<AcpiSampler> {
        let lib = Library::load("advapi32.dll")?;
        let open_query: FnPerfOpenQueryHandle = unsafe { core::mem::transmute(lib.proc(b"PerfOpenQueryHandle\0")?) };
        let close_query: FnPerfCloseQueryHandle = unsafe { core::mem::transmute(lib.proc(b"PerfCloseQueryHandle\0")?) };
        let add_counters: FnPerfAddCounters = unsafe { core::mem::transmute(lib.proc(b"PerfAddCounters\0")?) };
        let query_counter_data: FnPerfQueryCounterData =
            unsafe { core::mem::transmute(lib.proc(b"PerfQueryCounterData\0")?) };

        let mut h_query: isize = 0;
        // SAFETY: puntatore valido a variabile locale.
        let status = unsafe { open_query(core::ptr::null(), &mut h_query) };
        if status != 0 {
            return None;
        }

        // PERF_COUNTER_IDENTIFIER (40 byte) + InstanceName L"*\0" (4 byte) + padding (4 byte) = 48 byte
        let mut id_buf = [0u8; 48];
        // GUID: 52bc5412-dac2-449c-8bc2-96443888fe6b
        let guid = windows_sys::core::GUID {
            data1: 0x52bc5412,
            data2: 0xdac2,
            data3: 0x449c,
            data4: [0x8b, 0xc2, 0x96, 0x44, 0x38, 0x88, 0xfe, 0x6b],
        };
        unsafe {
            core::ptr::copy_nonoverlapping(&raw const guid as *const u8, id_buf.as_mut_ptr(), 16);
        }
        id_buf[20..24].copy_from_slice(&48u32.to_ne_bytes()); // Size = 48
        id_buf[24..28].copy_from_slice(&3u32.to_ne_bytes()); // CounterId = 3 (High Precision Temp)
        id_buf[28..32].copy_from_slice(&0xFFFF_FFFFu32.to_ne_bytes()); // InstanceId = wildcard
        // Nome istanza: L"*\0"
        id_buf[40] = b'*';
        id_buf[41] = 0;
        id_buf[42] = 0;
        id_buf[43] = 0;

        // SAFETY: id_buf è lungo esattamente 48 byte.
        let add_status = unsafe { add_counters(h_query, id_buf.as_mut_ptr(), id_buf.len() as u32) };
        if add_status != 0 {
            unsafe { close_query(h_query) };
            return None;
        }

        let is_cpu_ec = Self::is_verified_ec_model();

        Some(AcpiSampler {
            _lib: lib,
            close_query,
            query_counter_data,
            h_query,
            is_cpu_ec,
            last_temp: None,
            fixed_ticks: 0,
            buf: vec![0u8; 4096],
        })
    }

    /// Verifica se il BIOS della macchina corrisponde a un modello verificato (es. Lenovo 83HN).
    fn is_verified_ec_model() -> bool {
        let subkey: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\BIOS\0".encode_utf16().collect();
        let val_mfg: Vec<u16> = "SystemManufacturer\0".encode_utf16().collect();
        let val_prod: Vec<u16> = "SystemProductName\0".encode_utf16().collect();

        let mut mfg_buf = [0u16; 64];
        let mut mfg_bytes = (mfg_buf.len() * 2) as u32;
        let ok_mfg = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                val_mfg.as_ptr(),
                RRF_RT_REG_SZ,
                core::ptr::null_mut(),
                mfg_buf.as_mut_ptr() as _,
                &mut mfg_bytes,
            )
        };

        let mut prod_buf = [0u16; 64];
        let mut prod_bytes = (prod_buf.len() * 2) as u32;
        let ok_prod = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                val_prod.as_ptr(),
                RRF_RT_REG_SZ,
                core::ptr::null_mut(),
                prod_buf.as_mut_ptr() as _,
                &mut prod_bytes,
            )
        };

        if ok_mfg == 0 && ok_prod == 0 {
            let mfg = String::from_utf16_lossy(&mfg_buf).to_ascii_uppercase();
            let prod = String::from_utf16_lossy(&prod_buf).to_ascii_uppercase();
            if mfg.contains("LENOVO") && prod.contains("83HN") {
                return true;
            }
        }

        false
    }

    /// Legge le zone termiche disponibili con la temperatura corrente in °C.
    pub fn read(&mut self) -> Option<ThermalZone> {
        let mut needed = 0u32;
        let status = unsafe {
            (self.query_counter_data)(self.h_query, self.buf.as_mut_ptr(), self.buf.len() as u32, &mut needed)
        };

        if status != 0 || needed < 64 {
            return None;
        }

        let slice = &self.buf[..needed as usize];

        // Layout PerfLib V2 per Thermal Zone:
        // Cerca l'istanza TZ01 o la prima istanza nella risposta
        // L'istanza contiene il nome in UTF-16 preceduto dal valore del contatore
        let mut temp_raw: Option<u32> = None;
        let mut instance_name = String::new();

        // Ricerca euristica nel buffer PerfLib per contatore a 32/64 bit plausibile (2732..4000 = 0..127 °C)
        // e nome dell'istanza UTF-16 contenente "TZ"
        let mut i = 64;
        while i + 8 <= slice.len() {
            let val = u32::from_le_bytes(slice[i..i + 4].try_into().unwrap_or([0; 4]));
            if (2732..=4000).contains(&val) {
                temp_raw = Some(val);
                break;
            }
            i += 4;
        }

        // Cerca il nome istanza UTF-16 ("\_TZ" o "TZ")
        for w in slice.windows(10) {
            if w[0] == b'T' && w[1] == 0 && w[2] == b'Z' && w[3] == 0 {
                // Trovato inizio di "TZ"
                let mut name_chars = Vec::new();
                for &[b0, b1] in slice[w.as_ptr() as usize - slice.as_ptr() as usize..].as_chunks::<2>().0 {
                    let code = u16::from_le_bytes([b0, b1]);
                    if code == 0 || name_chars.len() >= 32 {
                        break;
                    }
                    name_chars.push(code);
                }
                instance_name = String::from_utf16_lossy(&name_chars);
                break;
            }
        }

        let raw = temp_raw?;
        // °C = (HP - 2732) / 10
        let temp_c = ((raw as i32 - 2732) / 10) as i16;

        // Rilevamento valore fisso (spec §5.5: resta identica per 10 minuti = 600 tick da 1 s)
        if Some(temp_c) == self.last_temp {
            self.fixed_ticks = self.fixed_ticks.saturating_add(1);
        } else {
            self.fixed_ticks = 0;
            self.last_temp = Some(temp_c);
        }
        let is_fixed = self.fixed_ticks >= 600;

        let name = if instance_name.is_empty() { "TZ01".to_string() } else { instance_name };

        let label = if self.is_cpu_ec { "CPU (EC)".to_string() } else { format!("Zona ACPI {name}") };

        Some(ThermalZone { name, label, temp_c, is_cpu_ec: self.is_cpu_ec, is_fixed })
    }
}

impl Drop for AcpiSampler {
    fn drop(&mut self) {
        if self.h_query != 0 {
            unsafe { (self.close_query)(self.h_query) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_acpi_thermal_zone() {
        if let Some(mut sampler) = AcpiSampler::new() {
            let zone = sampler.read().expect("lettura zona ACPI");
            assert!(zone.temp_c > 10 && zone.temp_c < 115, "temp implausibile: {}", zone.temp_c);
            assert!(!zone.label.is_empty());
        }
    }
}
