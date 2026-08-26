//! `synctus` — the management tool.
//!
//! Typing `synctus` with no arguments opens a numbered menu. That is the whole
//! interface: read the numbers, type one, press enter. No flags to remember, no
//! subcommand tree to explore, and it works over a bare SSH session with no
//! terminal capabilities beyond printing text.
//!
//! Deliberately *not* a full-screen TUI. A relay server is administered over SSH
//! on a connection that may be slow; a full-screen app that redraws on every
//! keypress is worse there than plain lines, and it makes the output impossible to
//! copy into a bug report. Non-interactive subcommands exist too, so the same tool
//! can be used from a script.

use anyhow::{Context, Result};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use synctus_server::admin;
use synctus_server::config::ServerConfig;
use synctus_server::service::{RunState, Service};

/// Where the installer puts the config.
const DEFAULT_CONFIG: &str = "/etc/synctus/server.toml";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        // No arguments is the intended path: open the menu.
        None => {
            if std::io::stdin().is_terminal() {
                menu()
            } else {
                // Piped input with no subcommand would hang waiting for a choice.
                print_help();
                Ok(())
            }
        }
        Some("status") => print_status(),
        Some("start") => act("启动", |s| s.start()),
        Some("stop") => act("停止", |s| s.stop()),
        Some("restart") => act("重启", |s| s.restart()),
        Some("logs") => print_logs(80),
        Some("config") => print_config(),
        Some("check") => check_config(),
        Some("-h") | Some("--help") | Some("help") => {
            print_help();
            Ok(())
        }
        Some("-V") | Some("--version") => {
            println!("synctus {}", synctus_server::version());
            Ok(())
        }
        Some(other) => {
            eprintln!("未知命令: {other}\n");
            print_help();
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------- the menu

fn menu() -> Result<()> {
    loop {
        let service = Service::detect();
        let state = service.state();

        print_header(&service, state);
        println!();
        println!("  1) 查看状态与在线设备");
        println!("  2) 查看日志");
        println!(
            "  3) {}",
            if state == RunState::Running {
                "停止服务"
            } else {
                "启动服务"
            }
        );
        println!("  4) 重启服务");
        println!(
            "  5) {}开机自启",
            match service.enabled() {
                Some(true) => "关闭",
                _ => "开启",
            }
        );
        println!("  6) 修改配置");
        println!("  7) 检查配置");
        println!("  8) 显示客户端连接信息");
        println!("  0) 退出");
        println!();

        let choice = prompt("请输入选项数字: ")?;

        // A blank line is a common accident; re-showing the menu is friendlier
        // than an error.
        if choice.is_empty() {
            continue;
        }

        println!();
        let result = match choice.as_str() {
            "1" => print_status(),
            "2" => print_logs(40),
            "3" => {
                if state == RunState::Running {
                    act("停止", |s| s.stop())
                } else {
                    act("启动", |s| s.start())
                }
            }
            "4" => act("重启", |s| s.restart()),
            "5" => toggle_autostart(&service),
            "6" => edit_config(),
            "7" => check_config(),
            "8" => print_client_info(),
            "0" | "q" | "quit" | "exit" => return Ok(()),
            other => {
                println!("没有选项 {other}。");
                Ok(())
            }
        };

        // A failed action must not end the session: the user is mid-task and
        // probably wants to read the log or fix the config next.
        if let Err(e) = result {
            println!("\x1b[31m失败：{e:#}\x1b[0m");
            if !synctus_server::service::is_root() {
                println!("提示：管理服务需要 root 权限，试试 `sudo synctus`。");
            }
        }

        println!();
        prompt("按回车返回菜单…")?;
        println!();
    }
}

fn print_header(service: &Service, state: RunState) {
    let (colour, mark) = match state {
        RunState::Running => ("\x1b[32m", "●"),
        RunState::Stopped => ("\x1b[31m", "○"),
        RunState::Unknown => ("\x1b[33m", "?"),
    };

    println!(
        "\x1b[1mSynctus 中继服务器\x1b[0m  {}",
        synctus_server::version()
    );
    println!(
        "状态 {colour}{mark} {}\x1b[0m　管理方式 {}　开机自启 {}",
        state.label(),
        service.manager().label(),
        match service.enabled() {
            Some(true) => "已开启",
            Some(false) => "已关闭",
            None => "未知",
        }
    );

    // Live numbers when the daemon is reachable. Silent when it is not: the state
    // line above already said the service is down.
    if let Ok(status) = query_status() {
        println!(
            "运行 {}　房间 {}　设备 {}　监听 {}{}",
            status.uptime_text(),
            status.rooms,
            status.devices,
            status.bind,
            if status.tls {
                "　TLS 已启用"
            } else {
                "　\x1b[33m未启用 TLS\x1b[0m"
            }
        );
    }
}

// ------------------------------------------------------------------ actions

fn act(what: &str, f: impl Fn(&Service) -> Result<()>) -> Result<()> {
    let service = Service::detect();
    f(&service).with_context(|| format!("{what}服务失败"))?;
    println!("已{what}。");

    // Report what actually happened rather than assuming the command worked:
    // systemd returns success for `start` on a unit that then immediately dies.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let state = Service::detect().state();
    println!("当前状态：{}", state.label());
    if state == RunState::Stopped && what != "停止" {
        println!("服务没有保持运行，选「查看日志」看原因。");
    }
    Ok(())
}

fn toggle_autostart(service: &Service) -> Result<()> {
    let enable = !matches!(service.enabled(), Some(true));
    service.set_enabled(enable)?;
    println!("已{}开机自启。", if enable { "开启" } else { "关闭" });
    Ok(())
}

fn print_status() -> Result<()> {
    let service = Service::detect();
    let state = service.state();
    println!(
        "服务状态：{}（{}）",
        state.label(),
        service.manager().label()
    );

    match query_status() {
        Ok(status) => {
            println!();
            println!("  版本        {}", status.version);
            println!("  已运行      {}", status.uptime_text());
            println!("  监听        {}", status.bind);
            println!(
                "  TLS         {}",
                if status.tls {
                    "已启用".to_string()
                } else {
                    "\x1b[33m未启用（房间与设备标识以明文传输）\x1b[0m".to_string()
                }
            );
            println!("  房间 / 设备 {} / {}", status.rooms, status.devices);
            println!("  已接受连接  {}", status.accepted);
            println!(
                "  已拒绝连接  {}{}",
                status.rejected,
                if status.rejected > 0 {
                    "　（多为配对码不一致）"
                } else {
                    ""
                }
            );

            // The WebUI address lives in the config file, not the daemon.
            let cfg = ServerConfig::load(&config_path()).unwrap_or_default();
            if let Some(bind) = &cfg.web_bind {
                println!("  Web 面板    http://{bind}");
            }

            match query_rooms() {
                Ok(rooms) if !rooms.is_empty() => {
                    println!();
                    println!("在线设备：");
                    for room in rooms {
                        println!(
                            "  房间 {}　{} 台设备　{} 秒前活动",
                            room.room,
                            room.devices.len(),
                            room.idle_secs
                        );
                        for d in room.devices {
                            let owner = if d.user.is_empty() {
                                "未分组".to_string()
                            } else {
                                d.user.clone()
                            };
                            let name = if d.name.is_empty() {
                                d.id.clone()
                            } else {
                                d.name
                            };
                            println!("    - [{owner}] {name}　已连接 {} 秒", d.connected_secs);
                        }
                    }
                }
                Ok(_) => {
                    println!();
                    println!("目前没有设备连接。");
                }
                Err(e) => println!("\n无法读取房间列表：{e:#}"),
            }
        }
        Err(e) => {
            if state == RunState::Running {
                // Running but unreachable is worth explaining: it usually means the
                // socket path or its permissions are wrong.
                println!("\n服务在运行，但无法读取实时状态：{e:#}");
                println!("检查 {} 是否存在且当前用户可访问。", admin::DEFAULT_SOCKET);
            } else {
                println!("\n服务未运行，没有实时状态可显示。");
            }
        }
    }

    Ok(())
}

fn print_logs(lines: usize) -> Result<()> {
    let service = Service::detect();
    let logs = service.logs(lines)?;
    if logs.trim().is_empty() {
        println!("暂无日志。");
    } else {
        println!("{logs}");
    }
    Ok(())
}

fn print_config() -> Result<()> {
    let path = config_path();
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("读取配置失败: {}", path.display()))?;
    println!("配置文件：{}\n", path.display());
    println!("{text}");
    Ok(())
}

