//! Wire protocol between a client and the relay server.
//!
//! Frames are length-prefixed and, for anything carrying user data, the body is
//! an opaque AEAD box the relay cannot read:
//!
//! ```text
//! ┌────────────┬──────────┬──────────────────────┐
//! │ len: u32be │ kind: u8 │ body: len-1 bytes    │
//! └────────────┴──────────┴──────────────────────┘
//! ```
//!
//! Handshake:
//!
//! ```text
//! client ──▶ Hello    { proto, room_id, device_id }
//! server ──▶ Challenge{ nonce }
//! client ──▶ Auth     { hmac(auth_key, nonce) }
//! server ──▶ Welcome  { server_time, peers }        (or Error)
//!   …then Relay / Presence / Ping frames in both directions
//! ```
//!
//! The relay only learns the room id, device ids and message sizes. `Relay`
//! bodies are end-to-end encrypted with the room's message key.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{MAX_FRAME_LEN, PROTOCOL_VERSION};

/// Frame discriminants. Explicit values because they are on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Hello = 1,
    Challenge = 2,
    Auth = 3,
    Welcome = 4,
    Error = 5,
    /// Encrypted peer payload, forwarded verbatim to the other devices.
    Relay = 6,
    /// A peer connected or disconnected.
    Presence = 7,
    Ping = 8,
    Pong = 9,
}

