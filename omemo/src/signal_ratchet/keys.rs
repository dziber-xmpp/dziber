use rand_core::{CryptoRng, RngCore};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};
use xeddsa::{Sign, Verify};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const CURVE25519_KEY_LEN: usize = 32;
pub const CURVE25519_SIGNATURE_LEN: usize = 64;

/// A Curve25519 key pair usable for X25519 Diffie-Hellman.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct KeyPair {
    secret: [u8; CURVE25519_KEY_LEN],
    public: [u8; CURVE25519_KEY_LEN],
}

impl KeyPair {
    /// Generate a new random key pair.
    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut secret = [0u8; CURVE25519_KEY_LEN];
        rng.fill_bytes(&mut secret);
        Self::from_secret(secret)
    }

    /// Create a key pair from raw secret bytes.
    ///
    /// The public key is derived using the same clamping X25519 applies.
    pub fn from_secret(secret: [u8; CURVE25519_KEY_LEN]) -> Self {
        let static_secret = X25519StaticSecret::from(secret);
        let public = X25519PublicKey::from(&static_secret).to_bytes();
        Self { secret, public }
    }

    pub fn secret_key_bytes(&self) -> &[u8; CURVE25519_KEY_LEN] {
        &self.secret
    }

    pub fn public_key_bytes(&self) -> &[u8; CURVE25519_KEY_LEN] {
        &self.public
    }

    /// Perform an X25519 Diffie-Hellman exchange with `other_public`.
    pub fn diffie_hellman(&self, other_public: &[u8; CURVE25519_KEY_LEN]) -> Option<[u8; CURVE25519_KEY_LEN]> {
        let public = X25519PublicKey::from(*other_public);
        let secret = X25519StaticSecret::from(self.secret);
        let shared = secret.diffie_hellman(&public);
        if shared.was_contributory() {
            Some(*shared.as_bytes())
        } else {
            None
        }
    }
}

impl core::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeyPair")
            .field("public", &hex::encode(self.public))
            .finish_non_exhaustive()
    }
}

/// Serialize a raw Curve25519 public key into the libsignal public-key format.
///
/// Legacy libsignal/Java always prefixes public-key bytes with `0x05`.
pub fn serialize_public_key(key: &[u8; CURVE25519_KEY_LEN]) -> [u8; CURVE25519_KEY_LEN + 1] {
    let mut out = [0u8; CURVE25519_KEY_LEN + 1];
    out[0] = 0x05;
    out[1..].copy_from_slice(key);
    out
}

/// An identity key pair. The private part is also used for XEdDSA signatures.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct IdentityKeyPair {
    inner: KeyPair,
}

impl IdentityKeyPair {
    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        Self { inner: KeyPair::generate(rng) }
    }

    pub fn from_secret(secret: [u8; CURVE25519_KEY_LEN]) -> Self {
        Self { inner: KeyPair::from_secret(secret) }
    }

    pub fn key_pair(&self) -> &KeyPair {
        &self.inner
    }

    pub fn public_key_bytes(&self) -> &[u8; CURVE25519_KEY_LEN] {
        self.inner.public_key_bytes()
    }

    pub fn secret_key_bytes(&self) -> &[u8; CURVE25519_KEY_LEN] {
        self.inner.secret_key_bytes()
    }
}

impl core::fmt::Debug for IdentityKeyPair {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IdentityKeyPair")
            .field("public", &hex::encode(self.inner.public_key_bytes()))
            .finish_non_exhaustive()
    }
}

/// A one-time prekey.
#[derive(Clone, Debug, Zeroize, ZeroizeOnDrop)]
pub struct PreKey {
    pub id: u32,
    pub key_pair: KeyPair,
}

impl PreKey {
    pub fn generate<R: RngCore + CryptoRng>(id: u32, rng: &mut R) -> Self {
        Self { id, key_pair: KeyPair::generate(rng) }
    }
}

/// A signed prekey, including a signature by the identity key.
#[derive(Clone, Debug, Zeroize, ZeroizeOnDrop)]
pub struct SignedPreKey {
    pub id: u32,
    pub key_pair: KeyPair,
    pub signature: [u8; CURVE25519_SIGNATURE_LEN],
}

/// Verify an XEdDSA signature over `message` with the given Curve25519 identity public key.
pub fn verify_signature(
    identity_public: &[u8; CURVE25519_KEY_LEN],
    message: &[u8],
    signature: &[u8; CURVE25519_SIGNATURE_LEN],
) -> bool {
    let public_key = xeddsa::xed25519::PublicKey::from(&X25519PublicKey::from(*identity_public));
    public_key.verify(message, signature).is_ok()
}

impl SignedPreKey {
    /// Generate a signed prekey and sign its serialized public key with `identity`.
    pub fn generate<R: RngCore + CryptoRng>(
        id: u32,
        identity: &IdentityKeyPair,
        rng: &mut R,
    ) -> Self {
        let key_pair = KeyPair::generate(rng);
        let to_sign = serialize_public_key(key_pair.public_key_bytes());
        let private_key = xeddsa::xed25519::PrivateKey::from(identity.secret_key_bytes());
        let signature = private_key.sign(&to_sign, rng);
        Self { id, key_pair, signature }
    }

    /// Verify the signature using the provided identity public key.
    pub fn verify_signature(&self, identity_public: &[u8; CURVE25519_KEY_LEN]) -> bool {
        let to_sign = serialize_public_key(self.key_pair.public_key_bytes());
        verify_signature(identity_public, &to_sign, &self.signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    #[test]
    fn signed_pre_key_signature_roundtrip() {
        let mut rng = thread_rng();
        let identity = IdentityKeyPair::generate(&mut rng);
        let signed_pre_key = SignedPreKey::generate(42, &identity, &mut rng);
        assert!(signed_pre_key.verify_signature(identity.public_key_bytes()));
    }

    #[test]
    fn diffie_hellman_is_symmetric() {
        let mut rng = thread_rng();
        let a = KeyPair::generate(&mut rng);
        let b = KeyPair::generate(&mut rng);
        let ab = a.diffie_hellman(b.public_key_bytes()).unwrap();
        let ba = b.diffie_hellman(a.public_key_bytes()).unwrap();
        assert_eq!(ab, ba);
    }
}
