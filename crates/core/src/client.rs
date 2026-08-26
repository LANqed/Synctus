//! The reconnecting client engine shared by desktop and Android.
//!
//! The UI never touches sockets. It sends [`Command`]s into a channel and reads
//! [`Event`]s out of another one, which keeps the front-ends free of async code
//! and makes the engine testable on its own.
//!
//! ```text
//!   UI ──Command──▶ ┌───────────────┐ ──Event──▶ UI
//!                   │ Client (task) │
//!                   └───────┬───────┘
//!                     frames │ E2E-sealed
//!                            ▼
//!                          relay
//! ```
//!
//! Reconnection uses exponential backoff with jitter. On every successful
//! connect the last local status is republished so a peer that was waiting sees
//! us immediately, and the relay's retained state gives us theirs.

use anyhow::{anyhow, bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::config::ClientConfig;
use crate::crypto::RoomKeys;
use crate::model::{Nudge, PeerMessage, StatusSnapshot, Todo};
use crate::proto::{self, Frame};

/// Connection state, surfaced so the UI can show a badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Online,
    /// Disconnected with a human-readable reason.
    Offline(String),
    /// Terminal: the invite code or protocol version is wrong, so retrying is
    /// pointless until the user changes something.
    Rejected(String),
}

/// Things the UI asks the engine to do.
///
/// `Publish` dominates the size of this enum because a status snapshot is a
/// couple of hundred bytes. Boxing it would add an allocation to the hottest
/// path here for no real gain: commands are moved a handful of times per minute.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Command {
    /// Publish a new local status snapshot.
    Publish(StatusSnapshot),
    /// Publish the local to-do list.
    PublishTodos(Vec<Todo>),
    /// Send an interaction to the peer.
    Nudge(Nudge),
    /// Re-read config and reconnect. Used after the user edits settings.
    Reconnect(Box<ClientConfig>),
    /// Stop the engine.
    Shutdown,
}

/// Things the engine tells the UI.
///
/// Same reasoning as [`Command`]: `PeerStatus` is the large variant and the
/// enum is moved rarely enough that boxing would cost more than it saves.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Event {
    State(ConnState),
    /// A peer's status arrived and decrypted successfully.
    PeerStatus(StatusSnapshot),
    PeerTodos {
        device_id: String,
        items: Vec<Todo>,
    },
    /// A peer poked us.
    Nudge(Nudge),
    /// A peer device connected or disconnected, per the relay.
    PeerPresence {
        device_id: String,
        online: bool,
    },
    /// Non-fatal problem worth showing in the log pane.
    Warning(String),
}

/// Handle the UI keeps. Dropping it stops the engine.
#[derive(Clone)]
pub struct ClientHandle {
    tx: mpsc::Sender<Command>,
}

impl ClientHandle {
    /// Queue a command. Fails only once the engine is gone.
    pub fn send(&self, cmd: Command) -> Result<()> {
        self.tx
            .try_send(cmd)
            .map_err(|e| anyhow!("命令队列已满或客户端已退出: {e}"))
    }

    /// Async variant that waits for room in the queue.
    pub async fn send_async(&self, cmd: Command) -> Result<()> {
        self.tx.send(cmd).await.context("客户端已退出")
    }

    pub fn publish(&self, snapshot: StatusSnapshot) -> Result<()> {
        self.send(Command::Publish(snapshot))
    }

    pub fn nudge(&self, nudge: Nudge) -> Result<()> {
        self.send(Command::Nudge(nudge))
    }
}

/// Backoff schedule for reconnects, in milliseconds.
const BACKOFF_START_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 30_000;

/// Compute the next delay: double, cap, then add up to 25% jitter so two peers
/// restarted together do not hammer the relay in lockstep.
fn next_backoff(current: u64) -> u64 {
    let doubled = (current * 2).min(BACKOFF_MAX_MS);
    let jitter = rand::random::<u64>() % (doubled / 4 + 1);
    doubled + jitter
}

