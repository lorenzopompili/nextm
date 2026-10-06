//! Rilevamento delle unità disco montate e lettura dello spazio libero e totale.

use crate::disk::MountedDisk;

windows_link::link!("kernel32.dll" "system" fn GetLogicalDrives() -> u32);
windows_link::link!("kernel32.dll" "system" fn GetDriveTypeW(lprootpathname: *const u16) -> u32);
windows_link::link!("kernel32.dll" "system" fn GetDiskFreeSpaceExW(
    lpdirectoryname: *const u16,
    lpfreebytesavailabletocaller: *mut u64,
    lptotalnumberofbytes: *mut u64,
    lptotalnumberoffreebytes: *mut u64,
) -> i32);
windows_link::link!("kernel32.dll" "system" fn SetErrorMode(umode: u32) -> u32);
windows_link::link!("kernel32.dll" "system" fn CreateFileW(
    lpfilename: *const u16,
    dwdesiredaccess: u32,
    dwsharemode: u32,
    lpsecurityattributes: *const core::ffi::c_void,
    dwcreationdisposition: u32,
    dwflagsandattributes: u32,
    htemplatefile: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void);
windows_link::link!("kernel32.dll" "system" fn CloseHandle(hobject: *mut core::ffi::c_void) -> i32);
windows_link::link!("kernel32.dll" "system" fn DeviceIoControl(
    hdevice: *mut core::ffi::c_void,
    dwiocontrolcode: u32,
    lpinbuffer: *const core::ffi::c_void,
    ninbuffersize: u32,
    lpoutbuffer: *mut core::ffi::c_void,
    noutbuffersize: u32,
    lpbytesreturned: *mut u32,
    lpoverlapped: *mut core::ffi::c_void,
) -> i32);

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct DiskPerformance {
    pub bytes_read: i64,
    pub bytes_written: i64,
    pub read_time: i64,
    pub write_time: i64,
    pub idle_time: i64,
    pub read_count: u32,
    pub write_count: u32,
    pub queue_depth: u32,
    pub overall_granularity: u32,
    pub query_time: i64,
    pub storage_device_number: u32,
    pub storage_manager_name: [u16; 8],
}

const SEM_FAILCRITICALERRORS: u32 = 0x0001;

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_RAMDISK: u32 = 6;

/// Esegue la scansione di tutti i dischi montati e restituisce lo spazio per ciascuna unità valida e pronta.
pub fn scan_mounted_disks() -> Vec<MountedDisk> {
    let mut disks = Vec::new();

    // Disabilita finestre di errore critico (es. unità rimovibile senza supporto inserito)
    let old_mode = unsafe { SetErrorMode(SEM_FAILCRITICALERRORS) };
    let mask = unsafe { GetLogicalDrives() };

    for i in 0..26 {
        if (mask & (1 << i)) != 0 {
            let letter = (b'A' + i) as char;
            let root = [letter as u16, b':' as u16, b'\\' as u16, 0];
            let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };

            // Consideriamo unità fisse (SSD/HDD), rimovibili (chiavette/dischi USB pronti) e ramdisk
            if drive_type == DRIVE_FIXED || drive_type == DRIVE_REMOVABLE || drive_type == DRIVE_RAMDISK {
                let mut free_caller: u64 = 0;
                let mut total: u64 = 0;
                let mut free_total: u64 = 0;

                let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free_caller, &mut total, &mut free_total) };

                if ok != 0 && total > 0 {
                    disks.push(MountedDisk::new(letter, total, free_caller));
                }
            }
        }
    }

    unsafe { SetErrorMode(old_mode) };

    disks
}

/// Interroga i byte letti e scritti totali di un'unità montata tramite `IOCTL_DISK_PERFORMANCE`.
pub fn query_disk_io(letter: char) -> Option<(u64, u64)> {
    let path = [b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16, letter as u16, b':' as u16, 0];
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            3, // FILE_SHARE_READ | FILE_SHARE_WRITE
            core::ptr::null(),
            3, // OPEN_EXISTING
            0,
            core::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle == (-1isize as *mut core::ffi::c_void) {
        return None;
    }
    let mut perf = DiskPerformance::default();
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            0x00070020, // IOCTL_DISK_PERFORMANCE
            core::ptr::null(),
            0,
            (&raw mut perf).cast(),
            core::mem::size_of::<DiskPerformance>() as u32,
            &mut returned,
            core::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(handle) };
    if ok != 0 { Some((perf.bytes_read.max(0) as u64, perf.bytes_written.max(0) as u64)) } else { None }
}

/// Campionatore continuo della velocità di I/O (lettura e scrittura B/s) per tutte le unità montate.
#[derive(Default)]
pub struct DiskSpeedSampler {
    prev_counters: std::collections::BTreeMap<char, (u64, u64)>,
    prev_disks: Vec<MountedDisk>,
    last_tick_ms: u64,
}

impl DiskSpeedSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Campiona tutte le unità montate e restituisce lo stato aggiornato con spazio e velocità live.
    pub fn sample(&mut self) -> Vec<MountedDisk> {
        let now_ms = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        // Evita frequenze di campionamento inferiori a 750 ms per non produrre numeri rumorosi o divisioni anomale
        if self.last_tick_ms > 0 && now_ms < self.last_tick_ms + 750 && !self.prev_disks.is_empty() {
            return self.prev_disks.clone();
        }

        let mut disks = scan_mounted_disks();
        let delta_ms = if self.last_tick_ms > 0 && now_ms > self.last_tick_ms {
            (now_ms - self.last_tick_ms).max(1)
        } else {
            1000
        };
        self.last_tick_ms = now_ms;

        for d in &mut disks {
            if let Some((cur_read, cur_write)) = query_disk_io(d.letter) {
                if let Some(&(prev_read, prev_write)) = self.prev_counters.get(&d.letter) {
                    let d_read = cur_read.saturating_sub(prev_read);
                    let d_write = cur_write.saturating_sub(prev_write);
                    d.read_bps = (d_read * 1000).checked_div(delta_ms).unwrap_or(0);
                    d.write_bps = (d_write * 1000).checked_div(delta_ms).unwrap_or(0);
                }
                self.prev_counters.insert(d.letter, (cur_read, cur_write));
            }
        }
        self.prev_disks = disks.clone();
        disks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_at_least_one_valid_mounted_disk() {
        let disks = scan_mounted_disks();
        assert!(!disks.is_empty(), "deve trovare almeno un disco montato");
        for d in &disks {
            assert!(d.letter.is_ascii_uppercase());
            assert!(d.total_bytes > 0);
            assert!(d.free_bytes <= d.total_bytes);
        }
    }

    #[test]
    fn test_disk_performance_ioctl() {
        if let Some((r, w)) = query_disk_io('C') {
            eprintln!("Drive C: I/O Read = {r} bytes, Write = {w} bytes");
        }
    }

    #[test]
    fn test_disk_speed_sampler() {
        let mut sampler = DiskSpeedSampler::new();
        let s1 = sampler.sample();
        assert!(!s1.is_empty());
        let s2 = sampler.sample();
        assert_eq!(s1.len(), s2.len());
    }
}