fn check_config() -> Result<()> {
    let path = config_path();
    let cfg = ServerConfig::load(&path)?;
    let problems = cfg.problems();

    println!("配置文件：{}", path.display());
    if problems.is_empty() {
        println!("\x1b[32m配置没有问题。\x1b[0m");
    } else {
        println!("\x1b[33m发现 {} 处问题：\x1b[0m", problems.len());
        for p in &problems {
            println!("  - {p}");
        }
    }
    Ok(())
}

/// Edit the settings people actually change, one prompt at a time.
///
/// Not an editor: the whole point of this tool is that someone who does not want
/// to learn TOML can still change the port. Enter keeps the current value, so
/// pressing enter through the whole thing is a no-op.
fn edit_config() -> Result<()> {
    let path = config_path();
    let mut cfg = ServerConfig::load(&path)?;

    println!("直接回车保留当前值。\n");

    if let Some(v) = ask("监听地址", &cfg.bind)? {
        cfg.bind = v;
    }

    let cert = cfg.cert_path.clone().unwrap_or_default();
    if let Some(v) = ask("TLS 证书路径（留空表示不启用）", &cert)? {
        cfg.cert_path = Some(v).filter(|s| !s.is_empty());
    }

    let key = cfg.key_path.clone().unwrap_or_default();
    if let Some(v) = ask("TLS 私钥路径", &key)? {
        cfg.key_path = Some(v).filter(|s| !s.is_empty());
    }

    if let Some(v) = ask_num("每房间最大设备数", cfg.max_devices_per_room as u64)? {
        cfg.max_devices_per_room = v as usize;
    }
    if let Some(v) = ask_num("最大房间数", cfg.max_rooms as u64)? {
        cfg.max_rooms = v as usize;
    }
    if let Some(v) = ask_num("空闲超时（秒）", cfg.idle_timeout_secs)? {
        cfg.idle_timeout_secs = v;
    }
    if let Some(v) = ask_num("每设备每秒转发上限", cfg.rate_limit_per_sec as u64)? {
        cfg.rate_limit_per_sec = v as u32;
    }

    // The WebUI: a browser dashboard showing users/devices and a disconnect
    // button. Entering an address turns it on; clearing it turns it off.
    println!("\nWebUI 管理面板（回车跳过则保持不变）：");
    let web_bind = cfg.web_bind.clone().unwrap_or_default();
    if let Some(v) = ask("监听地址（例如 127.0.0.1:9090，留空关闭）", &web_bind)? {
        cfg.web_bind = Some(v).filter(|s| !s.is_empty());
    }
    if cfg.web_bind.is_some() {
        let web_password = cfg.web_password.clone().unwrap_or_default();
        if let Some(v) = ask("管理员密码", &web_password)? {
            cfg.web_password = Some(v).filter(|s| !s.is_empty());
        }
    } else {
        cfg.web_password = None;
    }

    // Validate before writing: saving a config the daemon will refuse turns a
    // typo into a service that does not come back up.
    let problems = cfg.problems();
    if !problems.is_empty() {
        println!("\n\x1b[33m这些问题会导致服务无法启动：\x1b[0m");
        for p in &problems {
            println!("  - {p}");
        }
        if prompt("仍要保存吗？(y/N) ")?.to_lowercase() != "y" {
            println!("已放弃修改。");
            return Ok(());
        }
    }

    cfg.save(&path)?;
    println!("\n已保存到 {}。", path.display());

    if Service::detect().state() == RunState::Running
        && prompt("现在重启服务使其生效吗？(Y/n) ")?.to_lowercase() != "n"
    {
        act("重启", |s| s.restart())?;
    }
    Ok(())
}

