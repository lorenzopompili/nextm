//! Versione reale di Windows (senza la "compatibilità" che falsa GetVersionEx).

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

/// (major, minor, build) di Windows, es. (10, 0, 26200).
pub fn windows_version() -> (u32, u32, u32) {
    // SAFETY: struttura azzerata con la dimensione corretta.
    unsafe {
        let mut v: OSVERSIONINFOW = core::mem::zeroed();
        v.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
        RtlGetVersion(&mut v);
        (v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_windows_10_family() {
        let (major, _, build) = windows_version();
        assert_eq!(major, 10);
        assert!(build >= 10_240);
    }
}
