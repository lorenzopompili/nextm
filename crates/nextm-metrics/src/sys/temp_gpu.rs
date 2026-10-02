//! Lettura della temperatura della GPU tramite D3DKMTQueryAdapterInfo (WDDM).
//!
//! Risolve `D3DKMTEnumAdapters2`, `D3DKMTQueryAdapterInfo` e `D3DKMTCloseAdapter` da `gdi32.dll`
//! solo all'attivazione (gdi32 funge da forwarder per dxcore; nessun import statico).
//! Scarta gli adattatori software (Basic Render) e quelli che non restituiscono dati (spec §5.5).

use crate::sys::dll::Library;
use crate::temp::GpuAdapter;

#[repr(C)]
#[derive(Clone, Copy)]
struct Luid {
    low_part: u32,
    high_part: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D3dkmtAdapterinfo2 {
    h_adapter: u32,
    adapter_luid: Luid,
    num_sources: u32,
    b_present_move_regions_preferred: u32,
}

#[repr(C)]
struct D3dkmtEnumadapters2 {
    num_adapters: u32,
    p_adapters: *mut D3dkmtAdapterinfo2,
}

#[repr(C)]
struct D3dkmtQueryadapterinfo {
    h_adapter: u32,
    query_type: u32,
    p_private_driver_data: *mut core::ffi::c_void,
    private_driver_data_size: u32,
}

#[repr(C)]
struct D3dkmtCloseadapter {
    h_adapter: u32,
}

#[repr(C, align(8))]
struct D3dkmtAdapterPerfdata {
    physical_adapter_index: u32,
    _pad: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    temperature: u32,
    power_state_override: u8,
    _pad2: [u8; 3],
}

const KMTQAITYPE_ADAPTERREGISTRYINFO: u32 = 8;
const KMTQAITYPE_ADAPTERPERFDATA: u32 = 62;

type FnEnumAdapters = unsafe extern "system" fn(*mut D3dkmtEnumadapters2) -> i32;
type FnQueryAdapterInfo = unsafe extern "system" fn(*const D3dkmtQueryadapterinfo) -> i32;
type FnCloseAdapter = unsafe extern "system" fn(*const D3dkmtCloseadapter) -> i32;

/// Campionatore della temperatura della GPU.
pub struct GpuSampler {
    _lib: Library,
    enum_adapters: FnEnumAdapters,
    query_info: FnQueryAdapterInfo,
    close_adapter: FnCloseAdapter,
}

impl GpuSampler {
    /// Carica gdi32.dll e risolve i puntatori D3DKMT.
    pub fn new() -> Option<GpuSampler> {
        let lib = Library::load("gdi32.dll")?;
        let enum_adapters: FnEnumAdapters = unsafe { core::mem::transmute(lib.proc(b"D3DKMTEnumAdapters2\0")?) };
        let query_info: FnQueryAdapterInfo = unsafe { core::mem::transmute(lib.proc(b"D3DKMTQueryAdapterInfo\0")?) };
        let close_adapter: FnCloseAdapter = unsafe { core::mem::transmute(lib.proc(b"D3DKMTCloseAdapter\0")?) };

        Some(GpuSampler { _lib: lib, enum_adapters, query_info, close_adapter })
    }

    /// Elenca tutti gli adattatori fisici con le relative temperature correnti.
    pub fn list_adapters(&self) -> Vec<GpuAdapter> {
        let mut raw_adapters = [D3dkmtAdapterinfo2 {
            h_adapter: 0,
            adapter_luid: Luid { low_part: 0, high_part: 0 },
            num_sources: 0,
            b_present_move_regions_preferred: 0,
        }; 8];

        let mut enum_struct = D3dkmtEnumadapters2 { num_adapters: 8, p_adapters: raw_adapters.as_mut_ptr() };

        // SAFETY: puntatore valido con buffer allocato nello stack.
        let status = unsafe { (self.enum_adapters)(&mut enum_struct) };
        if status != 0 {
            return Vec::new();
        }

        let mut results = Vec::new();

        for (i, raw) in raw_adapters.iter().enumerate().take(enum_struct.num_adapters as usize) {
            let h = raw.h_adapter;
            let luid_val = (raw.adapter_luid.low_part as u64) | ((raw.adapter_luid.high_part as u64) << 32);

            // Nome adattatore da KMTQAITYPE_ADAPTERREGISTRYINFO (8)
            let mut reg_buf = [0u16; 260 * 4];
            let q_reg = D3dkmtQueryadapterinfo {
                h_adapter: h,
                query_type: KMTQAITYPE_ADAPTERREGISTRYINFO,
                p_private_driver_data: reg_buf.as_mut_ptr() as _,
                private_driver_data_size: (reg_buf.len() * 2) as u32,
            };
            let mut name = String::new();
            if unsafe { (self.query_info)(&q_reg) } == 0 {
                let end = reg_buf[..260].iter().position(|&c| c == 0).unwrap_or(260);
                name = String::from_utf16_lossy(&reg_buf[..end]);
            }
            if name.is_empty() {
                name = format!("GPU #{i}");
            }

            // Temperatura da KMTQAITYPE_ADAPTERPERFDATA (62)
            let mut perf = D3dkmtAdapterPerfdata {
                physical_adapter_index: 0,
                _pad: 0,
                memory_frequency: 0,
                max_memory_frequency: 0,
                max_memory_frequency_oc: 0,
                memory_bandwidth: 0,
                pcie_bandwidth: 0,
                fan_rpm: 0,
                power: 0,
                temperature: 0,
                power_state_override: 0,
                _pad2: [0; 3],
            };

            let q_perf = D3dkmtQueryadapterinfo {
                h_adapter: h,
                query_type: KMTQAITYPE_ADAPTERPERFDATA,
                p_private_driver_data: &raw mut perf as _,
                private_driver_data_size: core::mem::size_of::<D3dkmtAdapterPerfdata>() as u32,
            };

            let perf_status = unsafe { (self.query_info)(&q_perf) };
            // Chiude l'adapter handle
            unsafe { (self.close_adapter)(&D3dkmtCloseadapter { h_adapter: h }) };

            if perf_status == 0 {
                // temperature in decimi di grado (0.1 °C). 0 indica nessun dato.
                let temp_c = if perf.temperature > 0 && perf.temperature < 1500 {
                    Some((perf.temperature / 10) as i16)
                } else {
                    None
                };

                results.push(GpuAdapter { name, luid: luid_val, temp_c });
            }
        }

        results
    }

    /// Legge la temperatura dell'adattatore selezionato o del primo disponibile.
    pub fn read(&self, selected_luid: Option<u64>) -> Option<GpuAdapter> {
        let list = self.list_adapters();
        if let Some(luid) = selected_luid {
            list.into_iter().find(|a| a.luid == luid)
        } else {
            list.into_iter().find(|a| a.temp_c.is_some())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gpu_temperature() {
        if let Some(sampler) = GpuSampler::new() {
            let list = sampler.list_adapters();
            for gpu in list {
                if let Some(temp) = gpu.temp_c {
                    assert!(temp > 0 && temp < 120, "temperatura GPU implausibile: {temp}");
                }
            }
            if let Some(gpu) = sampler.read(None)
                && let Some(temp) = gpu.temp_c
            {
                assert!(temp > 0 && temp < 120, "temperatura GPU implausibile: {temp}");
            }
        }
    }
}
