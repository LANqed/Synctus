//! Local state: to-do list, pomodoro engine and the peer view.
//!
//! All of it is UI-framework agnostic and synchronously testable — the desktop
//! and Android layers only translate user input into calls here and render the
//! result.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::config::PomodoroConfig;
use crate::model::{PomodoroPhase, PomodoroState, Presence, StatusSnapshot, Todo};

/// To-do list plus the pomodoro counters that survive a restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalData {
    pub todos: Vec<Todo>,
    /// Focus rounds finished today.
    pub completed_today: u32,
    /// Local date (`YYYY-MM-DD` in UTC) the counter belongs to.
    pub counter_date: String,
    /// Rounds completed in the current set, for long-break scheduling.
    pub round: u32,
}

impl LocalData {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("解析数据失败: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("读取数据失败: {}", path.display())),
        }
    }

    /// Atomic save, same reasoning as [`crate::config::ClientConfig::save`].
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn open_count(&self) -> u32 {
        self.todos.iter().filter(|t| !t.done).count() as u32
    }

    pub fn done_today(&self, now: i64) -> u32 {
        let today = utc_date(now);
        self.todos
            .iter()
            .filter(|t| t.done && t.done_at.map(|d| utc_date(d) == today).unwrap_or(false))
            .count() as u32
    }

    /// Reset the daily counter when the date rolled over. Call before reading
    /// the counters so a session that spans midnight starts a fresh day.
    pub fn roll_day(&mut self, now: i64) {
        let today = utc_date(now);
        if self.counter_date != today {
            self.counter_date = today;
            self.completed_today = 0;
            self.round = 0;
        }
    }
}

/// `YYYY-MM-DD` for a unix-ms timestamp, in UTC.
///
/// Hand-rolled to avoid pulling in `chrono` for one function; the counter only
/// needs a stable day boundary, not calendar correctness in every timezone.
fn utc_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // 1970-01-01 as a civil date, algorithm from Howard Hinnant's date library.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// What happened on the last [`Pomodoro::tick`], so the UI knows when to notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroEvent {
    Nothing,
    /// A focus round finished; payload is the phase that follows.
    FocusFinished(PomodoroPhase),
    /// A break finished.
    BreakFinished,
}

/// The pomodoro timer.
///
/// Deadline-based rather than tick-based: the timer stays correct across sleep,
/// process suspension and Android's doze mode, and only the deadline is synced.
#[derive(Debug, Clone)]
pub struct Pomodoro {
    cfg: PomodoroConfig,
    state: PomodoroState,
}

impl Pomodoro {
    pub fn new(cfg: PomodoroConfig, round: u32, completed_today: u32) -> Self {
        Self {
            cfg,
            state: PomodoroState {
                round,
                completed_today,
                ..Default::default()
            },
        }
    }

    pub fn state(&self) -> PomodoroState {
        self.state
    }

    pub fn config(&self) -> PomodoroConfig {
        self.cfg
    }

    pub fn set_config(&mut self, cfg: PomodoroConfig) {
        self.cfg = cfg;
    }

    fn phase_minutes(&self, phase: PomodoroPhase) -> u32 {
        match phase {
            PomodoroPhase::Focus => self.cfg.focus_min.max(1),
            PomodoroPhase::ShortBreak => self.cfg.short_break_min.max(1),
            PomodoroPhase::LongBreak => self.cfg.long_break_min.max(1),
            PomodoroPhase::Idle => 0,
        }
    }

    /// Begin `phase` at `now`.
    pub fn start(&mut self, phase: PomodoroPhase, now: i64) {
        if phase == PomodoroPhase::Idle {
            self.stop();
            return;
        }
        self.state.phase = phase;
        self.state.paused_left_ms = None;
        self.state.ends_at = Some(now + self.phase_minutes(phase) as i64 * 60_000);
    }

    /// Start a focus round.
    pub fn start_focus(&mut self, now: i64) {
        self.start(PomodoroPhase::Focus, now);
    }

    /// Freeze the countdown, keeping the remaining time.
    pub fn pause(&mut self, now: i64) {
        if self.state.phase != PomodoroPhase::Idle && !self.state.paused() {
            self.state.paused_left_ms = Some(self.state.remaining_ms(now));
        }
    }

    /// Resume from a pause by pushing the deadline out.
    pub fn resume(&mut self, now: i64) {
        if let Some(left) = self.state.paused_left_ms.take() {
            self.state.ends_at = Some(now + left);
        }
    }

    pub fn toggle_pause(&mut self, now: i64) {
        if self.state.paused() {
            self.resume(now);
        } else {
            self.pause(now);
        }
    }

    /// Cancel the timer, keeping today's counters.
    pub fn stop(&mut self) {
        self.state.phase = PomodoroPhase::Idle;
        self.state.ends_at = None;
        self.state.paused_left_ms = None;
    }

