#![windows_subsystem = "windows"]
//! DS3 Pause: a pause button for Dark Souls III (or any single-player game).
//!
//! It suspends every thread of the game process when you press the hotkey and
//! resumes them when you press it again. It does not read or write game memory
//! and does not touch game files. Offline / single-player only.
//!
//! It has no window of its own: status goes to `DS3 Pause.log` next to the exe.

mod audio;
mod net;
mod sessionwin;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::Beep;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SUSPEND_RESUME,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const STILL_ACTIVE: u32 = 259;
/// How long the controller combo must be held before releasing it counts.
const PAD_HOLD_MS: u128 = 250;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Local time as HH:MM:SS.mmm, for log lines.
fn timestamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

/// Append a timestamped line to DS3 Pause.log (there is no console).
macro_rules! log {
    ($($a:tt)*) => {{
        if let Ok(mut g) = LOG.lock() {
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "[{}] {}", timestamp(), format_args!($($a)*));
            }
        }
    }};
}

type NtProcFn = unsafe extern "system" fn(HANDLE) -> i32;

/// Handle of the currently suspended process (0 = nothing suspended).
static SUSPENDED: AtomicIsize = AtomicIsize::new(0);
static RESUME_PTR: AtomicIsize = AtomicIsize::new(0);
static SUSPEND_COUNT: AtomicU32 = AtomicU32::new(0);
/// Set when Windows is shutting down / restarting / logging off.
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Audio sessions muted for the current pause (main thread only).
    static MUTED: std::cell::RefCell<Option<audio::Muted>> = const { std::cell::RefCell::new(None) };
}

fn set_muted(m: audio::Muted) {
    MUTED.with(|c| *c.borrow_mut() = Some(m));
}

fn restore_audio() {
    if let Some(m) = MUTED.with(|c| c.borrow_mut().take()) {
        m.restore();
    }
}

/// Shutdown guard: give the game back its threads and sound so it can close
/// (and finish any save) normally instead of being killed while frozen.
fn on_end_session() {
    SESSION_ENDING.store(true, Ordering::SeqCst);
    resume_now();
    restore_audio();
}

struct Config {
    process: String,
    key: i32,
    key_name: String,
    controller: bool,
    pad_mask: u16,
    pad_name: String,
    only_when_focused: bool,
    beep: bool,
}


const DEFAULT_INI: &str = "\
; DS3 Pause settings. Restart DS3 Pause after editing.

[Settings]
; Name of the game executable to pause.
Process=DarkSoulsIII.exe

; Keyboard key that toggles pause. Names: PAUSE, F1..F12, A..Z, 0..9,
; INSERT, HOME, END, PAGEUP, PAGEDOWN, SCROLLLOCK, NUMPAD0..NUMPAD9,
; or a virtual-key code like 0x13.
Key=PAUSE

; Gamepad (XInput) combo that toggles pause. Hold all buttons, then release.
; Buttons: A B X Y LB RB BACK START L3 R3 UP DOWN LEFT RIGHT. Join with +.
Controller=true
PadCombo=BACK+START

; true = pausing only works while the game window is in front
; (resuming always works). Stops accidental pauses while alt-tabbed.
OnlyWhenFocused=true

; Beep when pausing (high) and resuming (low). Off by default.
Beep=false

";

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn parse_key(s: &str) -> Option<i32> {
    let u = s.trim().to_ascii_uppercase();
    if let Some(hex) = u.strip_prefix("0X") {
        return i32::from_str_radix(hex, 16).ok();
    }
    if u.len() == 1 {
        let c = u.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as i32);
        }
    }
    if let Some(n) = u.strip_prefix('F').and_then(|n| n.parse::<i32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = u.strip_prefix("NUMPAD").and_then(|n| n.parse::<i32>().ok()) {
        if (0..=9).contains(&n) {
            return Some(0x60 + n);
        }
    }
    Some(match u.as_str() {
        "PAUSE" | "BREAK" => 0x13,
        "SCROLLLOCK" | "SCROLL" => 0x91,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "TAB" => 0x09,
        "BACKSPACE" => 0x08,
        _ => return None,
    })
}