/// What the two users need to type into their clients.
fn print_client_info() -> Result<()> {
    let path = config_path();
    let cfg = ServerConfig::load(&path).unwrap_or_default();

    let port = cfg.bind.rsplit(':').next().unwrap_or("8787").to_string();

    println!("在三端的「设置 → 服务器」里填入：\n");
    println!("  地址    <这台机器的域名或公网 IP>:{port}");
    println!(
        "  TLS     {}",
        if cfg.tls_enabled() {
            "开启"
        } else {
            "关闭（仅限内网测试）"
        }
    );
    println!();
    println!("两个人必须填入\x1b[1m同一个配对码\x1b[0m，在任一端点「生成」得到。");
    println!("配对码就是密钥：服务器只转发密文，读不到任何状态内容。");

    if !cfg.tls_enabled() {
        println!();
        println!("\x1b[33m当前未启用 TLS。\x1b[0m消息内容仍是端到端加密的，");
        println!("但房间与设备标识会以明文经过网络。公网部署建议配置证书。");
    }

    if cfg.bind.starts_with("127.") {
        println!();
        println!(
            "\x1b[33m注意：\x1b[0m监听地址是 {}，只有本机能连接。",
            cfg.bind
        );
        println!("要让其他设备连入，改成 0.0.0.0:{port}。");
    }

    Ok(())
}

