//! The Synctus relay daemon.
//!
//! Authenticates devices into a room derived from the users' shared invite code,
//! then forwards opaque, end-to-end encrypted frames between them. It cannot read
//! any status, to-do or nudge content.
//!
//! Normally managed through the `synctus` tool rather than run by hand:
//!
//! ```text
//! synctus-server --config /etc/synctus/server.toml
//! SYNCTUS_BIND=0.0.0.0:8787 synctus-server
//! ```

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

use synctus_server::admin;
use synctus_server::config::ServerConfig;
use synctus_server::conn;
use synctus_server::hub::Hub;

fn main() -> Result<()> {
    // Two worker threads: the relay is I/O bound and each connection does almost
    // no work, so this keeps memory low on a small VPS while leaving one thread
    // spare for a blocking DNS lookup.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("创建运行时失败")?;
    runtime.block_on(run())
}

async fn run() -> Result<()> {
    init_tracing();

    let args = Args::parse()?;
    if args.help {
        print_help();
        return Ok(());
    }

    let mut cfg = match &args.config {
        Some(path) => ServerConfig::load(path)?,
        None => ServerConfig::default(),
    };
    cfg.apply_env();

    // Refuse to start on a config that cannot work, rather than starting and
    // misbehaving in a way the operator has to diagnose from logs.
    let problems = cfg.problems();
    if !problems.is_empty() {
        for p in &problems {
            tracing::error!("配置问题: {p}");
        }
        anyhow::bail!("配置有 {} 处问题，已拒绝启动", problems.len());
    }

    let cfg = Arc::new(cfg);
    let hub = Hub::new(
        cfg.max_devices_per_room,
        cfg.max_rooms,
        cfg.bind.clone(),
        cfg.tls_enabled(),
    );

    let listener = TcpListener::bind(&cfg.bind)
        .await
        .with_context(|| format!("监听失败: {}", cfg.bind))?;

    let tls = if cfg.tls_enabled() {
        Some(load_tls(&cfg)?)
    } else {
        tracing::warn!(
            "未配置 TLS 证书，客户端与服务器之间为明文传输。\
             消息内容仍是端到端加密的，但房间与设备标识会暴露给网络中间人。\
             生产环境请配置 cert_path/key_path，或在反向代理上终止 TLS。"
        );
        None
    };

    tracing::info!(
        bind = %cfg.bind,
        tls = tls.is_some(),
        version = synctus_server::version(),
        "Synctus 中继服务器已启动"
    );

    // The admin socket, so `synctus` can report live state. Failure is not fatal:
    // the relay's job is to relay, and a missing socket only degrades the status
    // display.
    #[cfg(unix)]
    {
        let socket = args
            .admin_socket
            .clone()
            .unwrap_or_else(|| PathBuf::from(admin::DEFAULT_SOCKET));
        match spawn_admin(socket.clone(), hub.clone(), cfg.clone()).await {
            Ok(()) => tracing::info!(socket = %socket.display(), "管理套接字已就绪"),
            Err(e) => tracing::warn!(
                socket = %socket.display(),
                error = %format!("{e:#}"),
                "管理套接字创建失败，`synctus` 将无法显示实时状态"
            ),
        }
    }

    // The WebUI. Off by default; the config file decides whether it exists at
    // all, so this only runs when the operator asked for it.
    if let (Some(bind), Some(password)) = (&cfg.web_bind, &cfg.web_password) {
        match synctus_server::web::spawn(bind, password, hub.clone()) {
            Ok(()) => {
                tracing::info!("Web 管理面板已启用: http://{bind}（用户名任意，密码见配置）");
            }
            Err(e) => tracing::error!(error = %format!("{e:#}"), "WebUI 启动失败"),
        }
    }

    // Periodic stats, at a cadence that is useful without spamming the journal.
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.tick().await;
            loop {
                tick.tick().await;
                let s = hub.stats().await;
                tracing::info!(
                    rooms = s.rooms,
                    devices = s.devices,
                    accepted = s.accepted,
                    rejected = s.rejected,
                    "当前状态"
                );
            }
        });
    }

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, addr) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        // Transient accept errors (fd exhaustion) must not kill
                        // the server.
                        tracing::warn!(error = %e, "接受连接失败");
                        continue;
                    }
                };
                stream.set_nodelay(true).ok();

                let hub = hub.clone();
                let cfg = cfg.clone();
                let tls = tls.clone();
                let peer = addr.to_string();

                tokio::spawn(async move {
                    let result = match tls {
                        Some(acceptor) => match acceptor.accept(stream).await {
                            Ok(tls_stream) => conn::serve(tls_stream, peer.clone(), hub, cfg).await,
                            Err(e) => {
                                tracing::debug!(%peer, error = %e, "TLS 握手失败");
                                return;
                            }
                        },
                        None => conn::serve(stream, peer.clone(), hub, cfg).await,
                    };
                    if let Err(e) = result {
                        tracing::debug!(%peer, error = %format!("{e:#}"), "连接异常结束");
                    }
                });
            }

            _ = &mut shutdown => {
                tracing::info!("收到退出信号，正在关闭");
                return Ok(());
            }
        }
    }
}

