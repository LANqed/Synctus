//! Desktop notifications for peer interactions and pomodoro transitions.
//!
//! Uses the platform notification service (Action Center on Windows, the
//! freedesktop spec on Linux) rather than drawing our own toast, so
//! do-not-disturb and notification history behave as the user expects.

use synctus_core::model::{Nudge, PomodoroPhase};

const APP_NAME: &str = "Synctus";

/// Show a notification. Failures are logged, never propagated: a missing
/// notification daemon must not break the sync loop.
fn show(summary: &str, body: &str) {
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
            .urgency(notify_rust::Urgency::Normal);
    }

    if let Err(e) = notification.show() {
        tracing::debug!(error = %e, "发送系统通知失败");
    }
}

/// Announce an incoming poke.
pub fn nudge(nudge: &Nudge) {
    show(
        &format!("{} {}", nudge.kind.emoji(), nudge.from_name),
        &nudge.body(),
    );
}

/// Announce the end of a focus round, naming the break that follows.
pub fn focus_finished(next: PomodoroPhase) {
    show("🍅 专注完成", &format!("该{}了，休息一下吧", next.label()));
}

/// Announce the end of a break.
pub fn break_finished() {
    show("☕ 休息结束", "准备开始下一个专注回合");
}

/// Tell the user a newer release exists. Deliberately does not auto-install.
pub fn update_available(version: &str) {
    show(
        "有新版本可用",
        &format!("Synctus {version} 已发布，可在托盘菜单中打开下载页"),
    );
}

/// Report a peer coming online or going offline.
pub fn peer_presence(name: &str, online: bool) {
    if online {
        show("对方上线了", &format!("{name} 已连接"));
    } else {
        show("对方离线了", &format!("{name} 已断开"));
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
        focus_finished(PomodoroPhase::ShortBreak);
        break_finished();
        update_available("9.9.9");
    }
}
