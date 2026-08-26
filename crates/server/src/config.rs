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
}
