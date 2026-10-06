//! Campionamento nativo di GPU Engine (3D %) e VRAM (memoria video) tramite D3DKMT (gdi32.dll).
//!
//! Non richiede SDK esterni (NVAPI, ADL, RivaTuner), funziona su qualsiasi GPU (NVIDIA, AMD, Intel),
//! e non introduce dipendenze runtime esterne (gdi32.dll è già presente nel sistema operativo).

use crate::sys::dll::Library;

#[repr(C)]
#[derive(Clone, Copy, Default)]
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

#[repr(C)]
struct D3dkmtQuerystatistics {
    query_type: u32,
    adapter_luid: Luid,
    h_process: *mut core::ffi::c_void,
    query_result: [u8; 1024],
}

const KMTQAITYPE_ADAPTERREGISTRYINFO: u32 = 8;
const D3DKMT_QUERYSTATISTICS_ADAPTER: u32 = 0;
const D3DKMT_QUERYSTATISTICS_SEGMENT: u32 = 3;
const D3DKMT_QUERYSTATISTICS_NODE: u32 = 5;

type FnEnumAdapters = unsafe extern "system" fn(*mut D3dkmtEnumadapters2) -> i32;
type FnQueryAdapterInfo = unsafe extern "system" fn(*const D3dkmtQueryadapterinfo) -> i32;
type FnCloseAdapter = unsafe extern "system" fn(*const D3dkmtCloseadapter) -> i32;
type FnQueryStatistics = unsafe extern "system" fn(*mut D3dkmtQuerystatistics) -> i32;

/// Metriche aggregate della GPU: utilizzo 3D e memoria video dedicata (VRAM).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuStats {
    pub name: String,
    pub luid: u64,
    pub gpu_pct: Option<u8>,
    pub vram_used_bytes: u64,
    pub vram_total_bytes: u64,
}

/// Campionatore delle metriche di carico e memoria della GPU.
pub struct GpuMetricsSampler {
    _lib: Library,
    enum_adapters: FnEnumAdapters,
    query_info: FnQueryAdapterInfo,
    close_adapter: FnCloseAdapter,
    query_stats: FnQueryStatistics,
    prev_node_time: u64,
    last_tick_ms: u64,
    cached_name: Option<(u64, String)>,
}

impl GpuMetricsSampler {
    /// Inizializza il campionatore caricando `gdi32.dll` dinamicamente.
    pub fn new() -> Option<Self> {
        let lib = Library::load("gdi32.dll")?;
        let enum_adapters: FnEnumAdapters = unsafe { core::mem::transmute(lib.proc(b"D3DKMTEnumAdapters2\0")?) };
        let query_info: FnQueryAdapterInfo = unsafe { core::mem::transmute(lib.proc(b"D3DKMTQueryAdapterInfo\0")?) };
        let close_adapter: FnCloseAdapter = unsafe { core::mem::transmute(lib.proc(b"D3DKMTCloseAdapter\0")?) };
        let query_stats: FnQueryStatistics = unsafe { core::mem::transmute(lib.proc(b"D3DKMTQueryStatistics\0")?) };

        Some(Self {
            _lib: lib,
            enum_adapters,
            query_info,
            close_adapter,
            query_stats,
            prev_node_time: 0,
            last_tick_ms: 0,
            cached_name: None,
        })
    }

