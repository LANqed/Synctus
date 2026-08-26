//! Synctus desktop client.
//!
//! A small always-on-top overlay showing what the other person is doing, plus a
//! tray menu, a pomodoro timer and a to-do list. All status content is
//! end-to-end encrypted; the relay only forwards ciphertext.
//!
//! ```text
//! synctus                 # normal start
//! synctus --minimised     # start hidden, used by the autostart entry
//! ```

// Hide the console window on Windows release builds. Without this, launching the
// GUI from Explorer flashes a black window, and the autostart entry would leave
// one open. Debug builds keep it so `tracing` output is visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod autostart;
mod notify;
mod paths;
mod sensors;
mod tray;
mod ui;

use anyhow::Result;
use eframe::egui;

use app::{App, UiRequest};
use ui::{UiState, OVERLAY_SIZE};

fn main() -> Result<()> {
    let args = Args::parse();
    if args.help {
        print_help();
        return Ok(());
    }
    if args.version {
        println!("synctus-desktop {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    init_tracing();

    let config_path = paths::config_file()?;
    let data_path = paths::data_file()?;
    let app = App::new(config_path, data_path)?;

    // Position the overlay where the user last dragged it.
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(OVERLAY_SIZE)
        .with_min_inner_size([200.0, 72.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_title("Synctus");

    if app.cfg.overlay_always_on_top {
        viewport = viewport.with_always_on_top();
    }
    if let (Some(x), Some(y)) = (app.cfg.overlay_x, app.cfg.overlay_y) {
        viewport = viewport.with_position([x as f32, y as f32]);
    }

    let start_hidden = args.minimised || !app.cfg.show_overlay;
    if start_hidden {
        viewport = viewport.with_visible(false);
    }

    let options = eframe::NativeOptions {
        viewport,
        // No persistence: the config file is the single source of truth, and
        // eframe's own store would duplicate the window geometry.
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "Synctus",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(Desktop::new(app, start_hidden)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("启动界面失败: {e}"))?;

    Ok(())
}

struct Desktop {
    app: App,
    ui: UiState,
    /// `None` until the tray is created on the event-loop thread.
    tray: Option<tray::Tray>,
    tray_failed: bool,
    /// Whether the overlay window is currently visible.
    overlay_visible: bool,
    /// Last label pushed to the tray, to avoid redundant updates.
    last_tray_label: String,
    /// Last accountability values pushed to the tray. Updating a menu item on
    /// every frame would flicker on Windows, so the tuple acts as a change guard.
    last_tray_accountability: (u32, u32, u32, bool, bool),
}

impl Desktop {
    fn new(app: App, start_hidden: bool) -> Self {
        Self {
            app,
            ui: UiState::default(),
            tray: None,
            tray_failed: false,
            overlay_visible: !start_hidden,
            last_tray_label: String::new(),
            // Deliberately not the real initial values, so the first sync always
            // pushes something.
            last_tray_accountability: (u32::MAX, u32::MAX, u32::MAX, false, false),
        }
    }

    /// Create the tray on first update: `tray-icon` requires the platform event
    /// loop to already be running.
    fn ensure_tray(&mut self) {
        if self.tray.is_some() || self.tray_failed {
            return;
        }
        match tray::Tray::new("Synctus") {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => {
                // A missing StatusNotifier host on Linux is common; the overlay
                // still works, so this is a warning rather than fatal.
                self.app.note(format!("托盘不可用，将只使用悬浮窗: {e:#}"));
                self.tray_failed = true;
            }
        }
    }

    fn sync_tray_label(&mut self) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        let state = self.app.pomodoro.state();
        let label = match state.phase {
            synctus_core::model::PomodoroPhase::Idle => "开始专注".to_string(),
            _ if state.paused() => {
                format!("继续（{}）", state.remaining_text(synctus_core::now_ms()))
            }
            phase => format!(
                "暂停（{} {}）",
                phase.label(),
                state.remaining_text(synctus_core::now_ms())
            ),
        };
        if label != self.last_tray_label {
            tray.set_pomodoro_label(&label);
            self.last_tray_label = label;
        }

        // The tooltip and the nag entry are what the tray contributes to keeping
        // each other honest, so they follow the numbers rather than the timer.
        let (mine, theirs) = self.app.focus_comparison();
        let goal = self.app.cfg.accountability.daily_goal_min;
        let peer_focusing = self.app.peer_is_focusing();
        let peer_slacking = self.app.peer_is_slacking();
        let signature = (mine, theirs, goal, peer_focusing, peer_slacking);

        if signature != self.last_tray_accountability {
            tray.set_accountability(mine, theirs, goal, peer_focusing, peer_slacking);
            self.last_tray_accountability = signature;
        }
    }

    /// Apply a request. Returns true when the app should exit.
    fn handle(&mut self, ctx: &egui::Context, request: UiRequest) -> bool {
        // Overlay visibility is a window concern, so it is handled here rather
        // than in `App`.
        if request == UiRequest::ToggleOverlay {
            self.overlay_visible = !self.overlay_visible;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(self.overlay_visible));
            if self.overlay_visible {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.app.cfg.show_overlay = self.overlay_visible;
            self.app.save_config();
            return false;
        }

        self.app.handle(request)
    }

    /// Remember where the user dragged the overlay.
    fn remember_position(&mut self, ctx: &egui::Context) {
        let position = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
        if let Some(pos) = position {
            let (x, y) = (pos.x as i32, pos.y as i32);
            if self.app.cfg.overlay_x != Some(x) || self.app.cfg.overlay_y != Some(y) {
                self.app.cfg.overlay_x = Some(x);
                self.app.cfg.overlay_y = Some(y);
                // Not saved every frame: `shutdown` writes the final position.
            }
        }
    }
}

impl eframe::App for Desktop {
    /// Transparent background so the rounded overlay has no visible box.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.ensure_tray();
        self.app.tick();
        self.sync_tray_label();

        // Tray events first, so a menu click is applied this frame.
        let mut requests: Vec<UiRequest> = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();

        if self.overlay_visible {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    if let Some(request) = ui::overlay(ui, &mut self.app, &mut self.ui) {
                        requests.push(request);
                    }
                });
        }

        if self.app.show_settings && ui::settings(&ctx, &mut self.app, &mut self.ui) {
            self.app.show_settings = false;
        }

        for request in requests {
            if self.handle(&ctx, request) {
                self.app.shutdown();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        self.remember_position(&ctx);

        // Closing the overlay window hides it instead of quitting: the tray keeps
        // the app running, which is the whole point of a status widget.
        if ctx.input(|i| i.viewport().close_requested()) && self.tray.is_some() {
            self.overlay_visible = false;
            self.app.cfg.show_overlay = false;
            self.app.save_config();
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Repaint once a second: enough for the countdown and staleness display,
        // cheap enough to stay near-zero CPU.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.shutdown();
    }
}

/// Register a CJK-capable font.
///
/// egui's bundled fonts have no CJK coverage, so without this every Chinese
/// label renders as tofu. The font is loaded from the OS rather than embedded to
/// keep the binary small.
fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        // Windows
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        // Linux
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    ];

    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };

        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        // Appended, not prepended: keep egui's own glyphs (icons, arrows) as the
        // first choice and fall back to the CJK face.
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        tracing::debug!(font = path, "已加载中文字体");
        return;
    }

    tracing::warn!("未找到中文字体，界面中的中文可能显示为方块");
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SYNCTUS_LOG")
        .unwrap_or_else(|_| EnvFilter::new("synctus=info,synctus_core=info,warn"));
    fmt().with_env_filter(filter).with_target(false).init();
}

#[derive(Default)]
struct Args {
    minimised: bool,
    help: bool,
    version: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Self::default();
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--minimised" | "--minimized" | "-m" => args.minimised = true,
                "-h" | "--help" => args.help = true,
                "-V" | "--version" => args.version = true,
                _ => {}
            }
        }
        args
    }
}

fn print_help() {
    println!(
        "Synctus 桌面端 {}

用法:
  synctus-desktop [选项]

选项:
  -m, --minimised   启动时隐藏悬浮窗（开机自启使用）
  -h, --help        显示本帮助
  -V, --version     显示版本

环境变量:
  SYNCTUS_CONFIG_DIR  覆盖配置目录（便携模式）
  SYNCTUS_LOG         日志过滤，如 synctus=debug

首次使用请在设置中填入与对方相同的配对码。",
        env!("CARGO_PKG_VERSION")
    );
}
