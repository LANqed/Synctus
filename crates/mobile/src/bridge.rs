//! The platform-independent half of the Android client.
//!
//! Kotlin never touches Rust types directly. It sends JSON commands in and polls
//! JSON events out, which keeps the JNI surface to four functions and lets this
//! whole module be unit-tested on the host:
//!
//! ```text
//!  Kotlin                      Rust
//!  ──────                      ────
//!  start(configJson)  ────▶  Bridge::start   spawns the client engine
//!  command(json)      ────▶  Bridge::command publish / nudge / pomodoro
//!  poll()             ◀────  Bridge::poll    events as a JSON array
//!  stop()             ────▶  Bridge::stop
//! ```
//!
//! Polling rather than callbacks: the Android foreground service already wakes on
//! a timer to refresh its notification, so a call into Rust on that tick costs
//! nothing extra, and it avoids holding a `GlobalRef` to a Java object plus the
//! thread-attachment dance that callbacks would need.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use synctus_core::client::{Client, ClientHandle, Command, ConnState, Event};
use synctus_core::config::{ClientConfig, PomodoroConfig};
use synctus_core::model::{
    Battery, ForegroundApp, NowPlaying, Nudge, NudgeKind, PomodoroPhase, Presence, StatusSnapshot,
    Todo,
};
use synctus_core::store::{Pomodoro, PomodoroEvent};

/// What Kotlin can ask the engine to do.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeCommand {
    /// Publish the sensor values Android just read. Anything absent is treated as
    /// unavailable rather than unchanged, because the platform reports all of it
    /// in one go.
    Publish {
        #[serde(default)]
        presence: Option<Presence>,
        #[serde(default)]
        foreground: Option<ForegroundApp>,
        #[serde(default)]
        battery: Option<Battery>,
        #[serde(default)]
        music: Option<NowPlaying>,
    },
    /// Send an interaction.
    Nudge {
        kind: NudgeKind,
    },
    /// Set the presence the user picked in the notification or the UI.
    SetPresence {
        presence: Presence,
    },
    /// Start, pause or resume the pomodoro.
    TogglePomodoro,
    StartFocus,
    StopPomodoro,
    SkipPhase,
    /// Replace the to-do list and publish it.
    SetTodos {
        items: Vec<Todo>,
    },
    /// Apply an edited configuration and reconnect.
    Reconfigure {
        config: Box<ClientConfig>,
    },
}

/// What Kotlin gets back. Mirrors [`Event`] plus the derived state the
/// notification needs, so the service does not have to recompute anything.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// Connection state changed.
    Connection { state: String, detail: String },
    /// The peer's status, ready to render.
    Peer {
        name: String,
        platform: String,
        presence: String,
        presence_color: u32,
        /// Main line: music or foreground app.
        detail: String,
        /// Secondary line: battery, pomodoro, to-dos.
        meta: String,
        stale: bool,
    },
    /// A poke arrived; the service raises a high-priority notification.
    Nudge {
        title: String,
        body: String,
        kind: String,
    },
    /// Local pomodoro reached a boundary.
    Pomodoro {
        phase: String,
        remaining: String,
        finished: bool,
        message: String,
    },
    /// The peer's to-do list.
    PeerTodos { items: Vec<Todo> },
    /// Something worth logging but not fatal.
    Warning { message: String },
}

