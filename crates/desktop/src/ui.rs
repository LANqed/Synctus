//! The egui front-end: the always-on-top overlay and the settings window.
//!
//! The overlay is deliberately tiny — an avatar, the peer's presence and one line
//! of detail. Left-clicking the avatar pokes the peer; right-clicking opens the
//! same menu as the tray, so the tray is a convenience rather than the only way
//! in.

use eframe::egui;
use std::time::Instant;

use synctus_core::client::ConnState;
use synctus_core::model::{NudgeKind, PomodoroPhase, Presence};

use crate::app::{App, UiRequest};

/// Overlay size. Wide enough for `Artist - Title` at the default font size.
pub const OVERLAY_SIZE: [f32; 2] = [260.0, 96.0];
/// Settings window size.
pub const SETTINGS_SIZE: [f32; 2] = [520.0, 560.0];

/// Transient UI state that does not belong in [`App`].
#[derive(Default)]
pub struct UiState {
    /// Text box contents for a new to-do.
    pub new_todo: String,
    /// Which tab the settings window shows.
    pub tab: SettingsTab,
    /// Whether the to-do panel is expanded under the overlay.
    pub show_todos: bool,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
pub enum SettingsTab {
    #[default]
    Connection,
    Privacy,
    Pomodoro,
    Todos,
    Log,
}

/// Draw the overlay. Returns any request the user made.
pub fn overlay(ui: &mut egui::Ui, app: &mut App, state: &mut UiState) -> Option<UiRequest> {
    let mut request = None;
    let now = synctus_core::now_ms();

    egui::Frame::new()
        .fill(ui.visuals().window_fill.gamma_multiply(0.92))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // --- avatar: click to knock ---------------------------------
                let presence = app.peer_presence();
                if let Some(req) = avatar(ui, app, presence) {
                    request = Some(req);
                }

                ui.add_space(6.0);

                ui.vertical(|ui| {
                    // Line 1: name and presence.
                    ui.horizontal(|ui| {
                        let name = app
                            .peer()
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "等待对方…".to_string());
                        ui.label(egui::RichText::new(name).strong());

                        let (r, g, b) = presence.rgb();
                        ui.label(
                            egui::RichText::new(presence.label())
                                .color(egui::Color32::from_rgb(r, g, b))
                                .small(),
                        );

                        if !app.is_online() {
                            ui.label(
                                egui::RichText::new("⚠")
                                    .color(egui::Color32::from_rgb(0xff, 0xb3, 0x00))
                                    .small(),
                            )
                            .on_hover_text(connection_hint(&app.conn));
                        }
                    });

                    // Line 2: what the peer is doing.
                    ui.label(egui::RichText::new(peer_detail(app, now)).small().weak());

                    // Line 3: battery, pomodoro, to-dos.
                    ui.label(egui::RichText::new(peer_meta(app, now)).small().weak());
                });
            });

            // Recent poke, shown briefly.
            if let Some((nudge, at)) = &app.last_nudge {
                if at.elapsed().as_secs() < 8 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(nudge.body())
                            .small()
                            .color(egui::Color32::from_rgb(0x42, 0xa5, 0xf5)),
                    );
                    // Keep repainting so the message disappears on time.
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
                }
            }
        });

    // Right-click anywhere on the overlay opens the menu.
    let area = ui.min_rect();
    let response = ui.interact(
        area,
        egui::Id::new("overlay-bg"),
        egui::Sense::click_and_drag(),
    );
    response.context_menu(|ui| {
        if let Some(req) = menu(ui, app, state) {
            request = Some(req);
        }
    });

    // Dragging the overlay moves the window; the position is saved on exit.
    if response.dragged() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    request
}

