//! Application state and the glue between the UI, the sensors and the client
//! engine.
//!
//! The UI thread owns an [`App`] and calls [`App::tick`] once per frame. Anything
//! that could block — socket I/O, WinRT/D-Bus queries — happens on other threads
//! and arrives here through channels.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use synctus_core::client::{Client, ClientHandle, Command, ConnState, Event};
use synctus_core::config::ClientConfig;
use synctus_core::model::{Nudge, NudgeKind, PomodoroPhase, Presence, StatusSnapshot, Todo};
use synctus_core::store::{LocalData, PeerView, Pomodoro, PomodoroEvent};

use crate::sensors::{self, Sample};

/// Requests the tray and overlay can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiRequest {
    /// Poke the peer.
    Nudge(NudgeKind),
    SetPresence(Presence),
    TogglePomodoro,
    StartFocus,
    StopPomodoro,
    SkipPhase,
    ToggleOverlay,
    OpenSettings,
    CheckUpdate,
    Reconnect,
    Quit,
}

/// A line in the in-app log pane, so users can diagnose connection trouble
/// without a terminal.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: Instant,
    pub text: String,
}

pub struct App {
    pub cfg: ClientConfig,
    config_path: PathBuf,
    data_path: PathBuf,

    pub data: LocalData,
    pub pomodoro: Pomodoro,
    pub peers: PeerView,

    /// `None` until a pairing code is configured.
    client: Option<ClientHandle>,
    events: Option<tokio::sync::mpsc::Receiver<Event>>,
    runtime: tokio::runtime::Runtime,

    pub conn: ConnState,
    pub log: Vec<LogLine>,

    /// Latest sensor sample, refreshed by the sampler thread.
    sample: Sample,
    sample_rx: mpsc::Receiver<Sample>,
    /// Capacity-1 channel: a full queue means the previous request is still in
    /// flight, so the round is skipped rather than queued up.
    sample_tick: mpsc::SyncSender<()>,
    last_sample_request: Instant,

    /// Last snapshot handed to the engine, used to avoid republishing unchanged
    /// state.
    last_published: Option<StatusSnapshot>,
    last_publish_at: Instant,

    /// Presence the user picked explicitly. `None` lets the pomodoro and idle
    /// detection decide.
    manual_presence: Option<Presence>,

    /// Newest incoming poke, for the overlay animation.
    pub last_nudge: Option<(Nudge, Instant)>,
    /// Set when a newer release is found.
    pub update: Option<synctus_core::update::UpdateInfo>,
    update_rx: Option<mpsc::Receiver<Option<synctus_core::update::UpdateInfo>>>,

    pub show_settings: bool,
    /// Text being edited in the settings pane, applied on save.
    pub draft: Option<ClientConfig>,
}

impl App {
    pub fn new(config_path: PathBuf, data_path: PathBuf) -> Result<Self> {
        let cfg = ClientConfig::load(&config_path)?;
        let mut data = LocalData::load(&data_path)?;
        data.roll_day(synctus_core::now_ms());

        let runtime = tokio::runtime::Builder::new_multi_thread()
            // The client is one socket; a single worker is plenty and keeps
            // idle CPU and memory low.
            .worker_threads(1)
            .enable_all()
            .build()?;

        let pomodoro = Pomodoro::new(cfg.pomodoro, data.round, data.completed_today);
        let (sample_rx, sample_tick) = spawn_sampler();

        let mut app = Self {
            cfg,
            config_path,
            data_path,
            data,
            pomodoro,
            peers: PeerView::default(),
            client: None,
            events: None,
            runtime,
            conn: ConnState::Offline("未连接".into()),
            log: Vec::new(),
            sample: Sample::default(),
            sample_rx,
            sample_tick,
            last_sample_request: Instant::now(),
            last_published: None,
            last_publish_at: Instant::now() - Duration::from_secs(3600),
            manual_presence: None,
            last_nudge: None,
            update: None,
            update_rx: None,
            show_settings: false,
            draft: None,
        };

        if app.cfg.is_paired() {
            app.connect();
        } else {
            app.note("尚未配对：请在设置中填入配对码");
            app.show_settings = true;
        }

        // The registry key or desktop entry can be removed behind our back (by a
        // startup manager, or another install). Trust the OS over the config file
        // so the settings checkbox shows reality.
        match crate::autostart::is_enabled() {
            Ok(actual) if actual != app.cfg.autostart => {
                app.cfg.autostart = actual;
                app.save_config();
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %format!("{e:#}"), "无法查询自启动状态"),
        }

        if app.cfg.check_updates {
            app.start_update_check();
        }

        Ok(app)
    }