fn parse_pad(s: &str) -> Option<u16> {
    let mut mask = 0u16;
    for part in s.split('+') {
        mask |= match part.trim().to_ascii_uppercase().as_str() {
            "UP" => 0x0001,
            "DOWN" => 0x0002,
            "LEFT" => 0x0004,
            "RIGHT" => 0x0008,
            "START" | "MENU" => 0x0010,
            "BACK" | "VIEW" | "SELECT" => 0x0020,
            "L3" | "LS" => 0x0040,
            "R3" | "RS" => 0x0080,
            "LB" | "L1" => 0x0100,
            "RB" | "R1" => 0x0200,
            "A" => 0x1000,
            "B" => 0x2000,
            "X" => 0x4000,
            "Y" => 0x8000,
            "" => 0,
            _ => return None,
        };
    }
    (mask != 0).then_some(mask)
}

fn load_config() -> Config {
    let path = exe_dir().join("DS3 Pause.ini");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            let _ = std::fs::write(&path, DEFAULT_INI);
            DEFAULT_INI.to_string()
        }
    };
    let mut cfg = Config {
        process: "DarkSoulsIII.exe".into(),
        key: 0x13,
        key_name: "PAUSE".into(),
        controller: true,
        pad_mask: 0x0030,
        pad_name: "BACK+START".into(),
        only_when_focused: true,
        beep: false,
    };
    let as_bool = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on");
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "process" if !v.is_empty() => cfg.process = v.to_string(),
            "key" => match parse_key(v) {
                Some(code) => {
                    cfg.key = code;
                    cfg.key_name = v.to_string();
                }
                None => log!("! Unknown Key '{v}', using PAUSE"),
            },
            "controller" => cfg.controller = as_bool(v),
            "padcombo" => match parse_pad(v) {
                Some(m) => {
                    cfg.pad_mask = m;
                    cfg.pad_name = v.to_string();
                }
                None => log!("! Unknown PadCombo '{v}', using BACK+START"),
            },
            "onlywhenfocused" => cfg.only_when_focused = as_bool(v),
            "beep" => cfg.beep = as_bool(v),
            _ => {}
        }
    }
    cfg
}

fn find_pid(name: &str) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let exe = String::from_utf16_lossy(&e.szExeFile[..len]);
            if exe.eq_ignore_ascii_case(name) {
                found = Some(e.th32ProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
        found
    }
}

fn beep(on: bool, freq: u32, ms: u32) {
    if on {
        unsafe { Beep(freq, ms) };
    }
}

fn resume_now() {
    let h = SUSPENDED.swap(0, Ordering::SeqCst);
    let f = RESUME_PTR.load(Ordering::SeqCst);
    if h != 0 && f != 0 {
        let resume: NtProcFn = unsafe { std::mem::transmute(f) };
        // Undo every suspend we applied, in case of repeats.
        let n = SUSPEND_COUNT.swap(0, Ordering::SeqCst).max(1);
        for _ in 0..n {
            unsafe { resume(h as HANDLE) };
        }
    }
}

