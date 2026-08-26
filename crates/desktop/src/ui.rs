//! The egui front-end: the always-on-top overlay and the settings window.
//!
//! The overlay is deliberately small — an avatar, the peer's presence, one line of
//! detail and the two focus numbers side by side. Left-clicking the avatar pokes
//! the peer; right-clicking opens the same menu as the tray, so the tray is a
//! convenience rather than the only way in.
//!
//! The focus comparison is the part that does the actual work. Seeing "我 50 / TA
//! 75" is what makes someone start a round, far more than any notification.

use eframe::egui;
use std::time::Instant;

use synctus_core::client::ConnState;
use synctus_core::model::{NudgeKind, PomodoroPhase, Presence};

use crate::app::{App, UiRequest};

/// Overlay size. Wide enough for the status lines and the focus comparison, and
/// tall enough for the to-do list when it is expanded. The window is resizable,
/// so this is the default, not a ceiling.
pub const OVERLAY_SIZE: [f32; 2] = [300.0, 236.0];
/// Settings window size.
pub const SETTINGS_SIZE: [f32; 2] = [520.0, 600.0];

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
    Accountability,
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
                    // Line 1: name (with its user label), presence, and the nag
                    // button when earned.
                    ui.horizontal(|ui| {
                        let name = app
                            .peer()
                            .map(|p| match p.user.as_str() {
                                "" => p.name.clone(),
                                user => format!("{user} · {}", p.name),
                            })
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

                        // Caught them: offer the nag right where the evidence is,
                        // rather than buried in a menu.
                        if app.peer_is_slacking() {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(
                                            egui::RichText::new("👀 抓到了")
                                                .color(egui::Color32::from_rgb(0xef, 0x53, 0x50)),
                                        )
                                        .on_hover_text("对方正在专注，却开着摸鱼应用")
                                        .clicked()
                                    {
                                        request = Some(UiRequest::Nudge(NudgeKind::Nag));
                                    }
                                },
                            );
                        }
                    });

                    // Line 2: what the peer is doing.
                    ui.label(egui::RichText::new(peer_detail(app, now)).small().weak());

                    // Line 3: battery, pomodoro, to-dos.
                    ui.label(egui::RichText::new(peer_meta(app, now)).small().weak());
                });
            });

            // The focus comparison: the reason this tool exists.
            focus_row(ui, app);

            // My own distraction, called out so it is impossible to ignore.
            if let Some(app_name) = app.distracted_by.clone() {
                ui.label(
                    egui::RichText::new(format!("👀 你在专注中开着 {app_name}"))
                        .small()
                        .color(egui::Color32::from_rgb(0xef, 0x53, 0x50)),
                );
            }

            // The to-do list, shown when the user asks for it.
            if state.show_todos {
                overlay_todos(ui, app);
            }

            // Recent poke, shown briefly.
            if let Some((nudge, at)) = &app.last_nudge {
                if at.elapsed().as_secs() < 8 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(nudge.body())
                            .small()
                            .color(crate::theme::ACCENT),
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

/// Today's focus minutes for both people, as a bar plus numbers.
///
/// Two bars rather than one shared scale: the goals may differ, and what matters
/// is each person's progress against their own target, with the other's number
/// visible for comparison.
fn focus_row(ui: &mut egui::Ui, app: &App) {
    let goal = app.cfg.accountability.daily_goal_min;
    if goal == 0 {
        // Goals disabled: the row would be an empty bar, so it is omitted.
        return;
    }

    let (mine, theirs) = app.focus_comparison();
    let streak = app.my_streak();

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        // Mine.
        let progress = app.my_goal_progress();
        let done = mine >= goal;
        ui.add(
            egui::ProgressBar::new(progress)
                .desired_width(88.0)
                .desired_height(10.0)
                .fill(if done {
                    egui::Color32::from_rgb(0x4c, 0xaf, 0x50)
                } else {
                    crate::theme::ACCENT
                }),
        )
        .on_hover_text(format!("我：{mine}/{goal} 分钟"));

        ui.label(
            egui::RichText::new(format!("我 {mine}"))
                .small()
                .color(if done {
                    egui::Color32::from_rgb(0x4c, 0xaf, 0x50)
                } else {
                    ui.visuals().text_color()
                }),
        );

        // Theirs, when there is a peer to compare against.
        if let Some(peer) = app.peer() {
            ui.separator();
            let their_goal = peer.goal_min;
            let their_progress = peer.goal_progress();
            let their_done = peer.goal_met();

            ui.add(
                egui::ProgressBar::new(their_progress)
                    .desired_width(88.0)
                    .desired_height(10.0)
                    .fill(if their_done {
                        egui::Color32::from_rgb(0x4c, 0xaf, 0x50)
                    } else {
                        egui::Color32::from_rgb(0x9e, 0x9e, 0x9e)
                    }),
            )
            .on_hover_text(if their_goal > 0 {
                format!("{}：{theirs}/{their_goal} 分钟", peer.name)
            } else {
                format!("{}：{theirs} 分钟（未设目标）", peer.name)
            });

            ui.label(egui::RichText::new(format!("TA {theirs}")).small().weak());

            if peer.streak_days > 1 {
                ui.label(
                    egui::RichText::new(format!("🔥{}", peer.streak_days))
                        .small()
                        .weak(),
                )
                .on_hover_text(format!("对方连续达标 {} 天", peer.streak_days));
            }
        }

        if streak > 1 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("🔥{streak}"))
                        .small()
                        .color(egui::Color32::from_rgb(0xff, 0x8f, 0x00)),
                )
                .on_hover_text(format!("我连续达标 {streak} 天"));
            });
        }
    });
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
                crate::theme::ACCENT
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

