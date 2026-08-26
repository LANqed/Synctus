//! Server configuration, from a TOML file and/or environment variables.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Listen address.
    pub bind: String,

    /// PEM certificate chain. When both this and `key_path` are set the relay
    /// terminates TLS itself; leave empty when running behind a TLS-terminating
    /// reverse proxy.
    pub cert_path: Option<String>,
    pub key_path: Option<String>,

    /// Devices allowed in one room. Two is the intended case, but a person may
    /// run a PC and a phone, so the default leaves headroom.
    pub max_devices_per_room: usize,
    /// Total rooms held in memory. Bounds the memory a stranger can make the
    /// relay allocate by connecting with random room ids.
    pub max_rooms: usize,

    /// Seconds without a frame before a connection is dropped.
    pub idle_timeout_secs: u64,
    /// Heartbeat interval advertised to clients.
    pub heartbeat_secs: u32,

    /// Relayed frames per device per second, token-bucket. Status updates are
    /// event-driven, so a healthy client sends far fewer than this.
    pub rate_limit_per_sec: u32,
    /// Burst allowance on top of the steady rate.
    pub rate_limit_burst: u32,

    /// Seconds a client has to finish the handshake.
    pub handshake_timeout_secs: u64,

    /// The WebUI listen address, e.g. `127.0.0.1:9090`. `None` disables it.
    ///
    /// Reaching the relay from a browser means putting this on a real TCP port,
    /// so it needs a password; see [`ServerConfig::web_password`].
    pub web_bind: Option<String>,
    /// Admin password for the WebUI. Required when [`ServerConfig::web_bind`] is
    /// set.
    pub web_password: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8787".to_string(),
            cert_path: None,
            key_path: None,
            max_devices_per_room: 8,
            max_rooms: 10_000,
            idle_timeout_secs: 90,
            heartbeat_secs: 25,
            rate_limit_per_sec: 10,
            rate_limit_burst: 30,
            handshake_timeout_secs: 10,
            web_bind: None,
            web_password: None,
        }
    }
}

impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("解析服务器配置失败: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("读取服务器配置失败: {}", path.display())),
        }
    }

    /// Overlay `SYNCTUS_*` environment variables, which is how the container
    /// deployment is configured.
    pub fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("SYNCTUS_BIND") {
            self.bind = v;
        }
        if let Ok(v) = std::env::var("SYNCTUS_CERT") {
            self.cert_path = Some(v).filter(|s| !s.is_empty());
        }
        if let Ok(v) = std::env::var("SYNCTUS_KEY") {
            self.key_path = Some(v).filter(|s| !s.is_empty());
        }
        if let Some(v) = env_num("SYNCTUS_MAX_ROOMS") {
            self.max_rooms = v as usize;
        }
        if let Some(v) = env_num("SYNCTUS_RATE_LIMIT") {
            self.rate_limit_per_sec = v as u32;
        }
        if let Some(v) = env_num("SYNCTUS_IDLE_TIMEOUT") {
            self.idle_timeout_secs = v;
        }
    }

    /// TLS is enabled only when both halves of the keypair are configured.
    pub fn tls_enabled(&self) -> bool {
        self.cert_path.is_some() && self.key_path.is_some()
    }

    /// Write the config back, preserving the explanatory comments.
    ///
    /// `toml::to_string` would produce a valid but comment-free file, and the
    /// comments are how someone editing this by hand knows what the numbers mean.
    /// So the template is rendered with the current values substituted in.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建配置目录失败: {}", dir.display()))?;
        }

        let text = self.render();
        // Write to a temp file and rename, so a crash mid-write cannot leave the
        // server with a truncated config.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("写入配置失败: {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("替换配置失败: {}", path.display()))?;
        Ok(())
    }

    /// The config file as text, with comments.
    pub fn render(&self) -> String {
        let cert = self
            .cert_path
            .as_deref()
            .map(|p| format!("cert_path = \"{p}\""))
            .unwrap_or_else(|| {
                "# cert_path = \"/etc/letsencrypt/live/example.com/fullchain.pem\"".to_string()
            });
        let key = self
            .key_path
            .as_deref()
            .map(|p| format!("key_path = \"{p}\""))
            .unwrap_or_else(|| {
                "# key_path  = \"/etc/letsencrypt/live/example.com/privkey.pem\"".to_string()
            });

        // When disabled the keys are omitted entirely rather than emitted as
        // comments after `=`, which would not parse as TOML.
        let web = match (&self.web_bind, &self.web_password) {
            (Some(bind), Some(password)) => {
                format!("web_bind = \"{bind}\"\nweb_password = \"{password}\"")
            }
            _ => "# WebUI 管理面板：填入监听地址与管理员密码即可启用，例如：\n\
                 # web_bind = \"127.0.0.1:9090\"\n\
                 # web_password = \"强密码\""
                .to_string(),
        };

        format!(
            "# Synctus 中继服务器配置
#
# 由 synctus 管理工具生成。可以手工编辑，改完运行 `synctus` 选择重启。
# 环境变量优先于本文件。

# 监听地址。
bind = \"{bind}\"

# TLS 证书链与私钥（PEM）。两者都填写时服务器自行终止 TLS；
# 若在 Nginx / Caddy 等反向代理后运行，可留空由代理处理。
#
# 注意：不配置 TLS 时，房间标识与设备标识会以明文经过网络。
# 消息内容始终是端到端加密的，服务器无论如何都读不到。
{cert}
{key}

# 单个房间允许的设备数。两人使用时 2 就够，默认留出余量
# 以便一个人同时使用电脑和手机。
max_devices_per_room = {max_devices}

# 内存中保留的房间上限。这是防止陌生人用随机房间号
# 让服务器无限分配内存的边界。
max_rooms = {max_rooms}

# 超过该秒数没有收到任何帧就断开连接。必须大于 heartbeat_secs。
idle_timeout_secs = {idle}

# 通告给客户端的心跳间隔。
heartbeat_secs = {heartbeat}

# 每设备每秒可转发的消息数（令牌桶）与突发额度。
# 状态更新是事件驱动的，正常客户端远低于此值。
rate_limit_per_sec = {rate}
rate_limit_burst = {burst}

# 客户端完成握手的时限。
handshake_timeout_secs = {handshake}

# WebUI 管理面板：能看到每个用户有哪些设备、哪些在线，并可以断开设备。
# 两个都留空则关闭。
{web}
",
            bind = self.bind,
            cert = cert,
            key = key,
            max_devices = self.max_devices_per_room,
            max_rooms = self.max_rooms,
            idle = self.idle_timeout_secs,
            heartbeat = self.heartbeat_secs,
            rate = self.rate_limit_per_sec,
            burst = self.rate_limit_burst,
            handshake = self.handshake_timeout_secs,
            web = web,
        )
    }

    /// Check the values make sense together.
    ///
    /// Returns a list of problems rather than the first one: someone editing the
    /// file by hand wants to hear about all of them at once.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();

        if self.bind.parse::<std::net::SocketAddr>().is_err() {
            // A hostname is not accepted by `TcpListener::bind` in the form we
            // use, so catch it here rather than at startup.
            out.push(format!(
                "监听地址 `{}` 不是合法的 IP:端口（例如 0.0.0.0:8787）",
                self.bind
            ));
        }

        if self.idle_timeout_secs <= self.heartbeat_secs as u64 {
            out.push(format!(
                "空闲超时 {} 秒不大于心跳间隔 {} 秒，客户端会被反复断开",
                self.idle_timeout_secs, self.heartbeat_secs
            ));
        }

        if self.max_devices_per_room < 2 {
            out.push("每房间设备数小于 2，两个人无法同时在线".to_string());
        }

        if self.rate_limit_burst < self.rate_limit_per_sec {
            out.push(format!(
                "突发额度 {} 小于每秒限额 {}，实际速率会低于配置值",
                self.rate_limit_burst, self.rate_limit_per_sec
            ));
        }

        // The WebUI opens a TCP port that needs no other client authentication
        // than this password, so a password is mandatory when it is enabled.
        if let Some(bind) = &self.web_bind {
            if bind.parse::<std::net::SocketAddr>().is_err() {
                out.push(format!(
                    "WebUI 监听地址 `{bind}` 不是合法的 IP:端口（例如 127.0.0.1:9090）"
                ));
            }
            if self.web_password.as_deref().map_or(true, str::is_empty) {
                out.push("启用了 WebUI 但未设置管理员密码，拒绝启动".to_string());
            }
        }
        if self.web_password.is_some() && self.web_bind.is_none() {
            out.push("设置了 WebUI 密码但未设置监听地址，密码不会生效".to_string());
        }

        match (&self.cert_path, &self.key_path) {
            (Some(_), None) => out.push("配置了证书但没有私钥，TLS 不会启用".to_string()),
            (None, Some(_)) => out.push("配置了私钥但没有证书，TLS 不会启用".to_string()),
            (Some(cert), Some(key)) => {
                for (label, p) in [("证书", cert), ("私钥", key)] {
                    if !Path::new(p).is_file() {
                        out.push(format!("{label}文件不存在: {p}"));
                    }
                }
            }
            (None, None) => {}
        }

        out
    }
}

