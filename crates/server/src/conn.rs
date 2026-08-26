//! One task per connection: handshake, then bidirectional pumping.

use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use synctus_core::proto::{self, Challenge, Frame, Welcome};
use synctus_core::PROTOCOL_VERSION;

use crate::config::ServerConfig;
use crate::hub::Hub;
use crate::limiter::RateLimiter;

/// Serve one client until it disconnects.
///
/// `peer` is only used for logging. The function is generic over the stream so
/// the same code serves plain TCP and TLS, and can be driven by an in-memory
/// duplex in tests.
pub async fn serve<S>(stream: S, peer: String, hub: Arc<Hub>, cfg: Arc<ServerConfig>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = writer;

    // A client that connects and says nothing must not hold a slot forever.
    let handshake = tokio::time::timeout(
        Duration::from_secs(cfg.handshake_timeout_secs.max(1)),
        handshake(&mut reader, &mut writer, &hub, &cfg),
    )
    .await;

    let session = match handshake {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            // Tell the client why before closing, so the UI can show it instead
            // of a bare disconnect.
            let _ = proto::write_frame(&mut writer, &Frame::error("auth", format!("{e:#}"))).await;
            tracing::debug!(%peer, error = %format!("{e:#}"), "握手失败");
            // Counted so `synctus` can show it: a climbing rejection count is
            // almost always a mismatched invite code, and that is the first thing
            // to check when pairing does not work.
            hub.note_rejected();
            return Ok(());
        }
        Err(_) => {
            let _ = proto::write_frame(&mut writer, &Frame::error("timeout", "握手超时")).await;
            tracing::debug!(%peer, "握手超时");
            hub.note_rejected();
            return Ok(());
        }
    };

    let room = session.room.clone();
    let device = session.device.clone();
    tracing::info!(%peer, room = %short(&room), device = %device, "设备已连接");

    let result = pump(reader, writer, session, hub.clone(), cfg).await;

    hub.leave(&room, &device).await;
    tracing::info!(%peer, room = %short(&room), device = %device, "设备已断开");
    result
}

struct Session {
    room: String,
    device: String,
    rx: mpsc::Receiver<Frame>,
}

async fn handshake<R, W>(
    reader: &mut R,
    writer: &mut W,
    hub: &Arc<Hub>,
    cfg: &ServerConfig,
) -> Result<Session>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let hello = match proto::read_frame(reader).await? {
        Some(Frame::Hello(h)) => h,
        Some(other) => bail!("首帧应为 Hello，收到 {other:?}"),
        None => bail!("客户端未发送 Hello"),
    };

    if hello.proto != PROTOCOL_VERSION {
        bail!(
            "协议版本不匹配：服务器 {PROTOCOL_VERSION}，客户端 {}",
            hello.proto
        );
    }
    // Room ids are hex-encoded 16-byte values; reject anything else before it
    // becomes a map key.
    if hello.room.len() != 32 || !hello.room.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("房间标识格式错误");
    }
    if hello.device_id.is_empty() || hello.device_id.len() > 64 {
        bail!("设备标识长度非法");
    }

    let challenge = hub.challenge_for(&hello.room).await?;
    proto::write_frame(
        writer,
        &Frame::Challenge(Challenge {
            nonce: proto::b64(&challenge),
        }),
    )
    .await?;

    let auth = match proto::read_frame(reader).await? {
        Some(Frame::Auth(a)) => a,
        Some(other) => bail!("应为 Auth 帧，收到 {other:?}"),
        None => bail!("客户端未完成认证"),
    };
    let mac = proto::unb64(&auth.mac)?;
    if mac.len() != 32 {
        bail!("认证响应长度错误");
    }

    let joined = hub.join(&hello.room, &hello.device_id, &mac).await?;

    proto::write_frame(
        writer,
        &Frame::Welcome(Welcome {
            server_time: synctus_core::now_ms(),
            peers: joined.peers,
            heartbeat_secs: cfg.heartbeat_secs,
        }),
    )
    .await?;

    // Replay retained state so the client does not have to wait for the peer's
    // next update.
    for relay in joined.retained {
        proto::write_frame(writer, &Frame::Relay(relay)).await?;
    }

    Ok(Session {
        room: hello.room,
        device: hello.device_id,
        rx: joined.rx,
    })
}