/// The overlay's context menu.
///
/// Kept deliberately short: it renders inside the small always-on-top overlay,
/// and a long menu gets clipped by the window bounds ("显示不完全"). Everything
/// heavier — updates, reconnect, the full nudge list — lives in the tray menu and
/// in the separate settings window.
pub fn menu(ui: &mut egui::Ui, app: &App, state: &mut UiState) -> Option<UiRequest> {
    let mut request = None;

    ui.label(
        egui::RichText::new(format!("Synctus {}", env!("CARGO_PKG_VERSION")))
            .small()
            .weak(),
    );

    // Today's numbers at the top: the first thing worth seeing.
    let goal = app.cfg.accountability.daily_goal_min;
    if goal > 0 {
        let (mine, theirs) = app.focus_comparison();
        ui.label(
            egui::RichText::new(format!(
                "今日专注　我 {mine} / TA {theirs}　目标 {goal} 分钟"
            ))
            .small(),
        );
    }
    ui.separator();

    // Interaction, as a single submenu so it costs one line.
    ui.menu_button("互动", |ui| {
        let nag_hint = if app.peer_is_slacking() {
            "对方正在专注，却开着摸鱼应用"
        } else if app.peer_is_focusing() {
            "对方正在专注中"
        } else {
            "对方现在并没有在专注，催了也没意义"
        };
        if ui
            .add_enabled(
                app.peer_is_focusing(),
                egui::Button::new(format!(
                    "{} {}",
                    NudgeKind::Nag.emoji(),
                    NudgeKind::Nag.label()
                )),
            )
            .on_hover_text(nag_hint)
            .on_disabled_hover_text(nag_hint)
            .clicked()
        {
            request = Some(UiRequest::Nudge(NudgeKind::Nag));
            ui.close();
        }

        for kind in NudgeKind::ALL {
            if matches!(kind, NudgeKind::Nag) {
                continue;
            }
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

    // The pomodoro, right there in the overlay as requested.
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

    if ui.checkbox(&mut state.show_todos, "显示待办清单").changed() {
        // Handled by the caller reading `state.show_todos`.
    }

    ui.separator();
    if ui.button("设置…").clicked() {
        request = Some(UiRequest::OpenSettings);
        ui.close();
    }
    if ui.button("退出").clicked() {
        request = Some(UiRequest::Quit);
        ui.close();
    }

    request
}

/// The to-do list inside the overlay.
///
/// Own list, editable inline; the peer's list is read-only below a divider. The
/// peer's list is already rendered in the settings Todos tab, but the user asked
/// for it here too.
fn overlay_todos(ui: &mut egui::Ui, app: &mut App) {
    ui.separator();

    let mut toggled: Option<String> = None;
    let mut removed: Option<String> = None;
    let mut new_title = String::new();
    let mut add = false;

    // A compact single-line editor.
    let edit = egui::TextEdit::singleline(&mut new_title)
        .hint_text("新待办，回车添加")
        .desired_width(ui.available_width() - 44.0);
    let response = ui.add(edit);
    if response.lost_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && !new_title.trim().is_empty()
    {
        add = true;
    }

    for todo in &app.data.todos {
        ui.horizontal(|ui| {
            let mut done = todo.done;
            if ui.checkbox(&mut done, "").changed() {
                toggled = Some(todo.id.clone());
            }
            let text = if todo.done {
                egui::RichText::new(&todo.title).strikethrough().weak()
            } else {
                egui::RichText::new(&todo.title)
            };
            ui.label(text);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").clicked() {
                    removed = Some(todo.id.clone());
                }
            });
        });
    }

    // The peer's list, read-only.
    if let Some(peer) = app.peer() {
        let items = app.peers.todos(&peer.device_id);
        if !items.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("{} 的待办", peer.name))
                    .small()
                    .weak(),
            );
            for todo in items.iter().take(12) {
                let mark = if todo.done { "☑" } else { "☐" };
                ui.label(egui::RichText::new(format!("{mark} {}", todo.title)).small());
            }
        }
    }

    if add {
        app.add_todo(&new_title);
    }
    if let Some(id) = toggled {
        app.toggle_todo(&id);
    }
    if let Some(id) = removed {
        app.remove_todo(&id);
    }
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

