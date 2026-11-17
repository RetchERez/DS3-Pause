//! A hidden window that only listens for Windows shutting down, restarting
//! or logging off (top-level windows receive WM_QUERYENDSESSION; console and
//! windowless processes don't). Used by the shutdown guard.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Called when Windows starts ending the session.
pub static END_SESSION_HOOK: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_QUERYENDSESSION || (m == WM_ENDSESSION && w != 0) {
        if let Some(f) = END_SESSION_HOOK.get() {
            f();
        }
        return if m == WM_QUERYENDSESSION { 1 } else { 0 }; // never block shutdown
    }
    DefWindowProcW(h, m, w, l)
}

pub struct SessionWindow {
    _hwnd: HWND,
}

impl SessionWindow {
    pub fn new() -> SessionWindow {
        unsafe {
            let inst = GetModuleHandleW(null());
            let class: Vec<u16> = "DS3PauseSession\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            // Never shown; WS_EX_TOOLWINDOW keeps it out of Alt+Tab.
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), class.as_ptr(), WS_POPUP,
                0, 0, 0, 0, null_mut(), null_mut(), inst, null());
            SessionWindow { _hwnd: hwnd }
        }
    }

    /// Deliver pending window messages (call regularly).
    pub fn pump(&self) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
