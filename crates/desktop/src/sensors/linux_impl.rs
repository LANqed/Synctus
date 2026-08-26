//! Linux sensors.
//!
//! Everything here is best-effort and free of hard dependencies on a desktop
//! environment:
//!
//! * foreground window — X11 `_NET_ACTIVE_WINDOW`; returns `None` on Wayland,
//!   which has no equivalent by design.
//! * battery — sysfs (`/sys/class/power_supply`), no D-Bus needed.
//! * music — MPRIS over D-Bus, which every mainstream player implements.
//! * idle — X11 ScreenSaver extension.

use std::path::Path;
use synctus_core::model::{Battery, ForegroundApp, NowPlaying};

pub fn foreground() -> Option<ForegroundApp> {
    x11::active_window()
}

pub fn idle_secs() -> Option<u32> {
    x11::idle_secs()
}

/// Read battery state from sysfs.
///
/// Laptops expose `BAT0`/`BAT1`; the first battery present wins, which is right
/// for the overwhelming majority of machines.
pub fn battery() -> Option<Battery> {
    let base = Path::new("/sys/class/power_supply");
    let entries = std::fs::read_dir(base).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        // Skip AC adapters and anything that is not a battery.
        if read_trimmed(&path.join("type")).as_deref() != Some("Battery") {
            continue;
        }

        let Some(percent) =
            read_trimmed(&path.join("capacity")).and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };

        let status = read_trimmed(&path.join("status")).unwrap_or_default();
        // "Charging", "Full" and "Not charging" all mean mains power is present;
        // only "Discharging" means running on the battery.
        let charging = status != "Discharging";

        // `time_to_empty_now` is in seconds on some kernels and absent on many;
        // treat a missing file as "unknown" rather than guessing.
        let minutes_left = read_trimmed(&path.join("time_to_empty_now"))
            .and_then(|s| s.parse::<u32>().ok())
            .map(|secs| secs / 60);

        return Some(Battery {
            percent: percent.min(100) as u8,
            charging,
            minutes_left,
        });
    }
    None
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Query the first MPRIS player that reports something.
pub fn music() -> Option<NowPlaying> {
    mpris::now_playing()
}

/// X11 access through `x11rb`.
///
/// A connection is opened per sample rather than cached: sampling happens every
/// few seconds, and holding a connection across a session restart or DE crash
/// would leave a dead socket that silently stops reporting.
mod x11 {
    use super::ForegroundApp;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};

    pub fn active_window() -> Option<ForegroundApp> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen_num)?.root;

        let active_atom = intern(&conn, b"_NET_ACTIVE_WINDOW")?;
        let window = get_window_property(&conn, root, active_atom)?;
        if window == 0 {
            return None;
        }

        let title = window_title(&conn, window);
        let class = window_class(&conn, window);

        let app = class.clone().or_else(|| title.clone())?;
        Some(ForegroundApp {
            app,
            name: class,
            title: title.as_deref().and_then(crate::sensors::trim_title),
        })
    }

    fn intern(conn: &impl Connection, name: &[u8]) -> Option<u32> {
        conn.intern_atom(true, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    }

    fn get_window_property(conn: &impl Connection, window: Window, atom: u32) -> Option<Window> {
        let reply = conn
            .get_property(false, window, atom, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        // `value32` borrows `reply`, so the id is copied out before it drops.
        let id = reply.value32()?.next()?;
        Some(id)
    }

    /// Prefer `_NET_WM_NAME` (UTF-8) and fall back to the legacy `WM_NAME`.
    fn window_title(conn: &impl Connection, window: Window) -> Option<String> {
        if let Some(utf8) = intern(conn, b"UTF8_STRING") {
            if let Some(net_name) = intern(conn, b"_NET_WM_NAME") {
                if let Some(s) = text_property(conn, window, net_name, utf8) {
                    return Some(s);
                }
            }
        }
        text_property(
            conn,
            window,
            AtomEnum::WM_NAME.into(),
            AtomEnum::STRING.into(),
        )
    }

    /// `WM_CLASS` holds `instance\0class\0`; the class half is the friendlier
    /// name (`firefox` rather than `Navigator`).
    fn window_class(conn: &impl Connection, window: Window) -> Option<String> {
        let raw = conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        let text = String::from_utf8_lossy(&raw.value);
        let mut parts = text.split('\0').filter(|s| !s.is_empty());
        let instance = parts.next()?.to_string();
        Some(parts.next().map(str::to_string).unwrap_or(instance))
    }

    fn text_property(
        conn: &impl Connection,
        window: Window,
        property: u32,
        kind: u32,
    ) -> Option<String> {
        let reply = conn
            .get_property(false, window, property, kind, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if reply.value.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&reply.value).to_string())
    }

    pub fn idle_secs() -> Option<u32> {
        use x11rb::protocol::screensaver::ConnectionExt as _;

        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen_num)?.root;
        let info = conn.screensaver_query_info(root).ok()?.reply().ok()?;
        Some(info.ms_since_user_input / 1000)
    }
}

