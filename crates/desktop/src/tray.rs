//! System tray icon and its right-click menu.
//!
//! The tray is the primary entry point when the overlay is hidden. `tray-icon`
//! requires the icon to be created on the thread running the platform event loop,
//! which for us is the winit/eframe main thread — so [`Tray::new`] is called from
//! the first `update` rather than from `main`.
//!
//! Events are polled rather than pushed through an `EventLoopProxy`: the app
//! already repaints on a timer for the pomodoro countdown, so polling adds no
//! wake-ups of its own.

use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use synctus_core::model::{NudgeKind, Presence};

use crate::app::UiRequest;

/// Stable menu ids. Strings rather than an enum because that is what `muda`
/// gives back in its events.
mod id {
    pub const NUDGE_PREFIX: &str = "nudge:";
    pub const PRESENCE_PREFIX: &str = "presence:";
    pub const POMODORO_TOGGLE: &str = "pomodoro:toggle";
    pub const POMODORO_SKIP: &str = "pomodoro:skip";
    pub const POMODORO_STOP: &str = "pomodoro:stop";
    pub const OVERLAY_TOGGLE: &str = "overlay:toggle";
    pub const SETTINGS: &str = "settings";
    pub const UPDATE: &str = "update";
    pub const RECONNECT: &str = "reconnect";
    pub const QUIT: &str = "quit";
}

pub struct Tray {
    // Held to keep the icon alive; dropping it removes the tray entry.
    _icon: TrayIcon,
    /// Kept so the pomodoro label can be updated in place.
    toggle_item: MenuItem,
    /// The nag entry. Disabled unless the peer is actually in a focus round, so
    /// the menu itself tells you whether nagging would mean anything.
    nag_item: MenuItem,
    /// Shows today's two focus totals; the most useful line in the menu.
    summary_item: MenuItem,
    /// The tooltip is the only always-visible surface when the overlay is hidden,
    /// so it carries the comparison too.
    icon_handle: TrayIcon,
}

impl Tray {
    /// Create the tray icon. Must run on the event-loop thread.
    pub fn new(app_name: &str) -> Result<Self> {
        let menu = Menu::new();

        // A disabled item used as a header. `muda` has no label widget, so a
        // permanently disabled entry is the idiomatic way to show read-only text.
        let summary_item = MenuItem::with_id("summary", "今日专注　—", false, None);
        menu.append(&summary_item).context("添加统计菜单失败")?;
        menu.append(&PredefinedMenuItem::separator()).ok();

        // The two accountability actions are top-level: a nag hidden two clicks
        // deep does not get sent.
        let nag_item = MenuItem::with_id(
            format!("{}{:?}", id::NUDGE_PREFIX, NudgeKind::Nag),
            format!("{} {}", NudgeKind::Nag.emoji(), NudgeKind::Nag.label()),
            false,
            None,
        );
        menu.append(&nag_item).context("添加督促菜单失败")?;

        menu.append(&MenuItem::with_id(
            format!("{}{:?}", id::NUDGE_PREFIX, NudgeKind::FocusTogether),
            format!(
                "{} {}",
                NudgeKind::FocusTogether.emoji(),
                NudgeKind::FocusTogether.label()
            ),
            true,
            None,
        ))
        .context("添加督促菜单失败")?;

        let nudges = Submenu::new("其他互动", true);
        for kind in NudgeKind::ALL {
            // Already promoted to the top level.
            if matches!(kind, NudgeKind::Nag | NudgeKind::FocusTogether) {
                continue;
            }
            nudges
                .append(&MenuItem::with_id(
                    format!("{}{:?}", id::NUDGE_PREFIX, kind),
                    format!("{} {}", kind.emoji(), kind.label()),
                    true,
                    None,
                ))
                .context("添加互动菜单失败")?;
        }
        menu.append(&nudges).context("添加互动菜单失败")?;

        let presence = Submenu::new("我的状态", true);
        for p in [
            Presence::Active,
            Presence::Resting,
            Presence::Busy,
            Presence::Away,
        ] {
            presence
                .append(&MenuItem::with_id(
                    format!("{}{:?}", id::PRESENCE_PREFIX, p),
                    p.label(),
                    true,
                    None,
                ))
                .context("添加状态菜单失败")?;
        }
        menu.append(&presence).context("添加状态菜单失败")?;

        menu.append(&PredefinedMenuItem::separator()).ok();

        let toggle_item = MenuItem::with_id(id::POMODORO_TOGGLE, "开始专注", true, None);
        menu.append(&toggle_item).context("添加番茄钟菜单失败")?;
        menu.append(&MenuItem::with_id(
            id::POMODORO_SKIP,
            "跳过当前阶段",
            true,
            None,
        ))
        .ok();
        menu.append(&MenuItem::with_id(
            id::POMODORO_STOP,
            "停止番茄钟",
            true,
            None,
        ))
        .ok();

        menu.append(&PredefinedMenuItem::separator()).ok();
        menu.append(&MenuItem::with_id(
            id::OVERLAY_TOGGLE,
            "显示/隐藏悬浮窗",
            true,
            None,
        ))
        .ok();
        menu.append(&MenuItem::with_id(id::SETTINGS, "设置…", true, None))
            .ok();
        menu.append(&MenuItem::with_id(id::UPDATE, "检查更新", true, None))
            .ok();
        menu.append(&MenuItem::with_id(id::RECONNECT, "重新连接", true, None))
            .ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        menu.append(&MenuItem::with_id(id::QUIT, "退出", true, None))
            .ok();

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(app_name)
            .with_icon(default_icon()?)
            // Windows only: the left click should open the app, not the menu.
            .with_menu_on_left_click(false)
            .build()
            .context("创建托盘图标失败")?;

        Ok(Self {
            // `TrayIcon` is refcounted internally, so the clone is a handle to the
            // same icon rather than a second tray entry.
            icon_handle: icon.clone(),
            _icon: icon,
            toggle_item,
            nag_item,
            summary_item,
        })
    }

