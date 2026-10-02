//! Lettura della temperatura del disco tramite IOCTL_STORAGE_QUERY_PROPERTY.
//!
//! Non richiede privilegi di amministratore (apertura del volume con accesso 0).
//! L'etichetta distingue "SSD NVMe", "SSD" o "Disco" in base a bus e seek penalty (spec §5.5).

use core::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

use crate::temp::DiskDevice;

windows_link::link!("kernel32.dll" "system" fn CreateFileW(
    lpfilename: *const u16,
    dwdesiredaccess: u32,
    dwsharemode: u32,
    lpsecurityattributes: *mut core::ffi::c_void,
    dwcreationdisposition: u32,
    dwflagsandattributes: u32,
    htemplatefile: HANDLE,
) -> HANDLE);

windows_link::link!("kernel32.dll" "system" fn DeviceIoControl(
    hdevice: HANDLE,
    dwiocontrolcode: u32,
    lpinbuffer: *const core::ffi::c_void,
    ninbuffersize: u32,
    lpoutbuffer: *mut core::ffi::c_void,
    noutbuffersize: u32,
    lpbytesreturned: *mut u32,
    lpoverlapped: *mut core::ffi::c_void,
) -> i32);

const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

const STORAGE_PROPERTY_DEVICE: u32 = 0;
const STORAGE_PROPERTY_SEEK_PENALTY: u32 = 7;
const STORAGE_PROPERTY_TEMPERATURE: u32 = 52;
const BUS_TYPE_NVME: u32 = 17;

#[repr(C)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional_parameters: [u8; 1],
}

/// Campionatore della temperatura del disco di sistema.
pub struct DiskSampler {
    handle: HANDLE,
    label: String,
    drive_letter: char,
}

impl DiskSampler {
    /// Apre il disco del volume specificato (default 'C') con accesso 0.
    pub fn new() -> Option<DiskSampler> {
        Self::for_drive('C')
    }

    pub fn for_drive(drive_letter: char) -> Option<DiskSampler> {
        let path = format!("\\\\.\\{drive_letter}:\0");
        let wide: Vec<u16> = path.encode_utf16().collect();

        // SAFETY: path terminato da zero, accesso 0 (nessun diritto speciale richiesto).
        let handle = unsafe {
            CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null_mut(), OPEN_EXISTING, 0, null_mut())
        };

        if handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let label = Self::detect_label(handle);

        Some(DiskSampler { handle, label, drive_letter })
    }

    fn detect_label(handle: HANDLE) -> String {
        let mut is_nvme = false;
        let mut is_ssd = false;

        // 1. Prova StorageDeviceProperty per il bus type (NVMe = 17)
        let q_device =
            StoragePropertyQuery { property_id: STORAGE_PROPERTY_DEVICE, query_type: 0, additional_parameters: [0] };
        let mut dev_buf = [0u8; 256];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &raw const q_device as _,
                core::mem::size_of::<StoragePropertyQuery>() as u32,
                dev_buf.as_mut_ptr() as _,
                dev_buf.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        if ok != 0 && ret >= 32 {
            let bus_type = u32::from_le_bytes(dev_buf[28..32].try_into().unwrap_or([0; 4]));
            if bus_type == BUS_TYPE_NVME {
                is_nvme = true;
            }
        }

        // 2. Prova StorageDeviceSeekPenaltyProperty
        let q_seek = StoragePropertyQuery {
            property_id: STORAGE_PROPERTY_SEEK_PENALTY,
            query_type: 0,
            additional_parameters: [0],
        };
        let mut seek_buf = [0u8; 16];
        let mut ret_seek = 0u32;
        let ok_seek = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &raw const q_seek as _,
                core::mem::size_of::<StoragePropertyQuery>() as u32,
                seek_buf.as_mut_ptr() as _,
                seek_buf.len() as u32,
                &mut ret_seek,
                null_mut(),
            )
        };
        if ok_seek != 0 && ret_seek >= 9 {
            // Offset 8 è IncursSeekPenalty (BOOLEAN)
            if seek_buf[8] == 0 {
                is_ssd = true;
            }
        }

        if is_nvme {
            "SSD NVMe".to_string()
        } else if is_ssd {
            "SSD".to_string()
        } else {
            "Disco".to_string()
        }
    }

    /// Legge la temperatura corrente e le soglie (in °C interi).
    pub fn read(&self) -> Option<DiskDevice> {
        let query = StoragePropertyQuery {
            property_id: STORAGE_PROPERTY_TEMPERATURE,
            query_type: 0,
            additional_parameters: [0],
        };

        let mut buf = [0u8; 512];
        let mut returned = 0u32;

        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &raw const query as _,
                core::mem::size_of::<StoragePropertyQuery>() as u32,
                buf.as_mut_ptr() as _,
                buf.len() as u32,
                &mut returned,
                null_mut(),
            )
        };

        if ok == 0 || returned < 24 {
            return None;
        }

        let crit_raw = i16::from_le_bytes(buf[8..10].try_into().ok()?);
        let warn_raw = i16::from_le_bytes(buf[10..12].try_into().ok()?);
        let count = u16::from_le_bytes(buf[12..14].try_into().ok()?);

        if count == 0 || returned < 24 + 16 {
            return None;
        }

        // Primo sensore (indice 0: valore composito)
        let temp_c = i16::from_le_bytes(buf[26..28].try_into().ok()?);

        Some(DiskDevice {
            label: self.label.clone(),
            drive_letter: self.drive_letter,
            temp_c,
            warn_c: (warn_raw > 0).then_some(warn_raw),
            crit_c: (crit_raw > 0).then_some(crit_raw),
        })
    }
}

impl Drop for DiskSampler {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_system_disk_temperature() {
        if let Some(sampler) = DiskSampler::new()
            && let Some(data) = sampler.read()
        {
            assert!(data.temp_c > 0 && data.temp_c < 120, "temp implausibile: {}", data.temp_c);
            assert!(!data.label.is_empty());
        }
    }
}