    /// End the current phase immediately, as if the deadline had passed.
    pub fn skip(&mut self, now: i64) -> PomodoroEvent {
        if self.state.phase == PomodoroPhase::Idle {
            return PomodoroEvent::Nothing;
        }
        self.state.paused_left_ms = None;
        self.state.ends_at = Some(now);
        self.tick(now)
    }

    /// Advance the state machine. Call at any cadence; correctness depends only
    /// on `now`.
    pub fn tick(&mut self, now: i64) -> PomodoroEvent {
        if self.state.phase == PomodoroPhase::Idle || self.state.paused() {
            return PomodoroEvent::Nothing;
        }
        if self.state.remaining_ms(now) > 0 {
            return PomodoroEvent::Nothing;
        }

        match self.state.phase {
            PomodoroPhase::Focus => {
                self.state.completed_today = self.state.completed_today.saturating_add(1);
                self.state.round = self.state.round.saturating_add(1);

                let rounds = self.cfg.rounds_per_set.max(1);
                let next = if self.state.round % rounds == 0 {
                    PomodoroPhase::LongBreak
                } else {
                    PomodoroPhase::ShortBreak
                };

                if self.cfg.auto_continue {
                    self.start(next, now);
                } else {
                    self.stop();
                }
                PomodoroEvent::FocusFinished(next)
            }
            PomodoroPhase::ShortBreak | PomodoroPhase::LongBreak => {
                if self.cfg.auto_continue {
                    self.start(PomodoroPhase::Focus, now);
                } else {
                    self.stop();
                }
                PomodoroEvent::BreakFinished
            }
            PomodoroPhase::Idle => PomodoroEvent::Nothing,
        }
    }

    /// Presence implied by the current phase, when the option is enabled.
    pub fn implied_presence(&self) -> Option<Presence> {
        if !self.cfg.presence_follows_phase {
            return None;
        }
        match self.state.phase {
            PomodoroPhase::Focus => Some(Presence::Busy),
            PomodoroPhase::ShortBreak | PomodoroPhase::LongBreak => Some(Presence::Resting),
            PomodoroPhase::Idle => None,
        }
    }
}

/// The peer side of the world: latest snapshot and to-dos per device.
#[derive(Debug, Default)]
pub struct PeerView {
    devices: HashMap<String, StatusSnapshot>,
    todos: HashMap<String, Vec<Todo>>,
    online: HashMap<String, bool>,
}

impl PeerView {
    /// Store a snapshot, ignoring one older than what we already have.
    ///
    /// Out-of-order delivery is possible after a reconnect, when the relay
    /// replays a retained status while a fresh one is already in flight.
    pub fn apply_status(&mut self, snap: StatusSnapshot) -> bool {
        let id = snap.device_id.clone();
        if let Some(existing) = self.devices.get(&id) {
            if existing.at > snap.at {
                return false;
            }
        }
        self.online.insert(id.clone(), true);
        self.devices.insert(id, snap);
        true
    }

    pub fn apply_todos(&mut self, device_id: String, items: Vec<Todo>) {
        self.todos.insert(device_id, items);
    }

    pub fn set_online(&mut self, device_id: &str, online: bool) {
        self.online.insert(device_id.to_string(), online);
    }

    /// Drop everything; used when the connection is lost so the UI does not show
    /// stale peers as live.
    pub fn clear(&mut self) {
        self.online.values_mut().for_each(|v| *v = false);
    }

    pub fn devices(&self) -> impl Iterator<Item = &StatusSnapshot> {
        self.devices.values()
    }

