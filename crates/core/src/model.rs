//! Everything that is synchronised between two peers.
//!
//! These types are serialised *inside* the encrypted payload, so the relay
//! server never sees any of it. Field names are kept short because a snapshot is
//! re-published on every change.

use serde::{Deserialize, Serialize};

/// Which platform a peer device runs on. Used purely for the UI badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Android,
    Other,
}

impl Platform {
    /// Platform of the running binary.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Platform::Windows => "WIN",
            Platform::Linux => "TUX",
            Platform::Android => "AND",
            Platform::Other => "?",
        }
    }
}

/// Coarse availability, set explicitly by the user (tray menu on desktop,
/// notification action on Android) rather than inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// At the keyboard and interruptible.
    Active,
    /// Explicitly taking a break — the "是否休息" flag.
    Resting,
    /// Idle for a while, detected from input idle time.
    Away,
    /// Do not disturb: nudges are queued instead of shown.
    Busy,
    /// No device connected. Never published, only synthesised locally.
    Offline,
}

impl Presence {
    pub fn label(self) -> &'static str {
        match self {
            Presence::Active => "在忙",
            Presence::Resting => "休息中",
            Presence::Away => "离开",
            Presence::Busy => "免打扰",
            Presence::Offline => "离线",
        }
    }

    /// Accent colour as RGB, shared by the desktop overlay and the Android
    /// notification so both ends look the same.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Presence::Active => (0x4c, 0xaf, 0x50),
            Presence::Resting => (0x42, 0xa5, 0xf5),
            Presence::Away => (0xff, 0xb3, 0x00),
            Presence::Busy => (0xef, 0x53, 0x50),
            Presence::Offline => (0x9e, 0x9e, 0x9e),
        }
    }
}

/// The window (desktop) or app (Android) currently in the foreground.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundApp {
    /// Process / package name, e.g. `Code.exe` or `com.tencent.mm`.
    pub app: String,
    /// Human friendly name when we can resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Window title. Omitted entirely when the user disables title sharing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    /// 0-100.
    pub percent: u8,
    pub charging: bool,
    /// Minutes remaining, when the OS reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes_left: Option<u32>,
}

/// Media session snapshot, from GSMTC on Windows, MPRIS on Linux and
/// `MediaSession` on Android.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowPlaying {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// Player identity, e.g. `Spotify` / `org.mpris.MediaPlayer2.vlc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<String>,
    /// False while paused; we keep the track so the UI can show "⏸ …".
    pub playing: bool,
}

impl NowPlaying {
    /// `Artist - Title`, or just the title when the artist is unknown.
    pub fn one_line(&self) -> String {
        match &self.artist {
            Some(a) if !a.is_empty() => format!("{a} - {}", self.title),
            _ => self.title.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PomodoroPhase {
    Idle,
    Focus,
    ShortBreak,
    LongBreak,
}

impl PomodoroPhase {
    pub fn label(self) -> &'static str {
        match self {
            PomodoroPhase::Idle => "未开始",
            PomodoroPhase::Focus => "专注",
            PomodoroPhase::ShortBreak => "小休",
            PomodoroPhase::LongBreak => "长休",
        }
    }

    pub fn is_break(self) -> bool {
        matches!(self, PomodoroPhase::ShortBreak | PomodoroPhase::LongBreak)
    }
}

/// Pomodoro timer state. Only the *deadline* is synced, never a ticking
/// countdown, so the peer UI can interpolate locally without a message per
/// second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PomodoroState {
    pub phase: PomodoroPhase,
    /// Unix ms at which the current phase ends. `None` when idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    /// Milliseconds left when paused; lets the peer render a frozen timer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_left_ms: Option<i64>,
    /// Completed focus rounds in the current set.
    #[serde(default)]
    pub round: u32,
    /// Completed focus rounds today.
    #[serde(default)]
    pub completed_today: u32,
}

impl Default for PomodoroState {
    fn default() -> Self {
        Self {
            phase: PomodoroPhase::Idle,
            ends_at: None,
            paused_left_ms: None,
            round: 0,
            completed_today: 0,
        }
    }
}

impl PomodoroState {
    pub fn paused(&self) -> bool {
        self.paused_left_ms.is_some()
    }

    /// Milliseconds remaining at `now`, clamped at zero.
    pub fn remaining_ms(&self, now: i64) -> i64 {
        if let Some(left) = self.paused_left_ms {
            return left.max(0);
        }
        match self.ends_at {
            Some(end) => (end - now).max(0),
            None => 0,
        }
    }