async fn pump<R, W>(
    mut reader: R,
    mut writer: W,
    mut session: Session,
    hub: Arc<Hub>,
    cfg: Arc<ServerConfig>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut limiter = RateLimiter::new(cfg.rate_limit_per_sec, cfg.rate_limit_burst);
    let idle = Duration::from_secs(cfg.idle_timeout_secs.max(5));

    loop {
        tokio::select! {
            // Outbound: frames the hub fanned out to this device.
            frame = session.rx.recv() => match frame {
                Some(f) => proto::write_frame(&mut writer, &f).await?,
                // The hub dropped us, most likely a reconnect under the same id.
                None => return Ok(()),
            },

            // Inbound, with an idle timeout. Clients heartbeat well inside it.
            incoming = tokio::time::timeout(idle, proto::read_frame(&mut reader)) => {
                match incoming {
                    Err(_) => {
                        tracing::debug!(device = %session.device, "空闲超时");
                        return Ok(());
                    }
                    Ok(Ok(None)) => return Ok(()),
                    Ok(Err(e)) => return Err(e).context("读取客户端帧失败"),
                    Ok(Ok(Some(frame))) => {
                        if !handle(frame, &mut writer, &session, &hub, &mut limiter).await? {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// Returns `false` when the connection should close.
async fn handle<W>(
    frame: Frame,
    writer: &mut W,
    session: &Session,
    hub: &Arc<Hub>,
    limiter: &mut RateLimiter,
) -> Result<bool>
where
    W: AsyncWrite + Unpin,
{
    match frame {
        Frame::Relay(relay) => {
            if !limiter.allow() {
                // Drop the frame and say so, but keep the connection: a burst is
                // usually a bug, not an attack, and killing the link would make
                // the user's status disappear.
                proto::write_frame(
                    writer,
                    &Frame::error("rate_limited", "发送过于频繁，已丢弃该消息"),
                )
                .await?;
                return Ok(true);
            }
            hub.relay(&session.room, &session.device, relay).await?;
            Ok(true)
        }
        Frame::Ping => {
            proto::write_frame(writer, &Frame::Pong).await?;
            Ok(true)
        }
        Frame::Pong => Ok(true),
        // Re-handshaking on a live connection is not allowed; the client should
        // open a new one.
        Frame::Hello(_) | Frame::Auth(_) => {
            proto::write_frame(
                writer,
                &Frame::error("protocol", "连接已认证，不能重复握手"),
            )
            .await?;
            Ok(false)
        }
        other => {
            tracing::debug!(device = %session.device, ?other, "忽略客户端发来的服务端帧");
            Ok(true)
        }
    }
}

/// First 8 hex chars of a room id, so logs identify a room without recording it
/// in full.
fn short(room: &str) -> &str {
    &room[..room.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use synctus_core::crypto::RoomKeys;

    /// Drive a real handshake over an in-memory duplex.
    async fn connect_client(
        hub: Arc<Hub>,
        cfg: Arc<ServerConfig>,
        keys: &RoomKeys,
        device: &str,
    ) -> Result<(tokio::io::DuplexStream, Welcome)> {
        let (client, server) = tokio::io::duplex(16 * 1024);
        tokio::spawn(serve(server, "test".into(), hub, cfg));

        let mut client = client;
        proto::write_frame(
            &mut client,
            &Frame::hello(keys.room_id_hex(), device.to_string()),
        )
        .await?;

        let Some(Frame::Challenge(c)) = proto::read_frame(&mut client).await? else {
            bail!("expected Challenge");
        };
        let nonce = proto::unb64(&c.nonce)?;
        proto::write_frame(
            &mut client,
            &Frame::Auth(proto::Auth {
                mac: proto::b64(&keys.auth_response(&nonce)),
            }),
        )
        .await?;

        match proto::read_frame(&mut client).await? {
            Some(Frame::Welcome(w)) => Ok((client, w)),
            Some(Frame::Error(e)) => bail!("rejected: {} {}", e.code, e.message),
            other => bail!("unexpected: {other:?}"),
        }
    }

    fn setup() -> (Arc<Hub>, Arc<ServerConfig>) {
        (Hub::new(8, 100), Arc::new(ServerConfig::default()))
    }

    // Argon2id is slow; one derivation per test is enough.
    fn keys(code: &str) -> RoomKeys {
        RoomKeys::derive(code).unwrap()
    }

    #[tokio::test]
    async fn full_handshake_succeeds() {
        let (hub, cfg) = setup();
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (_c, welcome) = connect_client(hub, cfg, &k, "dev-a").await.unwrap();
        assert!(welcome.peers.is_empty());
        assert!(welcome.server_time > 0);
    }

    #[tokio::test]
    async fn wrong_invite_code_is_rejected() {
        let (hub, cfg) = setup();
        let good = keys("ABCD-EFGH-IJKL-MNOP");
        let _first = connect_client(hub.clone(), cfg.clone(), &good, "dev-a")
            .await
            .unwrap();

        // Same room id is impossible with a different code, so forge the room id
        // while using the wrong auth key — exactly the attack the hub gates.
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        tokio::spawn(serve(server, "test".into(), hub, cfg));
        proto::write_frame(
            &mut client,
            &Frame::hello(good.room_id_hex(), "attacker".into()),
        )
        .await
        .unwrap();
        let Some(Frame::Challenge(c)) = proto::read_frame(&mut client).await.unwrap() else {
            panic!("expected Challenge");
        };
        let nonce = proto::unb64(&c.nonce).unwrap();
        let bad = keys("ZZZZ-YYYY-XXXX-WWWW");
        proto::write_frame(
            &mut client,
            &Frame::Auth(proto::Auth {
                mac: proto::b64(&bad.auth_response(&nonce)),
            }),
        )
        .await
        .unwrap();

        let reply = proto::read_frame(&mut client).await.unwrap();
        assert!(
            matches!(reply, Some(Frame::Error(ref e)) if e.code == "auth"),
            "got {reply:?}"
        );
    }

    #[tokio::test]
    async fn payload_is_forwarded_verbatim_to_the_peer() {
        let (hub, cfg) = setup();
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (mut a, _) = connect_client(hub.clone(), cfg.clone(), &k, "dev-a")
            .await
            .unwrap();
        let (mut b, welcome) = connect_client(hub, cfg, &k, "dev-b").await.unwrap();
        assert_eq!(welcome.peers, vec!["dev-a".to_string()]);

        // `a` is told about `b` joining.
        assert!(matches!(
            proto::read_frame(&mut a).await.unwrap(),
            Some(Frame::Presence(p)) if p.device_id == "dev-b" && p.online
        ));

        let sealed = k.seal(b"secret", b"dev-a").unwrap();
        proto::write_frame(
            &mut a,
            &Frame::Relay(proto::Relay {
                from: "dev-a".into(),
                body: proto::b64(&sealed),
                retain: Some("status".into()),
            }),
        )
        .await
        .unwrap();

        let Some(Frame::Relay(got)) = proto::read_frame(&mut b).await.unwrap() else {
            panic!("expected Relay");
        };
        assert_eq!(got.from, "dev-a");
        // The relay could not have altered the payload: it still opens.
        let plain = k.open(&proto::unb64(&got.body).unwrap(), b"dev-a").unwrap();
        assert_eq!(plain, b"secret");
    }

    #[tokio::test]
    async fn retained_status_is_replayed_to_a_late_joiner() {
        let (hub, cfg) = setup();
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (mut a, _) = connect_client(hub.clone(), cfg.clone(), &k, "dev-a")
            .await
            .unwrap();

        let sealed = k.seal(b"my-status", b"dev-a").unwrap();
        proto::write_frame(
            &mut a,
            &Frame::Relay(proto::Relay {
                from: "dev-a".into(),
                body: proto::b64(&sealed),
                retain: Some("status".into()),
            }),
        )
        .await
        .unwrap();

        // Give the server task a moment to record the retained frame.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut b, _) = connect_client(hub, cfg, &k, "dev-b").await.unwrap();
        let Some(Frame::Relay(got)) = proto::read_frame(&mut b).await.unwrap() else {
            panic!("expected replayed Relay");
        };
        assert_eq!(got.from, "dev-a");
    }

    #[tokio::test]
    async fn ping_is_answered_with_pong() {
        let (hub, cfg) = setup();
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (mut a, _) = connect_client(hub, cfg, &k, "dev-a").await.unwrap();
        proto::write_frame(&mut a, &Frame::Ping).await.unwrap();
        assert!(matches!(
            proto::read_frame(&mut a).await.unwrap(),
            Some(Frame::Pong)
        ));
    }

    #[tokio::test]
    async fn protocol_version_mismatch_is_reported() {
        let (hub, cfg) = setup();
        let (mut client, server) = tokio::io::duplex(4096);
        tokio::spawn(serve(server, "test".into(), hub, cfg));

        proto::write_frame(
            &mut client,
            &Frame::Hello(proto::Hello {
                proto: PROTOCOL_VERSION + 99,
                room: "0".repeat(32),
                device_id: "dev".into(),
                version: "test".into(),
            }),
        )
        .await
        .unwrap();

        assert!(matches!(
            proto::read_frame(&mut client).await.unwrap(),
            Some(Frame::Error(e)) if e.code == "auth"
        ));
    }

    #[tokio::test]
    async fn malformed_room_id_is_rejected() {
        let (hub, cfg) = setup();
        let (mut client, server) = tokio::io::duplex(4096);
        tokio::spawn(serve(server, "test".into(), hub, cfg));

        proto::write_frame(&mut client, &Frame::hello("not-hex".into(), "dev".into()))
            .await
            .unwrap();
        assert!(matches!(
            proto::read_frame(&mut client).await.unwrap(),
            Some(Frame::Error(_))
        ));
    }

    #[tokio::test]
    async fn handshake_timeout_closes_a_silent_connection() {
        let (hub, mut cfg) = setup();
        Arc::get_mut(&mut cfg).unwrap().handshake_timeout_secs = 1;
        let (mut client, server) = tokio::io::duplex(4096);
        tokio::spawn(serve(server, "test".into(), hub, cfg));

        // Say nothing and wait for the server to give up.
        let reply = tokio::time::timeout(Duration::from_secs(5), proto::read_frame(&mut client))
            .await
            .expect("server should respond");
        assert!(matches!(reply.unwrap(), Some(Frame::Error(e)) if e.code == "timeout"));
    }

    #[tokio::test]
    async fn excess_traffic_is_rate_limited_without_dropping_the_link() {
        let (hub, mut cfg) = setup();
        {
            let c = Arc::get_mut(&mut cfg).unwrap();
            c.rate_limit_per_sec = 1;
            c.rate_limit_burst = 2;
        }
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (mut a, _) = connect_client(hub, cfg, &k, "dev-a").await.unwrap();

        let sealed = proto::b64(&k.seal(b"x", b"dev-a").unwrap());
        for _ in 0..6 {
            proto::write_frame(
                &mut a,
                &Frame::Relay(proto::Relay {
                    from: "dev-a".into(),
                    body: sealed.clone(),
                    retain: None,
                }),
            )
            .await
            .unwrap();
        }

        // At least one frame must come back as a rate-limit notice.
        let frame = tokio::time::timeout(Duration::from_secs(3), proto::read_frame(&mut a))
            .await
            .expect("server should answer")
            .unwrap();
        assert!(
            matches!(frame, Some(Frame::Error(ref e)) if e.code == "rate_limited"),
            "got {frame:?}"
        );

        // Connection still usable.
        proto::write_frame(&mut a, &Frame::Ping).await.unwrap();
    }

    #[tokio::test]
    async fn repeat_handshake_closes_the_connection() {
        let (hub, cfg) = setup();
        let k = keys("ABCD-EFGH-IJKL-MNOP");
        let (mut a, _) = connect_client(hub, cfg, &k, "dev-a").await.unwrap();
        proto::write_frame(&mut a, &Frame::hello(k.room_id_hex(), "dev-a".into()))
            .await
            .unwrap();

        let Some(Frame::Error(e)) = proto::read_frame(&mut a).await.unwrap() else {
            panic!("expected protocol error");
        };
        assert_eq!(e.code, "protocol");
    }

    #[test]
    fn short_room_id_is_truncated_safely() {
        assert_eq!(short("0123456789abcdef"), "01234567");
        assert_eq!(short("abc"), "abc");
    }
}
