//! Distraction detection.
//!
//! This is the part that turns a status widget into something that actually
//! keeps two people working: it notices when you open a time sink *during a focus
//! round* and says so.
//!
//! Two deliberate design choices:
//!
//! * **A grace period, not an instant alarm.** Alt-tabbing past a browser to
//!   check a reference is not slacking off. Only a distracting app that *stays*
//!   in the foreground counts, so the detector tracks how long it has been there
//!   rather than reacting to a single sample.
//! * **Nothing fires outside a focus round.** Watching someone's leisure time is
//!   surveillance, not accountability. The detector is inert unless the pomodoro
//!   says they chose to be focusing.

use crate::config::Accountability;
use crate::model::ForegroundApp;

/// What the detector concluded on the latest sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distraction {
    /// Nothing to report: not focusing, or the foreground app is fine.
    None,
    /// A distracting app is in the foreground but still inside the grace period.
    /// Not worth a notification, but the UI can show a countdown.
    Pending { app: String, secs: u32 },
    /// Crossed the grace period. Emitted **once** per continuous stretch, so the
    /// caller does not have to debounce notifications itself.
    Started { app: String },
    /// Back to work after a reported distraction, with how long it lasted.
    Ended { app: String, secs: u32 },
}

/// Tracks the foreground app across samples.
///
/// Time is passed in rather than read from the clock so the whole thing is
/// testable without sleeping.
#[derive(Debug, Default)]
pub struct DistractionTracker {
    /// The distracting app currently in the foreground, and when it started.
    current: Option<Current>,
}

#[derive(Debug)]
struct Current {
    app: String,
    since_ms: i64,
    /// Whether [`Distraction::Started`] was already emitted for this stretch.
    reported: bool,
}

impl DistractionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one sample.
    ///
    /// `focusing` comes from the local pomodoro; when it is false the tracker
    /// resets, because a distraction that ends when the round does is not a
    /// distraction.
    pub fn update(
        &mut self,
        foreground: Option<&ForegroundApp>,
        focusing: bool,
        cfg: &Accountability,
        now_ms: i64,
    ) -> Distraction {
        if !focusing || !cfg.warn_on_distraction {
            return self.reset(now_ms);
        }

        let Some(app) = foreground.map(|f| f.app.as_str()) else {
            // No foreground app at all (locked screen, Wayland, no permission).
            // Absence of evidence is not evidence of slacking.
            return self.reset(now_ms);
        };

        if !cfg.is_distracting(app) {
            return self.reset(now_ms);
        }

        let grace_ms = cfg.distraction_grace_secs as i64 * 1000;

        match &mut self.current {
            // Switched from one distracting app to another: restart the clock, so
            // hopping between two of them does not accumulate into a report.
            Some(current) if current.app != app => {
                let ended = current.reported.then(|| Distraction::Ended {
                    app: current.app.clone(),
                    secs: elapsed_secs(current.since_ms, now_ms),
                });
                self.current = Some(Current {
                    app: app.to_string(),
                    since_ms: now_ms,
                    reported: false,
                });
                ended.unwrap_or(Distraction::Pending {
                    app: app.to_string(),
                    secs: 0,
                })
            }

            Some(current) => {
                let elapsed = now_ms.saturating_sub(current.since_ms);
                if current.reported {
                    // Already reported; stay quiet until it ends.
                    Distraction::None
                } else if elapsed >= grace_ms {
                    current.reported = true;
                    Distraction::Started {
                        app: current.app.clone(),
                    }
                } else {
                    Distraction::Pending {
                        app: current.app.clone(),
                        secs: (elapsed / 1000) as u32,
                    }
                }
            }

            None => {
                self.current = Some(Current {
                    app: app.to_string(),
                    since_ms: now_ms,
                    reported: false,
                });
                // A zero-second grace period should fire immediately rather than
                // wait a whole sample interval.
                if grace_ms == 0 {
                    if let Some(current) = &mut self.current {
                        current.reported = true;
                        return Distraction::Started {
                            app: current.app.clone(),
                        };
                    }
                }
                Distraction::Pending {
                    app: app.to_string(),
                    secs: 0,
                }
            }
        }
    }

    /// Clear the current stretch, reporting its end if it was ever announced.
    fn reset(&mut self, now_ms: i64) -> Distraction {
        match self.current.take() {
            Some(current) if current.reported => Distraction::Ended {
                app: current.app,
                secs: elapsed_secs(current.since_ms, now_ms),
            },
            _ => Distraction::None,
        }
    }

    /// The app currently being watched, if any. For the UI's countdown.
    pub fn current_app(&self) -> Option<&str> {
        self.current.as_ref().map(|c| c.app.as_str())
    }

    /// Whether a distraction has been reported and is still ongoing.
    pub fn is_distracted(&self) -> bool {
        self.current.as_ref().map(|c| c.reported).unwrap_or(false)
    }
}