    /// Campiona la prima GPU attiva o quella specificata da `selected_luid`.
    pub fn sample(&mut self, selected_luid: Option<u64>) -> Option<GpuStats> {
        let mut raw_adapters = [D3dkmtAdapterinfo2 {
            h_adapter: 0,
            adapter_luid: Luid::default(),
            num_sources: 0,
            b_present_move_regions_preferred: 0,
        }; 8];

        let mut enum_struct = D3dkmtEnumadapters2 { num_adapters: 8, p_adapters: raw_adapters.as_mut_ptr() };

        let status = unsafe { (self.enum_adapters)(&mut enum_struct) };
        if status != 0 {
            return None;
        }

        let now_ms = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        let delta_ms =
            if self.last_tick_ms > 0 && now_ms > self.last_tick_ms { now_ms - self.last_tick_ms } else { 1000 };
        self.last_tick_ms = now_ms;

        let total_count = (enum_struct.num_adapters as usize).min(raw_adapters.len());
        let mut chosen_adapter: Option<(u32, Luid, u64)> = None;

        for raw in raw_adapters.iter().take(total_count) {
            let luid_val = (raw.adapter_luid.low_part as u64) | ((raw.adapter_luid.high_part as u64) << 32);
            if let Some(target) = selected_luid {
                if luid_val == target {
                    chosen_adapter = Some((raw.h_adapter, raw.adapter_luid, luid_val));
                    break;
                }
            } else if chosen_adapter.is_none() {
                chosen_adapter = Some((raw.h_adapter, raw.adapter_luid, luid_val));
            }
        }

        let (h_adapter, adapter_luid, luid_val) = chosen_adapter?;

        // 1. Nome GPU da KMTQAITYPE_ADAPTERREGISTRYINFO (o dalla cache)
        let name = if let Some((cached_luid, cached_n)) = &self.cached_name
            && *cached_luid == luid_val
        {
            cached_n.clone()
        } else {
            let mut reg_buf = [0u16; 260 * 4];
            let q_reg = D3dkmtQueryadapterinfo {
                h_adapter,
                query_type: KMTQAITYPE_ADAPTERREGISTRYINFO,
                p_private_driver_data: reg_buf.as_mut_ptr() as _,
                private_driver_data_size: (reg_buf.len() * 2) as u32,
            };
            let mut n = String::new();
            if unsafe { (self.query_info)(&q_reg) } == 0 {
                let end = reg_buf[..260].iter().position(|&c| c == 0).unwrap_or(260);
                n = String::from_utf16_lossy(&reg_buf[..end]);
            }
            if n.is_empty() {
                n = String::from("GPU");
            }
            self.cached_name = Some((luid_val, n.clone()));
            n
        };

        // Chiude adapter handle (non serve più per D3DKMTQueryStatistics, che usa adapter_luid)
        unsafe { (self.close_adapter)(&D3dkmtCloseadapter { h_adapter }) };

        // 2. Query Adapter per numero di segmenti e nodi
        let mut adapter_stats = D3dkmtQuerystatistics {
            query_type: D3DKMT_QUERYSTATISTICS_ADAPTER,
            adapter_luid,
            h_process: core::ptr::null_mut(),
            query_result: [0; 1024],
        };
        let ad_res = unsafe { (self.query_stats)(&mut adapter_stats) };
        if ad_res != 0 {
            return Some(GpuStats { name, luid: luid_val, gpu_pct: None, vram_used_bytes: 0, vram_total_bytes: 0 });
        }

        let nb_segments = u32::from_ne_bytes(adapter_stats.query_result[0..4].try_into().unwrap_or([0; 4]));
        let node_count = u32::from_ne_bytes(adapter_stats.query_result[4..8].try_into().unwrap_or([0; 4]));

        // 3. VRAM (Segmenti locali)
        let mut total_vram_limit = 0u64;
        let mut total_vram_committed = 0u64;

        for seg_idx in 0..nb_segments.min(16) {
            let mut seg_stats = D3dkmtQuerystatistics {
                query_type: D3DKMT_QUERYSTATISTICS_SEGMENT,
                adapter_luid,
                h_process: core::ptr::null_mut(),
                query_result: [0; 1024],
            };
            seg_stats.query_result[0..4].copy_from_slice(&seg_idx.to_ne_bytes());
            if unsafe { (self.query_stats)(&mut seg_stats) } == 0 {
                let limit = u64::from_ne_bytes(seg_stats.query_result[0..8].try_into().unwrap_or([0; 8]));
                let committed = u64::from_ne_bytes(seg_stats.query_result[8..16].try_into().unwrap_or([0; 8]));
                if limit > 0 {
                    // Prendi il segmento con limite maggiore o dedicato
                    if limit > total_vram_limit {
                        total_vram_limit = limit;
                        total_vram_committed = committed;
                    }
                }
            }
        }

        // 4. GPU 3D Engine Load (Node 0 running time)
        let mut current_node_time = 0u64;
        if node_count > 0 {
            let mut node_stats = D3dkmtQuerystatistics {
                query_type: D3DKMT_QUERYSTATISTICS_NODE,
                adapter_luid,
                h_process: core::ptr::null_mut(),
                query_result: [0; 1024],
            };
            // Node 0 (solitamente 3D Graphics Engine)
            node_stats.query_result[0..4].copy_from_slice(&0u32.to_ne_bytes());
            if unsafe { (self.query_stats)(&mut node_stats) } == 0 {
                // RunningTime in 100ns units
                current_node_time = u64::from_ne_bytes(node_stats.query_result[0..8].try_into().unwrap_or([0; 8]));
            }
        }

        let gpu_pct = if self.prev_node_time > 0 && current_node_time >= self.prev_node_time {
            let delta_100ns = current_node_time.saturating_sub(self.prev_node_time);
            let total_100ns = (delta_ms as f64) * 10_000.0;
            if total_100ns > 0.0 {
                let pct = ((delta_100ns as f64 / total_100ns) * 100.0).clamp(0.0, 100.0);
                Some(pct.round() as u8)
            } else {
                Some(0)
            }
        } else {
            None
        };
        self.prev_node_time = current_node_time;

        Some(GpuStats {
            name,
            luid: luid_val,
            gpu_pct,
            vram_used_bytes: total_vram_committed,
            vram_total_bytes: total_vram_limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_gpu_metrics() {
        if let Some(mut sampler) = GpuMetricsSampler::new()
            && let Some(stats) = sampler.sample(None)
        {
            assert!(!stats.name.is_empty());
            println!(
                "GPU: {}, VRAM: {} / {} MB",
                stats.name,
                stats.vram_used_bytes / 1024 / 1024,
                stats.vram_total_bytes / 1024 / 1024
            );
        }
    }
}
