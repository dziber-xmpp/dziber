use anyhow::{Result, anyhow};
use vodozemac::Curve25519PublicKey;
use vodozemac::olm::SessionCreationError;
use vodozemac::olm::{
    Account, InboundCreationResult, OlmMessage, Session, SessionConfig, SessionPickle,
};

/// Create an outbound Olm session (X3DH initiator).
pub fn create_outbound_session(
    account: &Account,
    their_identity_key: Curve25519PublicKey,
    their_one_time_key: Curve25519PublicKey,
) -> Session {
    account
        .create_outbound_session(
            SessionConfig::version_1(),
            their_identity_key,
            their_one_time_key,
        )
        .expect("outbound session creation failed")
}

/// Create an inbound Olm session (X3DH responder) from a PreKeyMessage.
pub fn create_inbound_session(
    account: &mut Account,
    their_identity_key: Curve25519PublicKey,
    pre_key_message: &vodozemac::olm::PreKeyMessage,
) -> std::result::Result<InboundCreationResult, SessionCreationError> {
    account.create_inbound_session(
        SessionConfig::version_1(),
        their_identity_key,
        pre_key_message,
    )
}

/// Encrypt plaintext with an Olm session.
pub fn encrypt(session: &mut Session, plaintext: &[u8]) -> OlmMessage {
    session.encrypt(plaintext).expect("olm encrypt failed")
}

/// Decrypt an OlmMessage.
pub fn decrypt(session: &mut Session, message: &OlmMessage) -> Result<Vec<u8>> {
    session
        .decrypt(message)
        .map_err(|e| anyhow!("Olm decrypt failed: {e}"))
}

/// Serialize a Session to encrypted bytes.
pub fn pickle_session(session: &Session, key: &[u8; 32]) -> Vec<u8> {
    session.pickle().encrypt(key).into_bytes()
}

/// Deserialize a Session from encrypted bytes.
pub fn unpickle_session(bytes: &[u8], key: &[u8; 32]) -> Option<Session> {
    let ciphertext = std::str::from_utf8(bytes).ok()?;
    let decrypted = SessionPickle::from_encrypted(ciphertext, key).ok()?;
    Some(Session::from_pickle(decrypted))
}
