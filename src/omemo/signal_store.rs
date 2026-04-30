use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wa_rs_libsignal::protocol::error::Result as SignalResult;
use wa_rs_libsignal::protocol::{
    Direction, GenericSignedPreKey, IdentityChange, IdentityKey, IdentityKeyPair, IdentityKeyStore,
    KeyPair, PreKeyId, PreKeyRecord, PreKeyStore, ProtocolAddress, SessionRecord, SessionStore,
    SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore, SignalProtocolError, Timestamp,
};

use super::account::OmemoAccount;

#[derive(Clone)]
pub struct SignalStore {
    inner: Arc<Mutex<SignalState>>,
}

#[derive(Clone)]
struct SignalState {
    identity: IdentityKeyPair,
    registration_id: u32,
    signed_prekey_id: SignedPreKeyId,
    signed_prekey: SignedPreKeyRecord,
    signed_prekeys: HashMap<SignedPreKeyId, SignedPreKeyRecord>,
    prekeys: HashMap<PreKeyId, PreKeyRecord>,
    sessions: HashMap<String, SessionRecord>,
    trusted: HashMap<String, IdentityKey>,
}

impl SignalStore {
    pub fn has_session_for(&self, name: &str, device_id: u32) -> bool {
        let key = ProtocolAddress::new(name.to_string(), device_id.into()).to_string();
        self.inner
            .lock()
            .expect("signal store poisoned")
            .sessions
            .contains_key(&key)
    }

    pub fn from_omemo_account_with_signed_prekey_secret(
        account: &OmemoAccount,
        forced_signed_prekey_secret: Option<[u8; 32]>,
    ) -> Option<Self> {
        let identity_secret = match account.identity_secret_key_bytes() { Some(v) => v, None => { tracing::info!("[signal_store] missing identity_secret_key_bytes"); return None; } };
        let identity_private = match Self::deserialize_private_tolerant(&identity_secret) { Some(v) => v, None => { tracing::info!("[signal_store] identity private deserialize failed"); return None; } };
        let identity = IdentityKeyPair::try_from(identity_private).ok()?;

        let (signed_prekey_id_u32, signed_prekey_secret) = if let Some(v) = forced_signed_prekey_secret {
            let spk_id = account
                .all_stored_fallback_secret_keys()
                .into_iter()
                .find_map(|(id, sec)| if sec == v { Some(id) } else { None })
                .unwrap_or(1);
            (spk_id, v)
        } else if let Some((id, sec)) = account.fallback_secret_key_bytes() {
            (id, sec)
        } else {
            let mut otk = account.all_stored_one_time_secret_keys();
            otk.sort_by_key(|(id, _)| *id);
            if let Some((id, sec)) = otk.iter().find(|(id, _)| *id == 1).copied().or_else(|| otk.first().copied()) {
                tracing::info!("[signal_store] fallback missing; using one-time prekey secret as signed-prekey substitute");
                (id, sec)
            } else {
                tracing::info!("[signal_store] missing fallback and no one-time secret keys");
                return None;
            }
        };
        let signed_prekey_private = match Self::deserialize_private_tolerant(&signed_prekey_secret) { Some(v) => v, None => { tracing::info!("[signal_store] signed prekey deserialize failed"); return None; } };
        let signed_prekey_public = signed_prekey_private.public_key().ok()?;
        let signed_prekey_keypair = KeyPair::new(signed_prekey_public, signed_prekey_private);
        let signed_prekey_sig = account.xeddsa_sign(&signed_prekey_keypair.public_key.serialize());
        let signed_prekey_id = SignedPreKeyId::from(signed_prekey_id_u32);
        let signed_prekey = SignedPreKeyRecord::new(
            signed_prekey_id,
            Timestamp::from_epoch_millis(chrono::Utc::now().timestamp_millis() as u64),
            &signed_prekey_keypair,
            &signed_prekey_sig,
        );
        let mut signed_prekeys = HashMap::new();
        signed_prekeys.insert(signed_prekey_id, signed_prekey.clone());
        // Compatibility: Conversations may still encrypt to stale signed-prekey ids
        // for this device. Expose records for all known local key ids so libsignal
        // can pick by requested id instead of failing early.
        for (id_u32, sec) in account.all_stored_fallback_secret_keys() {
            let id = SignedPreKeyId::from(id_u32);
            if signed_prekeys.contains_key(&id) {
                continue;
            }
            let Some(privk) = Self::deserialize_private_tolerant(&sec) else {
                continue;
            };
            let Ok(pubk) = privk.public_key() else {
                continue;
            };
            let kp = KeyPair::new(pubk, privk);
            let sig = account.xeddsa_sign(&kp.public_key.serialize());
            let rec = SignedPreKeyRecord::new(
                id,
                Timestamp::from_epoch_millis(chrono::Utc::now().timestamp_millis() as u64),
                &kp,
                &sig,
            );
            signed_prekeys.insert(id, rec);
        }

        let mut keyed = Vec::new();
        for (id_u32, secret) in account.all_stored_one_time_secret_keys() {
            if id_u32 == 0 {
                continue;
            }
            let private = match Self::deserialize_private_tolerant(&secret) {
                Some(v) => v,
                None => continue,
            };
            let public = match private.public_key() {
                Ok(v) => v,
                Err(_) => continue,
            };
            keyed.push((id_u32, public.serialize(), KeyPair::new(public, private)));
        }
        keyed.sort_by_key(|(id, _, _)| *id);

        let mut prekeys = HashMap::new();
        for (id_u32, _pk, keypair) in keyed.into_iter().take(100) {
            let id = PreKeyId::from(id_u32);
            prekeys.insert(id, PreKeyRecord::new(id, &keypair));
        }

        let registration_id = {
            let mut v = u16::from_le_bytes([identity_secret[0], identity_secret[1]]) as u32;
            v = (v % 16380).max(1);
            v
        };

        Some(Self {
            inner: Arc::new(Mutex::new(SignalState {
                identity,
                registration_id,
                signed_prekey_id,
                signed_prekey,
                signed_prekeys,
                prekeys,
                sessions: {
                    let mut out = HashMap::new();
                    if let Ok(raw) = crate::db::omemo::load_signal_sessions_blob() {
                        for (addr, bytes) in raw {
                            if let Ok(rec) = SessionRecord::deserialize(&bytes) {
                                out.insert(addr, rec);
                            }
                        }
                    }
                    out
                },
                trusted: HashMap::new(),
            })),
        })
    }

