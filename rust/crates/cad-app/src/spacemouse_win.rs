//! 3Dconnexion 3D mouse on Windows through Raw Input. No 3Dconnexion SDK: every HID multi-axis controller (usage
//! page 1, usage 8) sends its reports as `WM_INPUT`. The 3DxWare driver keeps running next to it; its keyboard /
//! mouse emulator is a separate HID joystick (usage 4) and is not read.
//!
//! The reports are received by a hidden message-only window on a thread of its own (the window of the application
//! belongs to the GUI toolkit, whose window procedure we do not subclass). The window registers with
//! `RIDEV_INPUTSINK`, so it receives the reports also while another window has the focus; the reports are only used
//! while the application is active (`Shared::active`), as `spacemouse.py` did.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices, HRAWINPUT, RAWINPUTDEVICE,
    RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDI_DEVICEINFO, RID_DEVICE_INFO, RID_INPUT, RIM_TYPEHID,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW, TranslateMessage, HWND_MESSAGE, MSG,
    WM_INPUT, WNDCLASSEXW,
};

use crate::spacemouse::{apply_report, button_action, HidState, VK_LAYOUT};

/// State shared between the listener thread and the window.
#[derive(Default)]
pub struct Shared {
    pub state: HidState,
    /// Button actions waiting to be taken by the window.
    pub actions: Vec<&'static str>,
    /// The application window is active: reports are used only then.
    pub active: bool,
    /// "Ovládat pohled 3D myší".
    pub enabled: bool,
    /// The window is created and Windows delivers the reports to it.
    pub registered: bool,
    /// Product id of every device handle seen.
    pids: HashMap<usize, Option<u16>>,
}

static SHARED: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn device_ids(handle: *mut core::ffi::c_void) -> Option<(u16, u16, u16, u16)> {
    // SAFETY: the info struct is plain data of the documented size
    unsafe {
        let mut info = RID_DEVICE_INFO::default();
        info.cbSize = std::mem::size_of::<RID_DEVICE_INFO>() as u32;
        let mut size = info.cbSize;
        let n = GetRawInputDeviceInfoW(handle, RIDI_DEVICEINFO, (&mut info as *mut RID_DEVICE_INFO).cast(), &mut size);
        if n == 0 || n == u32::MAX {
            return None;
        }
        let hid = info.Anonymous.hid;
        Some((hid.dwVendorId as u16, hid.dwProductId as u16, hid.usUsagePage, hid.usUsage))
    }
}

/// Connected multi-axis controllers as `(vendor id, product id)`.
pub fn devices() -> Vec<(u16, u16)> {
    // SAFETY: the list buffer has the size Windows asked for
    unsafe {
        let item = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut n = 0u32;
        if GetRawInputDeviceList(std::ptr::null_mut(), &mut n, item) == u32::MAX || n == 0 {
            return Vec::new();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); n as usize];
        let got = GetRawInputDeviceList(list.as_mut_ptr(), &mut n, item);
        if got == u32::MAX {
            return Vec::new();
        }
        list.iter()
            .take(got as usize)
            .filter(|d| d.dwType == RIM_TYPEHID)
            .filter_map(|d| device_ids(d.hDevice))
            .filter(|(_, _, page, usage)| *page == 1 && *usage == 8)
            .map(|(v, p, _, _)| (v, p))
            .collect()
    }
}

/// The HID reports of a `RAWHID` structure: `dwSizeHid`, `dwCount`, then `dwCount` reports of `dwSizeHid` bytes each.
pub fn extract_reports(raw: &[u8]) -> Vec<Vec<u8>> {
    if raw.len() < 8 {
        return Vec::new();
    }
    let item = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    let count = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
    let mut reports = Vec::new();
    for i in 0..count {
        // a size and a count come from the device: no product of them may overflow
        let Some(from) = i.checked_mul(item).and_then(|n| n.checked_add(8)) else { break };
        let Some(to) = from.checked_add(item) else { break };
        if item == 0 || to > raw.len() {
            break;
        }
        reports.push(raw[from..to].to_vec());
    }
    reports
}

/// The device and the HID reports of the bytes `GetRawInputData` gave: a `RAWINPUTHEADER` and then the `RAWHID` structure.
/// `None` for a buffer that is shorter than the header (the header is not read then), for input of another kind (mouse,
/// keyboard) and for one without a report.
fn parse_raw_input(bytes: &[u8]) -> Option<(*mut core::ffi::c_void, Vec<Vec<u8>>)> {
    let header = std::mem::size_of::<RAWINPUTHEADER>();
    if bytes.len() < header {
        return None;
    }
    // SAFETY: the buffer holds a whole header (checked above); the read copies it and needs no alignment
    let hdr = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<RAWINPUTHEADER>()) };
    if hdr.dwType != RIM_TYPEHID {
        return None;
    }
    Some((hdr.hDevice, extract_reports(&bytes[header..])))
}