    /// (Re)start the client engine with the current config.
    pub fn connect(&mut self) {
        if !self.cfg.is_paired() {
            self.note("配对码为空，无法连接");
            return;
        }

        // Reusing the engine across config changes would mean re-deriving keys
        // inside it; tearing it down is simpler and happens rarely.
        if let Some(handle) = self.client.take() {
            let _ = handle.send(Command::Shutdown);
        }

        match Client::spawn(self.cfg.clone()) {
            Ok((handle, events, client)) => {
                self.runtime.spawn(client.run());
                self.client = Some(handle);
                self.events = Some(events);
                self.conn = ConnState::Connecting;
                // Force a publish on the new connection.
                self.last_published = None;
                self.note(format!("正在连接 {}", self.cfg.server));
            }
            Err(e) => {
                self.conn = ConnState::Rejected(format!("{e:#}"));
                self.note(format!("启动客户端失败: {e:#}"));
            }
        }
    }

    /// Per-frame update: drain channels, advance the timer, publish if needed.
    pub fn tick(&mut self) {
        self.drain_samples();
        self.drain_events();
        self.drain_update();
        self.advance_pomodoro();
        self.request_sample_if_due();
        self.publish_if_changed();
    }

    fn drain_samples(&mut self) {
        while let Ok(sample) = self.sample_rx.try_recv() {
            self.sample = sample;
        }
    }

    fn request_sample_if_due(&mut self) {
        if self.last_sample_request.elapsed() >= self.cfg.poll_interval() {
            self.last_sample_request = Instant::now();
            // A full channel means the sampler is still busy with the previous
            // request, so skipping this round is correct.
            let _ = self.sample_tick.try_send(());
        }
    }

    fn drain_events(&mut self) {
        let Some(events) = self.events.as_mut() else {
            return;
        };

        // Collect first so the borrow on `self.events` ends before we mutate the
        // rest of `self`.
        let mut batch = Vec::new();
        while let Ok(event) = events.try_recv() {
            batch.push(event);
        }

        for event in batch {
            match event {
                Event::State(state) => {
                    match &state {
                        ConnState::Online => self.note("已连接"),
                        ConnState::Offline(why) => {
                            // Peers cannot be trusted as live once the link drops.
                            self.peers.clear();
                            self.note(format!("连接断开: {why}"));
                        }
                        ConnState::Rejected(why) => self.note(format!("连接被拒绝: {why}")),
                        ConnState::Connecting => {}
                    }
                    self.conn = state;
                }
                Event::PeerStatus(snap) => {
                    self.peers.apply_status(snap);
                }
                Event::PeerTodos { device_id, items } => {
                    self.peers.apply_todos(device_id, items);
                }
                Event::Nudge(nudge) => {
                    if !self.cfg.mute_nudges {
                        crate::notify::nudge(&nudge);
                    }
                    self.note(nudge.body());

                    // "一起专注" is the one interaction with a side effect: if we
                    // are idle, start a focus round so the two timers line up.
                    if nudge.kind == NudgeKind::FocusTogether
                        && self.pomodoro.state().phase == PomodoroPhase::Idle
                    {
                        self.handle(UiRequest::StartFocus);
                        self.note("已跟随对方开始专注");
                    }

                    self.last_nudge = Some((nudge, Instant::now()));
                }
                Event::PeerPresence { device_id, online } => {
                    // Only announce a device we have actually seen, so the
                    // Welcome frame's peer list does not produce a toast per
                    // reconnect. Resolve the name first: the `if let` scrutinee
                    // would otherwise hold a borrow of `self.peers`.
                    let name = self
                        .peers
                        .devices()
                        .find(|d| d.device_id == device_id)
                        .map(|d| d.name.clone());
                    if let Some(name) = name {
                        if !self.cfg.mute_nudges {
                            crate::notify::peer_presence(&name, online);
                        }
                        self.note(format!("{name} {}", if online { "上线" } else { "离线" }));
                    }
                    self.peers.set_online(&device_id, online);
                }
                Event::Warning(text) => self.note(text),
            }
        }
    }

