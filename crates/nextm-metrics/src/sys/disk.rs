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
}