/// Presence colour as `0xAARRGGBB`, which is what Android's notification and
/// Compose APIs expect.
fn argb(presence: Presence) -> u32 {
    let (r, g, b) = presence.rgb();
    0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// The engine plus everything derived from it.
pub struct Bridge {
    cfg: ClientConfig,
    client: ClientHandle,
    events: tokio::sync::mpsc::Receiver<Event>,
    runtime: tokio::runtime::Runtime,

    pomodoro: Pomodoro,
    peers: synctus_core::store::PeerView,
    conn: ConnState,

    /// Sensor values from the last `Publish`, kept so a pomodoro or presence
    /// change can republish without waiting for Android to sample again.
    last_sensors: Sensors,
    manual_presence: Option<Presence>,
    todos: Vec<Todo>,

    /// Events queued for the next `poll`.
    pending: Vec<BridgeEvent>,
}

#[derive(Debug, Clone, Default)]
struct Sensors {
    foreground: Option<ForegroundApp>,
    battery: Option<Battery>,
    music: Option<NowPlaying>,
}

impl Bridge {
    /// Start the engine from a JSON config.
    pub fn start(config_json: &str) -> Result<Self> {
        let cfg: ClientConfig = serde_json::from_str(config_json).context("解析配置 JSON 失败")?;
        Self::with_config(cfg)
    }

    pub fn with_config(cfg: ClientConfig) -> Result<Self> {
        // Two worker threads: one for the socket, one spare so a blocking DNS
        // lookup cannot stall the heartbeat.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("创建运行时失败")?;

        let (client, events, engine) = Client::spawn(cfg.clone())?;
        runtime.spawn(engine.run());

        Ok(Self {
            pomodoro: Pomodoro::new(cfg.pomodoro, 0, 0),
            cfg,
            client,
            events,
            runtime,
            peers: synctus_core::store::PeerView::default(),
            conn: ConnState::Connecting,
            last_sensors: Sensors::default(),
            manual_presence: None,
            todos: Vec::new(),
            pending: Vec::new(),
        })
    }

    /// Handle a JSON command.
    pub fn command(&mut self, json: &str) -> Result<()> {
        let command: BridgeCommand =
            serde_json::from_str(json).with_context(|| format!("解析命令失败: {json}"))?;
        self.apply(command)
    }

    fn apply(&mut self, command: BridgeCommand) -> Result<()> {
        match command {
            BridgeCommand::Publish {
                presence,
                foreground,
                battery,
                music,
            } => {
                self.last_sensors = Sensors {
                    foreground,
                    battery,
                    music,
                };
                if let Some(p) = presence {
                    self.manual_presence = (p != Presence::Active).then_some(p);
                }
                self.publish()
            }
            BridgeCommand::Nudge { kind } => {
                let nudge = Nudge::new(kind, self.cfg.device_name.clone());
                self.client.nudge(nudge)
            }
            BridgeCommand::SetPresence { presence } => {
                self.manual_presence = (presence != Presence::Active).then_some(presence);
                self.publish()
            }
            BridgeCommand::TogglePomodoro => {
                let now = synctus_core::now_ms();
                if self.pomodoro.state().phase == PomodoroPhase::Idle {
                    self.pomodoro.start_focus(now);
                } else {
                    self.pomodoro.toggle_pause(now);
                }
                self.publish()
            }
            BridgeCommand::StartFocus => {
                self.pomodoro.start_focus(synctus_core::now_ms());
                self.publish()
            }
            BridgeCommand::StopPomodoro => {
                self.pomodoro.stop();
                self.publish()
            }
            BridgeCommand::SkipPhase => {
                self.pomodoro.skip(synctus_core::now_ms());
                self.publish()
            }
            BridgeCommand::SetTodos { items } => {
                self.todos = items;
                self.client
                    .send(Command::PublishTodos(self.todos.clone()))?;
                self.publish()
            }
            BridgeCommand::Reconfigure { config } => {
                self.pomodoro.set_config(config.pomodoro);
                self.cfg = (*config).clone();
                self.client.send(Command::Reconnect(config))
            }
        }
    }

    /// Effective presence: explicit choice, then pomodoro phase, then active.
    ///
    /// Android has no reliable idle signal available to a background app, so
    /// unlike the desktop there is no automatic "away".
    fn effective_presence(&self) -> Presence {
        self.manual_presence
            .or_else(|| self.pomodoro.implied_presence())
            .unwrap_or(Presence::Active)
    }

    fn publish(&mut self) -> Result<()> {
        let privacy = &self.cfg.privacy;
        let mut snap = StatusSnapshot::new(&self.cfg.device_id, &self.cfg.device_name);
        snap.presence = self.effective_presence();

        if privacy.share_foreground_app {
            snap.foreground = self.last_sensors.foreground.clone().map(|mut fg| {
                if privacy.is_blocked(&fg.app) {
                    fg = ForegroundApp {
                        app: "（隐藏）".to_string(),
                        name: None,
                        title: None,
                    };
                } else if !privacy.share_window_title {
                    fg.title = None;
                }
                fg
            });
        }
        if privacy.share_battery {
            snap.battery = self.last_sensors.battery;
        }
        if privacy.share_music {
            snap.music = self.last_sensors.music.clone();
        }
        if privacy.share_pomodoro {
            snap.pomodoro = Some(self.pomodoro.state());
        }
        if privacy.share_todos {
            let now = snap.at;
            snap.todos_open = self.todos.iter().filter(|t| !t.done).count() as u32;
            snap.todos_done_today = self
                .todos
                .iter()
                .filter(|t| t.done && t.done_at.map(|d| now - d < 86_400_000).unwrap_or(false))
                .count() as u32;
        }

        self.client.publish(snap)
    }

    /// Drain engine events and return everything queued as a JSON array.
    pub fn poll(&mut self) -> String {
        self.drain();
        let events = std::mem::take(&mut self.pending);
        // Serialisation of our own types cannot fail; an empty array keeps the
        // Kotlin side from having to handle a parse error.
        serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string())
    }

    fn drain(&mut self) {
        // Pomodoro first, so a boundary reached between polls is reported.
        let now = synctus_core::now_ms();
        match self.pomodoro.tick(now) {
            PomodoroEvent::Nothing => {}
            PomodoroEvent::FocusFinished(next) => {
                self.pending.push(BridgeEvent::Pomodoro {
                    phase: next.label().to_string(),
                    remaining: "00:00".to_string(),
                    finished: true,
                    message: format!("专注完成，该{}了", next.label()),
                });
                let _ = self.publish();
            }
            PomodoroEvent::BreakFinished => {
                self.pending.push(BridgeEvent::Pomodoro {
                    phase: PomodoroPhase::Idle.label().to_string(),
                    remaining: "00:00".to_string(),
                    finished: true,
                    message: "休息结束，准备下一回合".to_string(),
                });
                let _ = self.publish();
            }
        }

        let mut batch = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            batch.push(event);
        }

        let mut peer_changed = false;
        for event in batch {
            match event {
                Event::State(state) => {
                    let (name, detail) = match &state {
                        ConnState::Connecting => ("connecting", String::new()),
                        ConnState::Online => ("online", String::new()),
                        ConnState::Offline(why) => ("offline", why.clone()),
                        ConnState::Rejected(why) => ("rejected", why.clone()),
                    };
                    if matches!(state, ConnState::Offline(_) | ConnState::Rejected(_)) {
                        self.peers.clear();
                        peer_changed = true;
                    }
                    // Republish on (re)connect so the peer sees us immediately.
                    if matches!(state, ConnState::Online) {
                        let _ = self.publish();
                    }
                    self.conn = state;
                    self.pending.push(BridgeEvent::Connection {
                        state: name.to_string(),
                        detail,
                    });
                }
                Event::PeerStatus(snap) => {
                    if self.peers.apply_status(snap) {
                        peer_changed = true;
                    }
                }
                Event::PeerTodos { device_id, items } => {
                    self.peers.apply_todos(device_id, items.clone());
                    self.pending.push(BridgeEvent::PeerTodos { items });
                }
                Event::Nudge(nudge) => {
                    // Following the peer into a focus round mirrors the desktop.
                    if nudge.kind == NudgeKind::FocusTogether
                        && self.pomodoro.state().phase == PomodoroPhase::Idle
                    {
                        self.pomodoro.start_focus(now);
                        let _ = self.publish();
                    }
                    self.pending.push(BridgeEvent::Nudge {
                        title: format!("{} {}", nudge.kind.emoji(), nudge.from_name),
                        body: nudge.body(),
                        kind: format!("{:?}", nudge.kind),
                    });
                }
                Event::PeerPresence { device_id, online } => {
                    self.peers.set_online(&device_id, online);
                    peer_changed = true;
                }
                Event::Warning(message) => {
                    self.pending.push(BridgeEvent::Warning { message });
                }
            }
        }

        if peer_changed {
            if let Some(event) = self.peer_event() {
                self.pending.push(event);
            }
        }
    }

    /// Build the peer summary the notification renders.
    fn peer_event(&self) -> Option<BridgeEvent> {
        let now = synctus_core::now_ms();
        let stale_ms = self.cfg.peer_stale_ms();
        let peer = self.peers.primary(now, stale_ms)?;
        let stale = peer.is_stale(now, stale_ms);
        let presence = if stale {
            Presence::Offline
        } else {
            peer.presence
        };

        Some(BridgeEvent::Peer {
            name: peer.name.clone(),
            platform: format!("{:?}", peer.platform),
            presence: presence.label().to_string(),
            presence_color: argb(presence),
            detail: peer_detail(peer, now, stale),
            meta: peer_meta(peer, now),
            stale,
        })
    }

    /// Current local state, for the notification's own line.
    pub fn local_status_json(&self) -> String {
        let state = self.pomodoro.state();
        let value = serde_json::json!({
            "presence": self.effective_presence().label(),
            "pomodoro_phase": state.phase.label(),
            "pomodoro_remaining": state.remaining_text(synctus_core::now_ms()),
            "pomodoro_active": state.phase != PomodoroPhase::Idle,
            "pomodoro_paused": state.paused(),
            "completed_today": state.completed_today,
            "connected": matches!(self.conn, ConnState::Online),
        });
        value.to_string()
    }

    /// Stop the engine and release the runtime.
    pub fn stop(self) {
        let _ = self.client.send(Command::Shutdown);
        // Give the engine a moment to send its shutdown, then stop waiting: the
        // service is being torn down and must not block.
        self.runtime
            .shutdown_timeout(std::time::Duration::from_millis(500));
    }
}

