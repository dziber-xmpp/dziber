use crate::signal_ratchet::keys::{CURVE25519_KEY_LEN, verify_signature};

/// A peer's public pre-key bundle, used to establish an outbound session.
#[derive(Clone, Debug)]
pub struct PreKeyBundle {
    pub registration_id: u32,
    pub device_id: u32,
    pub signed_pre_key_id: u32,
    pub signed_pre_key_public: [u8; CURVE25519_KEY_LEN],
    pub signed_pre_key_signature: [u8; 64],
    pub identity_key: [u8; CURVE25519_KEY_LEN],
    pub pre_key_id: u32,
    pub pre_key_public: [u8; CURVE25519_KEY_LEN],
}

impl PreKeyBundle {
    /// Verify that the signed prekey signature is valid for the bundle's identity key.
    pub fn verify_signature(&self) -> bool {
        let mut to_sign = [0u8; CURVE25519_KEY_LEN + 1];
        to_sign[0] = 0x05;
        to_sign[1..].copy_from_slice(&self.signed_pre_key_public);
        verify_signature(&self.identity_key, &to_sign, &self.signed_pre_key_signature)
    }
}