    /// `mm:ss` for the remaining time.
    pub fn remaining_text(&self, now: i64) -> String {
        let total = self.remaining_ms(now) / 1000;
        format!("{:02}:{:02}", total / 60, total % 60)
    }
}

/// A single to-do item. Todos are synced as a whole list, which is fine for the
/// handful of items a person keeps in a day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub done: bool,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
    /// Focus rounds spent on this item.
    #[serde(default)]
    pub pomodoros: u32,
}

impl Todo {
    pub fn new(title: impl Into<String>, now: i64) -> Self {
        Self {
            id: crate::crypto::random_id(8),
            title: title.into(),
            done: false,
            created_at: now,
            done_at: None,
            pomodoros: 0,
        }
    }
}

/// Everything one device publishes about itself. Re-sent on change and at least
/// once per heartbeat interval so the peer can detect staleness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub device_id: String,
    pub name: String,
    pub platform: Platform,
    /// Publisher clock, unix ms. Also used to drop out-of-order updates.
    pub at: i64,
    pub presence: Presence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ForegroundApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<Battery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<NowPlaying>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pomodoro: Option<PomodoroState>,
    /// Open / completed-today counters. The list itself is sent separately so a
    /// status update stays small.
    #[serde(default)]
    pub todos_open: u32,
    #[serde(default)]
    pub todos_done_today: u32,
    /// Seconds since the last input event, when the platform exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_secs: Option<u32>,
    /// Minutes focused today. This is the number that actually holds someone
    /// accountable, so it is synced separately from the round counter.
    #[serde(default)]
    pub focus_today_min: u32,
    /// Daily focus target in minutes. Synced so the peer's progress means
    /// something rather than being a bare number.
    #[serde(default)]
    pub goal_min: u32,
    /// Consecutive days the goal was met, including today once it is met.
    #[serde(default)]
    pub streak_days: u32,
}

impl StatusSnapshot {
    pub fn new(device_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            name: name.into(),
            platform: Platform::current(),
            at: crate::now_ms(),
            presence: Presence::Active,
            foreground: None,
            battery: None,
            music: None,
            pomodoro: None,
            todos_open: 0,
            todos_done_today: 0,
            idle_secs: None,
            focus_today_min: 0,
            goal_min: 0,
            streak_days: 0,
        }
    }

    /// True when the snapshot is older than `max_age_ms` and should be shown as
    /// stale / offline.
    pub fn is_stale(&self, now: i64, max_age_ms: i64) -> bool {
        now.saturating_sub(self.at) > max_age_ms
    }

    /// Progress towards the daily goal, 0.0 to 1.0. Zero when no goal is set.
    pub fn goal_progress(&self) -> f32 {
        if self.goal_min == 0 {
            return 0.0;
        }
        (self.focus_today_min as f32 / self.goal_min as f32).clamp(0.0, 1.0)
    }

    pub fn goal_met(&self) -> bool {
        self.goal_min > 0 && self.focus_today_min >= self.goal_min
    }

    /// Whether this device claims to be in a focus round right now.
    pub fn is_focusing(&self) -> bool {
        self.pomodoro
            .map(|p| p.phase == PomodoroPhase::Focus && !p.paused())
            .unwrap_or(false)
    }

    /// The foreground app as a single comparable string, for distraction checks.
    pub fn foreground_app(&self) -> Option<&str> {
        self.foreground.as_ref().map(|f| f.app.as_str())
    }
}

/// An interaction sent to the peer.
///
/// The list is ordered by how often it gets used for the tool's actual purpose:
/// nagging comes first, comfort last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeKind {
    /// Left-clicking the peer avatar / tapping the notification button.
    Knock,
    Hug,
    Coffee,
    /// "Come rest" suggestion.
    Rest,
    /// "Let's focus together" — invites a synced pomodoro.
    FocusTogether,
    /// "Stop slacking off." Sent when the peer is supposed to be focusing but
    /// clearly is not.
    Nag,
    /// Acknowledges a finished round or a met goal. Praise is the other half of
    /// accountability; without it the tool is only ever unpleasant.
    Cheer,
}

