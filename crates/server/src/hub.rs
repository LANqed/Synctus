//! Room registry and message fan-out.
//!
//! # How the relay gates a room without knowing the secret
//!
//! The relay never learns the invite code, so it cannot verify a client's HMAC
//! on its own. Instead it acts as a *witness*: every room gets one random
//! challenge, and every device in that room must answer it with the same MAC.
//! The first device to join defines the expected answer; later joiners are
//! compared against it.
//!
//! ```text
//! room created ──▶ random 32-byte challenge (kept until the room empties)
//!   device A ──▶ mac_A ──▶ accepted, expected := mac_A
//!   device B ──▶ mac_B ──▶ accepted only if mac_B == mac_A
//! ```
//!
//! Consequences, stated plainly:
//!
//! * Someone who knows a room id but not the invite code cannot join an
//!   occupied room, and cannot read anything even if they could, because
//!   payloads are end-to-end encrypted.
//! * Someone who knows a room id *can* occupy an empty room first and set a
//!   bogus expected MAC, locking the real users out until the room empties.
//!   Room ids are HKDF output over an 80-bit invite code, so guessing one is
//!   the hard part; this is a denial-of-service risk, not a confidentiality
//!   one.
//! * A captured MAC can be replayed while the room stays occupied. The
//!   client→relay hop is TLS, so capturing one requires already being on the
//!   inside of that hop.

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use synctus_core::proto::{Frame, Presence, Relay};

/// Per-device outbound queue depth. Frames are small and the writer task drains
/// continuously; a device that fills this is not reading its socket.
const DEVICE_QUEUE: usize = 64;

struct Device {
    tx: mpsc::Sender<Frame>,
}

struct Room {
    /// Fixed for the lifetime of the room, see the module docs.
    challenge: [u8; 32],
    /// Answer the first device gave, which later devices must match.
    expected_mac: Option<Vec<u8>>,
    devices: HashMap<String, Device>,
    /// Latest retained payload per slot (`status`, `todos`), replayed to
    /// joiners so a device that starts later sees the peer immediately.
    retained: HashMap<String, Relay>,
}

impl Room {
    fn new() -> Self {
        Self {
            challenge: synctus_core::crypto::random_challenge(),
            expected_mac: None,
            devices: HashMap::new(),
            retained: HashMap::new(),
        }
    }
}

/// What a joining device needs to bootstrap its view.
pub struct Joined {
    pub peers: Vec<String>,
    pub retained: Vec<Relay>,
    pub rx: mpsc::Receiver<Frame>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub rooms: usize,
    pub devices: usize,
}

/// Shared state for all connections.
pub struct Hub {
    rooms: Mutex<HashMap<String, Room>>,
    max_devices_per_room: usize,
    max_rooms: usize,
}

impl Hub {
    pub fn new(max_devices_per_room: usize, max_rooms: usize) -> Arc<Self> {
        Arc::new(Self {
            rooms: Mutex::new(HashMap::new()),
            max_devices_per_room: max_devices_per_room.max(2),
            max_rooms: max_rooms.max(1),
        })
    }

    /// Challenge for `room_id`, creating the room if needed.
    ///
    /// Creating the room here — before authentication — is what makes the
    /// challenge stable across the members of a room. `max_rooms` is the bound
    /// that keeps this from being an unbounded allocation primitive.
    pub async fn challenge_for(&self, room_id: &str) -> Result<[u8; 32]> {
        let mut rooms = self.rooms.lock().await;
        if !rooms.contains_key(room_id) && rooms.len() >= self.max_rooms {
            bail!("服务器房间数已达上限");
        }
        Ok(rooms
            .entry(room_id.to_string())
            .or_insert_with(Room::new)
            .challenge)
    }

