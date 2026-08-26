//! Persisted client configuration.
//!
//! Kept in `core` so the desktop and Android front-ends share one file format.
//! The crate deliberately does not decide *where* the file lives: the desktop
//! uses the OS config directory while Android passes its private app dir.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What the local device is allowed to publish. Everything defaults to the
/// least surprising choice: share the app, but not what the window says.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    pub share_foreground_app: bool,
    /// Window titles often contain file names or chat partners, so this is off
    /// by default even when the app name is shared.
    pub share_window_title: bool,
    pub share_battery: bool,
    pub share_music: bool,
    pub share_pomodoro: bool,
    pub share_todos: bool,
    /// Publish seconds-since-last-input, which drives automatic "away".
    pub share_idle: bool,
    /// Process names that are replaced with a placeholder before publishing.
    pub blocked_apps: Vec<String>,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            share_foreground_app: true,
            share_window_title: false,
            share_battery: true,
            share_music: true,
            share_pomodoro: true,
            share_todos: true,
            share_idle: true,
            blocked_apps: Vec::new(),
        }
    }
}

impl Privacy {
    /// Case-insensitive match against the block list.
    pub fn is_blocked(&self, app: &str) -> bool {
        let app = app.to_ascii_lowercase();
        self.blocked_apps
            .iter()
            .any(|b| !b.is_empty() && app.contains(&b.to_ascii_lowercase()))
    }
}

/// Pomodoro lengths, in minutes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PomodoroConfig {
    pub focus_min: u32,
    pub short_break_min: u32,
    pub long_break_min: u32,
    /// Focus rounds before a long break.
    pub rounds_per_set: u32,
    /// Start the next phase without waiting for a click.
    pub auto_continue: bool,
    /// Flip presence to `Resting` for the duration of a break phase.
    pub presence_follows_phase: bool,
}

impl Default for PomodoroConfig {
    fn default() -> Self {
        Self {
            focus_min: 25,
            short_break_min: 5,
            long_break_min: 15,
            rounds_per_set: 4,
            auto_continue: false,
            presence_follows_phase: true,
        }
    }
}

/// The accountability settings — what turns a status widget into something that
/// actually keeps two people working.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Accountability {
    /// Daily focus target in minutes. 0 disables goals and streaks entirely.
    pub daily_goal_min: u32,

    /// Warn *me* when I open a distracting app during a focus round.
    pub warn_on_distraction: bool,
    /// Apps that count as a distraction. Matched case-insensitively as a
    /// substring, so `bilibili` catches both the browser tab and the app.
    pub distracting_apps: Vec<String>,
    /// Seconds a distracting app must stay in the foreground before it counts.
    /// Prevents a flicker while alt-tabbing from tripping the alarm.
    pub distraction_grace_secs: u32,

    /// Tell the peer when I get distracted during a focus round.
    ///
    /// Off by default: being watched is something to opt into, not something a
    /// tool should assume. With it off the warning stays local.
    pub report_distraction_to_peer: bool,

    /// Let the peer's nag break through my do-not-disturb.
    pub allow_urgent_nudges: bool,

    /// Automatically congratulate the peer when they finish a round or hit their
    /// goal, so encouragement does not depend on someone remembering.
    pub auto_cheer: bool,
}

impl Default for Accountability {
    fn default() -> Self {
        Self {
            // Four classic pomodoros. A goal of zero would make the whole feature
            // invisible, and a default of zero is how features go unused.
            daily_goal_min: 100,
            warn_on_distraction: true,
            distracting_apps: default_distracting_apps(),
            distraction_grace_secs: 30,
            report_distraction_to_peer: false,
            allow_urgent_nudges: true,
            auto_cheer: true,
        }
    }
}

impl Accountability {
    /// Whether `app` is on the distraction list.
    pub fn is_distracting(&self, app: &str) -> bool {
        let app = app.to_ascii_lowercase();
        self.distracting_apps
            .iter()
            .any(|d| !d.is_empty() && app.contains(&d.to_ascii_lowercase()))
    }

    pub fn goals_enabled(&self) -> bool {
        self.daily_goal_min > 0
    }
}