impl NudgeKind {
    pub fn emoji(self) -> &'static str {
        match self {
            NudgeKind::Knock => "👋",
            NudgeKind::Hug => "🤗",
            NudgeKind::Coffee => "☕",
            NudgeKind::Rest => "🛋",
            NudgeKind::FocusTogether => "🍅",
            NudgeKind::Nag => "👀",
            NudgeKind::Cheer => "🎉",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NudgeKind::Knock => "敲一敲",
            NudgeKind::Hug => "抱抱",
            NudgeKind::Coffee => "请喝咖啡",
            NudgeKind::Rest => "去休息",
            NudgeKind::FocusTogether => "一起专注",
            NudgeKind::Nag => "别摸鱼了",
            NudgeKind::Cheer => "夸一夸",
        }
    }

    /// Sentence used when the sender adds no text of their own.
    fn default_text(self) -> &'static str {
        match self {
            NudgeKind::Knock => "敲了敲你",
            NudgeKind::Hug => "抱了抱你",
            NudgeKind::Coffee => "请你喝咖啡",
            NudgeKind::Rest => "让你去休息",
            NudgeKind::FocusTogether => "邀你一起专注",
            NudgeKind::Nag => "发现你在摸鱼",
            NudgeKind::Cheer => "为你鼓掌",
        }
    }

    /// Whether this should break through do-not-disturb.
    ///
    /// A nag that waits until the peer happens to look at their screen is
    /// useless, so it is the one interaction allowed to interrupt. Everything
    /// else respects [`Presence::Busy`].
    pub fn is_urgent(self) -> bool {
        matches!(self, NudgeKind::Nag)
    }

    /// Ordered for the menus: the accountability actions first.
    pub const ALL: [NudgeKind; 7] = [
        NudgeKind::Nag,
        NudgeKind::FocusTogether,
        NudgeKind::Cheer,
        NudgeKind::Knock,
        NudgeKind::Hug,
        NudgeKind::Coffee,
        NudgeKind::Rest,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nudge {
    pub kind: NudgeKind,
    /// Optional short text typed by the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Display name of the sender, so the receiver can render it without having
    /// a status snapshot yet.
    pub from_name: String,
    pub at: i64,
}

impl Nudge {
    pub fn new(kind: NudgeKind, from_name: impl Into<String>) -> Self {
        Self {
            kind,
            text: None,
            from_name: from_name.into(),
            at: crate::now_ms(),
        }
    }

    /// One-line body for a toast / notification.
    pub fn body(&self) -> String {
        match &self.text {
            Some(t) if !t.is_empty() => format!("{} {}：{t}", self.kind.emoji(), self.from_name),
            _ => format!(
                "{} {} {}",
                self.kind.emoji(),
                self.from_name,
                self.kind.default_text()
            ),
        }
    }
}

/// The plaintext payload exchanged between peers, i.e. what sits inside the
/// AEAD box.
///
/// `Status` is much larger than the ping variants. Boxing it would save nothing:
/// the value is serialised immediately and never stored in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PeerMessage {
    /// Full status. Retained by the relay so a peer that connects later gets it.
    Status(StatusSnapshot),
    /// Interaction. Never retained.
    Nudge(Nudge),
    /// Full to-do list of the sender.
    Todos {
        device_id: String,
        items: Vec<Todo>,
        at: i64,
    },
    /// Keeps the E2E channel warm and lets peers measure round-trip time.
    Ping {
        at: i64,
    },
    Pong {
        at: i64,
    },
}

impl PeerMessage {
    /// Whether the relay should store this as the sender's latest state.
    pub fn retained(&self) -> bool {
        matches!(self, PeerMessage::Status(_) | PeerMessage::Todos { .. })
    }

    /// Retain slot name. Distinct kinds must not overwrite each other.
    pub fn retain_slot(&self) -> Option<&'static str> {
        match self {
            PeerMessage::Status(_) => Some("status"),
            PeerMessage::Todos { .. } => Some("todos"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pomodoro_remaining_is_clamped_and_formatted() {
        let st = PomodoroState {
            phase: PomodoroPhase::Focus,
            ends_at: Some(10_000),
            ..Default::default()
        };
        assert_eq!(st.remaining_ms(12_000), 0);
        assert_eq!(st.remaining_text(4_000), "00:06");
    }

    #[test]
    fn paused_pomodoro_ignores_deadline() {
        let st = PomodoroState {
            phase: PomodoroPhase::Focus,
            ends_at: Some(0),
            paused_left_ms: Some(90_000),
            ..Default::default()
        };
        assert!(st.paused());
        assert_eq!(st.remaining_text(999_999), "01:30");
    }

    #[test]
    fn only_status_and_todos_are_retained() {
        let s = PeerMessage::Status(StatusSnapshot::new("d", "n"));
        assert_eq!(s.retain_slot(), Some("status"));
        assert!(!PeerMessage::Ping { at: 0 }.retained());
    }
}