/// Render the settings in their own native window.
///
/// A separate OS viewport rather than a window inside the overlay: the overlay is
/// a small always-on-top widget, and a full settings page drawn there gets
/// clipped. This is a real window with its own title and size.
pub fn settings_viewport(ctx: &egui::Context, app: &mut App, state: &mut UiState) {
    let viewport_id = egui::ViewportId::from_hash_of("synctus-settings");

    // `show_viewport_immediate` runs the closure now, on the main thread, so it
    // can borrow the app state directly.
    ctx.show_viewport_immediate(
        viewport_id,
        egui::ViewportBuilder::default()
            .with_title("Synctus 设置")
            .with_inner_size(SETTINGS_SIZE)
            .with_min_inner_size([420.0, 400.0])
            .with_app_id("synctus-settings"),
        |ui, _class| {
            let child_ctx = ui.ctx().clone();

            // The settings content is the same; only the container changed.
            if settings(ui, app, state) {
                app.show_settings = false;
            }

            // The user clicking the window's X button is the same as Cancel.
            if child_ctx.input(|i| i.viewport().close_requested()) {
                app.show_settings = false;
            }

            // Keep it alive while the window is open.
            child_ctx.request_repaint_after(std::time::Duration::from_secs(1));
        },
    );
}

/// The settings window body. Returns true when it should close.
pub fn settings(ui: &mut egui::Ui, app: &mut App, state: &mut UiState) -> bool {
    // `apply`/`cancel` are set inside the panel and read after it.
    let mut apply = false;
    let mut cancel = false;

    // A CentralPanel, not a Window: the native viewport is already the window, and
    // nesting a Window here would show two title bars.
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (SettingsTab::Connection, "连接"),
                (SettingsTab::Accountability, "督促"),
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
            .max_height(440.0)
            .show(ui, |ui| match state.tab {
                SettingsTab::Connection => connection_tab(ui, app),
                SettingsTab::Accountability => accountability_tab(ui, app),
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
    cancel
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
        ui.label("用户标识");
        ui.add(egui::TextEdit::singleline(&mut draft.user).desired_width(220.0));
    });
    ui.label(
        egui::RichText::new(
            "给自己这台设备起一个标识，例如「A」。对方的所有设备如果也填 A，就会被归到同一个用户下；服务器管理面板按这个分组。",
        )
        .small()
        .weak(),
    );
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

/// The accountability tab: daily goal, distraction detection, and who gets told.
///
/// This is the tab that decides how much the tool actually pushes you, so each
/// option says plainly what it does and who sees the result.
fn accountability_tab(ui: &mut egui::Ui, app: &mut App) {
    if app.draft.is_none() {
        app.draft = Some(app.cfg.clone());
    }

    // Today's state, read from the real config rather than the draft: it describes
    // what has happened, not what is being edited.
    let goal_now = app.cfg.accountability.daily_goal_min;
    if goal_now > 0 {
        let (mine, theirs) = app.focus_comparison();
        let streak = app.my_streak();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("今日：我 {mine} 分钟 / 对方 {theirs} 分钟")).strong(),
            );
            if streak > 1 {
                ui.label(
                    egui::RichText::new(format!("🔥 连续 {streak} 天"))
                        .color(egui::Color32::from_rgb(0xff, 0x8f, 0x00)),
                );
            }
        });
        ui.add(
            egui::ProgressBar::new(app.my_goal_progress()).text(format!("{mine}/{goal_now} 分钟")),
        );
        ui.separator();
    }

    let acc = &mut app.draft.as_mut().expect("just ensured").accountability;

    ui.label(egui::RichText::new("每日目标").strong());
    ui.horizontal(|ui| {
        ui.add(
            egui::Slider::new(&mut acc.daily_goal_min, 0..=600)
                .suffix(" 分钟")
                .step_by(5.0),
        );
    });
    if acc.daily_goal_min == 0 {
        ui.label(
            egui::RichText::new("目标为 0：不统计目标与连续天数，只同步状态。")
                .small()
                .weak(),
        );
    } else {
        ui.label(
            egui::RichText::new(format!(
                "约 {} 个 {} 分钟的专注回合。达标后会自动告知对方。",
                (acc.daily_goal_min as f32 / app.cfg.pomodoro.focus_min.max(1) as f32).ceil()
                    as u32,
                app.cfg.pomodoro.focus_min
            ))
            .small()
            .weak(),
        );
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("摸鱼检测").strong());
    ui.label(
        egui::RichText::new("只在专注回合进行中检查；休息和空闲时间不监控。")
            .small()
            .weak(),
    );
    ui.checkbox(&mut acc.warn_on_distraction, "专注时打开摸鱼应用就提醒我");

    ui.add_enabled_ui(acc.warn_on_distraction, |ui| {
        ui.indent("distraction", |ui| {
            ui.horizontal(|ui| {
                ui.label("宽限时间");
                ui.add(egui::Slider::new(&mut acc.distraction_grace_secs, 0..=300).suffix(" 秒"));
            });
            ui.label(
                egui::RichText::new("切过去查资料不算摸鱼；只有停留超过这个时间才提醒。")
                    .small()
                    .weak(),
            );

            ui.add_space(4.0);
            ui.checkbox(
                &mut acc.report_distraction_to_peer,
                "顺便告诉对方（默认关闭）",
            );
            ui.label(
                egui::RichText::new(if acc.report_distraction_to_peer {
                    "对方会收到你在专注时摸鱼的提醒。被盯着是自己选的。"
                } else {
                    "提醒只出现在本机，对方不会知道。"
                })
                .small()
                .weak(),
            );

            ui.add_space(6.0);
            ui.label(egui::RichText::new("摸鱼应用清单").strong());
            ui.label(
                egui::RichText::new("每行一个，匹配进程名或包名的一部分，不区分大小写。")
                    .small()
                    .weak(),
            );

            let mut list = acc.distracting_apps.join("\n");
            if ui
                .add(
                    egui::TextEdit::multiline(&mut list)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY)
                        .hint_text("bilibili\nsteam\nyoutube"),
                )
                .changed()
            {
                acc.distracting_apps = list
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        });
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("互动").strong());
    ui.checkbox(
        &mut acc.allow_urgent_nudges,
        "允许对方的「别摸鱼了」穿透免打扰",
    );
    ui.checkbox(&mut acc.auto_cheer, "对方达成目标时自动祝贺");
    ui.label(
        egui::RichText::new("鼓励要是靠人记得发，就不会发生，所以让它自动。")
            .small()
            .weak(),
    );
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
    ui.indent("pomodoro-note", |ui| {
        ui.label(
            egui::RichText::new("同时包含今日专注分钟数、每日目标与连续天数——督促功能依赖这一项。")
                .small()
                .weak(),
        );
    });
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