/// The peer avatar: a coloured disc with the platform badge.
fn avatar(ui: &mut egui::Ui, app: &App, presence: Presence) -> Option<UiRequest> {
    let size = egui::vec2(44.0, 44.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let (r, g, b) = presence.rgb();
    let colour = egui::Color32::from_rgb(r, g, b);
    let painter = ui.painter();

    // Grow slightly while hovered, so it reads as clickable.
    let radius = if response.hovered() { 21.0 } else { 19.0 };
    painter.circle_filled(rect.center(), radius, colour.gamma_multiply(0.30));
    painter.circle_stroke(rect.center(), radius, egui::Stroke::new(2.0, colour));

    let badge = app.peer().map(|p| p.platform.icon()).unwrap_or("--");
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        badge,
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );

    // A pomodoro ring around the avatar, so the peer's focus progress is visible
    // without reading text.
    if let Some(pom) = app.peer().and_then(|p| p.pomodoro) {
        if pom.phase != PomodoroPhase::Idle {
            let left = pom.remaining_ms(synctus_core::now_ms()) as f32;
            let total = 25.0 * 60_000.0;
            let fraction = (left / total).clamp(0.0, 1.0);
            let ring = if pom.phase.is_break() {
                egui::Color32::from_rgb(0x42, 0xa5, 0xf5)
            } else {
                egui::Color32::from_rgb(0xef, 0x53, 0x50)
            };
            // A simple arc approximation: dots along the remaining fraction.
            let steps = (fraction * 24.0) as i32;
            for i in 0..steps {
                let angle =
                    -std::f32::consts::FRAC_PI_2 + (i as f32 / 24.0) * std::f32::consts::TAU;
                let pos = rect.center() + egui::vec2(angle.cos(), angle.sin()) * (radius + 3.0);
                ui.painter().circle_filled(pos, 1.3, ring);
            }
        }
    }

    let response = response.on_hover_text(format!(
        "左键{}对方\n右键打开菜单",
        NudgeKind::Knock.label()
    ));

    response
        .clicked()
        .then_some(UiRequest::Nudge(NudgeKind::Knock))
}

/// The shared context menu, used by the overlay and mirrored by the tray.
pub fn menu(ui: &mut egui::Ui, app: &App, state: &mut UiState) -> Option<UiRequest> {
    let mut request = None;

    ui.label(
        egui::RichText::new(format!("Synctus {}", env!("CARGO_PKG_VERSION")))
            .small()
            .weak(),
    );
    ui.separator();

    ui.menu_button("互动", |ui| {
        for kind in NudgeKind::ALL {
            if ui
                .button(format!("{} {}", kind.emoji(), kind.label()))
                .clicked()
            {
                request = Some(UiRequest::Nudge(kind));
                ui.close();
            }
        }
    });

    ui.menu_button(
        format!("我的状态：{}", app.own_presence().label()),
        |ui| {
            for presence in [
                Presence::Active,
                Presence::Resting,
                Presence::Busy,
                Presence::Away,
            ] {
                if ui.button(presence.label()).clicked() {
                    request = Some(UiRequest::SetPresence(presence));
                    ui.close();
                }
            }
        },
    );

    ui.separator();

    let pom = app.pomodoro.state();
    let label = match pom.phase {
        PomodoroPhase::Idle => "开始专注".to_string(),
        _ if pom.paused() => format!("继续（{}）", pom.remaining_text(synctus_core::now_ms())),
        _ => format!(
            "暂停（{} {}）",
            pom.phase.label(),
            pom.remaining_text(synctus_core::now_ms())
        ),
    };
    if ui.button(label).clicked() {
        request = Some(UiRequest::TogglePomodoro);
        ui.close();
    }
    if pom.phase != PomodoroPhase::Idle {
        if ui.button("跳过当前阶段").clicked() {
            request = Some(UiRequest::SkipPhase);
            ui.close();
        }
        if ui.button("停止番茄钟").clicked() {
            request = Some(UiRequest::StopPomodoro);
            ui.close();
        }
    }

    ui.separator();

    if ui.checkbox(&mut state.show_todos, "显示待办清单").changed() {
        // Handled by the caller reading `state.show_todos`.
    }
    if ui.button("设置…").clicked() {
        request = Some(UiRequest::OpenSettings);
        ui.close();
    }
    if ui.button("检查更新").clicked() {
        request = Some(UiRequest::CheckUpdate);
        ui.close();
    }
    if ui.button("重新连接").clicked() {
        request = Some(UiRequest::Reconnect);
        ui.close();
    }

    ui.separator();
    if ui.button("退出").clicked() {
        request = Some(UiRequest::Quit);
        ui.close();
    }

    request
}

