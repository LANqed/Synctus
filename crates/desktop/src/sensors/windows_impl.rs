//! Windows sensors: Win32 for foreground window, battery and idle time, WinRT
//! (GSMTC) for the media session.
//!
//! The media query blocks on an async WinRT operation, so [`super::sample`] must
//! be called from a worker thread — which is how `main` drives it.

use synctus_core::model::{Battery, ForegroundApp, NowPlaying};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

pub fn foreground() -> Option<ForegroundApp> {
    // SAFETY: GetForegroundWindow takes no arguments and returns null when there
    // is no foreground window (e.g. during a lock screen).
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }

    let title = window_title(hwnd);
    let exe = process_name(hwnd);

    // Without either piece there is nothing worth publishing.
    let app = exe.clone().or_else(|| title.clone())?;
    Some(ForegroundApp {
        app,
        name: exe.and_then(|e| e.strip_suffix(".exe").map(str::to_string)),
        title: title.as_deref().and_then(super::trim_title),
    })
}

fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: `hwnd` is non-null and came from GetForegroundWindow. A window can
    // be destroyed between calls, in which case the length comes back as 0.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }

    // +1 for the terminating NUL that GetWindowTextW writes.
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: buffer is `len + 1` UTF-16 units, matching the length we pass.
    let written = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if written <= 0 {
        return None;
    }
    buf.truncate(written as usize);
    Some(String::from_utf16_lossy(&buf))
}

fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid: u32 = 0;
    // SAFETY: writing one u32 through a valid pointer.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }

    // QUERY_LIMITED_INFORMATION works without elevation for processes owned by
    // the same user, which is all we need.
    // SAFETY: standard OpenProcess call; the returned handle is checked and
    // always closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // Elevated or protected process: not an error, just not visible to us.
        return None;
    }

    let mut buf = [0u16; MAX_PATH as usize];
    let mut size = buf.len() as u32;
    // SAFETY: `size` describes `buf`; the function updates it to the length
    // actually written.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
    // SAFETY: `handle` is a valid, open handle we own.
    unsafe { CloseHandle(handle) };

    if ok == 0 || size == 0 {
        return None;
    }

    let full = String::from_utf16_lossy(&buf[..size as usize]);
    // Publish the file name only; the full path can leak a user name.
    full.rsplit(['\\', '/']).next().map(str::to_string)
}

pub fn battery() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS {
        ACLineStatus: 0,
        BatteryFlag: 0,
        BatteryLifePercent: 0,
        SystemStatusFlag: 0,
        BatteryLifeTime: 0,
        BatteryFullLifeTime: 0,
    };

    // SAFETY: writing into a fully initialised local struct.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }

    // 255 means "unknown", and bit 7 of BatteryFlag means "no battery" — a
    // desktop, where publishing 255% would be nonsense.
    if status.BatteryLifePercent == 255 || status.BatteryFlag & 0x80 != 0 {
        return None;
    }

    Some(Battery {
        percent: status.BatteryLifePercent.min(100),
        // ACLineStatus: 1 = on mains. Charging is implied while plugged in.
        charging: status.ACLineStatus == 1,
        // -1 (0xFFFFFFFF) means unknown.
        minutes_left: if status.BatteryLifeTime == u32::MAX {
            None
        } else {
            Some(status.BatteryLifeTime / 60)
        },
    })
}

pub fn idle_secs() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: cbSize is set as the API requires.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }

    // SAFETY: no preconditions.
    let now = unsafe { GetTickCount() };
    // Both values are millisecond tick counts that wrap every ~49 days;
    // wrapping_sub gives the correct delta across the wrap.
    Some(now.wrapping_sub(info.dwTime) / 1000)
}

/// Read the current media session through GSMTC.
///
/// Returns `None` when no app has registered a session, which is the normal
/// state when nothing is playing.
pub fn music() -> Option<NowPlaying> {
    // Any WinRT failure (missing session, app closing mid-query) is expected
    // rather than exceptional, so it collapses to `None`.
    media_session().ok().flatten()
}

fn media_session() -> windows::core::Result<Option<NowPlaying>> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    };

    // `join` blocks until the WinRT operation completes. Acceptable because this
    // runs on the dedicated sensor thread, never on the UI thread.
    let manager = SessionManager::RequestAsync()?.join()?;
    let session = match manager.GetCurrentSession() {
        Ok(s) => s,
        // No session at all.
        Err(_) => return Ok(None),
    };

    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = props.Title().map(|t| t.to_string()).unwrap_or_default();
    if title.trim().is_empty() {
        // A session with no title carries nothing worth showing.
        return Ok(None);
    }

    let artist = props
        .Artist()
        .map(|a| a.to_string())
        .ok()
        .filter(|s| !s.is_empty());
    let album = props
        .AlbumTitle()
        .map(|a| a.to_string())
        .ok()
        .filter(|s| !s.is_empty());

    let playing = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .map(|status| status == PlaybackStatus::Playing)
        .unwrap_or(false);

    // The AUMID is verbose (`Spotify.exe!Spotify`); keep the leading part, which
    // is the recognisable player name.
    let player = session
        .SourceAppUserModelId()
        .map(|id| id.to_string())
        .ok()
        .and_then(|id| {
            let short = id.split('!').next().unwrap_or(&id).to_string();
            let short = short.trim_end_matches(".exe").to_string();
            (!short.is_empty()).then_some(short)
        });

    Ok(Some(NowPlaying {
        title,
        artist,
        album,
        player,
        playing,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_percentage_is_in_range_or_absent() {
        // CI runners are desktops without a battery, so `None` is a valid result.
        if let Some(b) = battery() {
            assert!(b.percent <= 100);
        }
    }

    #[test]
    fn idle_time_is_available_in_an_interactive_session() {
        // On a headless build agent there may be no input desktop at all, so only
        // sanity-check the value when present.
        if let Some(secs) = idle_secs() {
            assert!(secs < 60 * 60 * 24 * 60, "implausible idle time: {secs}");
        }
    }

    #[test]
    fn foreground_and_music_queries_do_not_panic() {
        let _ = foreground();
        let _ = music();
    }
}