// ------------------------------------------------------------------ plumbing

fn config_path() -> PathBuf {
    // An override keeps the tool usable for a non-root install or a test setup.
    resolve_config_path(std::env::var("SYNCTUS_CONFIG").ok().as_deref())
}

/// The override rule, as a pure function.
///
/// Kept separate from the environment so the tests do not have to mutate a
/// process-global variable — four tests doing that in parallel is how a test
/// starts reading another test's value.
fn resolve_config_path(override_value: Option<&str>) -> PathBuf {
    if let Some(p) = override_value.map(str::trim).filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    PathBuf::from(DEFAULT_CONFIG)
}

/// Path of the admin socket.
///
/// Also read on non-Unix builds so the override behaviour, and its test, stay the
/// same everywhere even though connecting is Unix-only.
fn socket_path() -> PathBuf {
    resolve_socket_path(std::env::var("SYNCTUS_ADMIN_SOCKET").ok().as_deref())
}

fn resolve_socket_path(override_value: Option<&str>) -> PathBuf {
    if let Some(p) = override_value.map(str::trim).filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    PathBuf::from(admin::DEFAULT_SOCKET)
}

fn query_status() -> Result<admin::Status> {
    match ask_daemon(admin::Request::Status)? {
        admin::Response::Status(s) => Ok(s),
        admin::Response::Error { message } => anyhow::bail!("{message}"),
        other => anyhow::bail!("服务器返回了意外的响应: {other:?}"),
    }
}

fn query_rooms() -> Result<Vec<admin::RoomInfo>> {
    match ask_daemon(admin::Request::Rooms)? {
        admin::Response::Rooms { rooms } => Ok(rooms),
        admin::Response::Error { message } => anyhow::bail!("{message}"),
        other => anyhow::bail!("服务器返回了意外的响应: {other:?}"),
    }
}

/// One request, one response, over the admin socket.
#[cfg(unix)]
fn ask_daemon(request: admin::Request) -> Result<admin::Response> {
    let path = socket_path();
    ask_daemon_at(request, &path)
}

/// The actual socket exchange, with the path passed in.
///
/// Split out so the error path is testable without touching a process-global
/// environment variable.
#[cfg(unix)]
fn ask_daemon_at(request: admin::Request, path: &std::path::Path) -> Result<admin::Response> {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(path)
        .with_context(|| format!("连接管理套接字失败: {}", path.display()))?;
    // A hung daemon must not hang the tool.
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .ok();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(3)))
        .ok();

    let mut writer = &stream;
    let mut line = serde_json::to_vec(&request)?;
    line.push(b'\n');
    writer.write_all(&line).context("发送请求失败")?;
    writer.flush().ok();

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response).context("读取响应失败")?;
    if response.trim().is_empty() {
        anyhow::bail!("服务器没有响应");
    }
    serde_json::from_str(&response).context("解析响应失败")
}