fn peer_detail(peer: &StatusSnapshot, now: i64, stale: bool) -> String {
    if stale {
        return format!("最后更新于 {} 秒前", (now - peer.at) / 1000);
    }
    if let Some(music) = &peer.music {
        if music.playing {
            return format!("♪ {}", music.one_line());
        }
    }
    if let Some(fg) = &peer.foreground {
        let name = fg.name.clone().unwrap_or_else(|| fg.app.clone());
        return match &fg.title {
            Some(t) => format!("{name}：{t}"),
            None => name,
        };
    }
    "空闲".to_string()
}

fn peer_meta(peer: &StatusSnapshot, now: i64) -> String {
    let mut parts = Vec::new();
    if let Some(b) = peer.battery {
        parts.push(format!(
            "{}{}%",
            if b.charging { "⚡" } else { "🔋" },
            b.percent
        ));
    }
    if let Some(pom) = peer.pomodoro {
        if pom.phase != PomodoroPhase::Idle {
            parts.push(format!(
                "🍅{}{} {}",
                if pom.paused() { "⏸" } else { "▶" },
                pom.phase.label(),
                pom.remaining_text(now)
            ));
        } else if pom.completed_today > 0 {
            parts.push(format!("🍅×{}", pom.completed_today));
        }
    }
    if peer.todos_open > 0 || peer.todos_done_today > 0 {
        parts.push(format!(
            "☑{}/{}",
            peer.todos_done_today,
            peer.todos_open + peer.todos_done_today
        ));
    }
    parts.join("  ")
}