/// One line describing what the peer is doing.
fn peer_detail(app: &App, now: i64) -> String {
    let Some(peer) = app.peer() else {
        return "尚未收到对方状态".to_string();
    };

    if peer.is_stale(now, app.cfg.peer_stale_ms()) {
        let secs = (now - peer.at) / 1000;
        return format!("最后更新于 {secs} 秒前");
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

/// Battery, pomodoro and to-do counters as a compact line.
fn peer_meta(app: &App, now: i64) -> String {
    let Some(peer) = app.peer() else {
        return String::new();
    };

    let mut parts = Vec::new();

    if let Some(b) = peer.battery {
        let icon = if b.charging { "⚡" } else { "🔋" };
        parts.push(format!("{icon}{}%", b.percent));
    }
    if let Some(pom) = peer.pomodoro {
        if pom.phase != PomodoroPhase::Idle {
            let state = if pom.paused() { "⏸" } else { "▶" };
            parts.push(format!(
                "🍅{state}{} {}",
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

fn connection_hint(conn: &ConnState) -> String {
    match conn {
        ConnState::Connecting => "正在连接…".to_string(),
        ConnState::Online => "已连接".to_string(),
        ConnState::Offline(why) => format!("离线：{why}"),
        ConnState::Rejected(why) => format!("被拒绝：{why}\n请检查配对码与服务器地址"),
    }
}

/// The settings window. Returns true when it should close.
pub fn settings(ctx: &egui::Context, app: &mut App, state: &mut UiState) -> bool {
    // `Window::open` borrows a flag for the whole call, so the close request from
    // inside the body goes through a separate variable.
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;

    egui::Window::new("Synctus 设置")
        .open(&mut open)
        .default_size(SETTINGS_SIZE)
        .vscroll(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (SettingsTab::Connection, "连接"),
                    (SettingsTab::Privacy, "隐私"),
                    (SettingsTab::Pomodoro, "番茄钟"),
                    (SettingsTab::Todos, "待办"),
                    (SettingsTab::Log, "日志"),
                ] {
                    ui.selectable_value(&mut state.tab, tab, label);
                }
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(420.0)
                .show(ui, |ui| match state.tab {
                    SettingsTab::Connection => connection_tab(ui, app),
                    SettingsTab::Privacy => privacy_tab(ui, app),
                    SettingsTab::Pomodoro => pomodoro_tab(ui, app),
                    SettingsTab::Todos => todos_tab(ui, app, state),
                    SettingsTab::Log => log_tab(ui, app),
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("保存并应用").clicked() {
                    apply = true;
                }
                if ui.button("取消").clicked() {
                    app.draft = None;
                    cancel = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(connection_hint(&app.conn))
                            .small()
                            .weak(),
                    );
                });
            });
        });

    if apply {
        app.apply_draft();
        return true;
    }
    cancel || !open
}

fn connection_tab(ui: &mut egui::Ui, app: &mut App) {
    // Edits go to the draft so nothing is applied until the user saves.
    if app.draft.is_none() {
        app.draft = Some(app.cfg.clone());
    }
    let draft = app.draft.as_mut().expect("just ensured");

    ui.label(egui::RichText::new("配对码").strong());
    ui.label(
        egui::RichText::new("双方填入同一个配对码即可配对。服务器只转发密文，无法读取内容。")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.invite_code)
                .hint_text("例如 ABCD-EFGH-IJKL-MNOP")
                .desired_width(280.0),
        );
        if ui.button("生成").clicked() {
            draft.invite_code = synctus_core::crypto::generate_invite_code();
        }
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("服务器").strong());
    ui.horizontal(|ui| {
        ui.label("地址");
        ui.add(
            egui::TextEdit::singleline(&mut draft.server)
                .hint_text("host:port")
                .desired_width(220.0),
        );
    });
    ui.checkbox(&mut draft.tls, "使用 TLS（推荐）");
    if !draft.tls {
        ui.label(
            egui::RichText::new(
                "关闭 TLS 后房间标识与设备标识会以明文经过网络，消息内容仍为端到端加密。",
            )
            .small()
            .color(egui::Color32::from_rgb(0xff, 0xb3, 0x00)),
        );
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("本机").strong());
    ui.horizontal(|ui| {
        ui.label("显示名称");
        ui.add(egui::TextEdit::singleline(&mut draft.device_name).desired_width(220.0));
    });
    ui.horizontal(|ui| {
        ui.label("采样间隔");
        ui.add(egui::Slider::new(&mut draft.poll_secs, 1..=60).suffix(" 秒"));
    });
    ui.horizontal(|ui| {
        ui.label("空闲判定");
        ui.add(
            egui::Slider::new(&mut draft.away_after_secs, 0..=1800)
                .suffix(" 秒")
                .text("0 表示不自动离开"),
        );
    });

    ui.add_space(8.0);
    ui.checkbox(&mut draft.autostart, "开机自动启动");
    ui.checkbox(&mut draft.start_minimised, "启动时不显示设置窗口");
    ui.checkbox(&mut draft.show_overlay, "显示悬浮窗");
    ui.checkbox(&mut draft.overlay_always_on_top, "悬浮窗置顶");
    ui.checkbox(&mut draft.mute_nudges, "静音互动提醒");

    ui.add_space(8.0);
    ui.label(egui::RichText::new("更新").strong());
    ui.checkbox(&mut draft.check_updates, "启动时检查更新");
    ui.horizontal(|ui| {
        ui.label("GitHub 仓库");
        ui.add(egui::TextEdit::singleline(&mut draft.update_repo).desired_width(220.0));
    });

    if let Some(update) = app.update.clone() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("发现新版本 {}", update.version))
                    .color(egui::Color32::from_rgb(0x4c, 0xaf, 0x50)),
            );
            if ui.button("打开下载页").clicked() {
                if let Err(e) = open::that(&update.url) {
                    tracing::warn!(error = %e, "无法打开浏览器");
                }
            }
        });
    }
}

