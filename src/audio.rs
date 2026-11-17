//! Silence the game while it's paused.
//!
//! Suspending the game stops its sound code, but depending on the audio driver
//! the last chunk of sound can keep looping. So before freezing, we mute the
//! game's audio sessions in the Windows volume mixer, and restore each one's
//! previous mute state after resuming.
//!
//! Crash safety: while muted, a marker file sits next to the exe. If DS3 Pause
//! dies mid-pause, Windows may remember DS3 as muted; the next run sees the
//! marker and un-mutes the game as soon as its audio session exists.

use std::path::PathBuf;

use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Every audio session (on every output device) that belongs to `pid`.
fn sessions_of(pid: u32) -> Vec<ISimpleAudioVolume> {
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return out;
        };
        let Ok(devices) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        let n = devices.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(dev) = devices.Item(i) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = mgr.GetSessionEnumerator() else { continue };
            let m = list.GetCount().unwrap_or(0);
            for j in 0..m {
                let Ok(ctl) = list.GetSession(j) else { continue };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if ctl2.GetProcessId().ok() == Some(pid) {
                    if let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() {
                        out.push(vol);
                    }
                }
            }
        }
    }
    out
}

/// Sessions we muted, with the mute state each had before.
pub struct Muted {
    sessions: Vec<(ISimpleAudioVolume, bool)>,
    marker: PathBuf,
}

pub fn mute(pid: u32, marker: PathBuf) -> Muted {
    let mut sessions = Vec::new();
    for vol in sessions_of(pid) {
        unsafe {
            let was = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            if vol.SetMute(true, std::ptr::null()).is_ok() {
                sessions.push((vol, was));
            }
        }
    }
    if !sessions.is_empty() {
        let _ = std::fs::write(&marker, b"DS3 Pause muted the game; it will be un-muted on next run.\n");
    }
    Muted { sessions, marker }
}

impl Muted {
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    pub fn restore(self) {
        for (vol, was) in &self.sessions {
            unsafe {
                let _ = vol.SetMute(*was, std::ptr::null());
            }
        }
        let _ = std::fs::remove_file(&self.marker);
    }
}

/// Crash recovery: un-mute the game if a previous run left it muted.
/// Returns true once handled (no marker, or sessions found and un-muted).
pub fn recover(pid: u32, marker: &PathBuf) -> bool {
    if !marker.exists() {
        return true;
    }
    let sessions = sessions_of(pid);
    if sessions.is_empty() {
        return false; // game hasn't opened its audio yet; try again later
    }
    for vol in sessions {
        unsafe {
            let _ = vol.SetMute(false, std::ptr::null());
        }
    }
    let _ = std::fs::remove_file(marker);
    true
}
