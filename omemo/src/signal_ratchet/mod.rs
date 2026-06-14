//! Pure-Rust implementation of the legacy libsignal v3 protocol used by
//! OMEMO v0 (`eu.siacs.conversations.axolotl`).
//!
//! This module is intended to be extracted into an independent crate once the
//! API stabilizes. It intentionally avoids any dependency on dziber-specific
//! types so the move is mechanical.

pub mod bundle;
pub mod crypto;
pub mod keys;
pub mod proto;
pub mod session;

pub use bundle::PreKeyBundle;
pub use keys::{IdentityKeyPair, KeyPair, PreKey, SignedPreKey, CURVE25519_KEY_LEN, serialize_public_key, verify_signature};
pub use session::{CiphertextMessage, Session};

/// Errors returned by the legacy signal ratchet.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("protobuf error: {0}")]
    Proto(String),
    #[error("unsupported ciphertext version: 0x{0:02x}")]
    UnsupportedVersion(u8),
    #[error("invalid signature on signed prekey")]
    InvalidSignature,
    #[error("non-contributory DH key")]
    NonContributoryKey,
    #[error("bad message authentication code")]
    BadMac,
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("duplicate message")]
    DuplicateMessage,
    #[error("message gap too large: {0}")]
    MessageGapTooLarge(u32),
    #[error("no sending chain available")]
    NoSendingChain,
    #[error("missing local ratchet key")]
    MissingLocalKey,
    #[error("missing remote ratchet key")]
    MissingRemoteKey,
    #[error("missing remote identity")]
    MissingRemoteIdentity,
    #[error("invalid message: {0}")]
    InvalidMessage(String),
    #[error("invalid pickle format")]
    InvalidPickle,
}