#[cfg(not(unix))]
fn ask_daemon(_request: admin::Request) -> Result<admin::Response> {
    // Referenced so the override logic is still exercised on this platform.
    let _ = socket_path();
    anyhow::bail!("管理套接字仅在 Unix 系统上可用")
}

/// Read a line from the user.
fn prompt(text: &str) -> Result<String> {
    print!("{text}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let read = std::io::stdin()
        .read_line(&mut line)
        .context("读取输入失败")?;
    if read == 0 {
        // Ctrl-D: treat as quit rather than looping forever on empty input.
        println!();
        std::process::exit(0);
    }
    Ok(line.trim().to_string())
}

/// Ask for a value, showing the current one. `None` means "keep it".
fn ask(label: &str, current: &str) -> Result<Option<String>> {
    let shown = if current.is_empty() {
        "（空）"
    } else {
        current
    };
    let answer = prompt(&format!("{label} [{shown}]: "))?;
    Ok(if answer.is_empty() {
        None
    } else {
        Some(answer)
    })
}

/// Numeric variant that re-asks rather than silently keeping the old value when
/// the input is not a number.
fn ask_num(label: &str, current: u64) -> Result<Option<u64>> {
    loop {
        let answer = prompt(&format!("{label} [{current}]: "))?;
        if answer.is_empty() {
            return Ok(None);
        }
        match answer.parse::<u64>() {
            Ok(v) => return Ok(Some(v)),
            Err(_) => println!("请输入一个数字。"),
        }
    }
}

fn print_help() {
    println!(
        "Synctus 中继服务器管理工具 {}

用法:
  synctus              打开管理菜单（推荐）
  synctus <命令>       直接执行，便于脚本调用

命令:
  status    显示状态与在线设备
  start     启动服务
  stop      停止服务
  restart   重启服务
  logs      显示最近日志
  config    显示配置文件
  check     检查配置是否有问题

环境变量:
  SYNCTUS_CONFIG         配置文件路径，默认 {}
  SYNCTUS_ADMIN_SOCKET   管理套接字路径，默认 {}

管理服务需要 root 权限，通常写作 `sudo synctus`。",
        synctus_server::version(),
        DEFAULT_CONFIG,
        admin::DEFAULT_SOCKET,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_honours_the_override() {
        assert_eq!(
            resolve_config_path(Some("/tmp/synctus-test.toml")),
            PathBuf::from("/tmp/synctus-test.toml")
        );
        assert_eq!(resolve_config_path(None), PathBuf::from(DEFAULT_CONFIG));
    }

    #[test]
    fn a_blank_override_falls_back_to_the_default() {
        // An exported-but-empty variable is a common shell accident.
        for blank in [Some(""), Some("   ")] {
            assert_eq!(resolve_config_path(blank), PathBuf::from(DEFAULT_CONFIG));
        }
    }

    #[test]
    fn socket_path_honours_the_override() {
        assert_eq!(
            resolve_socket_path(Some("/tmp/s.sock")),
            PathBuf::from("/tmp/s.sock")
        );
        assert_eq!(
            resolve_socket_path(None),
            PathBuf::from(admin::DEFAULT_SOCKET)
        );
    }

    #[test]
    fn asking_the_daemon_without_one_running_is_an_error_not_a_hang() {
        #[cfg(unix)]
        {
            // No env mutation: the path is passed straight in, so this test
            // cannot be affected by (or affect) the others running in parallel.
            assert!(ask_daemon_at(
                admin::Request::Status,
                &PathBuf::from("/tmp/synctus-nonexistent-9f2a.sock")
            )
            .is_err());
        }
    }
}