fn handle_input(lparam: LPARAM) {
    let Some(shared) = SHARED.get() else { return };
    // SAFETY: GetRawInputData fills a buffer of the size it reported
    let (device, reports) = unsafe {
        let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
        let h = lparam as HRAWINPUT;
        let mut size = 0u32;
        if GetRawInputData(h, RID_INPUT, std::ptr::null_mut(), &mut size, header) != 0 || size == 0 {
            return;
        }
        // 8-byte aligned storage for the header structure
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        if GetRawInputData(h, RID_INPUT, buf.as_mut_ptr().cast(), &mut size, header) != size {
            return;
        }
        let bytes = std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size as usize);
        let Some(input) = parse_raw_input(bytes) else { return };
        input
    };
    let Ok(mut s) = shared.lock() else { return };
    if !s.active {
        return;
    }
    let key = device as usize;
    let pid = *s.pids.entry(key).or_insert_with(|| device_ids(device).map(|(_, p, _, _)| p));
    for r in reports {
        let out = apply_report(&mut s.state, &r);
        if s.enabled && pid.is_some_and(|p| VK_LAYOUT.contains(&p)) {
            for bit in out.pressed {
                if let Some(a) = button_action(bit) {
                    s.actions.push(a);
                }
            }
        }
    }
    drop(s);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        handle_input(lparam);
    }
    // SAFETY: forwarding an unhandled message to the default procedure
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn listener_thread() {
    // SAFETY: standard window class / message loop of a message-only window
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("StepViewerSpaceMouse");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        // asks Windows for the reports of all multi-axis controllers (also ones plugged in later)
        let rid = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 8, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        if RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) == 0 {
            return;
        }
        if let Some(s) = SHARED.get() {
            if let Ok(mut s) = s.lock() {
                s.registered = true;
            }
        }
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Starts the listener thread (once) and returns the shared state.
pub fn start(ctx: &eframe::egui::Context) -> Option<Arc<Mutex<Shared>>> {
    if let Some(s) = SHARED.get() {
        return Some(s.clone());
    }
    let shared = Arc::new(Mutex::new(Shared { enabled: true, ..Shared::default() }));
    let _ = SHARED.set(shared.clone());
    let _ = CTX.set(ctx.clone());
    std::thread::Builder::new().name("spacemouse".into()).spawn(listener_thread).ok()?;
    Some(shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_cut_from_the_raw_hid_structure() {
        // two reports of 7 bytes
        let mut raw = Vec::new();
        raw.extend_from_slice(&7u32.to_le_bytes());
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&[1, 10, 0, 20, 0, 30, 0]);
        raw.extend_from_slice(&[3, 1, 0, 0, 0, 0, 0]);
        let r = extract_reports(&raw);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], vec![1, 10, 0, 20, 0, 30, 0]);
        assert_eq!(r[1][0], 3);
        // a count that does not fit, an empty report size and a short buffer give what is complete
        raw.truncate(8 + 7 + 3);
        assert_eq!(extract_reports(&raw).len(), 1);
        assert!(extract_reports(&[0; 8]).is_empty());
        assert!(extract_reports(&[1, 2, 3]).is_empty());
    }

    /// A `RAWINPUT` buffer as Windows gives it: the header, then `RAWHID` (report size, count, the reports).
    fn raw_input(kind: u32, device: usize, hid: &[u8]) -> Vec<u8> {
        let hdr = RAWINPUTHEADER {
            dwType: kind,
            dwSize: (std::mem::size_of::<RAWINPUTHEADER>() + hid.len()) as u32,
            hDevice: device as *mut core::ffi::c_void,
            wParam: 0,
        };
        // SAFETY: a plain-data struct viewed as its bytes
        let mut bytes = unsafe { std::slice::from_raw_parts((&hdr as *const RAWINPUTHEADER).cast::<u8>(), std::mem::size_of::<RAWINPUTHEADER>()) }.to_vec();
        bytes.extend_from_slice(hid);
        bytes
    }

    #[test]
    fn a_raw_input_buffer_shorter_than_its_header_is_refused() {
        let header = std::mem::size_of::<RAWINPUTHEADER>();
        let mut hid = Vec::new();
        hid.extend_from_slice(&7u32.to_le_bytes());
        hid.extend_from_slice(&1u32.to_le_bytes());
        hid.extend_from_slice(&[1, 10, 0, 20, 0, 30, 0]);
        let whole = raw_input(RIM_TYPEHID, 0x1234, &hid);
        let (device, reports) = parse_raw_input(&whole).expect("a whole buffer");
        assert_eq!(device as usize, 0x1234);
        assert_eq!(reports, vec![vec![1, 10, 0, 20, 0, 30, 0]]);
        // every length below the header: nothing is read, nothing is made of it
        for len in 0..header {
            assert!(parse_raw_input(&whole[..len]).is_none(), "{len} bytes");
        }
        // exactly the header: input of the right kind with no report in it
        assert_eq!(parse_raw_input(&whole[..header]).map(|(_, r)| r.len()), Some(0));
        // a mouse or a keyboard is not a controller
        assert!(parse_raw_input(&raw_input(0, 1, &hid)).is_none());
        assert!(parse_raw_input(&raw_input(1, 1, &hid)).is_none());
    }

    #[test]
    fn report_sizes_that_overflow_give_no_report_and_no_panic() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&u32::MAX.to_le_bytes());
        raw.extend_from_slice(&u32::MAX.to_le_bytes());
        raw.extend_from_slice(&[0; 64]);
        assert!(extract_reports(&raw).is_empty());
    }

    /// Creates the real message-only window and registers for the raw input of multi-axis controllers.
    #[test]
    fn the_listener_registers_for_raw_input() {
        let ctx = eframe::egui::Context::default();
        let shared = start(&ctx).expect("listener");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !shared.lock().unwrap().registered {
            assert!(std::time::Instant::now() < end, "RegisterRawInputDevices did not succeed");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // a second start hands out the same state
        assert!(std::sync::Arc::ptr_eq(&shared, &start(&ctx).unwrap()));
    }
}
