//! The admin socket: how `synctus` talks to a running relay.
//!
//! A Unix socket in the runtime directory, one JSON request per line, one JSON
//! response back. Deliberately minimal:
//!
//! * **Unix socket, not TCP.** Filesystem permissions are the authorisation
//!   model. A TCP admin port would need its own authentication, and getting that
//!   wrong on a public interface is far worse than the convenience is worth.
//! * **Read-only.** The socket reports state; it never changes configuration or
//!   stops anything. Restarts go through the service manager, which is what
//!   actually owns the process lifecycle. That way there is no second, weaker
//!   path to controlling the daemon.
//!
//! On platforms without Unix sockets the socket is simply not created and the
//! management tool falls back to reading the service manager's own status.

use serde::{Deserialize, Serialize};

/// Where the socket lives.
///
/// Under `/run` because the socket is runtime state that should vanish on reboot;
/// systemd's `RuntimeDirectory=` creates and cleans it up automatically.
pub const DEFAULT_SOCKET: &str = "/run/synctus/admin.sock";

/// What the management tool asks for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Version, uptime, room and device counts.
    Status,
    /// One line per connected device, for the "who is connected" view.
    Rooms,
    /// Everything at once, for the WebUI.
    Snapshot,
}

/// What the daemon answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Response {
    Status(Status),
    Rooms {
        rooms: Vec<RoomInfo>,
    },
    Snapshot(Snapshot),
    /// The request could not be served.
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    /// Seconds since the daemon started.
    pub uptime_secs: u64,
    pub bind: String,
    pub tls: bool,
    pub rooms: usize,
    pub devices: usize,
    /// Connections accepted since start, and how many were rejected during the
    /// handshake. A high rejection count usually means a mismatched invite code.
    pub accepted: u64,
    pub rejected: u64,
}

/// A connected device, as the admin view knows it.
///
/// `user` and `name` are self-reported by the client in the handshake. They are
/// display metadata for grouping, never authorisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// User nickname this device belongs to; empty when unassigned.
    #[serde(default)]
    pub user: String,
    /// Seconds since the device connected.
    pub connected_secs: u64,
}

/// A room, described without revealing anything the relay should not know.
///
/// The room id is truncated: displaying it in full would record which rooms exist
/// on this server, and the operator does not need that to answer "is my peer
/// connected".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    /// First 8 hex characters of the room id.
    pub room: String,
    pub devices: Vec<DeviceInfo>,
    /// Seconds since the room's most recent activity.
    pub idle_secs: u64,
}

/// One user, with every device that claims that nickname.
///
/// Devices with no user label are grouped under `user == ""`, shown as 未分组.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroup {
    /// The nickname; empty means unassigned devices.
    pub user: String,
    pub devices: Vec<DeviceInfo>,
}

/// Everything the WebUI and the management tool show about live state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub status: Status,
    /// Devices grouped by user nickname, for the "who owns which device" view.
    pub users: Vec<UserGroup>,
    /// The rooms themselves.
    pub rooms: Vec<RoomInfo>,
}

impl Status {
    /// Uptime as a short human string: `3d 4h`, `12m`, `45s`.
    pub fn uptime_text(&self) -> String {
        let s = self.uptime_secs;
        if s < 60 {
            format!("{s}s")
        } else if s < 3600 {
            format!("{}m", s / 60)
        } else if s < 86_400 {
            format!("{}h {}m", s / 3600, (s % 3600) / 60)
        } else {
            format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let json = serde_json::to_string(&Request::Status).unwrap();
        assert_eq!(json, r#"{"cmd":"status"}"#);
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"cmd":"rooms"}"#).unwrap(),
            Request::Rooms
        ));
    }

    #[test]
    fn responses_round_trip() {
        let status = Status {
            version: "0.1.0".into(),
            uptime_secs: 90,
            bind: "0.0.0.0:8787".into(),
            tls: true,
            rooms: 1,
            devices: 2,
            accepted: 10,
            rejected: 1,
        };
        let json = serde_json::to_string(&Response::Status(status)).unwrap();
        match serde_json::from_str::<Response>(&json).unwrap() {
            Response::Status(s) => {
                assert_eq!(s.devices, 2);
                assert!(s.tls);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn uptime_reads_naturally_at_each_scale() {
        let mk = |secs| Status {
            version: "0".into(),
            uptime_secs: secs,
            bind: String::new(),
            tls: false,
            rooms: 0,
            devices: 0,
            accepted: 0,
            rejected: 0,
        };
        assert_eq!(mk(45).uptime_text(), "45s");
        assert_eq!(mk(90).uptime_text(), "1m");
        assert_eq!(mk(3_700).uptime_text(), "1h 1m");
        assert_eq!(mk(90_000).uptime_text(), "1d 1h");
    }

    #[test]
    fn an_unknown_command_fails_to_parse_rather_than_defaulting() {
        // Silently treating an unknown command as Status would make a typo look
        // like it worked.
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"shutdown"}"#).is_err());
    }
}