    /// Verify the MAC and register the device.
    pub async fn join(&self, room_id: &str, device_id: &str, mac: &[u8]) -> Result<Joined> {
        let mut rooms = self.rooms.lock().await;
        let room = match rooms.get_mut(room_id) {
            Some(r) => r,
            // The room is gone only if every device left between the challenge
            // and the auth frame. Tell the client to retry rather than guessing.
            None => bail!("房间状态已过期，请重新连接"),
        };

        match &room.expected_mac {
            Some(expected) if !ct_eq(expected, mac) => {
                bail!("配对码不匹配");
            }
            Some(_) => {}
            None => room.expected_mac = Some(mac.to_vec()),
        }

        if room.devices.len() >= self.max_devices_per_room && !room.devices.contains_key(device_id)
        {
            bail!("房间设备数已达上限");
        }

        let peers: Vec<String> = room.devices.keys().cloned().collect();
        let retained: Vec<Relay> = room
            .retained
            .values()
            // Never hand a device its own retained state back.
            .filter(|r| r.from != device_id)
            .cloned()
            .collect();

        let (tx, rx) = mpsc::channel(DEVICE_QUEUE);

        // Reconnect of the same device id replaces the old entry; dropping the
        // old sender ends its writer task.
        room.devices.insert(device_id.to_string(), Device { tx });

        // Announce the arrival to everyone else.
        let announce = Frame::Presence(Presence {
            device_id: device_id.to_string(),
            online: true,
            platform_hint: None,
        });
        Self::fan_out(room, device_id, &announce);

        Ok(Joined {
            peers,
            retained,
            rx,
        })
    }

    /// Deregister a device and tell the room. Removes the room once empty,
    /// which also rotates the challenge for whoever comes next.
    pub async fn leave(&self, room_id: &str, device_id: &str) {
        let mut rooms = self.rooms.lock().await;
        let Some(room) = rooms.get_mut(room_id) else {
            return;
        };
        room.devices.remove(device_id);

        let announce = Frame::Presence(Presence {
            device_id: device_id.to_string(),
            online: false,
            platform_hint: None,
        });
        Self::fan_out(room, device_id, &announce);

        if room.devices.is_empty() {
            rooms.remove(room_id);
        }
    }

    /// Forward an encrypted payload to the other devices in the room.
    ///
    /// `from` is taken from the authenticated connection, not from the frame, so
    /// a device cannot claim to be another one.
    pub async fn relay(&self, room_id: &str, from: &str, mut relay: Relay) -> Result<()> {
        let mut rooms = self.rooms.lock().await;
        let Some(room) = rooms.get_mut(room_id) else {
            bail!("房间不存在");
        };

        relay.from = from.to_string();

        if let Some(slot) = relay.retain.clone() {
            // Key by device *and* slot: two devices of the same person must not
            // overwrite each other's status.
            room.retained
                .insert(format!("{from}/{slot}"), relay.clone());
        }

        Self::fan_out(room, from, &Frame::Relay(relay));
        Ok(())
    }

    /// Send to every device except `exclude`, dropping any that cannot keep up.
    ///
    /// A full queue means the device is not draining its socket; cutting it
    /// loose is better than letting the room's memory grow. It will reconnect
    /// and pick up the retained state.
    fn fan_out(room: &mut Room, exclude: &str, frame: &Frame) {
        let mut dead = Vec::new();
        for (id, device) in room.devices.iter() {
            if id == exclude {
                continue;
            }
            if device.tx.try_send(frame.clone()).is_err() {
                dead.push(id.clone());
            }
        }
        for id in dead {
            tracing::warn!(device = %id, "设备发送队列阻塞，断开连接");
            room.devices.remove(&id);
        }
    }

    pub async fn stats(&self) -> Stats {
        let rooms = self.rooms.lock().await;
        Stats {
            rooms: rooms.len(),
            devices: rooms.values().map(|r| r.devices.len()).sum(),
        }
    }
}