fn env_num(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = ServerConfig::default();
        assert!(!cfg.tls_enabled());
        assert!(cfg.idle_timeout_secs > cfg.heartbeat_secs as u64);
    }

    #[test]
    fn tls_needs_both_cert_and_key() {
        let mut cfg = ServerConfig {
            cert_path: Some("a.pem".into()),
            ..ServerConfig::default()
        };
        assert!(!cfg.tls_enabled(), "cert alone must not enable TLS");
        cfg.key_path = Some("b.pem".into());
        assert!(cfg.tls_enabled());
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let cfg = ServerConfig::load(Path::new("no-such-server-config-9a1.toml")).unwrap();
        assert_eq!(cfg.bind, "0.0.0.0:8787");
    }

    #[test]
    fn rendered_config_parses_back_to_the_same_values() {
        // The renderer writes the file by hand to keep the comments, so a typo in
        // the template would produce a file the server cannot read.
        let cfg = ServerConfig {
            bind: "127.0.0.1:9000".into(),
            cert_path: Some("/tmp/cert.pem".into()),
            key_path: Some("/tmp/key.pem".into()),
            max_rooms: 42,
            rate_limit_per_sec: 7,
            rate_limit_burst: 21,
            ..ServerConfig::default()
        };

        let back: ServerConfig = toml::from_str(&cfg.render()).expect("rendered config must parse");
        assert_eq!(back.bind, "127.0.0.1:9000");
        assert_eq!(back.cert_path.as_deref(), Some("/tmp/cert.pem"));
        assert_eq!(back.key_path.as_deref(), Some("/tmp/key.pem"));
        assert_eq!(back.max_rooms, 42);
        assert_eq!(back.rate_limit_per_sec, 7);
        assert_eq!(back.rate_limit_burst, 21);
    }

    #[test]
    fn rendered_config_without_tls_leaves_the_keys_commented() {
        let cfg = ServerConfig::default();
        let text = cfg.render();
        assert!(text.contains("# cert_path"), "got:\n{text}");

        let back: ServerConfig = toml::from_str(&text).unwrap();
        assert!(!back.tls_enabled());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("synctus-srv-{}", std::process::id()));
        let path = dir.join("server.toml");
        let cfg = ServerConfig {
            bind: "0.0.0.0:1234".into(),
            max_devices_per_room: 4,
            ..ServerConfig::default()
        };
        cfg.save(&path).unwrap();

        let back = ServerConfig::load(&path).unwrap();
        assert_eq!(back.bind, "0.0.0.0:1234");
        assert_eq!(back.max_devices_per_room, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sane_config_has_no_problems() {
        assert!(ServerConfig::default().problems().is_empty());
    }

    #[test]
    fn problems_catches_each_mistake() {
        let bad = ServerConfig {
            bind: "example.com:8787".into(),
            idle_timeout_secs: 10,
            heartbeat_secs: 25,
            max_devices_per_room: 1,
            rate_limit_per_sec: 30,
            rate_limit_burst: 5,
            ..ServerConfig::default()
        };
        let problems = bad.problems();
        // All of them at once, so someone editing the file hears everything.
        assert_eq!(problems.len(), 4, "got {problems:?}");
        assert!(problems.iter().any(|p| p.contains("监听地址")));
        assert!(problems.iter().any(|p| p.contains("空闲超时")));
        assert!(problems.iter().any(|p| p.contains("每房间设备数")));
        assert!(problems.iter().any(|p| p.contains("突发额度")));
    }

    #[test]
    fn half_configured_tls_is_reported() {
        let cfg = ServerConfig {
            cert_path: Some("/nope/cert.pem".into()),
            ..ServerConfig::default()
        };
        assert!(cfg.problems().iter().any(|p| p.contains("没有私钥")));
    }

    #[test]
    fn a_missing_certificate_file_is_reported() {
        let cfg = ServerConfig {
            cert_path: Some("/definitely/not/here/cert.pem".into()),
            key_path: Some("/definitely/not/here/key.pem".into()),
            ..ServerConfig::default()
        };
        let problems = cfg.problems();
        assert_eq!(problems.len(), 2, "got {problems:?}");
        assert!(problems.iter().all(|p| p.contains("不存在")));
    }

    #[test]
    fn web_needs_a_password_when_enabled() {
        let cfg = ServerConfig {
            web_bind: Some("127.0.0.1:9090".into()),
            web_password: None,
            ..ServerConfig::default()
        };
        assert!(cfg.problems().iter().any(|p| p.contains("管理员密码")));
    }

    #[test]
    fn a_password_without_a_bind_is_reported() {
        let cfg = ServerConfig {
            web_bind: None,
            web_password: Some("secret".into()),
            ..ServerConfig::default()
        };
        assert!(cfg.problems().iter().any(|p| p.contains("未设置监听地址")));
    }

    #[test]
    fn a_fully_configured_web_is_fine() {
        let cfg = ServerConfig {
            web_bind: Some("127.0.0.1:9090".into()),
            web_password: Some("secret".into()),
            ..ServerConfig::default()
        };
        assert!(cfg.problems().is_empty(), "got {:?}", cfg.problems());
    }

    #[test]
    fn web_settings_survive_a_render_roundtrip() {
        let cfg = ServerConfig {
            web_bind: Some("0.0.0.0:9090".into()),
            web_password: Some("hunter2".into()),
            ..ServerConfig::default()
        };
        let back: ServerConfig = toml::from_str(&cfg.render()).unwrap();
        assert_eq!(back.web_bind.as_deref(), Some("0.0.0.0:9090"));
        assert_eq!(back.web_password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn disabled_web_renders_as_comments_not_invalid_toml() {
        // The disabled template must stay parseable — a `key = # comment` line
        // would be a syntax error the next time the daemon reads it.
        let cfg = ServerConfig::default();
        let text = cfg.render();
        assert!(text.contains("web_bind"));
        let back: ServerConfig = toml::from_str(&text).unwrap();
        assert!(back.web_bind.is_none());
        assert!(back.web_password.is_none());
    }
}