    fn deserialize_private_tolerant(secret: &[u8; 32]) -> Option<wa_rs_libsignal::protocol::PrivateKey> {
        if let Ok(v) = wa_rs_libsignal::protocol::PrivateKey::deserialize(secret) {
            return Some(v);
        }
        let mut pref = [0u8; 33];
        pref[0] = 0x05;
        pref[1..].copy_from_slice(secret);
        wa_rs_libsignal::protocol::PrivateKey::deserialize(&pref).ok()
    }

    pub fn from_omemo_account(account: &OmemoAccount) -> Option<Self> {
        Self::from_omemo_account_with_signed_prekey_secret(account, None)
    }

    pub fn from_omemo_account_with_previous_fallback(account: &OmemoAccount) -> Option<Self> {
        let all = account.all_stored_fallback_secret_keys();
        if all.len() < 2 {
            return None;
        }
        let current = account.fallback_secret_key_bytes().map(|(_, s)| s);
        let prev = all
            .into_iter()
            .map(|(_, s)| s)
            .find(|s| Some(*s) != current)?;
        Self::from_omemo_account_with_signed_prekey_secret(account, Some(prev))
    }

    pub fn signed_prekey_id(&self) -> u32 {
        self.inner
            .lock()
            .expect("signal store poisoned")
            .signed_prekey_id
            .into()
    }

