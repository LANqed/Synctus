//! Synctus relay server.
//!
//! A small C/S relay: it authenticates devices into a room derived from the
//! users' shared invite code, then forwards opaque, end-to-end encrypted frames
//! between them. It cannot read any status, to-do or nudge content.
//!
//! Run it on any VPS:
//!
//! ```text
//! synctus-server --config server.toml
//! SYNCTUS_BIND=0.0.0.0:8787 synctus-server
//! ```

mod config;
mod conn;
mod hub;
mod limiter;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

use config::ServerConfig;
use hub::Hub;

fn main() -> Result<()> {
    // A single-threaded runtime is enough: the relay is I/O bound and each
    // connection does almost no work. It also keeps memory low on a small VPS.
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
    let cfg = Arc::new(cfg);

    let hub = Hub::new(cfg.max_devices_per_room, cfg.max_rooms);

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
        version = env!("CARGO_PKG_VERSION"),
        "Synctus 中继服务器已启动"
    );

    // Periodic stats, at a cadence that is useful without spamming the journal.
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.tick().await;
            loop {
                tick.tick().await;
                let s = hub.stats().await;
                tracing::info!(rooms = s.rooms, devices = s.devices, "当前状态");
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
    help: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut config = None;
        let mut help = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-c" | "--config" => {
                    config = Some(PathBuf::from(args.next().context("--config 缺少路径参数")?));
                }
                "-h" | "--help" => help = true,
                "-V" | "--version" => {
                    println!("synctus-server {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                other => anyhow::bail!("未知参数: {other}"),
            }
        }
        Ok(Self { config, help })
    }
}

fn print_help() {
    println!(
        "Synctus 中继服务器 {}

用法:
  synctus-server [选项]

选项:
  -c, --config <路径>   TOML 配置文件
  -h, --help            显示本帮助
  -V, --version         显示版本

环境变量（覆盖配置文件）:
  SYNCTUS_BIND          监听地址，默认 0.0.0.0:8787
  SYNCTUS_CERT          TLS 证书链 PEM
  SYNCTUS_KEY           TLS 私钥 PEM
  SYNCTUS_MAX_ROOMS     最大房间数
  SYNCTUS_RATE_LIMIT    每设备每秒转发上限
  SYNCTUS_IDLE_TIMEOUT  空闲超时秒数
  SYNCTUS_LOG           日志过滤，如 synctus_server=debug

服务器只转发密文，无法读取任何状态内容。",
        env!("CARGO_PKG_VERSION")
    );
}
