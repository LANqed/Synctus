//! End-to-end encryption.
//!
//! Synctus pairs two people with a single shared *invite code* (the room
//! passphrase). Everything is derived from it, so there is no account system and
//! the relay never holds key material:
//!
//! ```text
//! invite code ──Argon2id(salt="synctus/room/v1")──► 32-byte room secret
//!                                │
//!         HKDF-SHA256(room secret, info=…)
//!                ├─► room_id      (16 bytes, sent in clear to the relay)
//!                ├─► auth_key     (32 bytes, proves room membership)
//!                └─► msg_key      (32 bytes, XChaCha20-Poly1305 payload key)
//! ```
//!
//! The relay authenticates a client by verifying an HMAC over a server-chosen
//! challenge, so the invite code itself never crosses the network. Payloads use
//! XChaCha20-Poly1305 with a random 24-byte nonce per message; with 192-bit
//! nonces random generation is safe for the message volumes here.

use anyhow::{bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// `Hmac::new_from_slice` is offered by both `Mac` and the AEAD crate's
/// `KeyInit`, which are both in scope here. Bind it once, explicitly.
fn hmac_init(key: &[u8]) -> HmacSha256 {
    <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts keys of any length")
}

/// Argon2id salt. Fixed on purpose: both peers must derive the same secret from
/// the invite code alone, so there is nowhere to store a random salt. The code
/// is expected to be high-entropy (see [`generate_invite_code`]) and Argon2id
/// only guards against weak user-chosen codes.
const ROOM_SALT: &[u8] = b"synctus/room/v1";

const INFO_ROOM_ID: &[u8] = b"synctus/v1/room-id";
const INFO_AUTH: &[u8] = b"synctus/v1/auth";
const INFO_MSG: &[u8] = b"synctus/v1/message";

/// Argon2id parameters: 64 MiB, 3 passes. Roughly 100 ms on a laptop and only
/// paid once at startup.
const ARGON_MEM_KIB: u32 = 64 * 1024;
const ARGON_PASSES: u32 = 3;
const ARGON_LANES: u32 = 1;

/// Alphabet without look-alike characters, for codes people read out loud.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Keys derived from one invite code.
#[derive(Clone)]
pub struct RoomKeys {
    room_id: [u8; 16],
    auth_key: [u8; 32],
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for RoomKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("RoomKeys")
            .field("room_id", &self.room_id_hex())
            .finish_non_exhaustive()
    }
}

impl RoomKeys {
    /// Stretch an invite code into room keys.
    ///
    /// Whitespace and case are normalised so `abcd-efgh` and `ABCD EFGH` are the
    /// same room; that lets users retype a code without worrying about format.
    pub fn derive(invite_code: &str) -> Result<Self> {
        let normalised = normalise_code(invite_code);
        if normalised.len() < 8 {
            bail!("配对码太短：至少需要 8 个有效字符");
        }

        let params = argon2::Params::new(ARGON_MEM_KIB, ARGON_PASSES, ARGON_LANES, Some(32))
            .map_err(|e| anyhow::anyhow!("argon2 参数无效: {e}"))?;
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let mut root = [0u8; 32];
        argon
            .hash_password_into(normalised.as_bytes(), ROOM_SALT, &mut root)
            .map_err(|e| anyhow::anyhow!("配对码派生失败: {e}"))?;

        let hk = Hkdf::<Sha256>::new(None, &root);

        let mut room_id = [0u8; 16];
        hk.expand(INFO_ROOM_ID, &mut room_id)
            .expect("16 bytes is a valid HKDF length");
        let mut auth_key = [0u8; 32];
        hk.expand(INFO_AUTH, &mut auth_key)
            .expect("32 bytes is a valid HKDF length");
        let mut msg_key = [0u8; 32];
        hk.expand(INFO_MSG, &mut msg_key)
            .expect("32 bytes is a valid HKDF length");

        let cipher = XChaCha20Poly1305::new_from_slice(&msg_key)
            .map_err(|_| anyhow::anyhow!("消息密钥长度错误"))?;

        // `root` and `msg_key` are copies of derived material; wipe the stack
        // copies we no longer need.
        root.fill(0);
        msg_key.fill(0);

        Ok(Self {
            room_id,
            auth_key,
            cipher,
        })
    }

    /// Room identifier the relay uses for routing. Reveals nothing about the
    /// invite code beyond membership.
    pub fn room_id(&self) -> [u8; 16] {
        self.room_id
    }