fn elapsed_secs(since_ms: i64, now_ms: i64) -> u32 {
    (now_ms.saturating_sub(since_ms) / 1000).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Accountability {
        Accountability {
            warn_on_distraction: true,
            distracting_apps: vec!["bilibili".into(), "steam".into()],
            distraction_grace_secs: 30,
            ..Accountability::default()
        }
    }

    fn app(name: &str) -> ForegroundApp {
        ForegroundApp {
            app: name.to_string(),
            name: None,
            title: None,
        }
    }

    #[test]
    fn nothing_fires_outside_a_focus_round() {
        let mut t = DistractionTracker::new();
        let bili = app("bilibili.exe");
        // Not focusing: leisure time is not the tool's business.
        for now in [0, 60_000, 600_000] {
            assert_eq!(t.update(Some(&bili), false, &cfg(), now), Distraction::None);
        }
    }

    #[test]
    fn a_brief_visit_stays_within_the_grace_period() {
        let mut t = DistractionTracker::new();
        let bili = app("bilibili.exe");
        let c = cfg();

        assert_eq!(
            t.update(Some(&bili), true, &c, 0),
            Distraction::Pending {
                app: "bilibili.exe".into(),
                secs: 0
            }
        );
        assert_eq!(
            t.update(Some(&bili), true, &c, 10_000),
            Distraction::Pending {
                app: "bilibili.exe".into(),
                secs: 10
            }
        );
        // Back to work before the grace period expired: nothing was reported, so
        // nothing needs to end.
        assert_eq!(
            t.update(Some(&app("code.exe")), true, &c, 20_000),
            Distraction::None
        );
    }

    #[test]
    fn staying_past_the_grace_period_reports_once() {
        let mut t = DistractionTracker::new();
        let bili = app("bilibili.exe");
        let c = cfg();

        t.update(Some(&bili), true, &c, 0);
        assert_eq!(
            t.update(Some(&bili), true, &c, 30_000),
            Distraction::Started {
                app: "bilibili.exe".into()
            }
        );
        // Subsequent samples must stay quiet rather than notify every poll.
        assert_eq!(t.update(Some(&bili), true, &c, 60_000), Distraction::None);
        assert_eq!(t.update(Some(&bili), true, &c, 120_000), Distraction::None);
        assert!(t.is_distracted());
    }

    #[test]
    fn returning_to_work_ends_a_reported_distraction() {
        let mut t = DistractionTracker::new();
        let bili = app("bilibili.exe");
        let c = cfg();

        t.update(Some(&bili), true, &c, 0);
        t.update(Some(&bili), true, &c, 30_000);

        assert_eq!(
            t.update(Some(&app("code.exe")), true, &c, 95_000),
            Distraction::Ended {
                app: "bilibili.exe".into(),
                secs: 95
            }
        );
        assert!(!t.is_distracted());
        // And stays quiet afterwards.
        assert_eq!(
            t.update(Some(&app("code.exe")), true, &c, 100_000),
            Distraction::None
        );
    }

    #[test]
    fn ending_the_focus_round_ends_the_distraction() {
        let mut t = DistractionTracker::new();
        let bili = app("bilibili.exe");
        let c = cfg();

        t.update(Some(&bili), true, &c, 0);
        t.update(Some(&bili), true, &c, 30_000);

        // Pomodoro stopped: the stretch closes rather than hanging around.
        assert_eq!(
            t.update(Some(&bili), false, &c, 45_000),
            Distraction::Ended {
                app: "bilibili.exe".into(),
                secs: 45
            }
        );
    }

    #[test]
    fn hopping_between_distractions_restarts_the_clock() {
        let mut t = DistractionTracker::new();
        let c = cfg();

        t.update(Some(&app("bilibili.exe")), true, &c, 0);
        // Switch at 20s, before the grace period would have expired.
        assert_eq!(
            t.update(Some(&app("steam.exe")), true, &c, 20_000),
            Distraction::Pending {
                app: "steam.exe".into(),
                secs: 0
            }
        );
        // The 30s grace period is measured from the switch, not from the start.
        assert_eq!(t.update(Some(&app("steam.exe")), true, &c, 40_000), {
            Distraction::Pending {
                app: "steam.exe".into(),
                secs: 20,
            }
        });
        assert_eq!(
            t.update(Some(&app("steam.exe")), true, &c, 50_000),
            Distraction::Started {
                app: "steam.exe".into()
            }
        );
    }

    #[test]
    fn switching_after_a_report_closes_the_previous_stretch() {
        let mut t = DistractionTracker::new();
        let c = cfg();

        t.update(Some(&app("bilibili.exe")), true, &c, 0);
        t.update(Some(&app("bilibili.exe")), true, &c, 30_000);

        assert_eq!(
            t.update(Some(&app("steam.exe")), true, &c, 60_000),
            Distraction::Ended {
                app: "bilibili.exe".into(),
                secs: 60
            }
        );
        // The new app is now being tracked from scratch.
        assert_eq!(t.current_app(), Some("steam.exe"));
        assert!(!t.is_distracted());
    }

    #[test]
    fn a_missing_foreground_app_is_not_treated_as_slacking() {
        let mut t = DistractionTracker::new();
        let c = cfg();
        // Wayland, a locked screen or a missing permission all give `None`.
        assert_eq!(t.update(None, true, &c, 0), Distraction::None);
        assert_eq!(t.update(None, true, &c, 60_000), Distraction::None);
    }

    #[test]
    fn matching_is_case_insensitive_and_by_substring() {
        let mut t = DistractionTracker::new();
        let c = cfg();
        // Package names, window class names and executables all differ in case
        // and decoration; substring matching is what makes one list work
        // everywhere.
        t.update(Some(&app("com.BiliBili.app")), true, &c, 0);
        assert_eq!(
            t.update(Some(&app("com.BiliBili.app")), true, &c, 30_000),
            Distraction::Started {
                app: "com.BiliBili.app".into()
            }
        );
    }

    #[test]
    fn disabling_the_warning_makes_the_tracker_inert() {
        let mut t = DistractionTracker::new();
        let c = Accountability {
            warn_on_distraction: false,
            ..cfg()
        };
        t.update(Some(&app("bilibili.exe")), true, &c, 0);
        assert_eq!(
            t.update(Some(&app("bilibili.exe")), true, &c, 300_000),
            Distraction::None
        );
    }

    #[test]
    fn zero_grace_period_reports_immediately() {
        let mut t = DistractionTracker::new();
        let c = Accountability {
            distraction_grace_secs: 0,
            ..cfg()
        };
        assert_eq!(
            t.update(Some(&app("steam.exe")), true, &c, 0),
            Distraction::Started {
                app: "steam.exe".into()
            }
        );
    }

    #[test]
    fn an_empty_list_never_matches() {
        let mut t = DistractionTracker::new();
        let c = Accountability {
            distracting_apps: Vec::new(),
            ..cfg()
        };
        t.update(Some(&app("bilibili.exe")), true, &c, 0);
        assert_eq!(
            t.update(Some(&app("bilibili.exe")), true, &c, 60_000),
            Distraction::None
        );
    }
}