    fn advance_pomodoro(&mut self) {
        let now = synctus_core::now_ms();
        match self.pomodoro.tick(now) {
            PomodoroEvent::Nothing => {}
            PomodoroEvent::FocusFinished(next) => {
                self.data.completed_today = self.pomodoro.state().completed_today;
                self.data.round = self.pomodoro.state().round;
                self.save_data();
                crate::notify::focus_finished(next);
                self.note("专注回合完成");
            }
            PomodoroEvent::BreakFinished => {
                crate::notify::break_finished();
                self.note("休息结束");
            }
        }
    }

    /// Presence to publish: an explicit choice wins, then the pomodoro phase,
    /// then idle detection.
    fn effective_presence(&self) -> Presence {
        if let Some(p) = self.manual_presence {
            return p;
        }
        if let Some(p) = self.pomodoro.implied_presence() {
            return p;
        }
        if self.cfg.away_after_secs > 0 {
            if let Some(idle) = self.sample.idle_secs {
                if idle >= self.cfg.away_after_secs {
                    return Presence::Away;
                }
            }
        }
        Presence::Active
    }

    /// Build the snapshot we would publish right now, honouring privacy settings.
    fn build_snapshot(&self) -> StatusSnapshot {
        let privacy = &self.cfg.privacy;
        let mut snap = StatusSnapshot::new(&self.cfg.device_id, &self.cfg.device_name);
        snap.presence = self.effective_presence();

        if privacy.share_foreground_app {
            snap.foreground = self.sample.foreground.clone().map(|mut fg| {
                if privacy.is_blocked(&fg.app) {
                    // Publish that something is in the foreground without saying
                    // what: hiding the field entirely would look like the app
                    // stopped reporting.
                    fg = synctus_core::model::ForegroundApp {
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
            snap.battery = self.sample.battery;
        }
        if privacy.share_music {
            snap.music = self.sample.music.clone();
        }
        if privacy.share_pomodoro {
            snap.pomodoro = Some(self.pomodoro.state());
        }
        if privacy.share_todos {
            snap.todos_open = self.data.open_count();
            snap.todos_done_today = self.data.done_today(snap.at);
        }
        if privacy.share_idle {
            snap.idle_secs = self.sample.idle_secs;
        }
        snap
    }

    /// Publish when something changed, or at least once per heartbeat so the peer
    /// can tell we are alive.
    fn publish_if_changed(&mut self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        if !matches!(self.conn, ConnState::Online) {
            return;
        }

        let snap = self.build_snapshot();
        let changed = match &self.last_published {
            // Compare everything except `at`, which always differs.
            Some(prev) => !snapshot_eq(prev, &snap),
            None => true,
        };
        let stale = self.last_publish_at.elapsed() >= Duration::from_secs(30);

        if (changed || stale) && client.publish(snap.clone()).is_ok() {
            self.last_published = Some(snap);
            self.last_publish_at = Instant::now();
        }
    }

    /// Push the to-do list to the peer.
    fn publish_todos(&mut self) {
        if !self.cfg.privacy.share_todos {
            return;
        }
        if let Some(client) = self.client.as_ref() {
            let _ = client.send(Command::PublishTodos(self.data.todos.clone()));
        }
    }

    /// Handle a request from the tray or the overlay.
    pub fn handle(&mut self, request: UiRequest) -> bool {
        match request {
            UiRequest::Nudge(kind) => {
                self.send_nudge(kind);
                false
            }
            UiRequest::SetPresence(p) => {
                // Selecting "Active" hands control back to the automatic logic.
                self.manual_presence = if p == Presence::Active { None } else { Some(p) };
                self.last_published = None;
                false
            }
            UiRequest::TogglePomodoro => {
                let now = synctus_core::now_ms();
                if self.pomodoro.state().phase == PomodoroPhase::Idle {
                    self.pomodoro.start_focus(now);
                } else {
                    self.pomodoro.toggle_pause(now);
                }
                self.last_published = None;
                false
            }
            UiRequest::StartFocus => {
                self.pomodoro.start_focus(synctus_core::now_ms());
                self.last_published = None;
                false
            }
            UiRequest::StopPomodoro => {
                self.pomodoro.stop();
                self.last_published = None;
                false
            }
            UiRequest::SkipPhase => {
                self.pomodoro.skip(synctus_core::now_ms());
                self.last_published = None;
                false
            }
            UiRequest::ToggleOverlay => {
                self.cfg.show_overlay = !self.cfg.show_overlay;
                self.save_config();
                false
            }
            UiRequest::OpenSettings => {
                self.draft = Some(self.cfg.clone());
                self.show_settings = true;
                false
            }
            UiRequest::CheckUpdate => {
                self.start_update_check();
                false
            }
            UiRequest::Reconnect => {
                self.connect();
                false
            }
            UiRequest::Quit => true,
        }
    }

    pub fn send_nudge(&mut self, kind: NudgeKind) {
        let Some(client) = self.client.as_ref() else {
            self.note("未连接，无法发送互动");
            return;
        };
        let nudge = Nudge::new(kind, self.cfg.device_name.clone());
        match client.nudge(nudge) {
            Ok(()) => self.note(format!("已发送 {}", kind.label())),
            Err(e) => self.note(format!("发送失败: {e:#}")),
        }
    }

    // --- to-dos -----------------------------------------------------------

    pub fn add_todo(&mut self, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        self.data
            .todos
            .push(Todo::new(title, synctus_core::now_ms()));
        self.save_data();
        self.publish_todos();
        self.last_published = None;
    }

    pub fn toggle_todo(&mut self, id: &str) {
        let now = synctus_core::now_ms();
        if let Some(todo) = self.data.todos.iter_mut().find(|t| t.id == id) {
            todo.done = !todo.done;
            todo.done_at = todo.done.then_some(now);
        }
        self.save_data();
        self.publish_todos();
        self.last_published = None;
    }

    pub fn remove_todo(&mut self, id: &str) {
        self.data.todos.retain(|t| t.id != id);
        self.save_data();
        self.publish_todos();
        self.last_published = None;
    }

    /// Drop completed items, which is how the list stays short day to day.
    pub fn clear_done_todos(&mut self) {
        self.data.todos.retain(|t| !t.done);
        self.save_data();
        self.publish_todos();
    }

    // --- persistence ------------------------------------------------------

    pub fn save_config(&mut self) {
        if let Err(e) = self.cfg.save(&self.config_path) {
            self.note(format!("保存配置失败: {e:#}"));
        }
    }

    pub fn save_data(&mut self) {
        self.data.completed_today = self.pomodoro.state().completed_today;
        self.data.round = self.pomodoro.state().round;
        if let Err(e) = self.data.save(&self.data_path) {
            self.note(format!("保存数据失败: {e:#}"));
        }
    }

    /// Apply an edited config: persist it, re-derive keys and reconnect.
    pub fn apply_draft(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };

        let autostart_changed = draft.autostart != self.cfg.autostart;
        let needs_reconnect = draft.server != self.cfg.server
            || draft.tls != self.cfg.tls
            || draft.invite_code != self.cfg.invite_code
            || draft.device_id != self.cfg.device_id;

        self.cfg = draft;
        self.pomodoro.set_config(self.cfg.pomodoro);
        self.save_config();

        if autostart_changed {
            match crate::autostart::set(self.cfg.autostart) {
                Ok(()) => self.note(if self.cfg.autostart {
                    "已启用开机自启"
                } else {
                    "已关闭开机自启"
                }),
                Err(e) => self.note(format!("设置开机自启失败: {e:#}")),
            }
        }

        if needs_reconnect {
            self.connect();
        } else {
            // Privacy toggles change what we publish, so force a refresh.
            self.last_published = None;
        }
        self.show_settings = false;
    }

    // --- updates ----------------------------------------------------------

    fn start_update_check(&mut self) {
        if self.update_rx.is_some() {
            // A check is already in flight.
            return;
        }
        let repo = self.cfg.update_repo.clone();
        let (tx, rx) = mpsc::channel();
        // Blocking HTTP on a detached thread: a slow or unreachable GitHub must
        // never stall the UI.
        std::thread::spawn(move || {
            let result =
                synctus_core::update::check(&repo, synctus_core::update::current_version());
            let _ = tx.send(result.ok().flatten());
        });
        self.update_rx = Some(rx);
    }

    fn drain_update(&mut self) {
        let Some(rx) = self.update_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(info)) => {
                self.note(format!("发现新版本 {}", info.version));
                crate::notify::update_available(&info.version);
                self.update = Some(info);
                self.update_rx = None;
            }
            Ok(None) => {
                self.update_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.update_rx = None;
            }
        }
    }

    // --- misc -------------------------------------------------------------

    /// Append a line to the in-app log, keeping only the recent tail.
    pub fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("{text}");
        self.log.push(LogLine {
            at: Instant::now(),
            text,
        });
        if self.log.len() > 200 {
            self.log.drain(..self.log.len() - 200);
        }
    }

