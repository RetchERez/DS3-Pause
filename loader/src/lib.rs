//! DS3 Pause drop-in loader (`dinput8.dll`).
//!
//! Dark Souls III loads `dinput8.dll` from its own folder before the system
//! one. This stand-in does two things:
//!   1. Forwards every DirectInput export to Windows' real
//!      `System32\dinput8.dll`, so input works exactly as before.
//!   2. Starts `DS3 Pause.exe` (from the same folder) in the background with
//!      `--pid <this game process>`.
//!
//! The pause itself must live in a separate process: it freezes every thread
//! of the game, and code inside the game would freeze along with it.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HMODULE, MAX_PATH};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CreateThread, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE;

const DLL_PROCESS_ATTACH: u32 = 1;
const E_FAIL: i32 = 0x80004005u32 as i32;
const S_FALSE: i32 = 1;
const PAUSE_EXE: &str = "DS3 Pause.exe";

static SELF_MODULE: OnceLock<usize> = OnceLock::new();
static REAL: OnceLock<usize> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows' real dinput8.dll, loaded on first use (never from DllMain).
fn real_proc(name: &[u8]) -> Option<usize> {
    let module = *REAL.get_or_init(|| unsafe {
        let mut buf = [0u16; MAX_PATH as usize];
        let n = GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) as usize;
        if n == 0 || n >= buf.len() {
            return 0;
        }
        let mut path = String::from_utf16_lossy(&buf[..n]);
        path.push_str("\\dinput8.dll");
        LoadLibraryW(wide(&path).as_ptr()) as usize
    });
    if module == 0 {
        return None;
    }
    unsafe { GetProcAddress(module as HMODULE, name.as_ptr()).map(|f| f as usize) }
}

// ---- forwarded exports -------------------------------------------------------

type DirectInput8CreateFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
type NoArgFn = unsafe extern "system" fn() -> i32;
type GetClassObjectFn = unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> i32;

#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    hinst: *mut c_void,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    match real_proc(b"DirectInput8Create\0") {
        Some(f) => std::mem::transmute::<usize, DirectInput8CreateFn>(f)(hinst, version, riid, out, outer),
        None => E_FAIL,
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> i32 {
    match real_proc(b"DllCanUnloadNow\0") {
        Some(f) => std::mem::transmute::<usize, NoArgFn>(f)(),
        None => S_FALSE,
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> i32 {
    match real_proc(b"DllGetClassObject\0") {
        Some(f) => std::mem::transmute::<usize, GetClassObjectFn>(f)(clsid, iid, out),
        None => E_FAIL,
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> i32 {
    match real_proc(b"DllRegisterServer\0") {
        Some(f) => std::mem::transmute::<usize, NoArgFn>(f)(),
        None => E_FAIL,
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> i32 {
    match real_proc(b"DllUnregisterServer\0") {
        Some(f) => std::mem::transmute::<usize, NoArgFn>(f)(),
        None => E_FAIL,
    }
}

// ---- starting DS3 Pause ------------------------------------------------------

unsafe extern "system" fn start_pause_tool(_: *mut c_void) -> u32 {
    let Some(&module) = SELF_MODULE.get() else { return 0 };
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(module as HMODULE, buf.as_mut_ptr(), buf.len() as u32) as usize;
    if n == 0 || n >= buf.len() {
        return 0;
    }
    let dll_path = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    let Some(dir) = dll_path.parent() else { return 0 };
    let exe = dir.join(PAUSE_EXE);
    if !exe.is_file() {
        return 0;
    }
    let exe_s = exe.to_string_lossy().into_owned();
    // Tell DS3 Pause exactly which process is the game (this one), so it never
    // attaches to a short-lived second DarkSoulsIII.exe. It handles the
    // single-instance hand-over itself.
    let mut cmd = wide(&format!("\"{exe_s}\" --pid {}", std::process::id()));
    let dir_w = wide(&dir.to_string_lossy());
    let mut si: STARTUPINFOW = std::mem::zeroed();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_SHOWMINNOACTIVE as u16; // minimized, never steals focus
    let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
    let ok = CreateProcessW(
        wide(&exe_s).as_ptr(),
        cmd.as_mut_ptr(),
        std::ptr::null(),
        std::ptr::null(),
        0,
        0,
        std::ptr::null(),
        dir_w.as_ptr(),
        &si,
        &mut pi,
    );
    if ok != 0 {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    0
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(module: HMODULE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        let _ = SELF_MODULE.set(module as usize);
        // Runs once the loader lock is released; keeps DllMain trivial.
        let h = CreateThread(std::ptr::null(), 0, Some(start_pause_tool), std::ptr::null(), 0, std::ptr::null_mut());
        if !h.is_null() {
            CloseHandle(h);
        }
    }
    1
}
