//! Synctus core: shared status model, end-to-end crypto, wire protocol and the
//! reconnecting client engine used by both the desktop and the mobile front-end.
//!
//! The point of the project is mutual accountability between two people: seeing
//! that the other is actually working, and being seen doing the same. The pieces
//! that serve that goal — the pomodoro engine, the daily focus goal and streak,
//! and distraction detection — all live here so the three front-ends behave
//! identically.
//!
//! Layering:
//!
//! ```text
//! desktop / mobile UI
//!        │  Event / Command
//! ┌──────▼───────────────────────────────┐
//! │ client.rs   reconnect + state store  │
//! ├──────────────────────────────────────┤
//! │ crypto.rs   XChaCha20-Poly1305 E2E   │  server never sees plaintext
//! ├──────────────────────────────────────┤
//! │ proto.rs    length-prefixed frames   │
//! ├──────────────────────────────────────┤
//! │ tls.rs      optional rustls tunnel   │
//! └──────────────────────────────────────┘
//! ```

pub mod client;
pub mod config;
pub mod crypto;
pub mod focus;
pub mod model;
pub mod proto;
pub mod store;

#[cfg(feature = "tls")]
pub mod tls;

#[cfg(feature = "update")]
pub mod update;

pub use client::{Client, ClientHandle, Command, Event};
pub use config::{Accountability, ClientConfig, Privacy};
pub use crypto::RoomKeys;
pub use focus::{Distraction, DistractionTracker};
pub use model::{
    Battery, ForegroundApp, NowPlaying, Nudge, NudgeKind, PomodoroPhase, PomodoroState, Presence,
    StatusSnapshot, Todo,
};

/// Wire protocol version. Bumped on any incompatible frame change; peers with a
/// different major version are rejected by the relay.
pub const PROTOCOL_VERSION: u16 = 1;

/// Largest frame we are willing to read. Status snapshots are ~1 KiB, so this
/// leaves plenty of headroom while bounding memory per connection.
pub const MAX_FRAME_LEN: usize = 64 * 1024;

/// Current unix timestamp in milliseconds.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