/// A starting list the user is expected to edit.
///
/// Deliberately generic and short: guessing someone's particular time sinks is
/// futile, but an empty list means the feature does nothing until configured.
fn default_distracting_apps() -> Vec<String> {
    [
        "bilibili",
        "youtube",
        "tiktok",
        "douyin",
        "netflix",
        "steam",
        "epicgames",
        "discord",
        "twitter",
        "instagram",
        "reddit",
        "zhihu",
        "weibo",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// Relay address as `host:port`.
    pub server: String,
    /// Verify the relay certificate against the system roots. Turn off only for
    /// a self-signed relay on a trusted network; payloads stay end-to-end
    /// encrypted either way.
    pub tls: bool,
    /// Name used for TLS verification when it differs from `server`'s host.
    pub tls_server_name: Option<String>,
    /// Shared pairing code. Both peers must type the same one.
    pub invite_code: String,
    /// Stable id for this installation.
    pub device_id: String,
    /// Name shown to the peer.
    pub device_name: String,

    pub privacy: Privacy,
    pub pomodoro: PomodoroConfig,
    pub accountability: Accountability,

    /// How often to sample local sensors, in seconds.
    pub poll_secs: u64,
    /// Idle seconds before presence flips to `Away`. 0 disables.
    pub away_after_secs: u32,
    /// Treat a peer snapshot older than this as offline.
    pub peer_stale_secs: u64,

    pub start_minimised: bool,
    pub autostart: bool,
    pub show_overlay: bool,
    /// Overlay position, persisted after dragging.
    pub overlay_x: Option<i32>,
    pub overlay_y: Option<i32>,
    /// Keep the overlay above other windows.
    pub overlay_always_on_top: bool,
    pub check_updates: bool,
    /// `owner/repo` used for the GitHub release check.
    pub update_repo: String,
    pub mute_nudges: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server: "127.0.0.1:8787".to_string(),
            tls: true,
            tls_server_name: None,
            invite_code: String::new(),
            device_id: crate::crypto::random_id(8),
            device_name: default_device_name(),
            privacy: Privacy::default(),
            pomodoro: PomodoroConfig::default(),
            accountability: Accountability::default(),
            poll_secs: 5,
            away_after_secs: 300,
            peer_stale_secs: 90,
            start_minimised: false,
            autostart: false,
            show_overlay: true,
            overlay_x: None,
            overlay_y: None,
            overlay_always_on_top: true,
            check_updates: true,
            update_repo: "LANqed/Synctus".to_string(),
            mute_nudges: false,
        }
    }
}

impl ClientConfig {
    /// Load from `path`, falling back to defaults when the file is missing.
    ///
    /// A malformed file is an error rather than a silent reset, so a typo does
    /// not wipe the user's invite code.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("解析配置失败: {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("读取配置失败: {}", path.display())),
        }
    }

    /// Write atomically: render to a temp file next to the target, then rename,
    /// so a crash mid-write cannot leave a truncated config.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建配置目录失败: {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).context("序列化配置失败")?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("写入配置失败: {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("替换配置失败: {}", path.display()))?;
        Ok(())
    }

    /// Ready to connect only once a pairing code exists.
    pub fn is_paired(&self) -> bool {
        self.invite_code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .count()
            >= 8
    }

    /// Host part of `server`, used as the default TLS name.
    pub fn server_name(&self) -> String {
        if let Some(name) = &self.tls_server_name {
            return name.clone();
        }
        // Handles `host:port` and bare `[v6]:port`.
        let s = &self.server;
        if let Some(rest) = s.strip_prefix('[') {
            if let Some((host, _)) = rest.split_once(']') {
                return host.to_string();
            }
        }
        s.rsplit_once(':').map(|(h, _)| h).unwrap_or(s).to_string()
    }

    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_secs.clamp(1, 60))
    }

    pub fn peer_stale_ms(&self) -> i64 {
        (self.peer_stale_secs.clamp(15, 3600) * 1000) as i64
    }
}

fn default_device_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    match crate::model::Platform::current() {
        crate::model::Platform::Android => "Android".into(),
        crate::model::Platform::Windows => "Windows PC".into(),
        crate::model::Platform::Linux => "Linux PC".into(),
        crate::model::Platform::Other => "Synctus".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let cfg = ClientConfig::load(Path::new("does-not-exist-4f2a.toml")).unwrap();
        assert_eq!(cfg.poll_secs, 5);
        assert!(!cfg.is_paired());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("synctus-cfg-{}", crate::crypto::random_id(4)));
        let path = dir.join("config.toml");
        let cfg = ClientConfig {
            invite_code: "ABCD-EFGH-IJKL-MNOP".into(),
            privacy: Privacy {
                blocked_apps: vec!["KeePass".into()],
                ..Privacy::default()
            },
            ..ClientConfig::default()
        };
        cfg.save(&path).unwrap();

        let back = ClientConfig::load(&path).unwrap();
        assert_eq!(back.invite_code, cfg.invite_code);
        assert!(back.is_paired());
        assert!(back.privacy.is_blocked("keepassxc.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_config_is_an_error() {
        let path =
            std::env::temp_dir().join(format!("synctus-bad-{}.toml", crate::crypto::random_id(4)));
        std::fs::write(&path, "server = [unclosed").unwrap();
        assert!(ClientConfig::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn server_name_strips_port_and_brackets() {
        let mut cfg = ClientConfig {
            server: "sync.example.com:8787".into(),
            ..ClientConfig::default()
        };
        assert_eq!(cfg.server_name(), "sync.example.com");
        cfg.server = "[2001:db8::1]:8787".into();
        assert_eq!(cfg.server_name(), "2001:db8::1");
        cfg.tls_server_name = Some("override".into());
        assert_eq!(cfg.server_name(), "override");
    }

    #[test]
    fn unknown_keys_and_partial_files_are_tolerated() {
        // Forward compatibility: an older client must survive a newer config.
        let path =
            std::env::temp_dir().join(format!("synctus-fwd-{}.toml", crate::crypto::random_id(4)));
        std::fs::write(&path, "server = \"a:1\"\nfuture_option = 42\n").unwrap();
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!(cfg.server, "a:1");
        let _ = std::fs::remove_file(&path);
    }
}