    pub fn peer_presence(&self) -> Presence {
        self.peers
            .presence(synctus_core::now_ms(), self.cfg.peer_stale_ms())
    }

    pub fn peer(&self) -> Option<&StatusSnapshot> {
        self.peers
            .primary(synctus_core::now_ms(), self.cfg.peer_stale_ms())
    }

    pub fn own_presence(&self) -> Presence {
        self.effective_presence()
    }

    pub fn is_online(&self) -> bool {
        matches!(self.conn, ConnState::Online)
    }

    /// Called on exit so nothing is lost and the peer sees us go offline.
    pub fn shutdown(&mut self) {
        self.save_data();
        self.save_config();
        if let Some(client) = self.client.take() {
            let _ = client.send(Command::Shutdown);
        }
    }
}

/// Compare two snapshots ignoring the timestamp, so an unchanged status is not
/// republished every poll.
fn snapshot_eq(a: &StatusSnapshot, b: &StatusSnapshot) -> bool {
    a.presence == b.presence
        && a.foreground == b.foreground
        && a.battery == b.battery
        && a.music == b.music
        && a.pomodoro == b.pomodoro
        && a.todos_open == b.todos_open
        && a.todos_done_today == b.todos_done_today
        // Idle seconds change constantly; only a coarse bucket is worth resending.
        && idle_bucket(a.idle_secs) == idle_bucket(b.idle_secs)
}

