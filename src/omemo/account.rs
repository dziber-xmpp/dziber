use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD as BASE64_NO_PAD;
use vodozemac::olm::{Account, AccountPickle};
use vodozemac::{Curve25519PublicKey, Curve25519SecretKey};
use xeddsa::Sign;
use xeddsa::xed25519::PrivateKey;

/// Thin wrapper around a vodozemac Olm Account.
pub struct OmemoAccount {
    pub inner: Account,
    pub device_id: u32,
}

impl std::fmt::Debug for OmemoAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OmemoAccount")
            .field("device_id", &self.device_id)
            .field("identity_key", &self.inner.curve25519_key())
            .finish()
    }
}

impl OmemoAccount {
    fn parse_u8_array_32(value: &serde_json::Value) -> Option<[u8; 32]> {
        let arr = value.as_array()?;
        if arr.len() != 32 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, v) in arr.iter().enumerate().take(32) {
            out[i] = u8::try_from(v.as_u64()?).ok()?;
        }
        Some(out)
    }

    fn key_id_to_u32(key_id: &str) -> Option<u32> {
        // Newer pickle formats may encode key ids as decimal strings.
        if let Ok(n) = key_id.parse::<u64>() {
            return u32::try_from(n).ok();
        }
        // Older libolm-compatible pickle format uses base64-encoded 8-byte ids.
        let raw = BASE64_NO_PAD.decode(key_id).ok()?;
        if raw.len() != 8 {
            return None;
        }
        let n = u64::from_be_bytes(raw.try_into().ok()?);
        u32::try_from(n).ok()
    }

    fn key_id_value_to_u32(value: &serde_json::Value) -> Option<u32> {
        if let Some(s) = value.as_str() {
            return Self::key_id_to_u32(s);
        }
        if let Some(n) = value.as_u64() {
            return u32::try_from(n).ok();
        }
        None
    }

    pub fn all_stored_one_time_keys(&self) -> Vec<(u32, Curve25519PublicKey)> {
        let pickle = self.inner.pickle();
        let json = match serde_json::to_value(&pickle) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        let Some(private_keys) = json["one_time_keys"]["private_keys"].as_object() else {
            return out;
        };

        for (key_id_b64, secret_val) in private_keys {
            let Some(id) = Self::key_id_to_u32(key_id_b64) else {
                continue;
            };
            let Some(arr) = secret_val.as_array() else {
                continue;
            };
            if arr.len() != 32 {
                continue;
            }
            let mut secret_bytes = [0u8; 32];
            let mut ok = true;
            for (i, v) in arr.iter().enumerate().take(32) {
                if let Some(b) = v.as_u64().and_then(|n| u8::try_from(n).ok()) {
                    secret_bytes[i] = b;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let secret = Curve25519SecretKey::from_slice(&secret_bytes);
            let public = Curve25519PublicKey::from(&secret);
            out.push((id, public));
        }
        out
    }

    pub fn all_stored_one_time_secret_keys(&self) -> Vec<(u32, [u8; 32])> {
        let pickle = self.inner.pickle();
        let json = match serde_json::to_value(&pickle) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        let Some(private_keys) = json["one_time_keys"]["private_keys"].as_object() else {
            return out;
        };
        for (key_id_b64, secret_val) in private_keys {
            let Some(id) = Self::key_id_to_u32(key_id_b64) else {
                continue;
            };
            let Some(secret) = Self::parse_u8_array_32(secret_val) else {
                continue;
            };
            out.push((id, secret));
        }
        out
    }

    pub fn fallback_secret_key_bytes(&self) -> Option<(u32, [u8; 32])> {
        let pickle = self.inner.pickle();
        let json = serde_json::to_value(&pickle).ok()?;
        let current_id =
            Self::key_id_value_to_u32(&json["fallback_keys"]["fallback_key"]["key_id"]);
        let current_secret = Self::parse_u8_array_32(&json["fallback_keys"]["fallback_key"]["key"]);
        if let (Some(id), Some(secret)) = (current_id, current_secret) {
            return Some((id, secret));
        }

        let prev_id =
            Self::key_id_value_to_u32(&json["fallback_keys"]["previous_fallback_key"]["key_id"]);
        let prev_secret =
            Self::parse_u8_array_32(&json["fallback_keys"]["previous_fallback_key"]["key"]);
        if let (Some(id), Some(secret)) = (prev_id, prev_secret) {
            return Some((id, secret));
        }

        None
    }

    pub fn generate(device_id: u32) -> Self {
        let mut inner = Account::new();
        inner.generate_one_time_keys(100);
        inner.generate_fallback_key();
        Self { inner, device_id }
    }

    pub fn all_stored_fallback_keys(&self) -> Vec<(u32, Curve25519PublicKey)> {
        let pickle = self.inner.pickle();
        let json = match serde_json::to_value(&pickle) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for node in [
            &json["fallback_keys"]["fallback_key"],
            &json["fallback_keys"]["previous_fallback_key"],
        ] {
            let Some(id) = Self::key_id_value_to_u32(&node["key_id"]) else {
                continue;
            };
            let Some(secret_bytes) = Self::parse_u8_array_32(&node["key"]) else {
                continue;
            };
            let secret = Curve25519SecretKey::from_slice(&secret_bytes);
            let public = Curve25519PublicKey::from(&secret);
            if !out.iter().any(|(existing_id, _)| *existing_id == id) {
                out.push((id, public));
            }
        }
        out
    }

    /// Sign data with the Curve25519 identity key using XEdDSA.
    /// This produces signatures compatible with libsignal-java (Conversations).
    ///
    /// We extract the DH secret key from vodozemac's pickle serialization
    /// (which is `Serialize`-backed) to avoid vendoring the crate.
    pub fn xeddsa_sign(&self, data: &[u8]) -> Vec<u8> {
        let pickle = self.inner.pickle();
        let json = serde_json::to_value(&pickle).expect("AccountPickle serialization failed");
        let dh_arr = json["diffie_hellman_key"]
            .as_array()
            .expect("diffie_hellman_key missing from pickle");
        let mut secret_bytes = [0u8; 32];
        for (i, v) in dh_arr.iter().enumerate().take(32) {
            secret_bytes[i] = v.as_u64().expect("non-integer byte in pickle") as u8;
        }
        let private_key = PrivateKey::from(&secret_bytes);
        let signature: [u8; 64] = private_key.sign(data, rand::thread_rng());
        signature.to_vec()
    }

    pub fn pickle(&self, key: &[u8; 32]) -> Vec<u8> {
        self.inner.pickle().encrypt(key).into_bytes()
    }

    pub fn unpickle(bytes: &[u8], key: &[u8; 32]) -> Option<Self> {
        let ciphertext = std::str::from_utf8(bytes).ok()?;
        let decrypted = AccountPickle::from_encrypted(ciphertext, key).ok()?;
        Some(Self {
            inner: Account::from_pickle(decrypted),
            device_id: 0,
        })
    }
}
