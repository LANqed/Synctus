//! Talking to whatever init system is actually present.
//!
//! Debian and friends use systemd; Alpine uses OpenRC. Rather than making the
//! user care, the tool detects which one manages the service and speaks to it.
//! Anything else falls back to a bare "not managed" state, where the TUI still
//! shows status and edits config but cannot start or stop.
//!
//! Every operation returns the command output on failure. A management tool that
//! says "failed" without saying why just sends the user to look for logs.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Which init system manages `synctus-server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManager {
    Systemd,
    OpenRc,
    /// No recognised init system, or the service is not installed under one.
    None,
}

impl ServiceManager {
    pub fn label(self) -> &'static str {
        match self {
            ServiceManager::Systemd => "systemd",
            ServiceManager::OpenRc => "OpenRC",
            ServiceManager::None => "未托管",
        }
    }

    /// Detect the init system.
    ///
    /// Presence of the *unit file* is what matters, not just of the init system:
    /// a Debian box has systemd whether or not Synctus is installed, and claiming
    /// to manage a service that does not exist produces confusing errors.
    pub fn detect() -> Self {
        if Path::new("/etc/systemd/system/synctus-server.service").exists()
            || Path::new("/lib/systemd/system/synctus-server.service").exists()
            || Path::new("/usr/lib/systemd/system/synctus-server.service").exists()
        {
            return ServiceManager::Systemd;
        }
        if Path::new("/etc/init.d/synctus-server").exists() {
            return ServiceManager::OpenRc;
        }
        ServiceManager::None
    }
}

/// Whether the service is currently running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Stopped,
    /// Could not be determined — no service manager, or the query failed.
    Unknown,
}

impl RunState {
    pub fn label(self) -> &'static str {
        match self {
            RunState::Running => "运行中",
            RunState::Stopped => "已停止",
            RunState::Unknown => "未知",
        }
    }
}

/// Manages the relay service through the detected init system.
pub struct Service {
    manager: ServiceManager,
}

impl Service {
    pub fn detect() -> Self {
        Self {
            manager: ServiceManager::detect(),
        }
    }

    pub fn manager(&self) -> ServiceManager {
        self.manager
    }

    /// Whether the service is running.
    pub fn state(&self) -> RunState {
        match self.manager {
            ServiceManager::Systemd => {
                // `is-active` exits non-zero when inactive, so the exit status is
                // the answer; stdout is only used to distinguish "unknown".
                match run("systemctl", &["is-active", "synctus-server"]) {
                    Ok(out) if out.trim() == "active" => RunState::Running,
                    Ok(_) => RunState::Stopped,
                    // A failed exit code with `inactive`/`failed` on stdout still
                    // means stopped rather than unknown.
                    Err(Failure { stdout, .. }) => match stdout.trim() {
                        "inactive" | "failed" | "activating" | "deactivating" => RunState::Stopped,
                        _ => RunState::Unknown,
                    },
                }
            }
            ServiceManager::OpenRc => match run("rc-service", &["synctus-server", "status"]) {
                Ok(out) => {
                    if out.contains("started") {
                        RunState::Running
                    } else {
                        RunState::Stopped
                    }
                }
                Err(_) => RunState::Stopped,
            },
            ServiceManager::None => RunState::Unknown,
        }
    }

    /// Whether the service starts at boot.
    pub fn enabled(&self) -> Option<bool> {
        match self.manager {
            ServiceManager::Systemd => match run("systemctl", &["is-enabled", "synctus-server"]) {
                Ok(out) => Some(out.trim() == "enabled"),
                Err(_) => Some(false),
            },
            ServiceManager::OpenRc => {
                // `rc-update show` lists enabled services with their runlevels.
                match run("rc-update", &["show", "default"]) {
                    Ok(out) => Some(out.contains("synctus-server")),
                    Err(_) => None,
                }
            }
            ServiceManager::None => None,
        }
    }

    pub fn start(&self) -> Result<()> {
        self.action("start")
    }

    pub fn stop(&self) -> Result<()> {
        self.action("stop")
    }

    pub fn restart(&self) -> Result<()> {
        self.action("restart")
    }

    /// Enable or disable start-at-boot.
    pub fn set_enabled(&self, enable: bool) -> Result<()> {
        match self.manager {
            ServiceManager::Systemd => {
                let verb = if enable { "enable" } else { "disable" };
                run("systemctl", &[verb, "synctus-server"])
                    .map(|_| ())
                    .map_err(|f| f.into_error(&format!("systemctl {verb}")))
            }
            ServiceManager::OpenRc => {
                let args: Vec<&str> = if enable {
                    vec!["add", "synctus-server", "default"]
                } else {
                    vec!["del", "synctus-server", "default"]
                };
                run("rc-update", &args)
                    .map(|_| ())
                    .map_err(|f| f.into_error("rc-update"))
            }
            ServiceManager::None => bail!("未检测到服务管理器，无法设置开机启动"),
        }
    }

