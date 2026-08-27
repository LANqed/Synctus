//! Desktop notifications for peer interactions and pomodoro transitions.
//!
//! Uses the platform notification service (Action Center on Windows, the
//! freedesktop spec on Linux) rather than drawing our own toast, so
//! do-not-disturb and notification history behave as the user expects.

use synctus_core::model::{Nudge, PomodoroPhase};

const APP_NAME: &str = "Synctus";

/// How loudly a notification asks to be noticed.
///
/// Windows toasts are *silent unless a sound is named*: `notify-rust` maps a
/// missing `sound_name` to `<audio silent="true" />`. So the sound has to be
/// chosen here for both platforms rather than left to the default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Alert {
    /// Informational; must not interrupt.
    Silent,
    /// A short chime, for pokes and peer events.
    Chime,
    /// The pomodoro boundary cue: louder, and meant to be heard from across the
    /// room when the user has already looked away from the screen.
    Reminder,
}

impl Alert {
    /// Freedesktop sound theme name, played by the notification daemon.
    #[cfg(target_os = "linux")]
    fn xdg_name(self) -> Option<&'static str> {
        match self {
            Alert::Silent => None,
            Alert::Chime => Some("message-new-instant"),
            Alert::Reminder => Some("alarm-clock-elapsed"),
        }
    }

    /// Windows toast sound, one of the names `winrt-notification` accepts.
    #[cfg(windows)]
    fn toast_name(self) -> Option<&'static str> {
        match self {
            Alert::Silent => None,
            Alert::Chime => Some("IM"),
            Alert::Reminder => Some("Reminder"),
        }
    }
}

/// Show a notification. Failures are logged, never propagated: a missing
/// notification daemon must not break the sync loop.
fn show(summary: &str, body: &str, alert: Alert) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(APP_NAME)
        .summary(summary)
        .body(body)
        .timeout(notify_rust::Timeout::Milliseconds(6_000));

    // The freedesktop hints are meaningless on Windows, where the crate uses the
    // toast API instead.
    #[cfg(target_os = "linux")]
    {
        notification
            .hint(notify_rust::Hint::Category("im.received".to_string()))
            .urgency(if alert == Alert::Reminder {
                notify_rust::Urgency::Critical
            } else {
                notify_rust::Urgency::Normal
            });
        if let Some(sound) = alert.xdg_name() {
            notification.sound_name(sound);
        }
    }

    #[cfg(windows)]
    if let Some(sound) = alert.toast_name() {
        notification.sound_name(sound);
    }

    if let Err(e) = notification.show() {
        tracing::debug!(error = %e, "发送系统通知失败");
        // The toast never appeared, so its sound never played either. A plain
        // `MessageBeep` still gets the user's attention, which is the part of
        // "带提示音" that must not depend on the notification service.
        #[cfg(windows)]
        if alert != Alert::Silent {
            beep();
        }
    }
}

/// Last-resort audible cue when the toast itself could not be shown.
///
/// Declared by hand rather than pulled from `windows-sys`: the crate puts
/// `MessageBeep` behind `Win32_System_Diagnostics_Debug`, and enabling that
/// feature makes the `windows-gnu` toolchain reach for `dlltool.exe`, which a
/// stock MSVC-oriented install does not ship. `user32` is already linked.
#[cfg(windows)]
fn beep() {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBeep(utype: u32) -> i32;
    }

    // `MB_ICONASTERISK`: the system notification sound.
    const MB_ICONASTERISK: u32 = 0x40;
    unsafe {
        let _ = MessageBeep(MB_ICONASTERISK);
    }
}

/// Announce an incoming poke.
pub fn nudge(nudge: &Nudge) {
    show(
        &format!("{} {}", nudge.kind.emoji(), nudge.from_name),
        &nudge.body(),
        Alert::Chime,
    );
}

/// Announce the end of a focus round: work is over, go and rest.
pub fn focus_finished(next: PomodoroPhase, minutes: u32) {
    show(
        "🍅 番茄钟工作结束了",
        &format!(
            "专注了 {minutes} 分钟，该休息了（接下来是{}）",
            next.label()
        ),
        Alert::Reminder,
    );
}

/// Warn that the break is about to end, so the user is back at the keyboard when
/// the next round starts rather than fifteen seconds into it.
pub fn break_ending_soon(seconds: u32) {
    show(
        "☕ 休息时间快结束了",
        &format!("还剩 {seconds} 秒，尽快转换为工作状态"),
        Alert::Reminder,
    );
}

/// Announce the end of a break.
pub fn break_finished() {
    show(
        "☕ 休息结束",
        "回到工作状态，开始下一个专注回合",
        Alert::Reminder,
    );
}

/// Celebrate meeting the daily goal.
pub fn goal_reached(goal_min: u32, streak_days: u32) {
    let body = if streak_days > 1 {
        format!("今天的 {goal_min} 分钟完成了，已连续 {streak_days} 天")
    } else {
        format!("今天的 {goal_min} 分钟完成了")
    };
    show("🎉 达成目标", &body, Alert::Chime);
}

/// Warn that a distracting app is open during a focus round.
///
/// Aimed at the user themselves: catching yourself is the point, and this fires
/// whether or not the peer is told.
pub fn distraction(app: &str, remaining: &str) {
    show(
        "👀 还在专注中",
        &format!("{app} 打开了，这一轮还剩 {remaining}"),
        Alert::Chime,
    );
}

/// Tell the user a newer release exists. Deliberately does not auto-install.
pub fn update_available(version: &str) {
    show(
        "有新版本可用",
        &format!("Synctus {version} 已发布，可在托盘菜单中打开下载页"),
        Alert::Silent,
    );
}

/// Report a peer coming online or going offline.
pub fn peer_presence(name: &str, online: bool) {
    if online {
        show("对方上线了", &format!("{name} 已连接"), Alert::Silent);
    } else {
        show("对方离线了", &format!("{name} 已断开"), Alert::Silent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synctus_core::model::NudgeKind;

    #[test]
    fn nudge_notifications_do_not_panic_without_a_daemon() {
        // On a headless CI box there is no notification service; `show` must
        // swallow that.
        let n = Nudge::new(NudgeKind::Knock, "对方");
        nudge(&n);
    }

    #[test]
    fn pomodoro_notifications_do_not_panic() {
        focus_finished(PomodoroPhase::ShortBreak, 25);
        break_ending_soon(15);
        break_finished();
        update_available("9.9.9");
    }

    /// The pomodoro cues are the ones the user asked to be audible, and a Windows
    /// toast is silent unless a sound is named, so the mapping is worth pinning.
    #[test]
    fn only_informational_alerts_are_silent() {
        #[cfg(windows)]
        {
            assert_eq!(Alert::Reminder.toast_name(), Some("Reminder"));
            assert_eq!(Alert::Chime.toast_name(), Some("IM"));
            assert_eq!(Alert::Silent.toast_name(), None);
        }
        #[cfg(target_os = "linux")]
        {
            assert!(Alert::Reminder.xdg_name().is_some());
            assert!(Alert::Chime.xdg_name().is_some());
            assert_eq!(Alert::Silent.xdg_name(), None);
        }
    }

    #[test]
    fn accountability_notifications_do_not_panic() {
        goal_reached(100, 1);
        goal_reached(100, 7);
        distraction("bilibili.exe", "12:34");
    }
}
