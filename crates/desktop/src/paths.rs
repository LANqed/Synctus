//! Where the desktop client keeps its files.
//!
//! One directory for everything, resolved through the platform's standard config
//! location so an uninstall is a single `rm -r`.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Environment variable that relocates the whole config directory. This is what
/// makes a portable install possible.
const OVERRIDE_VAR: &str = "SYNCTUS_CONFIG_DIR";

/// `%APPDATA%\Synctus` on Windows, `~/.config/synctus` on Linux, unless
/// overridden by `SYNCTUS_CONFIG_DIR`.
pub fn config_dir() -> Result<PathBuf> {
    resolve_config_dir(std::env::var(OVERRIDE_VAR).ok().as_deref())
}

/// The resolution rule, separated from the environment so it is testable without
/// mutating process-global state.
fn resolve_config_dir(override_value: Option<&str>) -> Result<PathBuf> {
    if let Some(dir) = override_value.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir));
    }

    let dirs =
        directories::ProjectDirs::from("dev", "Synctus", "Synctus").context("无法确定配置目录")?;
    Ok(dirs.config_dir().to_path_buf())
}

pub fn config_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn data_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("data.json"))
}

/// Absolute path of the running executable, used for the autostart entry.
pub fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("无法获取程序路径")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_when_set() {
        let dir = resolve_config_dir(Some("/tmp/synctus-test")).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/synctus-test"));
    }

    #[test]
    fn blank_override_falls_back_to_the_platform_default() {
        // An exported-but-empty variable is a common shell accident and must not
        // send the config to the filesystem root.
        for blank in [Some(""), Some("   "), None] {
            let dir = resolve_config_dir(blank).unwrap();
            assert!(dir.is_absolute(), "got {}", dir.display());
            assert!(dir.components().count() > 1, "got {}", dir.display());
        }
    }

    #[test]
    fn file_names_sit_under_the_config_dir() {
        let base = resolve_config_dir(Some("/tmp/synctus-test")).unwrap();
        assert_eq!(base.join("config.toml").file_name().unwrap(), "config.toml");
        assert_eq!(base.join("data.json").file_name().unwrap(), "data.json");
    }

    #[test]
    fn current_exe_is_resolvable() {
        assert!(current_exe().unwrap().is_absolute());
    }
}