fn main() {
    let cfg = load_config();

    // `--pid N` (from the dinput8.dll loader): attach to exactly that game
    // process. Without it (run by hand), find the game by name.
    let args: Vec<String> = std::env::args().collect();
    let target_pid: Option<u32> = args
        .iter()
        .position(|a| a == "--pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());

    // Single instance: two copies would fight over the pause key and the
    // mute state. Launched by the loader, wait a few seconds for an older copy
    // that is still exiting (e.g. its game just closed or relaunched).
    let mutex_name: Vec<u16> = "Local\\DS3PauseSingleInstance\0".encode_utf16().collect();
    let mut tries = if target_pid.is_some() { 30 } else { 1 };
    let _mutex = loop {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let m = unsafe { windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            break m;
        }
        unsafe { CloseHandle(m) };
        tries -= 1;
        if tries == 0 {
            return; // another copy is running
        }
        std::thread::sleep(Duration::from_millis(500));
    };

    if let Ok(f) = std::fs::File::create(exe_dir().join("DS3 Pause.log")) {
        *LOG.lock().unwrap() = Some(f);
    }

    audio::init();
    let mute_marker = exe_dir().join("DS3 Pause.muted");
    let mut audio_recovered = true;
    let mut last_audio_check = Instant::now();

    // Hidden window so the shutdown guard hears Windows ending the session.
    let session_window = sessionwin::SessionWindow::new();

    let ntdll = unsafe { GetModuleHandleA(b"ntdll.dll\0".as_ptr()) };
    let sus = unsafe { GetProcAddress(ntdll, b"NtSuspendProcess\0".as_ptr()) };
    let res = unsafe { GetProcAddress(ntdll, b"NtResumeProcess\0".as_ptr()) };
    let (Some(sus), Some(res)) = (sus, res) else {
        log!("Could not find NtSuspendProcess/NtResumeProcess in ntdll.");
        return;
    };
    let suspend: NtProcFn = unsafe { std::mem::transmute(sus) };
    RESUME_PTR.store(res as isize, Ordering::SeqCst);
    let _ = sessionwin::END_SESSION_HOOK.set(on_end_session);
    // Ask Windows to notify us before ordinary apps (like the game) at
    // shutdown, so the game is resumed before Windows asks it to close.
    unsafe { windows_sys::Win32::System::Threading::SetProcessShutdownParameters(0x3FF, 0) };

    log!("DS3 Pause {}  -  offline only (pausing is refused while DS3 is online)", env!("CARGO_PKG_VERSION"));
    log!("  Game:       {}", cfg.process);
    log!("  Keyboard:   {}", cfg.key_name);
    if cfg.controller {
        log!("  Controller: hold {} then release", cfg.pad_name);
    }
    log!("  Settings:   {}", exe_dir().join("DS3 Pause.ini").display());
    log!("Closes by itself when the game closes.\n");

    let mut pid: Option<u32> = None;
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut last_scan = Instant::now() - Duration::from_secs(10);
    let mut key_was_down = false;
    let mut pad_held_since: Option<Instant> = None; // full combo held since, waiting for release
    let mut paused_at: Option<Instant> = None;
    // Sticky: once the game is seen online, pausing stays off for the rest of
    // this game session (DS3 Pause exits with the game, so it resets).
    let mut online = false;
    let mut last_net_check = Instant::now();
    let mut open_failures = 0u32;

    log!("Waiting for {} ...", cfg.process);

    loop {
        std::thread::sleep(Duration::from_millis(10));
        session_window.pump();
        if SESSION_ENDING.load(Ordering::SeqCst) {
            log!("Windows is shutting down or logging off: game resumed and un-muted.");
            return;
        }

        // --- track the game process ---------------------------------------
        if !handle.is_null() {
            let mut code = 0u32;
            let alive = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE;
            if !alive {
                log!("Game closed.");
                restore_audio();
                // No window to close, so always quit with the game.
                return;
            }
        }
        if handle.is_null() && last_scan.elapsed() >= Duration::from_secs(1) {
            last_scan = Instant::now();
            if let Some(p) = target_pid.or_else(|| find_pid(&cfg.process)) {
                let h = unsafe {
                    OpenProcess(PROCESS_SUSPEND_RESUME | PROCESS_QUERY_LIMITED_INFORMATION, 0, p)
                };
                if h.is_null() {
                    if target_pid.is_some() {
                        open_failures += 1;
                        if open_failures >= 10 {
                            log!("Game process {p} is gone or inaccessible; exiting.");
                            return;
                        }
                    }
                    log!("Found the game (pid {p}) but access was denied. Run DS3 Pause as administrator.");
                    beep(cfg.beep, 300, 120);
                    beep(cfg.beep, 300, 120);
                    last_scan = Instant::now() + Duration::from_secs(2); // retry in ~3 s
                } else {
                    // Recovery: if a previous DS3 Pause was force-killed while the
                    // game was paused, the game is still frozen. Resuming a
                    // process that is not suspended does nothing, so this is safe.
                    let resume: NtProcFn = unsafe { std::mem::transmute(res) };
                    unsafe { resume(h) };
                    pid = Some(p);
                    handle = h;
                    let had_marker = mute_marker.exists();
                    audio_recovered = audio::recover(p, &mute_marker);
                    if had_marker && audio_recovered {
                        log!("Un-muted the game (left muted by an earlier run).");
                    }
                    log!("Attached to {} (pid {p}). Ready.", cfg.process);
                }
            }
        }
        // A previous run died while the game was muted: un-mute it once its
        // audio session shows up.
        if !audio_recovered && last_audio_check.elapsed() >= Duration::from_secs(1) {
            last_audio_check = Instant::now();
            if let Some(p) = pid {
                audio_recovered = audio::recover(p, &mute_marker);
                if audio_recovered {
                    log!("Un-muted the game (left muted by an earlier run).");
                }
            }
        }

        // --- watch for the game going online (every second) ---------------
        if !online && paused_at.is_none() && last_net_check.elapsed() >= Duration::from_secs(1) {
            last_net_check = Instant::now();
            if let Some(p) = pid {
                if let Ok(signs) = net::online_signs(p) {
                    if !signs.is_empty() {
                        online = true;
                        log!("DS3 is online ({}). Pausing disabled until the game restarts offline.", signs.join(", "));
                    }
                }
            }
        }

        // --- read inputs (edge-triggered) ----------------------------------
        let key_down = unsafe { GetAsyncKeyState(cfg.key) } as u16 & 0x8000 != 0;
        let mut source: Option<&str> = (key_down && !key_was_down).then_some("keyboard");
        key_was_down = key_down;

        if cfg.controller {
            let mut held = false;
            for i in 0..4u32 {
                let mut st: XINPUT_STATE = unsafe { std::mem::zeroed() };
                if unsafe { XInputGetState(i, &mut st) } == 0
                    && st.Gamepad.wButtons & cfg.pad_mask == cfg.pad_mask
                {
                    held = true;
                }
            }
            // Fire on release so the game never sees the buttons still held
            // the moment it wakes up. The combo must be held a moment first,
            // so a split-second blip (Steam Input, a controller hiccup, a
            // brushed button) can't pause the game by accident.
            if held {
                pad_held_since.get_or_insert_with(Instant::now);
            } else if let Some(since) = pad_held_since.take() {
                let ms = since.elapsed().as_millis();
                if ms >= PAD_HOLD_MS {
                    source = Some("controller");
                } else {
                    log!("Ignored controller combo: held only {ms} ms (needs {PAD_HOLD_MS} ms).");
                }
            }
        }

        let Some(source) = source else { continue };
        let Some(p) = pid else { continue };

        // --- toggle ---------------------------------------------------------
        if paused_at.is_none() {
            if cfg.only_when_focused {
                let mut fg_pid = 0u32;
                unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut fg_pid) };
                if fg_pid != p {
                    continue;
                }
            }
            // Offline-only: never freeze a game that is (or was) online.
            if online {
                log!("Ignored pause: DS3 went online this session.");
                continue;
            }
            match net::online_signs(p) {
                Ok(signs) if signs.is_empty() => {}
                Ok(signs) => {
                    online = true;
                    log!("Ignored pause: DS3 is online ({}). Disabled until the game restarts offline.", signs.join(", "));
                    continue;
                }
                Err(()) => {
                    // Can't prove we're offline, so don't pause.
                    log!("Ignored pause: couldn't confirm DS3 is offline.");
                    continue;
                }
            }
            // Mute first so no sound keeps looping while the game is frozen.
            let m = audio::mute(p, mute_marker.clone());
            log!("Muted {} audio session(s).", m.count());
            if m.count() > 0 {
                std::thread::sleep(Duration::from_millis(40)); // let the mixer apply it
            }
            set_muted(m);
            let status = unsafe { suspend(handle) };
            if status >= 0 {
                SUSPENDED.store(handle as isize, Ordering::SeqCst);
                SUSPEND_COUNT.fetch_add(1, Ordering::SeqCst);
                paused_at = Some(Instant::now());
                log!("PAUSED ({source})");
                beep(cfg.beep, 1200, 90);
            } else {
                log!("Pause failed (NTSTATUS 0x{:08X}).", status as u32);
                restore_audio();
                beep(cfg.beep, 300, 80);
            }
        } else {
            resume_now();
            restore_audio();
            let secs = paused_at.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
            log!("RESUMED ({source}) after {secs:.1}s");
            beep(cfg.beep, 600, 90);
        }
    }
}