/// Bucket idle seconds so a ticking counter does not cause a publish per poll.
fn idle_bucket(idle: Option<u32>) -> Option<u32> {
    idle.map(|s| s / 60)
}

/// Sensor sampling thread.
///
/// Sampling touches WinRT and D-Bus, which can block for tens of milliseconds;
/// doing it on the UI thread would show up as stutter in the overlay.
fn spawn_sampler() -> (mpsc::Receiver<Sample>, mpsc::SyncSender<()>) {
    let (sample_tx, sample_rx) = mpsc::channel();
    let (tick_tx, tick_rx) = mpsc::sync_channel::<()>(1);

    std::thread::Builder::new()
        .name("synctus-sensors".into())
        .spawn(move || {
            // Sample once immediately so the first frame has data.
            let _ = sample_tx.send(sensors::sample());
            // Ends when the App is dropped and the sender goes away.
            while tick_rx.recv().is_ok() {
                if sample_tx.send(sensors::sample()).is_err() {
                    return;
                }
            }
        })
        .expect("spawn sensor thread");

    (sample_rx, tick_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use synctus_core::model::{Battery, ForegroundApp};

    fn snap() -> StatusSnapshot {
        StatusSnapshot::new("dev", "Name")
    }

    #[test]
    fn timestamp_alone_does_not_count_as_a_change() {
        let a = snap();
        let mut b = a.clone();
        b.at += 10_000;
        assert!(snapshot_eq(&a, &b));
    }

    #[test]
    fn presence_and_battery_changes_are_detected() {
        let a = snap();
        let mut b = a.clone();
        b.presence = Presence::Resting;
        assert!(!snapshot_eq(&a, &b));

        let mut c = a.clone();
        c.battery = Some(Battery {
            percent: 50,
            charging: false,
            minutes_left: None,
        });
        assert!(!snapshot_eq(&a, &c));
    }

    #[test]
    fn idle_seconds_only_matter_per_minute() {
        let mut a = snap();
        a.idle_secs = Some(10);
        let mut b = a.clone();
        b.idle_secs = Some(50);
        assert!(
            snapshot_eq(&a, &b),
            "same minute must not trigger a publish"
        );

        b.idle_secs = Some(130);
        assert!(!snapshot_eq(&a, &b));
    }

    #[test]
    fn foreground_app_changes_are_detected() {
        let a = snap();
        let mut b = a.clone();
        b.foreground = Some(ForegroundApp {
            app: "code.exe".into(),
            name: Some("code".into()),
            title: None,
        });
        assert!(!snapshot_eq(&a, &b));
    }

    #[test]
    fn idle_bucket_rounds_down_to_minutes() {
        assert_eq!(idle_bucket(Some(0)), Some(0));
        assert_eq!(idle_bucket(Some(59)), Some(0));
        assert_eq!(idle_bucket(Some(61)), Some(1));
        assert_eq!(idle_bucket(None), None);
    }
}
