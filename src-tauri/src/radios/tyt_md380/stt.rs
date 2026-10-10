//! Windows: the radio through ST's own DFU driver, STTub30.
//!
//! TYT's CPS installs STTub30 for the radio, and libusb-style access (nusb's
//! WinUSB) cannot open a device that driver owns. So on Windows the radio is
//! found by STTub30's device-interface GUID and every DFU request is handed to
//! the driver as a `DeviceIoControl` — dmrconfig's `dfu-windows.c` (BSD-3),
//! transcribed.
//!
//! Measured on Tim's Intel PC (s136, Windows 10.0.26200, AMD64, STTub30 from
//! `oem22.inf`) with a PowerShell transcription of exactly this: identify and a
//! full read, 1.2 s, the image identical to the Mac's read but for the one
//! byte the radio itself rewrites.
//!
//! This Rust port was then run by Tim on Windows with v26.10.10 (2026-10-10)
//! and works.
//!
//! The app never installs or changes a driver (Tim, s136). With STTub30 absent
//! the caller falls back to WinUSB via nusb — which works only if the user has
//! put WinUSB on the radio themselves — and otherwise says plainly that no
//! usable driver is installed.

use std::ffi::c_void;
use std::time::Duration;

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::protocol::Link;

/// STTub30's device-interface class for 0483:DF11 (dmrconfig `dfu_init`).
const GUID_0483_DF11: GUID = GUID::from_u128(0x3fe809ab_fb91_4cb5_a643_69670d52366e);

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, 0x805, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
const PU_VENDOR_REQUEST: u32 = (0x22 << 16) | (0x805 << 2);
/// `URB_FUNCTION_CLASS_INTERFACE`.
const URB_CLASS_INTERFACE: u16 = 0x1B;
const DIR_OUT: u32 = 0;
const DIR_IN: u32 = 1;
/// The request header STTub30 takes: Function u16 @0, Direction u32 @4,
/// Request u8 @8, Value u16 @10, Index u16 @12, Length u32 @16 — C layout,
/// 20 bytes — then the OUT data.
const HEADER: usize = 20;

fn header(direction: u32, request: u8, value: u16, length: u32) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0..2].copy_from_slice(&URB_CLASS_INTERFACE.to_le_bytes());
    h[4..8].copy_from_slice(&direction.to_le_bytes());
    h[8] = request;
    h[10..12].copy_from_slice(&value.to_le_bytes());
    h[16..20].copy_from_slice(&length.to_le_bytes());
    h
}

pub(crate) struct SttLink {
    handle: HANDLE,
}

// The handle is used from one thread at a time (one radio operation per
// session); it is only moved between threads, never shared.
unsafe impl Send for SttLink {}

impl Drop for SttLink {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// The device path of the first present STTub30 interface, if any.
fn find_path() -> Option<Vec<u16>> {
    unsafe {
        let set = SetupDiGetClassDevsW(
            &GUID_0483_DF11,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == -1 {
            return None;
        }
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let mut path = None;
        if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &GUID_0483_DF11, 0, &mut iface) != 0 {
            let mut need = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &iface, std::ptr::null_mut(), 0, &mut need, std::ptr::null_mut());
            // A u32-aligned buffer: the detail struct starts with a u32.
            let mut buf = vec![0u32; (need as usize).div_ceil(4).max(2)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &iface, detail, need, &mut need, std::ptr::null_mut()) != 0 {
                let start = (detail as *const u8).add(4) as *const u16;
                let max = (need as usize - 4) / 2;
                let units = std::slice::from_raw_parts(start, max);
                let len = units.iter().position(|&u| u == 0).unwrap_or(max);
                let mut p = units[..len].to_vec();
                p.push(0);
                path = Some(p);
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        path
    }
}

/// Whether STTub30 has a present device for 0483:DF11 — without opening it.
pub(crate) fn present() -> bool {
    find_path().is_some()
}

/// Open the radio through STTub30, or `None` when that driver does not own it.
pub(crate) fn open() -> Result<Option<SttLink>, String> {
    let Some(path) = find_path() else { return Ok(None) };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        return Err(format!(
            "the MD-380 is on USB but could not be opened through its driver (Windows error \
             {err}). Close TYT's CPS or any other program using the radio, then try again."
        ));
    }
    Ok(Some(SttLink { handle }))
}

impl SttLink {
    fn ioctl(&mut self, input: &[u8], out: &mut [u8], what: &str) -> Result<usize, String> {
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                PU_VENDOR_REQUEST,
                input.as_ptr() as *const c_void,
                input.len() as u32,
                if out.is_empty() { std::ptr::null_mut() } else { out.as_mut_ptr() as *mut c_void },
                out.len() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!("USB request {what} failed (Windows error {err})"));
        }
        Ok(got as usize)
    }
}

impl Link for SttLink {
    fn ctl_out(&mut self, request: u8, value: u16, data: &[u8]) -> Result<(), String> {
        let mut rq = header(DIR_OUT, request, value, data.len() as u32).to_vec();
        rq.extend_from_slice(data);
        self.ioctl(&rq, &mut [], &request.to_string()).map(|_| ())
    }

    fn ctl_in(&mut self, request: u8, value: u16, len: u16) -> Result<Vec<u8>, String> {
        let rq = header(DIR_IN, request, value, len as u32);
        let mut out = vec![0u8; len as usize];
        let got = self.ioctl(&rq, &mut out, &request.to_string())?;
        out.truncate(got);
        Ok(out)
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header bytes the PowerShell reader sent on Tim's Intel PC (s136).
    #[test]
    fn the_request_header_matches_what_the_radio_answered() {
        let h = header(DIR_IN, 2, 0x0102, 1024);
        assert_eq!(
            h,
            [0x1B, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0x02, 0x01, 0, 0, 0, 0, 0x00, 0x04, 0, 0]
        );
        assert_eq!(PU_VENDOR_REQUEST, 0x0022_2014);
    }
}