impl FrameKind {
    fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            1 => FrameKind::Hello,
            2 => FrameKind::Challenge,
            3 => FrameKind::Auth,
            4 => FrameKind::Welcome,
            5 => FrameKind::Error,
            6 => FrameKind::Relay,
            7 => FrameKind::Presence,
            8 => FrameKind::Ping,
            9 => FrameKind::Pong,
            other => bail!("未知帧类型: {other}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub proto: u16,
    /// Hex room id from [`crate::crypto::RoomKeys::room_id_hex`].
    pub room: String,
    /// Stable per-installation id, so the same person on two devices shows up
    /// twice.
    pub device_id: String,
    /// Client build, for diagnostics only.
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Challenge {
    /// Base64 (standard, padded) 32-byte nonce.
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    /// Base64 HMAC over the challenge.
    pub mac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    /// Relay clock in unix ms, so clients can warn about large skew.
    pub server_time: i64,
    /// Device ids already in the room.
    #[serde(default)]
    pub peers: Vec<String>,
    /// How often the relay expects a Ping, in seconds.
    pub heartbeat_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorFrame {
    pub code: String,
    pub message: String,
}

/// Encrypted peer payload. Only `body` is confidential; the rest is routing
/// metadata the relay needs and is bound into the AEAD `aad`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relay {
    /// Sender device id. Filled by the sender and re-checked by the relay.
    pub from: String,
    /// Base64 `nonce || ciphertext`.
    pub body: String,
    /// Retain slot name when the relay should remember this as latest state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub device_id: String,
    pub online: bool,
    #[serde(default)]
    pub platform_hint: Option<String>,
}

/// A decoded frame.
#[derive(Debug, Clone)]
pub enum Frame {
    Hello(Hello),
    Challenge(Challenge),
    Auth(Auth),
    Welcome(Welcome),
    Error(ErrorFrame),
    Relay(Relay),
    Presence(Presence),
    Ping,
    Pong,
}

impl Frame {
    fn kind(&self) -> FrameKind {
        match self {
            Frame::Hello(_) => FrameKind::Hello,
            Frame::Challenge(_) => FrameKind::Challenge,
            Frame::Auth(_) => FrameKind::Auth,
            Frame::Welcome(_) => FrameKind::Welcome,
            Frame::Error(_) => FrameKind::Error,
            Frame::Relay(_) => FrameKind::Relay,
            Frame::Presence(_) => FrameKind::Presence,
            Frame::Ping => FrameKind::Ping,
            Frame::Pong => FrameKind::Pong,
        }
    }

    /// Serialise the body. Ping/Pong carry none.
    fn body(&self) -> Result<Vec<u8>> {
        Ok(match self {
            Frame::Hello(v) => serde_json::to_vec(v)?,
            Frame::Challenge(v) => serde_json::to_vec(v)?,
            Frame::Auth(v) => serde_json::to_vec(v)?,
            Frame::Welcome(v) => serde_json::to_vec(v)?,
            Frame::Error(v) => serde_json::to_vec(v)?,
            Frame::Relay(v) => serde_json::to_vec(v)?,
            Frame::Presence(v) => serde_json::to_vec(v)?,
            Frame::Ping | Frame::Pong => Vec::new(),
        })
    }

    fn decode(kind: FrameKind, body: &[u8]) -> Result<Self> {
        Ok(match kind {
            FrameKind::Hello => Frame::Hello(serde_json::from_slice(body)?),
            FrameKind::Challenge => Frame::Challenge(serde_json::from_slice(body)?),
            FrameKind::Auth => Frame::Auth(serde_json::from_slice(body)?),
            FrameKind::Welcome => Frame::Welcome(serde_json::from_slice(body)?),
            FrameKind::Error => Frame::Error(serde_json::from_slice(body)?),
            FrameKind::Relay => Frame::Relay(serde_json::from_slice(body)?),
            FrameKind::Presence => Frame::Presence(serde_json::from_slice(body)?),
            FrameKind::Ping => Frame::Ping,
            FrameKind::Pong => Frame::Pong,
        })
    }

    /// Convenience constructor for the common error reply.
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Frame::Error(ErrorFrame {
            code: code.to_string(),
            message: message.into(),
        })
    }

    pub fn hello(room: String, device_id: String) -> Self {
        Frame::Hello(Hello {
            proto: PROTOCOL_VERSION,
            room,
            device_id,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }
}

/// Write one frame. Callers should `flush` when a batch is done.
pub async fn write_frame<W>(w: &mut W, frame: &Frame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = frame.body()?;
    let len = body.len() + 1;
    if len > MAX_FRAME_LEN {
        bail!("帧过大: {len} 字节");
    }

    // One buffer, one write: avoids a partially written frame if the socket dies
    // between the header and the body.
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_be_bytes());
    buf.push(frame.kind() as u8);
    buf.extend_from_slice(&body);

    w.write_all(&buf).await.context("写入帧失败")?;
    w.flush().await.context("刷新连接失败")?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF.
pub async fn read_frame<R>(r: &mut R) -> Result<Option<Frame>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 4];
    match r.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("读取帧头失败"),
    }

    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        bail!("帧长度为 0");
    }
    if len > MAX_FRAME_LEN {
        // Refuse before allocating: this is the memory bound per connection.
        bail!("帧长度超限: {len} > {MAX_FRAME_LEN}");
    }

    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.context("读取帧体失败")?;

    let kind = FrameKind::from_u8(buf[0])?;
    Frame::decode(kind, &buf[1..]).map(Some)
}

/// Base64 helpers, kept here so both ends agree on the alphabet.
pub fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn unb64(text: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .context("base64 解码失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_frames_over_a_pipe() {
        let (mut a, mut b) = tokio::io::duplex(4096);

        write_frame(&mut a, &Frame::hello("room1".into(), "dev1".into()))
            .await
            .unwrap();
        write_frame(&mut a, &Frame::Ping).await.unwrap();

        let Some(Frame::Hello(h)) = read_frame(&mut b).await.unwrap() else {
            panic!("expected Hello");
        };
        assert_eq!(h.room, "room1");
        assert_eq!(h.proto, PROTOCOL_VERSION);
        assert!(matches!(
            read_frame(&mut b).await.unwrap(),
            Some(Frame::Ping)
        ));
    }

    #[tokio::test]
    async fn clean_eof_yields_none() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversized_length_is_rejected_without_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(u32::MAX).to_be_bytes()).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }

    #[test]
    fn base64_roundtrip() {
        let data = [0u8, 1, 2, 250, 255];
        assert_eq!(unb64(&b64(&data)).unwrap(), data);
    }
}