/// The engine.
pub struct Client {
    cfg: ClientConfig,
    keys: Arc<RoomKeys>,
    events: mpsc::Sender<Event>,
    commands: mpsc::Receiver<Command>,
    /// Last published status, replayed after a reconnect.
    last_status: Option<StatusSnapshot>,
    last_todos: Option<Vec<Todo>>,
}

impl Client {
    /// Build the engine and return the handle plus the event stream.
    ///
    /// Key derivation runs here, so this is a good place for the UI to show a
    /// brief "pairing…" state: Argon2id takes ~100 ms.
    pub fn spawn(cfg: ClientConfig) -> Result<(ClientHandle, mpsc::Receiver<Event>, Client)> {
        if !cfg.is_paired() {
            bail!("尚未设置配对码");
        }
        let keys = Arc::new(RoomKeys::derive(&cfg.invite_code)?);

        // Bounded channels: if the UI stops draining events we would rather drop
        // than grow without limit.
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (evt_tx, evt_rx) = mpsc::channel(256);

        let client = Client {
            cfg,
            keys,
            events: evt_tx,
            commands: cmd_rx,
            last_status: None,
            last_todos: None,
        };
        Ok((ClientHandle { tx: cmd_tx }, evt_rx, client))
    }

    /// Run until [`Command::Shutdown`] or the handle is dropped.
    pub async fn run(mut self) {
        let mut backoff = BACKOFF_START_MS;

        loop {
            self.emit(Event::State(ConnState::Connecting)).await;

            match self.session().await {
                Ok(SessionEnd::Shutdown) => return,
                Ok(SessionEnd::Reconnect) => {
                    // Config changed: connect again straight away.
                    backoff = BACKOFF_START_MS;
                    continue;
                }
                Ok(SessionEnd::Closed) => {
                    self.emit(Event::State(ConnState::Offline("连接已关闭".into())))
                        .await;
                }
                Err(SessionError::Rejected(msg)) => {
                    // Wrong code or protocol: stop retrying and wait for the user
                    // to send a Reconnect command.
                    self.emit(Event::State(ConnState::Rejected(msg))).await;
                    match self.wait_for_reconnect().await {
                        Some(cfg) => {
                            if let Err(e) = self.reconfigure(cfg) {
                                self.emit(Event::Warning(format!("{e:#}"))).await;
                            }
                            backoff = BACKOFF_START_MS;
                            continue;
                        }
                        None => return,
                    }
                }
                Err(SessionError::Transport(e)) => {
                    self.emit(Event::State(ConnState::Offline(format!("{e:#}"))))
                        .await;
                }
            }

            // Sleep before retrying, but stay responsive to commands so the UI
            // can force an immediate reconnect.
            let delay = Duration::from_millis(backoff);
            backoff = next_backoff(backoff);
            match self.sleep_or_command(delay).await {
                SleepOutcome::Elapsed => {}
                SleepOutcome::Reconnect(cfg) => {
                    if let Err(e) = self.reconfigure(cfg) {
                        self.emit(Event::Warning(format!("{e:#}"))).await;
                    }
                    backoff = BACKOFF_START_MS;
                }
                SleepOutcome::Shutdown => return,
            }
        }
    }

    fn reconfigure(&mut self, cfg: Box<ClientConfig>) -> Result<()> {
        let keys = RoomKeys::derive(&cfg.invite_code)?;
        self.keys = Arc::new(keys);
        self.cfg = *cfg;
        Ok(())
    }

    async fn emit(&self, event: Event) {
        // A full or closed channel means the UI is gone or wedged; dropping the
        // event is the right call, the next one carries the same state.
        let _ = self.events.try_send(event);
    }