    /// Keep the pomodoro entry's label in sync with the timer.
    pub fn set_pomodoro_label(&self, label: &str) {
        self.toggle_item.set_text(label);
    }

    /// Update the accountability parts of the menu and the tooltip.
    ///
    /// `peer_focusing` gates the nag entry: an action that does nothing useful
    /// should look unavailable rather than silently disappoint.
    pub fn set_accountability(
        &self,
        mine: u32,
        theirs: u32,
        goal: u32,
        peer_focusing: bool,
        peer_slacking: bool,
    ) {
        let summary = if goal > 0 {
            format!("今日专注　我 {mine} / TA {theirs}　目标 {goal}")
        } else {
            format!("今日专注　我 {mine} / TA {theirs}")
        };
        self.summary_item.set_text(&summary);

        self.nag_item.set_enabled(peer_focusing);
        self.nag_item.set_text(if peer_slacking {
            // Say what was caught, so the menu entry itself is the evidence.
            format!("{} 抓到了，别摸鱼了", NudgeKind::Nag.emoji())
        } else {
            format!("{} {}", NudgeKind::Nag.emoji(), NudgeKind::Nag.label())
        });

        // The tooltip is what a user sees when the overlay is hidden.
        let tooltip = if goal > 0 {
            format!("Synctus\n今日专注：我 {mine} / TA {theirs}（目标 {goal} 分钟）")
        } else {
            format!("Synctus\n今日专注：我 {mine} / TA {theirs}")
        };
        let _ = self.icon_handle.set_tooltip(Some(tooltip));
    }

    /// Drain tray and menu events, mapping them to requests.
    ///
    /// Both receivers are unbounded crossbeam channels filled from the platform
    /// event loop, so this never blocks.
    pub fn poll(&self) -> Vec<UiRequest> {
        let mut out = Vec::new();

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(request) = map_menu_id(&event.id) {
                out.push(request);
            }
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            // A left click on the icon shows the overlay; the menu handles the
            // rest.
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                out.push(UiRequest::ToggleOverlay);
            }
        }

        out
    }
}

