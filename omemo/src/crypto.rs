use aes::Aes256;
use aes_gcm::{
    Aes128Gcm, Key as AesGcmKey, Nonce as AesGcmNonce,
    aead::{Aead, KeyInit},
};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
#[cfg(test)]
use cbc::cipher::BlockEncryptMut;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// OMEMO v0 payload decryption: AES-256-CBC + truncated HMAC-SHA-256.
///
/// `key` is the 32-byte random key recovered from the Double Ratchet.
/// `expected_hmac` is the 16-byte truncated HMAC recovered alongside the key.
pub fn decrypt_payload_v0(
    ciphertext: &[u8],
    key: &[u8; 32],
    expected_hmac: &[u8; 16],
) -> Option<Vec<u8>> {
    let (enc_key, auth_key, iv) = derive_keys_v0(key);

    let mut mac = <Hmac<Sha256> as hmac::Mac>::new_from_slice(&auth_key).ok()?;
    mac.update(ciphertext);
    let tag = mac.finalize().into_bytes();
    if &tag[..16] != expected_hmac {
        return None;
    }

    let mut buf = vec![0u8; ciphertext.len()];
    let dec = cbc::Decryptor::<Aes256>::new(&enc_key.into(), &iv.into());
    let pt_len = dec
        .decrypt_padded_b2b_mut::<Pkcs7>(ciphertext, &mut buf)
        .ok()?
        .len();
    buf.truncate(pt_len);

    Some(buf)
}

/// OMEMO v0 Conversations-style payload decryption: AES-128-GCM.
///
/// `ciphertext` is the payload bytes without the authentication tag.
/// `key` is the 16-byte AES key recovered from the Double Ratchet plaintext.
/// `auth_tag` is the 16-byte GCM tag recovered from the Double Ratchet plaintext.
/// `nonce` is the 12-byte IV from the OMEMO `<iv/>` element.
pub fn decrypt_payload_v0_conversations(
    ciphertext: &[u8],
    key: &[u8; 16],
    auth_tag: &[u8; 16],
    nonce: &[u8; 12],
) -> Option<Vec<u8>> {
    let mut ct = Vec::with_capacity(ciphertext.len() + auth_tag.len());
    ct.extend_from_slice(ciphertext);
    ct.extend_from_slice(auth_tag);
    let key = AesGcmKey::<Aes128Gcm>::from_slice(key);
    let cipher = Aes128Gcm::new(key);
    let nonce = AesGcmNonce::from_slice(nonce);
    cipher.decrypt(nonce, ct.as_slice()).ok()
}

/// OMEMO v0 Conversations-style payload encryption: AES-128-GCM.
///
/// Returns `(ciphertext_without_tag, key, auth_tag, nonce)` where:
/// - `ciphertext_without_tag` is the AES-128-GCM ciphertext without the tag
/// - `key` is the 16-byte AES key
/// - `auth_tag` is the 16-byte GCM authentication tag
/// - `nonce` is the 12-byte IV
pub fn encrypt_payload_v0_conversations(
    plaintext: &[u8],
) -> (Vec<u8>, [u8; 16], [u8; 16], [u8; 12]) {
    let mut key_bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_bytes);
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);

    let key = AesGcmKey::<Aes128Gcm>::from_slice(&key_bytes);
    let cipher = Aes128Gcm::new(key);
    let nonce = AesGcmNonce::from_slice(&nonce_bytes);

    let ciphertext_with_tag = cipher
        .encrypt(nonce, plaintext)
        .expect("AES-128-GCM encryption failed");
    let ciphertext_len = ciphertext_with_tag.len() - 16;
    let mut ciphertext = vec![0u8; ciphertext_len];
    ciphertext.copy_from_slice(&ciphertext_with_tag[..ciphertext_len]);
    let mut auth_tag = [0u8; 16];
    auth_tag.copy_from_slice(&ciphertext_with_tag[ciphertext_len..]);

    (ciphertext, key_bytes, auth_tag, nonce_bytes)
}

/// Derive encryption key, authentication key, and IV from the 32-byte payload key (v0 only).
pub(crate) fn derive_keys_v0(key: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 16]) {
    let hkdf = Hkdf::<Sha256>::new(Some(&[0u8; 32]), key);
    let mut okm = [0u8; 80];
    hkdf.expand(b"OMEMO Payload", &mut okm).unwrap();

    let mut enc_key = [0u8; 32];
    let mut auth_key = [0u8; 32];
    let mut iv = [0u8; 16];
    enc_key.copy_from_slice(&okm[0..32]);
    auth_key.copy_from_slice(&okm[32..64]);
    iv.copy_from_slice(&okm[64..80]);

    (enc_key, auth_key, iv)
}

/// OMEMO v0 payload encryption: AES-256-CBC + truncated HMAC-SHA-256.
///
/// Returns `(ciphertext, key, hmac)` where `key` is the random 32-byte payload key
/// and `hmac` is the 16-byte truncated authentication tag.
#[cfg(test)]
pub fn encrypt_payload_v0(plaintext: &[u8]) -> (Vec<u8>, [u8; 32], [u8; 16]) {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);

    let (enc_key, auth_key, iv) = derive_keys_v0(&key);

    let mut buf = vec![0u8; plaintext.len() + 32];
    let enc = cbc::Encryptor::<Aes256>::new(&enc_key.into(), &iv.into());
    let ct = enc
        .encrypt_padded_b2b_mut::<Pkcs7>(plaintext, &mut buf)
        .expect("AES-256-CBC encryption failed");
    let ciphertext = ct.to_vec();

    let mut mac = <Hmac<Sha256> as hmac::Mac>::new_from_slice(&auth_key)
        .expect("HMAC-SHA256 key creation failed");
    mac.update(&ciphertext);
    let tag = mac.finalize().into_bytes();

    let mut hmac = [0u8; 16];
    hmac.copy_from_slice(&tag[..16]);

    (ciphertext, key, hmac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_keys_v0_is_deterministic() {
        let key = [0xA5u8; 32];
        let a = derive_keys_v0(&key);
        let b = derive_keys_v0(&key);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
        assert_eq!(a.2, b.2);
    }

    #[test]
    fn encrypt_payload_v0_roundtrip() {
        let plaintext = b"OMEMO v0 CBC payload";
        let (ciphertext, key, hmac) = encrypt_payload_v0(plaintext);
        let decrypted = decrypt_payload_v0(&ciphertext, &key, &hmac).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());

        let mut bad_hmac = hmac;
        bad_hmac[0] ^= 0xFF;
        assert!(decrypt_payload_v0(&ciphertext, &key, &bad_hmac).is_none());
    }

    #[test]
    fn encrypt_payload_v0_conversations_roundtrip() {
        let plaintext = b"conversations style payload";
        let (ciphertext, key, auth_tag, nonce) = encrypt_payload_v0_conversations(plaintext);
        assert_eq!(key.len(), 16);
        assert_eq!(auth_tag.len(), 16);
        assert_eq!(nonce.len(), 12);

        let decrypted =
            decrypt_payload_v0_conversations(&ciphertext, &key, &auth_tag, &nonce).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }
}