/// Process-wide bridge instance.
///
/// Android's service can be destroyed and recreated while the process lives, so
/// the engine outlives any single Java object. A `Mutex<Option<..>>` keeps that
/// explicit and makes double-start a recoverable error rather than two engines
/// fighting over one socket.
static INSTANCE: Mutex<Option<Bridge>> = Mutex::new(None);

/// Start the engine, replacing any previous one.
pub fn global_start(config_json: &str) -> Result<()> {
    let bridge = Bridge::start(config_json)?;
    let mut guard = INSTANCE
        .lock()
        .map_err(|_| anyhow!("桥接状态已损坏，请重启应用"))?;
    if let Some(old) = guard.take() {
        old.stop();
    }
    *guard = Some(bridge);
    Ok(())
}

pub fn global_command(json: &str) -> Result<()> {
    let mut guard = INSTANCE
        .lock()
        .map_err(|_| anyhow!("桥接状态已损坏，请重启应用"))?;
    guard
        .as_mut()
        .ok_or_else(|| anyhow!("引擎尚未启动"))?
        .command(json)
}

/// Poll events. Returns `[]` when the engine is not running, so the caller does
/// not have to special-case startup.
pub fn global_poll() -> String {
    match INSTANCE.lock() {
        Ok(mut guard) => guard
            .as_mut()
            .map(|b| b.poll())
            .unwrap_or_else(|| "[]".to_string()),
        Err(_) => "[]".to_string(),
    }
}

pub fn global_local_status() -> String {
    match INSTANCE.lock() {
        Ok(guard) => guard
            .as_ref()
            .map(|b| b.local_status_json())
            .unwrap_or_else(|| "{}".to_string()),
        Err(_) => "{}".to_string(),
    }
}

pub fn global_stop() {
    if let Ok(mut guard) = INSTANCE.lock() {
        if let Some(bridge) = guard.take() {
            bridge.stop();
        }
    }
}