/// Constant-time byte comparison, so a wrong MAC leaks no timing information.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(from: &str, retain: Option<&str>) -> Relay {
        Relay {
            from: from.into(),
            body: "AAAA".into(),
            retain: retain.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn same_room_gets_one_stable_challenge() {
        let hub = Hub::new(8, 100);
        let a = hub.challenge_for("room").await.unwrap();
        let b = hub.challenge_for("room").await.unwrap();
        assert_eq!(a, b);
        let other = hub.challenge_for("other").await.unwrap();
        assert_ne!(a, other);
    }

    #[tokio::test]
    async fn first_device_sets_the_expected_mac() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        assert!(hub.join("room", "a", b"mac-one").await.is_ok());
        assert!(hub.join("room", "b", b"mac-one").await.is_ok());
        assert!(
            hub.join("room", "c", b"mac-two").await.is_err(),
            "a different MAC must be rejected"
        );
    }

    #[tokio::test]
    async fn joiner_sees_existing_peers_and_retained_state() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        hub.relay("room", "a", relay("a", Some("status")))
            .await
            .unwrap();

        let joined = hub.join("room", "b", b"mac").await.unwrap();
        assert_eq!(joined.peers, vec!["a".to_string()]);
        assert_eq!(joined.retained.len(), 1);
        assert_eq!(joined.retained[0].from, "a");
    }

    #[tokio::test]
    async fn own_retained_state_is_not_echoed_back() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        hub.relay("room", "a", relay("a", Some("status")))
            .await
            .unwrap();

        // Same device reconnecting.
        let again = hub.join("room", "a", b"mac").await.unwrap();
        assert!(again.retained.is_empty());
    }

    #[tokio::test]
    async fn relay_reaches_the_peer_but_not_the_sender() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let mut a = hub.join("room", "a", b"mac").await.unwrap();
        let mut b = hub.join("room", "b", b"mac").await.unwrap();

        // `a` was told that `b` arrived.
        assert!(
            matches!(a.rx.try_recv(), Ok(Frame::Presence(p)) if p.device_id == "b" && p.online)
        );

        hub.relay("room", "a", relay("a", None)).await.unwrap();
        assert!(matches!(b.rx.try_recv(), Ok(Frame::Relay(r)) if r.from == "a"));
        assert!(
            a.rx.try_recv().is_err(),
            "sender must not receive its own frame"
        );
    }

    #[tokio::test]
    async fn sender_identity_is_overwritten_by_the_connection() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        let mut b = hub.join("room", "b", b"mac").await.unwrap();
        let _ = b.rx.try_recv();

        // `a` lies about who it is; the hub must correct it.
        hub.relay("room", "a", relay("spoofed", None))
            .await
            .unwrap();
        assert!(matches!(b.rx.try_recv(), Ok(Frame::Relay(r)) if r.from == "a"));
    }

    #[tokio::test]
    async fn leaving_notifies_and_empty_rooms_are_dropped() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        let mut b = hub.join("room", "b", b"mac").await.unwrap();
        let _ = b.rx.try_recv();

        hub.leave("room", "a").await;
        assert!(matches!(b.rx.try_recv(), Ok(Frame::Presence(p)) if !p.online));
        assert_eq!(hub.stats().await.devices, 1);

        hub.leave("room", "b").await;
        assert_eq!(hub.stats().await.rooms, 0);
    }

    #[tokio::test]
    async fn challenge_rotates_after_the_room_empties() {
        let hub = Hub::new(8, 100);
        let first = hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        hub.leave("room", "a").await;

        let second = hub.challenge_for("room").await.unwrap();
        assert_ne!(first, second, "a fresh room must not reuse the challenge");
    }

    #[tokio::test]
    async fn device_and_room_limits_are_enforced() {
        let hub = Hub::new(2, 1);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        let _b = hub.join("room", "b", b"mac").await.unwrap();
        assert!(hub.join("room", "c", b"mac").await.is_err());

        // max_rooms = 1, and the one room is occupied.
        assert!(hub.challenge_for("another").await.is_err());
    }

    #[tokio::test]
    async fn reconnect_replaces_the_previous_connection() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let old = hub.join("room", "a", b"mac").await.unwrap();
        let _new = hub.join("room", "a", b"mac").await.unwrap();
        // Old queue is orphaned, so its writer task ends.
        drop(old);
        assert_eq!(hub.stats().await.devices, 1);
    }

    #[tokio::test]
    async fn two_devices_retain_separate_status_slots() {
        let hub = Hub::new(8, 100);
        hub.challenge_for("room").await.unwrap();
        let _a = hub.join("room", "a", b"mac").await.unwrap();
        let _b = hub.join("room", "b", b"mac").await.unwrap();
        hub.relay("room", "a", relay("a", Some("status")))
            .await
            .unwrap();
        hub.relay("room", "b", relay("b", Some("status")))
            .await
            .unwrap();

        let c = hub.join("room", "c", b"mac").await.unwrap();
        assert_eq!(
            c.retained.len(),
            2,
            "per-device retain slots must not collide"
        );
    }

    #[test]
    fn ct_eq_compares_correctly() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