/// MPRIS over D-Bus.
///
/// Implemented with raw `zbus` proxy calls rather than the `mpris` crate to avoid
/// pulling in a second D-Bus stack, and to keep the failure mode simple: anything
/// unexpected becomes `None`.
mod mpris {
    use super::NowPlaying;
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::names::OwnedBusName;
    use zbus::zvariant::{OwnedValue, Value};

    pub fn now_playing() -> Option<NowPlaying> {
        let conn = Connection::session().ok()?;

        for name in players(&conn)? {
            if let Some(np) = read_player(&conn, name.as_str()) {
                // First player with a title wins. Users rarely run two at once,
                // and picking deterministically beats merging.
                return Some(np);
            }
        }
        None
    }

    /// All bus names under `org.mpris.MediaPlayer2.`.
    fn players(conn: &Connection) -> Option<Vec<OwnedBusName>> {
        let proxy = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
        let names = proxy.list_names().ok()?;
        Some(
            names
                .into_iter()
                .filter(|n| n.as_str().starts_with("org.mpris.MediaPlayer2."))
                .collect(),
        )
    }

    fn read_player(conn: &Connection, bus_name: &str) -> Option<NowPlaying> {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            bus_name,
            "/org/mpris/MediaPlayer2",
            "org.freedesktop.DBus.Properties",
        )
        .ok()?;

        let metadata: HashMap<String, OwnedValue> = proxy
            .call("Get", &("org.mpris.MediaPlayer2.Player", "Metadata"))
            .ok()?;

        let title = string_of(metadata.get("xesam:title"))?;
        if title.trim().is_empty() {
            return None;
        }

        let status: String = proxy
            .call("Get", &("org.mpris.MediaPlayer2.Player", "PlaybackStatus"))
            .unwrap_or_else(|_| "Stopped".to_string());

        // Identity is the display name the player chose; fall back to the bus name
        // suffix.
        let player: Option<String> = proxy
            .call("Get", &("org.mpris.MediaPlayer2", "Identity"))
            .ok()
            .or_else(|| {
                bus_name
                    .strip_prefix("org.mpris.MediaPlayer2.")
                    .map(str::to_string)
            });

        Some(NowPlaying {
            title,
            artist: first_of(metadata.get("xesam:artist")),
            album: string_of(metadata.get("xesam:album")),
            player,
            playing: status == "Playing",
        })
    }

    /// `OwnedValue` derefs to `Value`, which is how the variant is inspected
    /// without cloning.
    fn string_of(value: Option<&OwnedValue>) -> Option<String> {
        match value.map(|v| &**v) {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// `xesam:artist` is an array of strings; take the first non-empty one.
    fn first_of(value: Option<&OwnedValue>) -> Option<String> {
        match value.map(|v| &**v) {
            Some(Value::Array(arr)) => arr.iter().find_map(|v| match v {
                Value::Str(s) if !s.is_empty() => Some(s.to_string()),
                _ => None,
            }),
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_reads_are_bounded_or_absent() {
        // CI containers have no battery, so absence is the expected result there.
        if let Some(b) = battery() {
            assert!(b.percent <= 100);
        }
    }

    #[test]
    fn queries_do_not_panic_without_a_display() {
        // No X11 or D-Bus session on a build agent; all of these must return
        // `None` instead of failing.
        let _ = foreground();
        let _ = idle_secs();
        let _ = music();
    }

    #[test]
    fn read_trimmed_handles_missing_files() {
        assert_eq!(read_trimmed(Path::new("/proc/self/nonexistent-xyz")), None);
    }
}