    pub fn todos(&self, device_id: &str) -> &[Todo] {
        self.todos
            .get(device_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The peer we display: most recently updated device that is not stale.
    ///
    /// A person may run Synctus on both a PC and a phone; showing the freshest
    /// one avoids flapping between them.
    pub fn primary(&self, now: i64, stale_ms: i64) -> Option<&StatusSnapshot> {
        self.devices
            .values()
            .filter(|s| self.online.get(&s.device_id).copied().unwrap_or(false))
            .filter(|s| !s.is_stale(now, stale_ms))
            .max_by_key(|s| s.at)
            .or_else(|| self.devices.values().max_by_key(|s| s.at))
    }

    /// Effective presence, downgraded to `Offline` when the data is stale.
    pub fn presence(&self, now: i64, stale_ms: i64) -> Presence {
        match self.primary(now, stale_ms) {
            Some(s) if !s.is_stale(now, stale_ms) => s.presence,
            _ => Presence::Offline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn cfg() -> PomodoroConfig {
        PomodoroConfig {
            focus_min: 25,
            short_break_min: 5,
            long_break_min: 15,
            rounds_per_set: 4,
            auto_continue: false,
            presence_follows_phase: true,
        }
    }

    #[test]
    fn focus_completes_and_suggests_a_short_break() {
        let mut p = Pomodoro::new(cfg(), 0, 0);
        p.start_focus(0);
        assert_eq!(p.tick(10 * MIN), PomodoroEvent::Nothing);

        let ev = p.tick(25 * MIN);
        assert_eq!(ev, PomodoroEvent::FocusFinished(PomodoroPhase::ShortBreak));
        assert_eq!(p.state().completed_today, 1);
        // auto_continue is off, so the timer parks in Idle.
        assert_eq!(p.state().phase, PomodoroPhase::Idle);
    }

    #[test]
    fn fourth_round_suggests_a_long_break() {
        let mut c = cfg();
        c.auto_continue = true;
        let mut p = Pomodoro::new(c, 0, 0);

        let mut now = 0;
        let mut last = PomodoroEvent::Nothing;
        for _ in 0..4 {
            p.start_focus(now);
            now += 25 * MIN;
            last = p.tick(now);
            // Skip past whatever break was started automatically.
            p.stop();
        }
        assert_eq!(last, PomodoroEvent::FocusFinished(PomodoroPhase::LongBreak));
        assert_eq!(p.state().round, 4);
    }

    #[test]
    fn pause_freezes_and_resume_extends_the_deadline() {
        let mut p = Pomodoro::new(cfg(), 0, 0);
        p.start_focus(0);
        p.pause(5 * MIN);
        assert_eq!(p.state().remaining_ms(60 * MIN), 20 * MIN);

        p.resume(60 * MIN);
        assert_eq!(p.state().remaining_ms(60 * MIN), 20 * MIN);
        assert_eq!(p.state().ends_at, Some(80 * MIN));
        assert_eq!(p.tick(70 * MIN), PomodoroEvent::Nothing);
    }

    #[test]
    fn skip_ends_the_phase_now() {
        let mut p = Pomodoro::new(cfg(), 0, 0);
        p.start_focus(0);
        assert!(matches!(p.skip(MIN), PomodoroEvent::FocusFinished(_)));
    }

    #[test]
    fn presence_follows_phase_when_enabled() {
        let mut p = Pomodoro::new(cfg(), 0, 0);
        assert_eq!(p.implied_presence(), None);
        p.start_focus(0);
        assert_eq!(p.implied_presence(), Some(Presence::Busy));
        p.start(PomodoroPhase::LongBreak, 0);
        assert_eq!(p.implied_presence(), Some(Presence::Resting));

        let mut c = cfg();
        c.presence_follows_phase = false;
        p.set_config(c);
        assert_eq!(p.implied_presence(), None);
    }

    #[test]
    fn older_snapshots_are_ignored() {
        let mut view = PeerView::default();
        let mut new = StatusSnapshot::new("dev", "Peer");
        new.at = 1_000;
        assert!(view.apply_status(new.clone()));

        let mut old = new.clone();
        old.at = 500;
        old.presence = Presence::Away;
        assert!(!view.apply_status(old));
        assert_eq!(
            view.primary(1_000, 60_000).unwrap().presence,
            Presence::Active
        );
    }

    #[test]
    fn stale_peer_reads_as_offline() {
        let mut view = PeerView::default();
        let mut snap = StatusSnapshot::new("dev", "Peer");
        snap.at = 0;
        view.apply_status(snap);
        assert_eq!(view.presence(10_000, 90_000), Presence::Active);
        assert_eq!(view.presence(200_000, 90_000), Presence::Offline);
    }

    #[test]
    fn freshest_device_wins_as_primary() {
        let mut view = PeerView::default();
        let mut pc = StatusSnapshot::new("pc", "PC");
        pc.at = 1_000;
        let mut phone = StatusSnapshot::new("phone", "Phone");
        phone.at = 2_000;
        view.apply_status(pc);
        view.apply_status(phone);
        assert_eq!(view.primary(2_000, 90_000).unwrap().device_id, "phone");
    }

    #[test]
    fn done_today_counts_only_today() {
        let day = 86_400_000;
        let data = LocalData {
            todos: vec![
                Todo {
                    id: "a".into(),
                    title: "x".into(),
                    done: true,
                    created_at: 0,
                    done_at: Some(day + 100),
                    pomodoros: 0,
                },
                Todo {
                    id: "b".into(),
                    title: "y".into(),
                    done: true,
                    created_at: 0,
                    done_at: Some(100),
                    pomodoros: 0,
                },
                Todo {
                    id: "c".into(),
                    title: "z".into(),
                    done: false,
                    created_at: 0,
                    done_at: None,
                    pomodoros: 0,
                },
            ],
            ..Default::default()
        };
        assert_eq!(data.done_today(day + 500), 1);
        assert_eq!(data.open_count(), 1);
    }

    #[test]
    fn utc_date_matches_known_epochs() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_700_000_000_000), "2023-11-14");
    }

    #[test]
    fn day_rollover_resets_counters() {
        let mut data = LocalData {
            completed_today: 5,
            round: 3,
            counter_date: "1970-01-01".into(),
            ..Default::default()
        };
        data.roll_day(86_400_000 * 2);
        assert_eq!(data.completed_today, 0);
        assert_eq!(data.round, 0);
        assert_eq!(data.counter_date, "1970-01-03");
    }
}