    /// Wait for a `Reconnect` after a terminal rejection, ignoring other
    /// commands. `None` means the handle was dropped.
    async fn wait_for_reconnect(&mut self) -> Option<Box<ClientConfig>> {
        while let Some(cmd) = self.commands.recv().await {
            match cmd {
                Command::Reconnect(cfg) => return Some(cfg),
                Command::Shutdown => return None,
                _ => {}
            }
        }
        None
    }

    async fn sleep_or_command(&mut self, delay: Duration) -> SleepOutcome {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return SleepOutcome::Elapsed,
                cmd = self.commands.recv() => match cmd {
                    Some(Command::Reconnect(cfg)) => return SleepOutcome::Reconnect(cfg),
                    Some(Command::Shutdown) | None => return SleepOutcome::Shutdown,
                    // Remember offline edits so they are published on connect.
                    Some(Command::Publish(s)) => self.last_status = Some(s),
                    Some(Command::PublishTodos(t)) => self.last_todos = Some(t),
                    // Nudges are dropped while offline rather than queued: a poke
                    // that arrives ten minutes late is worse than none.
                    Some(Command::Nudge(_)) => {}
                },
            }
        }
    }

    /// One connection attempt, from TCP connect to disconnect.
    async fn session(&mut self) -> std::result::Result<SessionEnd, SessionError> {
        #[cfg(feature = "tls")]
        let stream = crate::tls::connect(&self.cfg.server, self.cfg.tls, &self.cfg.server_name())
            .await
            .map_err(SessionError::Transport)?;

        #[cfg(not(feature = "tls"))]
        let stream = {
            if self.cfg.tls {
                return Err(SessionError::Rejected(
                    "此构建未包含 TLS 支持，请在配置中关闭 tls".into(),
                ));
            }
            tokio::net::TcpStream::connect(&self.cfg.server)
                .await
                .with_context(|| format!("连接服务器失败: {}", self.cfg.server))
                .map_err(SessionError::Transport)?
        };

        let (reader, writer) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut writer = writer;

        self.handshake(&mut reader, &mut writer).await?;
        self.emit(Event::State(ConnState::Online)).await;

        // Republish state so the peer sees us without waiting for the next poll.
        if let Some(status) = self.last_status.clone() {
            self.publish_status(&mut writer, &status)
                .await
                .map_err(SessionError::Transport)?;
        }
        if let Some(todos) = self.last_todos.clone() {
            self.publish_todos(&mut writer, &todos)
                .await
                .map_err(SessionError::Transport)?;
        }

        self.pump(&mut reader, &mut writer).await
    }

    async fn handshake<R, W>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> std::result::Result<(), SessionError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let hello = Frame::hello(self.keys.room_id_hex(), self.cfg.device_id.clone());
        proto::write_frame(writer, &hello)
            .await
            .map_err(SessionError::Transport)?;

        let challenge = match proto::read_frame(reader)
            .await
            .map_err(SessionError::Transport)?
        {
            Some(Frame::Challenge(c)) => c,
            Some(Frame::Error(e)) => {
                return Err(SessionError::Rejected(format!("{}: {}", e.code, e.message)))
            }
            Some(other) => {
                return Err(SessionError::Transport(anyhow!(
                    "握手时收到意外帧: {other:?}"
                )))
            }
            None => return Err(SessionError::Transport(anyhow!("服务器在握手时关闭连接"))),
        };

        let nonce = proto::unb64(&challenge.nonce).map_err(SessionError::Transport)?;
        let mac = self.keys.auth_response(&nonce);
        proto::write_frame(
            writer,
            &Frame::Auth(proto::Auth {
                mac: proto::b64(&mac),
            }),
        )
        .await
        .map_err(SessionError::Transport)?;

        match proto::read_frame(reader)
            .await
            .map_err(SessionError::Transport)?
        {
            Some(Frame::Welcome(w)) => {
                for peer in w.peers {
                    self.emit(Event::PeerPresence {
                        device_id: peer,
                        online: true,
                    })
                    .await;
                }
                // A large clock skew breaks pomodoro deadlines and staleness, so
                // warn rather than silently misbehave.
                let skew = (crate::now_ms() - w.server_time).abs();
                if skew > 60_000 {
                    self.emit(Event::Warning(format!(
                        "本机时间与服务器相差 {} 秒，番茄钟与状态时间可能不准",
                        skew / 1000
                    )))
                    .await;
                }
                Ok(())
            }
            Some(Frame::Error(e)) => {
                Err(SessionError::Rejected(format!("{}: {}", e.code, e.message)))
            }
            Some(other) => Err(SessionError::Transport(anyhow!(
                "认证后收到意外帧: {other:?}"
            ))),
            None => Err(SessionError::Transport(anyhow!("认证后连接被关闭"))),
        }
    }

    /// Main loop: forward commands out, dispatch frames in, keep the link warm.
    async fn pump<R, W>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> std::result::Result<SessionEnd, SessionError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; skip it, we just handshaked.
        heartbeat.tick().await;

        loop {
            tokio::select! {
                cmd = self.commands.recv() => match cmd {
                    Some(Command::Publish(status)) => {
                        self.last_status = Some(status.clone());
                        self.publish_status(writer, &status).await.map_err(SessionError::Transport)?;
                    }
                    Some(Command::PublishTodos(items)) => {
                        self.last_todos = Some(items.clone());
                        self.publish_todos(writer, &items).await.map_err(SessionError::Transport)?;
                    }
                    Some(Command::Nudge(nudge)) => {
                        self.send_peer(writer, &PeerMessage::Nudge(nudge))
                            .await
                            .map_err(SessionError::Transport)?;
                    }
                    Some(Command::Reconnect(cfg)) => {
                        self.reconfigure(cfg).map_err(|e| SessionError::Rejected(format!("{e:#}")))?;
                        return Ok(SessionEnd::Reconnect);
                    }
                    Some(Command::Shutdown) | None => return Ok(SessionEnd::Shutdown),
                },

                frame = proto::read_frame(reader) => {
                    match frame.map_err(SessionError::Transport)? {
                        Some(f) => self.handle_frame(f, writer).await?,
                        None => return Ok(SessionEnd::Closed),
                    }
                }

                _ = heartbeat.tick() => {
                    proto::write_frame(writer, &Frame::Ping)
                        .await
                        .map_err(SessionError::Transport)?;
                }
            }
        }
    }

    async fn handle_frame<W>(
        &mut self,
        frame: Frame,
        writer: &mut W,
    ) -> std::result::Result<(), SessionError>
    where
        W: AsyncWrite + Unpin,
    {
        match frame {
            Frame::Relay(relay) => {
                // Ignore our own echo, which happens if the relay ever fans out
                // to the whole room including the sender.
                if relay.from == self.cfg.device_id {
                    return Ok(());
                }
                match self.decrypt_peer(&relay) {
                    Ok(msg) => self.dispatch_peer(msg, writer).await?,
                    Err(e) => {
                        // Almost always a mismatched invite code. Report once
                        // rather than tearing the session down.
                        self.emit(Event::Warning(format!("无法解密对方消息: {e:#}")))
                            .await;
                    }
                }
                Ok(())
            }
            Frame::Presence(p) => {
                self.emit(Event::PeerPresence {
                    device_id: p.device_id,
                    online: p.online,
                })
                .await;
                Ok(())
            }
            Frame::Ping => {
                proto::write_frame(writer, &Frame::Pong)
                    .await
                    .map_err(SessionError::Transport)?;
                Ok(())
            }
            Frame::Pong => Ok(()),
            Frame::Error(e) => {
                self.emit(Event::Warning(format!(
                    "服务器错误 {}: {}",
                    e.code, e.message
                )))
                .await;
                Ok(())
            }
            // Handshake frames after the handshake mean the peer or relay is
            // confused; treat as a protocol error and reconnect.
            other => Err(SessionError::Transport(anyhow!(
                "会话中收到意外帧: {other:?}"
            ))),
        }
    }

    async fn dispatch_peer<W>(
        &mut self,
        msg: PeerMessage,
        writer: &mut W,
    ) -> std::result::Result<(), SessionError>
    where
        W: AsyncWrite + Unpin,
    {
        match msg {
            PeerMessage::Status(s) => self.emit(Event::PeerStatus(s)).await,
            PeerMessage::Todos {
                device_id, items, ..
            } => self.emit(Event::PeerTodos { device_id, items }).await,
            PeerMessage::Nudge(n) => self.emit(Event::Nudge(n)).await,
            PeerMessage::Ping { at } => {
                self.send_peer(writer, &PeerMessage::Pong { at })
                    .await
                    .map_err(SessionError::Transport)?;
            }
            PeerMessage::Pong { .. } => {}
        }
        Ok(())
    }

    /// Decrypt a relayed payload.
    ///
    /// The sender's device id is the AEAD `aad`, so a peer inside the room cannot
    /// re-attribute someone else's sealed message to a different device.
    fn decrypt_peer(&self, relay: &proto::Relay) -> Result<PeerMessage> {
        let sealed = proto::unb64(&relay.body)?;
        self.keys.open_json(&sealed, relay.from.as_bytes())
    }

    async fn publish_status<W>(&self, writer: &mut W, status: &StatusSnapshot) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.send_peer(writer, &PeerMessage::Status(status.clone()))
            .await
    }

    async fn publish_todos<W>(&self, writer: &mut W, items: &[Todo]) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.send_peer(
            writer,
            &PeerMessage::Todos {
                device_id: self.cfg.device_id.clone(),
                items: items.to_vec(),
                at: crate::now_ms(),
            },
        )
        .await
    }

    async fn send_peer<W>(&self, writer: &mut W, msg: &PeerMessage) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let sealed = self
            .keys
            .seal_json(msg, self.cfg.device_id.as_bytes())
            .context("加密消息失败")?;
        let frame = Frame::Relay(proto::Relay {
            from: self.cfg.device_id.clone(),
            body: proto::b64(&sealed),
            retain: msg.retain_slot().map(|s| s.to_string()),
        });
        proto::write_frame(writer, &frame).await
    }
}