    /// Recent log lines, newest last.
    ///
    /// systemd has the journal; OpenRC writes to a file, so the two paths differ.
    pub fn logs(&self, lines: usize) -> Result<String> {
        match self.manager {
            ServiceManager::Systemd => run(
                "journalctl",
                &[
                    "-u",
                    "synctus-server",
                    "-n",
                    &lines.to_string(),
                    "--no-pager",
                    "--output=short",
                ],
            )
            .map_err(|f| f.into_error("journalctl")),
            ServiceManager::OpenRc => {
                let path = "/var/log/synctus-server.log";
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("读取日志失败: {path}"))?;
                let tail: Vec<&str> = text
                    .lines()
                    .rev()
                    .take(lines)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                Ok(tail.join("\n"))
            }
            ServiceManager::None => bail!("未检测到服务管理器，无法读取日志"),
        }
    }

    fn action(&self, verb: &str) -> Result<()> {
        match self.manager {
            ServiceManager::Systemd => run("systemctl", &[verb, "synctus-server"])
                .map(|_| ())
                .map_err(|f| f.into_error(&format!("systemctl {verb}"))),
            ServiceManager::OpenRc => run("rc-service", &["synctus-server", verb])
                .map(|_| ())
                .map_err(|f| f.into_error(&format!("rc-service {verb}"))),
            ServiceManager::None => {
                bail!("未检测到服务管理器。请先用安装脚本安装服务，或手动运行 synctus-server")
            }
        }
    }
}

/// A command that ran but failed.
pub struct Failure {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Failure {
    /// Turn the captured output into an error worth showing a user.
    fn into_error(self, what: &str) -> anyhow::Error {
        let detail = if !self.stderr.trim().is_empty() {
            self.stderr.trim().to_string()
        } else if !self.stdout.trim().is_empty() {
            self.stdout.trim().to_string()
        } else {
            format!("退出码 {:?}", self.code)
        };
        anyhow::anyhow!("{what} 失败: {detail}")
    }
}

/// Run a command, capturing output.
///
/// `Err` carries the output so the caller can distinguish "failed because X" from
/// "could not run at all".
fn run(program: &str, args: &[&str]) -> std::result::Result<String, Failure> {
    let output = match Command::new(program).args(args).output() {
        Ok(o) => o,
        Err(e) => {
            return Err(Failure {
                stdout: String::new(),
                stderr: format!("无法执行 {program}: {e}"),
                code: None,
            })
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(Failure {
            stdout,
            stderr,
            code: output.status.code(),
        })
    }
}

/// Whether the process has the privileges to manage a system service.
///
/// Checked so the TUI can say "run with sudo" up front rather than after the user
/// picks an action and watches it fail.
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` takes no arguments, cannot fail and has no side
        // effects.
        unsafe { libc_geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_does_not_panic() {
        // On a dev box this is normally `None`; the point is that it returns.
        let m = ServiceManager::detect();
        assert!(!m.label().is_empty());
    }

    #[test]
    fn an_unmanaged_service_reports_unknown_rather_than_lying() {
        let svc = Service {
            manager: ServiceManager::None,
        };
        assert_eq!(svc.state(), RunState::Unknown);
        assert_eq!(svc.enabled(), None);
    }

    #[test]
    fn unmanaged_actions_explain_what_to_do() {
        let svc = Service {
            manager: ServiceManager::None,
        };
        let err = svc.start().unwrap_err().to_string();
        assert!(err.contains("安装脚本"), "got: {err}");
        assert!(svc.set_enabled(true).is_err());
        assert!(svc.logs(10).is_err());
    }

    #[test]
    fn a_missing_program_is_reported_not_silently_ignored() {
        let failure = run("definitely-not-a-real-program-9f2a", &[]).unwrap_err();
        assert!(failure.stderr.contains("无法执行"));
        assert!(failure.code.is_none());
    }

    #[test]
    fn failure_messages_prefer_stderr() {
        let f = Failure {
            stdout: "out".into(),
            stderr: "the actual problem".into(),
            code: Some(1),
        };
        let msg = f.into_error("systemctl start").to_string();
        assert!(msg.contains("systemctl start"));
        assert!(msg.contains("the actual problem"));
    }

    #[test]
    fn failure_falls_back_to_stdout_then_to_the_exit_code() {
        let only_stdout = Failure {
            stdout: "inactive".into(),
            stderr: String::new(),
            code: Some(3),
        };
        assert!(only_stdout.into_error("x").to_string().contains("inactive"));

        let neither = Failure {
            stdout: String::new(),
            stderr: String::new(),
            code: Some(3),
        };
        assert!(neither.into_error("x").to_string().contains("3"));
    }

    #[test]
    fn labels_are_present_for_every_state() {
        for s in [RunState::Running, RunState::Stopped, RunState::Unknown] {
            assert!(!s.label().is_empty());
        }
        for m in [
            ServiceManager::Systemd,
            ServiceManager::OpenRc,
            ServiceManager::None,
        ] {
            assert!(!m.label().is_empty());
        }
    }
}