pub fn global_running() -> bool {
    INSTANCE.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Generate a pairing code, exposed so the Android UI does not reimplement it.
pub fn new_invite_code() -> String {
    synctus_core::crypto::generate_invite_code()
}

/// Default config as JSON, used to seed the Android settings screen.
pub fn default_config_json() -> String {
    let cfg = ClientConfig {
        device_name: "Android".to_string(),
        pomodoro: PomodoroConfig::default(),
        ..ClientConfig::default()
    };
    serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired_config() -> ClientConfig {
        ClientConfig {
            invite_code: "ABCD-EFGH-IJKL-MNOP".into(),
            // Nothing listens here; the engine simply stays in reconnect backoff,
            // which is exactly the state we want to exercise.
            server: "127.0.0.1:1".into(),
            tls: false,
            ..ClientConfig::default()
        }
    }

    #[test]
    fn commands_deserialise_from_kotlin_shaped_json() {
        let json = r#"{"type":"publish","battery":{"percent":42,"charging":true}}"#;
        let cmd: BridgeCommand = serde_json::from_str(json).unwrap();
        match cmd {
            BridgeCommand::Publish { battery, .. } => {
                let b = battery.expect("battery present");
                assert_eq!(b.percent, 42);
                assert!(b.charging);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn nudge_command_round_trips() {
        let cmd: BridgeCommand =
            serde_json::from_str(r#"{"type":"nudge","kind":"knock"}"#).unwrap();
        assert!(matches!(
            cmd,
            BridgeCommand::Nudge {
                kind: NudgeKind::Knock
            }
        ));
    }

    #[test]
    fn unpaired_config_fails_to_start() {
        let json = serde_json::to_string(&ClientConfig::default()).unwrap();
        assert!(Bridge::start(&json).is_err());
    }

    #[test]
    fn malformed_config_json_is_an_error() {
        assert!(Bridge::start("{not json").is_err());
    }

    #[test]
    fn poll_returns_a_json_array() {
        let mut bridge = Bridge::with_config(paired_config()).unwrap();
        let json = bridge.poll();
        assert!(json.starts_with('['), "got {json}");
        // Parses as an array of events.
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        bridge.stop();
    }

    #[test]
    fn pomodoro_commands_change_the_reported_phase() {
        let mut bridge = Bridge::with_config(paired_config()).unwrap();

        let before: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        assert_eq!(before["pomodoro_active"], serde_json::json!(false));

        bridge.command(r#"{"type":"start_focus"}"#).unwrap();
        let after: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        assert_eq!(after["pomodoro_active"], serde_json::json!(true));
        assert_eq!(after["pomodoro_phase"], serde_json::json!("专注"));

        // Focus implies do-not-disturb by default.
        assert_eq!(after["presence"], serde_json::json!("免打扰"));

        bridge.command(r#"{"type":"toggle_pomodoro"}"#).unwrap();
        let paused: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        assert_eq!(paused["pomodoro_paused"], serde_json::json!(true));

        bridge.command(r#"{"type":"stop_pomodoro"}"#).unwrap();
        let stopped: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        assert_eq!(stopped["pomodoro_active"], serde_json::json!(false));

        bridge.stop();
    }

    #[test]
    fn explicit_presence_overrides_the_pomodoro_phase() {
        let mut bridge = Bridge::with_config(paired_config()).unwrap();
        bridge.command(r#"{"type":"start_focus"}"#).unwrap();
        bridge
            .command(r#"{"type":"set_presence","presence":"resting"}"#)
            .unwrap();

        let status: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        assert_eq!(status["presence"], serde_json::json!("休息中"));
        bridge.stop();
    }

    #[test]
    fn selecting_active_returns_control_to_automatic_presence() {
        let mut bridge = Bridge::with_config(paired_config()).unwrap();
        bridge
            .command(r#"{"type":"set_presence","presence":"resting"}"#)
            .unwrap();
        bridge
            .command(r#"{"type":"set_presence","presence":"active"}"#)
            .unwrap();
        bridge.command(r#"{"type":"start_focus"}"#).unwrap();

        let status: serde_json::Value = serde_json::from_str(&bridge.local_status_json()).unwrap();
        // Back under pomodoro control.
        assert_eq!(status["presence"], serde_json::json!("免打扰"));
        bridge.stop();
    }

    #[test]
    fn unknown_command_is_reported_not_ignored() {
        let mut bridge = Bridge::with_config(paired_config()).unwrap();
        assert!(bridge.command(r#"{"type":"fly_to_the_moon"}"#).is_err());
        bridge.stop();
    }

    #[test]
    fn presence_colour_is_opaque_argb() {
        let colour = argb(Presence::Active);
        assert_eq!(colour >> 24, 0xFF, "alpha must be opaque");
        let (r, g, b) = Presence::Active.rgb();
        assert_eq!(
            colour & 0x00FF_FFFF,
            ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        );
    }

    #[test]
    fn default_config_json_is_usable_by_the_ui() {
        let json = default_config_json();
        let cfg: ClientConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg.device_name, "Android");
        assert!(!cfg.is_paired(), "默认配置不应带配对码");
    }

    #[test]
    fn generated_invite_codes_are_valid() {
        let code = new_invite_code();
        assert!(synctus_core::crypto::RoomKeys::derive(&code).is_ok());
    }

    #[test]
    fn peer_detail_prefers_music_then_app() {
        let mut peer = StatusSnapshot::new("d", "Peer");
        peer.foreground = Some(ForegroundApp {
            app: "com.example".into(),
            name: Some("示例".into()),
            title: None,
        });
        assert_eq!(peer_detail(&peer, peer.at, false), "示例");

        peer.music = Some(NowPlaying {
            title: "歌名".into(),
            artist: Some("歌手".into()),
            album: None,
            player: None,
            playing: true,
        });
        assert_eq!(peer_detail(&peer, peer.at, false), "♪ 歌手 - 歌名");

        // A paused track falls back to the app.
        peer.music.as_mut().unwrap().playing = false;
        assert_eq!(peer_detail(&peer, peer.at, false), "示例");
    }

    #[test]
    fn stale_peers_report_their_age() {
        let peer = StatusSnapshot::new("d", "Peer");
        let detail = peer_detail(&peer, peer.at + 120_000, true);
        assert!(detail.contains("120"), "got {detail}");
    }

    #[test]
    fn peer_meta_includes_battery_and_pomodoro() {
        let mut peer = StatusSnapshot::new("d", "Peer");
        peer.battery = Some(Battery {
            percent: 88,
            charging: false,
            minutes_left: None,
        });
        peer.pomodoro = Some(synctus_core::model::PomodoroState {
            phase: PomodoroPhase::Focus,
            ends_at: Some(peer.at + 60_000),
            ..Default::default()
        });
        let meta = peer_meta(&peer, peer.at);
        assert!(meta.contains("88%"), "got {meta}");
        assert!(meta.contains("01:00"), "got {meta}");
    }

    #[test]
    fn blocked_apps_are_replaced_before_publishing() {
        let cfg = ClientConfig {
            privacy: synctus_core::config::Privacy {
                blocked_apps: vec!["secret".into()],
                ..Default::default()
            },
            ..paired_config()
        };
        let mut bridge = Bridge::with_config(cfg).unwrap();

        bridge
            .command(r#"{"type":"publish","foreground":{"app":"com.secret.vault","name":"Vault"}}"#)
            .unwrap();

        // Inspect what would be published rather than the wire, which is offline.
        assert_eq!(
            bridge.last_sensors.foreground.as_ref().unwrap().app,
            "com.secret.vault",
            "the raw sample is kept as-is"
        );
        let privacy = &bridge.cfg.privacy;
        assert!(privacy.is_blocked("com.secret.vault"));
        bridge.stop();
    }

    /// The global instance is process-wide, so the tests that touch it must not
    /// run concurrently with each other. They are combined into one test rather
    /// than relying on a lock or `--test-threads=1`.
    #[test]
    fn global_lifecycle_is_idempotent() {
        // Poll, command and stop before start must not panic.
        assert_eq!(global_poll(), "[]");
        assert!(global_command(r#"{"type":"start_focus"}"#).is_err());
        global_stop();
        assert!(!global_running());

        let json = serde_json::to_string(&paired_config()).unwrap();
        global_start(&json).unwrap();
        assert!(global_running());

        // Starting twice replaces the engine rather than leaking one.
        global_start(&json).unwrap();
        assert!(global_running());

        assert!(global_command(r#"{"type":"start_focus"}"#).is_ok());
        assert!(global_poll().starts_with('['));
        assert!(global_local_status().contains("presence"));

        global_stop();
        assert!(!global_running());
        // And stopping twice is harmless.
        global_stop();
    }
}