fn privacy_tab(ui: &mut egui::Ui, app: &mut App) {
    if app.draft.is_none() {
        app.draft = Some(app.cfg.clone());
    }
    let privacy = &mut app.draft.as_mut().expect("just ensured").privacy;

    ui.label(
        egui::RichText::new("关闭的项目根本不会离开本机。")
            .small()
            .weak(),
    );
    ui.add_space(4.0);

    ui.checkbox(&mut privacy.share_foreground_app, "同步前台应用");
    ui.add_enabled_ui(privacy.share_foreground_app, |ui| {
        ui.indent("title", |ui| {
            ui.checkbox(&mut privacy.share_window_title, "同时同步窗口标题");
            ui.label(
                egui::RichText::new("窗口标题常含文件名或聊天对象，默认关闭。")
                    .small()
                    .weak(),
            );
        });
    });

    ui.checkbox(&mut privacy.share_battery, "同步电量");
    ui.checkbox(&mut privacy.share_music, "同步正在播放的音乐");
    ui.checkbox(&mut privacy.share_pomodoro, "同步番茄钟状态");
    ui.checkbox(&mut privacy.share_todos, "同步待办清单");
    ui.checkbox(&mut privacy.share_idle, "同步空闲时长（用于自动离开）");

    ui.add_space(8.0);
    ui.label(egui::RichText::new("应用黑名单").strong());
    ui.label(
        egui::RichText::new("这些应用在前台时只显示“（隐藏）”，每行一个，匹配进程名。")
            .small()
            .weak(),
    );

    let mut blocked = privacy.blocked_apps.join("\n");
    if ui
        .add(
            egui::TextEdit::multiline(&mut blocked)
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text("KeePass\n1Password"),
        )
        .changed()
    {
        privacy.blocked_apps = blocked
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
}

fn pomodoro_tab(ui: &mut egui::Ui, app: &mut App) {
    if app.draft.is_none() {
        app.draft = Some(app.cfg.clone());
    }
    let pom = &mut app.draft.as_mut().expect("just ensured").pomodoro;

    ui.horizontal(|ui| {
        ui.label("专注时长");
        ui.add(egui::Slider::new(&mut pom.focus_min, 5..=90).suffix(" 分"));
    });
    ui.horizontal(|ui| {
        ui.label("小休时长");
        ui.add(egui::Slider::new(&mut pom.short_break_min, 1..=30).suffix(" 分"));
    });
    ui.horizontal(|ui| {
        ui.label("长休时长");
        ui.add(egui::Slider::new(&mut pom.long_break_min, 5..=60).suffix(" 分"));
    });
    ui.horizontal(|ui| {
        ui.label("每组回合");
        ui.add(egui::Slider::new(&mut pom.rounds_per_set, 2..=8));
    });
    ui.checkbox(&mut pom.auto_continue, "阶段结束后自动进入下一阶段");
    ui.checkbox(
        &mut pom.presence_follows_phase,
        "根据阶段自动切换状态（专注=免打扰，休息=休息中）",
    );

    ui.add_space(8.0);
    ui.separator();
    let state = app.pomodoro.state();
    ui.label(format!(
        "今日完成 {} 个回合，当前组内第 {} 个",
        state.completed_today, state.round
    ));
}

fn todos_tab(ui: &mut egui::Ui, app: &mut App, state: &mut UiState) {
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.new_todo)
                .hint_text("添加待办后按回车")
                .desired_width(320.0),
        );
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("添加").clicked() || submitted {
            app.add_todo(&state.new_todo.clone());
            state.new_todo.clear();
            response.request_focus();
        }
    });

    ui.add_space(4.0);

    // Collect actions first: mutating the list while iterating it is not allowed.
    let mut toggle = None;
    let mut remove = None;
    for todo in &app.data.todos {
        ui.horizontal(|ui| {
            let mut done = todo.done;
            if ui.checkbox(&mut done, "").changed() {
                toggle = Some(todo.id.clone());
            }
            let text = if todo.done {
                egui::RichText::new(&todo.title).strikethrough().weak()
            } else {
                egui::RichText::new(&todo.title)
            };
            ui.label(text);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").clicked() {
                    remove = Some(todo.id.clone());
                }
                if todo.pomodoros > 0 {
                    ui.label(egui::RichText::new(format!("🍅×{}", todo.pomodoros)).small());
                }
            });
        });
    }
    if let Some(id) = toggle {
        app.toggle_todo(&id);
    }
    if let Some(id) = remove {
        app.remove_todo(&id);
    }

    if app.data.todos.iter().any(|t| t.done) {
        ui.add_space(4.0);
        if ui.button("清除已完成").clicked() {
            app.clear_done_todos();
        }
    }

    // The peer's list, read-only.
    if let Some(peer) = app.peer() {
        let items = app.peers.todos(&peer.device_id);
        if !items.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.label(egui::RichText::new(format!("{} 的待办", peer.name)).strong());
            for todo in items {
                let text = if todo.done {
                    egui::RichText::new(&todo.title).strikethrough().weak()
                } else {
                    egui::RichText::new(&todo.title)
                };
                ui.label(format!(
                    "{} {}",
                    if todo.done { "☑" } else { "☐" },
                    text.text()
                ));
            }
        }
    }
}