    pub fn prekey_ids_sorted(&self) -> Vec<u32> {
        let guard = self.inner.lock().expect("signal store poisoned");
        let mut ids = guard
            .prekeys
            .keys()
            .map(|id| {
                let x: u32 = (*id).into();
                x
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

}

#[async_trait::async_trait]
impl IdentityKeyStore for SignalStore {
    async fn get_identity_key_pair(&self) -> SignalResult<IdentityKeyPair> {
        Ok(self.inner.lock().expect("signal store poisoned").identity.clone())
    }

    async fn get_local_registration_id(&self) -> SignalResult<u32> {
        Ok(self.inner.lock().expect("signal store poisoned").registration_id)
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> SignalResult<IdentityChange> {
        let mut guard = self.inner.lock().expect("signal store poisoned");
        let key = address.to_string();
        let changed = guard.trusted.get(&key).is_some_and(|existing| existing != identity);
        guard.trusted.insert(key, *identity);
        Ok(IdentityChange::from_changed(changed))
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> SignalResult<bool> {
        let guard = self.inner.lock().expect("signal store poisoned");
        let key = address.to_string();
        Ok(guard
            .trusted
            .get(&key)
            .map(|known| known == identity)
            .unwrap_or(true))
    }

    async fn get_identity(&self, address: &ProtocolAddress) -> SignalResult<Option<IdentityKey>> {
        Ok(self
            .inner
            .lock()
            .expect("signal store poisoned")
            .trusted
            .get(&address.to_string())
            .copied())
    }
}

#[async_trait::async_trait]
impl PreKeyStore for SignalStore {
    async fn get_pre_key(&self, prekey_id: PreKeyId) -> SignalResult<PreKeyRecord> {
        let guard = self.inner.lock().expect("signal store poisoned");
        if let Some(rec) = guard.prekeys.get(&prekey_id).cloned() {
            return Ok(rec);
        }

        let req: u32 = prekey_id.into();
        if req == 0
            && let Some((fallback_id, rec)) = guard.prekeys.iter().min_by_key(|(id, _)| {
                let v: u32 = (**id).into();
                v
            })
        {
            let fid: u32 = (*fallback_id).into();
            tracing::info!(
                "[OMEMO signal] prekey id=0 requested; falling back to smallest local prekey id={}",
                fid
            );
            return Ok(rec.clone());
        }

        Err(SignalProtocolError::InvalidPreKeyId)
    }

    async fn save_pre_key(&mut self, prekey_id: PreKeyId, record: &PreKeyRecord) -> SignalResult<()> {
        self.inner
            .lock()
            .expect("signal store poisoned")
            .prekeys
            .insert(prekey_id, record.clone());
        Ok(())
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> SignalResult<()> {
        self.inner.lock().expect("signal store poisoned").prekeys.remove(&prekey_id);
        Ok(())
    }
}

#[async_trait::async_trait]
impl SignedPreKeyStore for SignalStore {
    async fn get_signed_pre_key(&self, signed_prekey_id: SignedPreKeyId) -> SignalResult<SignedPreKeyRecord> {
        let guard = self.inner.lock().expect("signal store poisoned");
        if let Some(rec) = guard.signed_prekeys.get(&signed_prekey_id) {
            return Ok(rec.clone());
        }
        let req: u32 = signed_prekey_id.into();
        let have: u32 = guard.signed_prekey_id.into();
        tracing::info!(
            "[OMEMO signal] signed-prekey id not found requested={} local={} candidates={}; using local",
            req,
            have,
            guard.signed_prekeys.len()
        );
        Ok(guard.signed_prekey.clone())
    }

    async fn save_signed_pre_key(
        &mut self,
        signed_prekey_id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> SignalResult<()> {
        let mut guard = self.inner.lock().expect("signal store poisoned");
        guard.signed_prekey_id = signed_prekey_id;
        guard.signed_prekey = record.clone();
        guard.signed_prekeys.insert(signed_prekey_id, record.clone());
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionStore for SignalStore {
    async fn load_session(&self, address: &ProtocolAddress) -> SignalResult<Option<SessionRecord>> {
        let key = address.to_string();
        let guard = self.inner.lock().expect("signal store poisoned");
        if let Some(rec) = guard.sessions.get(&key) {
            return Ok(Some(rec.clone()));
        }

        // Compatibility fallback: allow JID resource/bare mismatches for the same
        // device id key suffix (e.g. "user@host/res.123" vs "user@host.123").
        let Some(dot) = key.rfind('.') else {
            return Ok(None);
        };
        let req_name = &key[..dot];
        let req_dev = &key[dot + 1..];
        let req_bare = req_name.split('/').next().unwrap_or(req_name);
        for (k, rec) in &guard.sessions {
            let Some(kdot) = k.rfind('.') else {
                continue;
            };
            if &k[kdot + 1..] != req_dev {
                continue;
            }
            let k_name = &k[..kdot];
            let k_bare = k_name.split('/').next().unwrap_or(k_name);
            if k_bare == req_bare {
                tracing::info!(
                    "[OMEMO signal] load_session fallback hit req={} matched={}",
                    key, k
                );
                return Ok(Some(rec.clone()));
            }
        }
        Ok(None)
    }

    async fn store_session(&mut self, address: &ProtocolAddress, record: &SessionRecord) -> SignalResult<()> {
        let mut guard = self.inner.lock().expect("signal store poisoned");
        guard
            .sessions
            .insert(address.to_string(), record.clone());
        let mut raw = HashMap::new();
        for (addr, rec) in &guard.sessions {
            if let Ok(bytes) = rec.serialize() {
                raw.insert(addr.clone(), bytes);
            }
        }
        let _ = crate::db::omemo::save_signal_sessions_blob(&raw);
        Ok(())
    }
}