/// Serve the admin socket in the background.
///
/// One connection at a time, one line per request: the only client is a local
/// management tool asking for a status page, so concurrency here would be
/// complexity without a purpose.
#[cfg(unix)]
async fn spawn_admin(path: PathBuf, hub: Arc<Hub>, cfg: Arc<ServerConfig>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("创建运行目录失败: {}", dir.display()))?;
    }
    // A leftover socket from a killed process would make bind fail.
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("绑定管理套接字失败: {}", path.display()))?;

    // 0660: the service user and its group. Filesystem permissions are the whole
    // authorisation model, so this is the security boundary — world-writable here
    // would let any local user read who is connected.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))
        .with_context(|| format!("设置套接字权限失败: {}", path.display()))?;

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "管理套接字接受失败");
                    continue;
                }
            };

            let hub = hub.clone();
            let cfg = cfg.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let response = match serde_json::from_str::<admin::Request>(&line) {
                        Ok(admin::Request::Status) => {
                            let s = hub.stats().await;
                            admin::Response::Status(admin::Status {
                                version: synctus_server::version().to_string(),
                                uptime_secs: hub.uptime_secs(),
                                bind: cfg.bind.clone(),
                                tls: cfg.tls_enabled(),
                                rooms: s.rooms,
                                devices: s.devices,
                                accepted: s.accepted,
                                rejected: s.rejected,
                            })
                        }
                        Ok(admin::Request::Rooms) => admin::Response::Rooms {
                            rooms: hub.room_info().await,
                        },
                        Ok(admin::Request::Snapshot) => {
                            admin::Response::Snapshot(hub.snapshot().await)
                        }
                        Err(e) => admin::Response::Error {
                            message: format!("无法解析请求: {e}"),
                        },
                    };

                    let mut body = match serde_json::to_vec(&response) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::debug!(error = %e, "序列化管理响应失败");
                            return;
                        }
                    };
                    body.push(b'\n');
                    if write.write_all(&body).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SYNCTUS_LOG")
        .unwrap_or_else(|_| EnvFilter::new("synctus_server=info,warn"));
    fmt().with_env_filter(filter).with_target(false).init();
}

/// Build a TLS acceptor from the configured PEM files.
fn load_tls(cfg: &ServerConfig) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::ServerConfig as RustlsServerConfig;

    let cert_path = cfg.cert_path.as_ref().expect("checked by tls_enabled");
    let key_path = cfg.key_path.as_ref().expect("checked by tls_enabled");

    let cert_file =
        std::fs::File::open(cert_path).with_context(|| format!("打开证书失败: {cert_path}"))?;
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("解析证书失败: {cert_path}"))?;
    if certs.is_empty() {
        anyhow::bail!("证书文件中没有证书: {cert_path}");
    }

    let key_file =
        std::fs::File::open(key_path).with_context(|| format!("打开私钥失败: {key_path}"))?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))
        .with_context(|| format!("解析私钥失败: {key_path}"))?
        .with_context(|| format!("私钥文件中没有私钥: {key_path}"))?;

    let server_config = RustlsServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("证书与私钥不匹配")?;

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(server_config)))
}

/// Resolve on Ctrl-C or, on Unix, SIGTERM — the signal a container runtime or
/// systemd sends.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct Args {
    config: Option<PathBuf>,
    /// Overrides the admin socket path. Only meaningful on Unix, where the socket
    /// exists at all.
    #[cfg_attr(not(unix), allow(dead_code))]
    admin_socket: Option<PathBuf>,
    help: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut config = None;
        let mut admin_socket = None;
        let mut help = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-c" | "--config" => {
                    config = Some(PathBuf::from(args.next().context("--config 缺少路径参数")?));
                }
                "--admin-socket" => {
                    admin_socket = Some(PathBuf::from(
                        args.next().context("--admin-socket 缺少路径参数")?,
                    ));
                }
                "-h" | "--help" => help = true,
                "-V" | "--version" => {
                    println!("synctus-server {}", synctus_server::version());
                    std::process::exit(0);
                }
                other => anyhow::bail!("未知参数: {other}"),
            }
        }
        Ok(Self {
            config,
            admin_socket,
            help,
        })
    }
}

fn print_help() {
    println!(
        "Synctus 中继服务器 {}

用法:
  synctus-server [选项]

选项:
  -c, --config <路径>      TOML 配置文件
      --admin-socket <路径> 管理套接字，默认 {}
  -h, --help               显示本帮助
  -V, --version            显示版本

环境变量（覆盖配置文件）:
  SYNCTUS_BIND          监听地址，默认 0.0.0.0:8787
  SYNCTUS_CERT          TLS 证书链 PEM
  SYNCTUS_KEY           TLS 私钥 PEM
  SYNCTUS_MAX_ROOMS     最大房间数
  SYNCTUS_RATE_LIMIT    每设备每秒转发上限
  SYNCTUS_IDLE_TIMEOUT  空闲超时秒数
  SYNCTUS_LOG           日志过滤，如 synctus_server=debug

日常管理请直接运行 `synctus`。
服务器只转发密文，无法读取任何状态内容。",
        synctus_server::version(),
        admin::DEFAULT_SOCKET,
    );
}