enum SessionEnd {
    /// Peer or relay closed the socket.
    Closed,
    /// Config changed, reconnect immediately.
    Reconnect,
    Shutdown,
}

enum SessionError {
    /// Retryable.
    Transport(anyhow::Error),
    /// Not retryable without user action.
    Rejected(String),
}

enum SleepOutcome {
    Elapsed,
    Reconnect(Box<ClientConfig>),
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let mut d = BACKOFF_START_MS;
        for _ in 0..10 {
            d = next_backoff(d);
        }
        // Cap plus at most 25% jitter.
        assert!(d <= BACKOFF_MAX_MS + BACKOFF_MAX_MS / 4, "got {d}");
        assert!(d >= BACKOFF_MAX_MS);
    }

    #[test]
    fn backoff_at_least_doubles_before_the_cap() {
        assert!(next_backoff(1_000) >= 2_000);
    }

    #[test]
    fn unpaired_config_is_refused() {
        let cfg = ClientConfig::default();
        assert!(Client::spawn(cfg).is_err());
    }

    #[test]
    fn spawn_derives_keys_for_a_paired_config() {
        let cfg = ClientConfig {
            invite_code: "ABCD-EFGH-IJKL-MNOP".into(),
            ..ClientConfig::default()
        };
        let (handle, _events, client) = Client::spawn(cfg).expect("spawn");
        assert_eq!(client.keys.room_id_hex().len(), 32);
        // The handle must accept commands even before `run` is polled.
        assert!(handle.send(Command::Shutdown).is_ok());
    }
}