    /// Lowercase hex room id, used in logs and the relay's routing table.
    pub fn room_id_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.room_id)
    }

    /// Answer a relay challenge: `HMAC(auth_key, "synctus-auth" || challenge)`.
    pub fn auth_response(&self, challenge: &[u8]) -> [u8; 32] {
        let mut mac = hmac_init(&self.auth_key);
        mac.update(b"synctus-auth");
        mac.update(challenge);
        mac.finalize().into_bytes().into()
    }

    /// Constant-time check of a client's challenge response.
    pub fn verify_auth(&self, challenge: &[u8], response: &[u8]) -> bool {
        let mut mac = hmac_init(&self.auth_key);
        mac.update(b"synctus-auth");
        mac.update(challenge);
        mac.verify_slice(response).is_ok()
    }

    /// Encrypt a payload, returning `nonce || ciphertext`.
    ///
    /// `aad` is authenticated but not encrypted; the transport binds the sender
    /// device id there so a peer cannot be impersonated within the room.
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        let ct = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("加密失败"))?;

        let mut out = Vec::with_capacity(24 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Inverse of [`seal`]. Fails on a wrong invite code, a tampered payload or
    /// mismatched `aad`.
    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 24 + 16 {
            bail!("密文长度不足");
        }
        let (nonce_bytes, ct) = sealed.split_at(24);
        self.cipher
            .decrypt(XNonce::from_slice(nonce_bytes), Payload { msg: ct, aad })
            .map_err(|_| anyhow::anyhow!("解密失败：配对码不一致或数据被篡改"))
    }

    /// Seal a JSON-serialisable value.
    pub fn seal_json<T: serde::Serialize>(&self, value: &T, aad: &[u8]) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(value).context("序列化消息失败")?;
        self.seal(&json, aad)
    }

    /// Open and deserialise a JSON payload.
    pub fn open_json<T: serde::de::DeserializeOwned>(
        &self,
        sealed: &[u8],
        aad: &[u8],
    ) -> Result<T> {
        let plain = self.open(sealed, aad)?;
        serde_json::from_slice(&plain).context("解析消息失败")
    }
}

/// Strip separators and upper-case, so codes survive being retyped by hand.
fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Format a code as `XXXX-XXXX-XXXX-XXXX` for display.
pub fn format_invite_code(code: &str) -> String {
    let n = normalise_code(code);
    n.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// Generate a fresh invite code with 16 characters from a 32-symbol alphabet,
/// i.e. 80 bits of entropy.
pub fn generate_invite_code() -> String {
    let mut rng = rand::thread_rng();
    let raw: String = (0..16)
        .map(|_| {
            let i = (rng.next_u32() % CODE_ALPHABET.len() as u32) as usize;
            CODE_ALPHABET[i] as char
        })
        .collect();
    format_invite_code(&raw)
}

/// Random lowercase-hex identifier of `bytes` bytes, used for device and to-do
/// ids.
pub fn random_id(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    data_encoding::HEXLOWER.encode(&buf)
}

/// Fresh 32-byte challenge for the relay handshake.
pub fn random_challenge() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // Argon2id at 64 MiB is deliberately slow; derive once and reuse.
    fn keys(code: &str) -> RoomKeys {
        RoomKeys::derive(code).expect("derive")
    }

    #[test]
    fn separators_and_case_do_not_change_the_room() {
        let a = keys("abcd-efgh-ijkl-mnop");
        let b = keys("ABCD EFGH IJKL MNOP");
        assert_eq!(a.room_id(), b.room_id());
    }

    #[test]
    fn roundtrip_with_matching_aad() {
        let k = keys("test-code-1234567");
        let sealed = k.seal(b"hello", b"device-a").unwrap();
        assert_eq!(k.open(&sealed, b"device-a").unwrap(), b"hello");
    }

    #[test]
    fn aad_mismatch_is_rejected() {
        let k = keys("test-code-1234567");
        let sealed = k.seal(b"hello", b"device-a").unwrap();
        assert!(k.open(&sealed, b"device-b").is_err());
    }

    #[test]
    fn tampering_is_detected() {
        let k = keys("test-code-1234567");
        let mut sealed = k.seal(b"hello", b"aad").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(k.open(&sealed, b"aad").is_err());
    }

    #[test]
    fn nonce_is_fresh_per_message() {
        let k = keys("test-code-1234567");
        let a = k.seal(b"same", b"aad").unwrap();
        let b = k.seal(b"same", b"aad").unwrap();
        assert_ne!(a[..24], b[..24], "nonce must not repeat");
    }

    #[test]
    fn auth_response_verifies_and_rejects() {
        let k = keys("test-code-1234567");
        let other = keys("different-code-99");
        let challenge = random_challenge();
        let resp = k.auth_response(&challenge);
        assert!(k.verify_auth(&challenge, &resp));
        assert!(!other.verify_auth(&challenge, &resp));
        assert!(!k.verify_auth(&random_challenge(), &resp));
    }

    #[test]
    fn short_codes_are_refused() {
        assert!(RoomKeys::derive("ab-cd").is_err());
    }

    #[test]
    fn generated_codes_are_formatted_and_usable() {
        let code = generate_invite_code();
        assert_eq!(code.len(), 19, "16 chars + 3 dashes");
        assert!(RoomKeys::derive(&code).is_ok());
    }
}