fn map_menu_id(menu_id: &MenuId) -> Option<UiRequest> {
    let raw = menu_id.as_ref();

    if let Some(kind) = raw.strip_prefix(id::NUDGE_PREFIX) {
        return parse_nudge(kind).map(UiRequest::Nudge);
    }
    if let Some(presence) = raw.strip_prefix(id::PRESENCE_PREFIX) {
        return parse_presence(presence).map(UiRequest::SetPresence);
    }

    Some(match raw {
        id::POMODORO_TOGGLE => UiRequest::TogglePomodoro,
        id::POMODORO_SKIP => UiRequest::SkipPhase,
        id::POMODORO_STOP => UiRequest::StopPomodoro,
        id::OVERLAY_TOGGLE => UiRequest::ToggleOverlay,
        id::SETTINGS => UiRequest::OpenSettings,
        id::UPDATE => UiRequest::CheckUpdate,
        id::RECONNECT => UiRequest::Reconnect,
        id::QUIT => UiRequest::Quit,
        _ => return None,
    })
}

/// Ids embed the `Debug` name of the variant, so parsing mirrors that.
fn parse_nudge(name: &str) -> Option<NudgeKind> {
    NudgeKind::ALL
        .into_iter()
        .find(|k| format!("{k:?}") == name)
}

fn parse_presence(name: &str) -> Option<Presence> {
    [
        Presence::Active,
        Presence::Resting,
        Presence::Busy,
        Presence::Away,
    ]
    .into_iter()
    .find(|p| format!("{p:?}") == name)
}

/// Generate the tray icon in code.
///
/// Shipping a PNG would mean an asset to keep in sync across three build
/// targets; a 32×32 RGBA buffer costs 4 KiB and always matches the presence
/// colours used elsewhere.
fn default_icon() -> Result<Icon> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    let centre = (SIZE as f32 - 1.0) / 2.0;
    let outer = centre - 1.0;
    let inner = outer - 4.0;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - centre;
            let dy = y as f32 - centre;
            let dist = (dx * dx + dy * dy).sqrt();

            // A ring: opaque between `inner` and `outer`, with one pixel of
            // anti-aliasing on each edge so it does not look jagged at 16 px.
            let alpha = if dist > outer {
                0.0
            } else if dist > outer - 1.0 {
                outer - dist
            } else if dist > inner {
                1.0
            } else if dist > inner - 1.0 {
                dist - (inner - 1.0)
            } else {
                0.0
            };

            let (r, g, b) = (0x42, 0xa5, 0xf5);
            rgba.extend_from_slice(&[r, g, b, (alpha.clamp(0.0, 1.0) * 255.0) as u8]);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).context("生成托盘图标失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_the_expected_buffer_size() {
        // `from_rgba` validates this, so a failure here means the generator and
        // the declared size disagree.
        assert!(default_icon().is_ok());
    }

    #[test]
    fn nudge_ids_round_trip() {
        for kind in NudgeKind::ALL {
            let raw = format!("{}{:?}", id::NUDGE_PREFIX, kind);
            assert_eq!(
                map_menu_id(&MenuId::new(&raw)),
                Some(UiRequest::Nudge(kind)),
                "id {raw} must map back"
            );
        }
    }

    #[test]
    fn presence_ids_round_trip() {
        for p in [
            Presence::Active,
            Presence::Resting,
            Presence::Busy,
            Presence::Away,
        ] {
            let raw = format!("{}{:?}", id::PRESENCE_PREFIX, p);
            assert_eq!(
                map_menu_id(&MenuId::new(&raw)),
                Some(UiRequest::SetPresence(p))
            );
        }
    }

    #[test]
    fn fixed_ids_map_to_their_requests() {
        assert_eq!(map_menu_id(&MenuId::new(id::QUIT)), Some(UiRequest::Quit));
        assert_eq!(
            map_menu_id(&MenuId::new(id::SETTINGS)),
            Some(UiRequest::OpenSettings)
        );
        assert_eq!(map_menu_id(&MenuId::new("unknown-item")), None);
    }
}
