//! Run-at-login registration.
//!
//! Windows uses the per-user `Run` registry key and Linux an XDG autostart
//! desktop entry. Both are per-user, so neither needs elevation.

use anyhow::{Context, Result};

const APP_NAME: &str = "Synctus";

/// Build the launcher. `--minimised` keeps the overlay from stealing focus at
/// login.
fn launcher() -> Result<auto_launch::AutoLaunch> {
    let exe = crate::paths::current_exe()?;
    let path = exe.to_str().context("程序路径包含无效字符")?;

    auto_launch::AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(path)
        .set_args(&["--minimised"])
        .build()
        .context("构建自启动配置失败")
}

pub fn is_enabled() -> Result<bool> {
    launcher()?.is_enabled().context("查询自启动状态失败")
}

/// Enable or disable autostart. Idempotent.
pub fn set(enabled: bool) -> Result<()> {
    let launcher = launcher()?;
    if enabled {
        launcher.enable().context("启用开机自启失败")
    } else {
        // Disabling something already absent must not be an error, otherwise
        // toggling the setting twice fails.
        match launcher.disable() {
            Ok(()) => Ok(()),
            Err(e) if !launcher.is_enabled().unwrap_or(false) => {
                tracing::debug!(error = %e, "自启动项本就不存在");
                Ok(())
            }
            Err(e) => Err(e).context("关闭开机自启失败"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_targets_the_current_executable() {
        // The registry/desktop-entry write itself is not exercised here: it
        // would modify the developer's real session.
        let l = launcher().expect("builder should succeed");
        assert_eq!(l.get_app_name(), APP_NAME);
        assert!(!l.get_app_path().is_empty());
    }

    #[test]
    fn querying_state_does_not_error() {
        assert!(is_enabled().is_ok());
    }
}
