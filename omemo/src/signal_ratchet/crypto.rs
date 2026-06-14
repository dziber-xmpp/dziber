use aes::cipher::{BlockEncryptMut, BlockDecryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::signal_ratchet::keys::{CURVE25519_KEY_LEN, IdentityKeyPair, KeyPair};

pub const DISCONTINUITY_BYTES: [u8; 32] = [0xFF; 32];
pub const WHISPER_TEXT: &[u8] = b"WhisperText";
pub const WHISPER_RATCHET: &[u8] = b"WhisperRatchet";
pub const WHISPER_MESSAGE_KEYS: &[u8] = b"WhisperMessageKeys";

pub type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
pub type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// Derive bytes using HKDF-SHA256.
pub fn hkdf_sha256(
    salt: Option<&[u8]>,
    ikm: &[u8],
    info: &[u8],
    out_len: usize,
) -> Zeroizing<Vec<u8>> {
    let hk = hkdf::Hkdf::<Sha256>::new(salt, ikm);
    let mut out = Zeroizing::new(vec![0u8; out_len]);
    hk.expand(info, &mut out).expect("valid output length");
    out
}

/// Compute HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Encrypt using AES-256-CBC with PKCS#7 padding.
pub fn aes_cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256CbcEnc::new(key.into(), iv.into());
    cipher.encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(plaintext)
}

/// Decrypt using AES-256-CBC with PKCS#7 padding.
pub fn aes_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let cipher = Aes256CbcDec::new(key.into(), iv.into());
    cipher.decrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(ciphertext).ok()
}

/// Derive a new root key and chain key from a root key and a DH output.
pub fn kdf_root(
    root_key: &[u8; 32],
    dh_output: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    let okm = hkdf_sha256(Some(root_key), dh_output, WHISPER_RATCHET, 64);
    let mut new_root = [0u8; 32];
    let mut chain_key = [0u8; 32];
    new_root.copy_from_slice(&okm[..32]);
    chain_key.copy_from_slice(&okm[32..64]);
    (new_root, chain_key)
}

/// Advance a chain key and produce a message key.
pub fn kdf_chain(chain_key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let message_key = hmac_sha256(chain_key, &[0x01]);
    let next_chain_key = hmac_sha256(chain_key, &[0x02]);
    (next_chain_key, message_key)
}

/// Expand a 32-byte message key into cipher key, MAC key, and IV.
pub fn derive_message_keys(message_key: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 16]) {
    let okm = hkdf_sha256(None, message_key, WHISPER_MESSAGE_KEYS, 80);
    let mut cipher_key = [0u8; 32];
    let mut mac_key = [0u8; 32];
    let mut iv = [0u8; 16];
    cipher_key.copy_from_slice(&okm[..32]);
    mac_key.copy_from_slice(&okm[32..64]);
    iv.copy_from_slice(&okm[64..80]);
    (cipher_key, mac_key, iv)
}

/// Compute the libsignal v3 message authentication code.
///
/// `message` is the byte string `version_byte || protobuf`.
pub fn compute_mac(
    mac_key: &[u8; 32],
    sender_identity: &[u8; CURVE25519_KEY_LEN + 1],
    receiver_identity: &[u8; CURVE25519_KEY_LEN + 1],
    message: &[u8],
) -> [u8; 8] {
    let mut mac = Hmac::<Sha256>::new_from_slice(mac_key).expect("valid key size");
    mac.update(sender_identity);
    mac.update(receiver_identity);
    mac.update(message);
    let result = mac.finalize().into_bytes();
    let mut truncated = [0u8; 8];
    truncated.copy_from_slice(&result[..8]);
    truncated
}

/// Build the master secret for an X3DH handshake from the initiator (Alice) side.
pub fn x3dh_master_secret_alice(
    our_identity: &IdentityKeyPair,
    our_base_key: &KeyPair,
    their_signed_pre_key: &[u8; CURVE25519_KEY_LEN],
    their_identity_key: &[u8; CURVE25519_KEY_LEN],
    their_one_time_pre_key: &[u8; CURVE25519_KEY_LEN],
) -> Option<Zeroizing<Vec<u8>>> {
    let dh1 = our_identity.key_pair().diffie_hellman(their_signed_pre_key)?;
    let dh2 = our_base_key.diffie_hellman(their_identity_key)?;
    let dh3 = our_base_key.diffie_hellman(their_signed_pre_key)?;
    let dh4 = our_base_key.diffie_hellman(their_one_time_pre_key)?;

    let mut secret = Zeroizing::new(Vec::with_capacity(160));
    secret.extend_from_slice(&DISCONTINUITY_BYTES);
    secret.extend_from_slice(&dh1);
    secret.extend_from_slice(&dh2);
    secret.extend_from_slice(&dh3);
    secret.extend_from_slice(&dh4);
    Some(secret)
}

/// Build the master secret for an X3DH handshake from the responder (Bob) side.
pub fn x3dh_master_secret_bob(
    our_identity: &IdentityKeyPair,
    our_signed_pre_key: &KeyPair,
    our_one_time_pre_key: &KeyPair,
    their_identity_key: &[u8; CURVE25519_KEY_LEN],
    their_base_key: &[u8; CURVE25519_KEY_LEN],
) -> Option<Zeroizing<Vec<u8>>> {
    let dh1 = our_signed_pre_key.diffie_hellman(their_identity_key)?;
    let dh2 = our_identity.key_pair().diffie_hellman(their_base_key)?;
    let dh3 = our_signed_pre_key.diffie_hellman(their_base_key)?;
    let dh4 = our_one_time_pre_key.diffie_hellman(their_base_key)?;

    let mut secret = Zeroizing::new(Vec::with_capacity(160));
    secret.extend_from_slice(&DISCONTINUITY_BYTES);
    secret.extend_from_slice(&dh1);
    secret.extend_from_slice(&dh2);
    secret.extend_from_slice(&dh3);
    secret.extend_from_slice(&dh4);
    Some(secret)
}

/// Derive the initial root key and chain key from an X3DH master secret.
pub fn derive_initial_root_and_chain(master_secret: &[u8]) -> ([u8; 32], [u8; 32]) {
    let salt = [0u8; 32];
    let okm = hkdf_sha256(Some(&salt), master_secret, WHISPER_TEXT, 64);
    let mut root = [0u8; 32];
    let mut chain = [0u8; 32];
    root.copy_from_slice(&okm[..32]);
    chain.copy_from_slice(&okm[32..64]);
    (root, chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    #[test]
    fn x3dh_secrets_match() {
        let mut rng = thread_rng();
        let alice_identity = IdentityKeyPair::generate(&mut rng);
        let alice_base = KeyPair::generate(&mut rng);
        let bob_identity = IdentityKeyPair::generate(&mut rng);
        let bob_signed_pre = KeyPair::generate(&mut rng);
        let bob_one_time = KeyPair::generate(&mut rng);

        let alice_secret = x3dh_master_secret_alice(
            &alice_identity,
            &alice_base,
            bob_signed_pre.public_key_bytes(),
            bob_identity.public_key_bytes(),
            bob_one_time.public_key_bytes(),
        )
        .unwrap();

        let bob_secret = x3dh_master_secret_bob(
            &bob_identity,
            &bob_signed_pre,
            &bob_one_time,
            alice_identity.public_key_bytes(),
            alice_base.public_key_bytes(),
        )
        .unwrap();

        assert_eq!(alice_secret.as_slice(), bob_secret.as_slice());
    }
}