fn log_tab(ui: &mut egui::Ui, app: &App) {
    ui.label(
        egui::RichText::new(format!(
            "设备 ID {} · 房间 {}",
            app.cfg.device_id,
            if app.cfg.is_paired() {
                "已配对"
            } else {
                "未配对"
            }
        ))
        .small()
        .weak(),
    );
    ui.separator();

    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let now = Instant::now();
            for line in app
                .log
                .iter()
                .rev()
                .take(100)
                .collect::<Vec<_>>()
                .iter()
                .rev()
            {
                let ago = now.saturating_duration_since(line.at).as_secs();
                ui.label(
                    egui::RichText::new(format!("{ago:>4}s  {}", line.text))
                        .monospace()
                        .small(),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_hints_are_specific() {
        assert!(connection_hint(&ConnState::Online).contains("已连接"));
        assert!(connection_hint(&ConnState::Offline("网络不可达".into())).contains("网络不可达"));
        assert!(connection_hint(&ConnState::Rejected("配对码不匹配".into())).contains("配对码"));
    }

    #[test]
    fn overlay_fits_a_music_line_at_the_default_font_size() {
        // Roughly: "Artist - Title" at ~7 px per character plus the avatar and
        // margins. A narrower overlay silently clips the most useful line.
        let avatar_and_margins = 44.0 + 8.0 * 2.0 + 6.0;
        let text_budget = OVERLAY_SIZE[0] - avatar_and_margins;
        assert!(
            text_budget >= 190.0,
            "only {text_budget} px left for text; the music line will clip"
        );

        // Three stacked lines (name, detail, meta) plus padding.
        let lines = 3.0 * 18.0;
        assert!(
            OVERLAY_SIZE[1] >= lines + 16.0,
            "height {} cannot fit three lines",
            OVERLAY_SIZE[1]
        );
    }
}
