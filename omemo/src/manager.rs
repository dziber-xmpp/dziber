use std::collections::{HashMap, HashSet};

use crate::account::OmemoAccount;
use crate::bundle::Bundle;
use crate::crypto;
use crate::message::{EncryptedMessage, KeysGroup, MessageHeader, MessageKey};
use crate::signal_ratchet;
use crate::store::OmemoStore;
use crate::trust::TrustStore;

/// Cached bundle info for peers (used during inbound session creation).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedBundle {
    pub identity_key: [u8; 32],
}

/// Strip a leading `0x05` libsignal public-key prefix if present.
fn raw_key(data: &[u8]) -> Option<[u8; 32]> {
    if data.len() == 32 {
        data.try_into().ok()
    } else if data.len() == 33 && data[0] == 0x05 {
        data[1..].try_into().ok()
    } else {
        None
    }
}

/// Build a libsignal `PreKeyBundle` from the legacy `Bundle` wire format.
fn legacy_bundle_from_bundle(bundle: &Bundle) -> Option<signal_ratchet::bundle::PreKeyBundle> {
    let identity_key = raw_key(&bundle.ik)?;
    let signed_pre_key_public = raw_key(&bundle.spk)?;
    let signed_pre_key_signature: [u8; 64] = bundle.spks.as_slice().try_into().ok()?;
    let (pre_key_id, pre_key_data) = bundle.prekeys.first()?;
    let pre_key_public = raw_key(pre_key_data)?;
    Some(signal_ratchet::bundle::PreKeyBundle {
        registration_id: 0,
        device_id: bundle.device_id,
        signed_pre_key_id: bundle.spk_id,
        signed_pre_key_public,
        signed_pre_key_signature,
        identity_key,
        pre_key_id: *pre_key_id,
        pre_key_public,
    })
}

pub struct OmemoManager {
    pub account: OmemoAccount,
    pub our_jid: Option<String>,
    pub legacy_sessions: HashMap<String, HashMap<u32, signal_ratchet::Session>>,
    pub device_lists: HashMap<String, Vec<u32>>,
    pub bundle_cache: HashMap<String, HashMap<u32, CachedBundle>>,
    pub trust_store: TrustStore,
    /// JIDs for which only v0 bundles were found (send v0 messages to these).
    pub v0_jids: HashSet<String>,
    pickle_key: [u8; 32],
    store: Box<dyn OmemoStore>,
}

impl OmemoManager {
    fn ensure_account_material(account: &mut OmemoAccount) -> bool {
        let mut changed = false;
        if account.fallback_secret_key_bytes().is_none() {
            tracing::info!("[OMEMO] Missing fallback key in account; generating one");
            account.inner.generate_fallback_key();
            changed = true;
        }
        if account.all_stored_one_time_keys().is_empty() {
            tracing::info!("[OMEMO] Missing one-time keys in account; generating 100");
            account.inner.generate_one_time_keys(100);
            changed = true;
        }
        changed
    }

    pub const MAX_COMPAT_DEVICE_ID: u32 = i32::MAX as u32;

    pub fn generate_compatible_device_id() -> u32 {
        let mut id: u32 = rand::random::<u32>() & Self::MAX_COMPAT_DEVICE_ID;
        if id == 0 {
            id = 1;
        }
        id
    }

    pub fn generate(device_id: u32, store: Box<dyn OmemoStore>) -> Self {
        let pickle_key = store.load_or_generate_pickle_key();
        let mut account = OmemoAccount::generate(device_id);
        let _ = Self::ensure_account_material(&mut account);
        let _ = store.save_account(&account, &pickle_key);
        Self {
            account,
            our_jid: None,
            legacy_sessions: HashMap::new(),
            device_lists: HashMap::new(),
            bundle_cache: HashMap::new(),
            trust_store: TrustStore::new(),
            v0_jids: HashSet::new(),
            pickle_key,
            store,
        }
    }

    pub fn load_or_generate(device_id: u32, store: Box<dyn OmemoStore>) -> Self {
        let pickle_key = store.load_or_generate_pickle_key();
        if let Ok(Some(mut account)) = store.load_account(&pickle_key) {
            let changed = Self::ensure_account_material(&mut account);
            if changed {
                // If OMEMO key material had to be regenerated, rotate device id as well.
                // This avoids stale remote caches encrypting to old prekey ids for the same device.
                let new_id = Self::generate_compatible_device_id();
                tracing::info!(
                    "[OMEMO] Key material regenerated; rotating device id {} -> {}",
                    account.device_id,
                    new_id
                );
                account.device_id = new_id;
                let _ = store.save_account(&account, &pickle_key);
            }
            if account.device_id == 0 || account.device_id > Self::MAX_COMPAT_DEVICE_ID {
                tracing::info!(
                    "[OMEMO] Incompatible device id {} detected; regenerating OMEMO state with compatible 31-bit device id",
                    account.device_id
                );
                return Self::generate(Self::generate_compatible_device_id(), store);
            }
            let legacy_sessions =
                store.load_legacy_sessions(&pickle_key).unwrap_or_default();
            let device_lists = store.load_device_lists().unwrap_or_default();
            let bundle_cache = store.load_bundle_cache().unwrap_or_default();
            let trust_store = store.load_trust_store().unwrap_or_default();
            return Self {
                account,
                our_jid: None,
                device_lists,
                bundle_cache,
                trust_store,
                v0_jids: HashSet::new(),
                legacy_sessions,
                pickle_key,
                store,
            };
        }
        let compatible_device_id = if device_id == 0 || device_id > Self::MAX_COMPAT_DEVICE_ID {
            Self::generate_compatible_device_id()
        } else {
            device_id
        };
        let mgr = Self::generate(compatible_device_id, store);
        let _ = mgr.store.save_account(&mgr.account, &mgr.pickle_key);
        mgr
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.store.save_account(&self.account, &self.pickle_key)?;
        self.store
            .save_legacy_sessions(&self.legacy_sessions, &self.pickle_key)?;
        self.store.save_device_lists(&self.device_lists)?;
        self.store.save_bundle_cache(&self.bundle_cache)?;
        self.store.save_trust_store(&self.trust_store)?;
        Ok(())
    }

    pub fn our_device_id(&self) -> u32 {
        self.account.device_id
    }

    pub fn set_our_jid(&mut self, jid: &str) {
        self.our_jid = Some(jid.to_string());
    }

    pub fn update_device_list(&mut self, jid: &str, devices: Vec<u32>) {
        // Merge rather than replace, so a stale IQ/device-list response doesn't
        // wipe devices we already learned from a more recent live update.
        let mut merged: std::collections::HashSet<u32> = self
            .device_lists
            .get(jid)
            .map(|list| list.iter().copied().collect())
            .unwrap_or_default();
        for d in &devices {
            merged.insert(*d);
        }
        let merged: Vec<u32> = merged.into_iter().collect::<Vec<_>>();
        self.device_lists.insert(jid.to_string(), merged.clone());
        self.trust_store.accept_all(jid, &merged);
    }

    pub fn mark_v0_jid(&mut self, jid: &str) {
        self.v0_jids.insert(jid.to_string());
    }

    pub fn clear_v0_jid(&mut self, jid: &str) {
        self.v0_jids.remove(jid);
    }

    pub fn cache_bundle(&mut self, jid: &str, device_id: u32, bundle: &Bundle) {
        let ik_bytes: [u8; 32] = match bundle.ik.as_slice().try_into() {
            Ok(b) => b,
            Err(_) => return,
        };
        self.bundle_cache
            .entry(jid.to_string())
            .or_default()
            .insert(device_id, CachedBundle { identity_key: ik_bytes });
    }

    /// Create a libsignal v3 session from a peer bundle and store it under the legacy session map.
    fn create_legacy_session(&mut self, jid: &str, device_id: u32, bundle: &Bundle) -> bool {
        let Some(legacy_bundle) = legacy_bundle_from_bundle(bundle) else {
            tracing::info!(
                "[OMEMO legacy] Failed to build legacy bundle for {} device {}",
                jid,
                device_id
            );
            return false;
        };
        let identity = signal_ratchet::keys::IdentityKeyPair::from_secret(
            self.account.identity_secret_key_bytes(),
        );
        let mut rng = rand::thread_rng();
        match signal_ratchet::Session::new_alice(&identity, &legacy_bundle, &mut rng) {
            Ok(session) => {
                self.legacy_sessions
                    .entry(jid.to_string())
                    .or_default()
                    .insert(device_id, session);
                true
            }
            Err(e) => {
                tracing::info!(
                    "[OMEMO legacy] new_alice failed for {} device {}: {}",
                    jid,
                    device_id,
                    e
                );
                false
            }
        }
    }

    pub fn create_session_from_bundle(
        &mut self,
        jid: &str,
        device_id: u32,
        bundle: &Bundle,
    ) -> bool {
        let had_legacy = self
            .legacy_sessions
            .get(jid)
            .and_then(|m| m.get(&device_id))
            .is_some();
        if had_legacy {
            return true;
        }
        if !self.create_legacy_session(jid, device_id, bundle) {
            return false;
        }
        self.cache_bundle(jid, device_id, bundle);
        true
    }

    /// Build a bundle using our own account keys. Used for self-chat sessions.
    pub fn self_bundle(&self) -> Option<Bundle> {
        let ik = self.account.inner.curve25519_key().to_bytes().to_vec();
        let (spk_id, spk_pub) = self.account.all_stored_fallback_keys().into_iter().next()?;
        let spk = spk_pub.to_bytes().to_vec();
        let mut spk_for_sig = Vec::with_capacity(33);
        spk_for_sig.push(0x05);
        spk_for_sig.extend_from_slice(&spk);
        let spks = self.account.xeddsa_sign(&spk_for_sig);
        let mut prekeys = self
            .account
            .all_stored_one_time_keys()
            .into_iter()
            .filter(|(id, _)| *id != 0)
            .map(|(id, pk)| (id, pk.to_bytes().to_vec()))
            .collect::<Vec<_>>();
        prekeys.sort_by_key(|(id, _)| *id);
        if prekeys.is_empty() {
            return None;
        }
        Some(Bundle {
            device_id: self.our_device_id(),
            spk_id,
            spk,
            spks,
            ik,
            prekeys,
        })
    }

    pub fn encrypt_message(&mut self, to: &str, body: &str) -> Option<EncryptedMessage> {
        let our_device_id = self.our_device_id();
        let our_jid = self.our_jid.as_ref()?.clone();
        let use_v0 = true;
        let is_self_chat = to == our_jid;
        let mut devices = self.device_lists.get(to).cloned().unwrap_or_default();
        if devices.is_empty() {
            if let Some(sess) = self.legacy_sessions.get(to) {
                devices.extend(sess.keys().copied());
            }
            if let Some(cached) = self.bundle_cache.get(to) {
                devices.extend(cached.keys().copied());
            }
        }
        devices.retain(|d| *d != 0);
        devices.sort_unstable();
        devices.dedup();
        if devices.is_empty() {
            tracing::info!(
                "[OMEMO encrypt] No known devices for {} (device list/sessions/bundle cache all empty)",
                to
            );
            return None;
        }
        tracing::info!(
            "[OMEMO encrypt] to={} our_jid={} our_device={} devices={:?} use_v0={}",
            to,
            our_jid,
            our_device_id,
            devices,
            use_v0
        );

        let (ciphertext, plaintext, iv) = {
            let (ct, key, auth_tag, nonce) =
                crypto::encrypt_payload_v0_conversations(body.as_bytes());
            let mut pt = Vec::with_capacity(32);
            pt.extend_from_slice(&key);
            pt.extend_from_slice(&auth_tag);
            (ct, pt, Some(nonce.to_vec()))
        };

        let mut key_groups = Vec::new();

        let mut recipient_keys = Vec::new();
        for device_id in &devices {
            if *device_id == our_device_id && !is_self_chat {
                continue;
            }
            if is_self_chat && *device_id == our_device_id {
                // Self-chat must use a fresh PreKey message for our own slot.
                // Reusing an old normal session here causes MAC mismatch when
                // decrypting our own carbon copy.
                if let Some(sessions) = self.legacy_sessions.get_mut(to) {
                    sessions.remove(device_id);
                }
                let bundle = match self.self_bundle() {
                    Some(b) => b,
                    None => continue,
                };
                let _ = self.create_session_from_bundle(to, *device_id, &bundle);
            }
            let session = match self
                .legacy_sessions
                .get_mut(to)
                .and_then(|m| m.get_mut(device_id))
            {
                Some(s) => s,
                None => {
                    tracing::info!(
                        "[OMEMO encrypt] No legacy session for recipient {} device {}",
                        to,
                        device_id
                    );
                    continue;
                }
            };
            let mut rng = rand::thread_rng();
            let ct = match session.encrypt(&plaintext, &mut rng) {
                Ok(c) => c,
                Err(e) => {
                    tracing::info!("[OMEMO encrypt] legacy encrypt failed: {}", e);
                    continue;
                }
            };
            let kex = ct.is_prekey();
            let data = ct.into_bytes();
            recipient_keys.push(MessageKey {
                rid: *device_id,
                kex,
                data,
            });
        }
        if !recipient_keys.is_empty() {
            key_groups.push(KeysGroup {
                jid: to.to_string(),
                keys: recipient_keys,
            });
        }

        if !is_self_chat {
            let mut own_devices = self.device_lists.get(&our_jid).cloned().unwrap_or_default();
            own_devices.retain(|d| *d != 0);
            let mut own_keys = Vec::new();
            for device_id in &own_devices {
                let session = match self
                    .legacy_sessions
                    .get_mut(&our_jid)
                    .and_then(|m| m.get_mut(device_id))
                {
                    Some(s) => s,
                    None => continue,
                };
                let mut rng = rand::thread_rng();
                let ct = match session.encrypt(&plaintext, &mut rng) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::info!("[OMEMO encrypt] legacy own encrypt failed: {}", e);
                        continue;
                    }
                };
                let kex = ct.is_prekey();
                let data = ct.into_bytes();
                own_keys.push(MessageKey {
                    rid: *device_id,
                    kex,
                    data,
                });
            }
            if !own_keys.is_empty() {
                key_groups.push(KeysGroup {
                    jid: our_jid,
                    keys: own_keys,
                });
            }
        }

        if key_groups.is_empty() {
            tracing::info!("[OMEMO encrypt] No key groups generated — returning None");
            return None;
        }

        Some(EncryptedMessage {
            is_v0: use_v0,
            header: MessageHeader {
                sid: our_device_id,
                keys: key_groups,
                iv,
            },
            payload: Some(ciphertext),
        })
    }

    pub fn decrypt_message(&mut self, from: &str, msg: &EncryptedMessage) -> Option<String> {
        fn hex(data: &[u8]) -> String {
            data.iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join("")
        }

        let our_device_id = self.our_device_id();
        let our_jid = self.our_jid.as_ref()?;
        let from_bare = from.split('/').next().unwrap_or(from);
        let from_addr = from.to_string();
        tracing::info!(
            "[OMEMO decrypt] from={} from_bare={} our_jid={} our_device={} sender_device={}",
            from,
            from_bare,
            our_jid,
            our_device_id,
            msg.header.sid
        );
        let ids = self
            .account
            .all_stored_one_time_secret_keys()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let mut ids_sorted = ids;
        ids_sorted.sort_unstable();
        let head = ids_sorted.iter().take(20).copied().collect::<Vec<_>>();
        tracing::info!(
            "[OMEMO decrypt] local prekeys count={} first20={:?}",
            ids_sorted.len(),
            head
        );

        // Find <keys> group matching our JID, or fall back to empty JID group (v0)
        let key_group = msg
            .header
            .keys
            .iter()
            .find(|g| g.jid == *our_jid)
            .or_else(|| msg.header.keys.iter().find(|g| g.jid.is_empty()));
        let key_group = match key_group {
            Some(g) => g,
            None => {
                tracing::info!(
                    "[OMEMO decrypt] No keys group for our JID {} (groups: {:?})",
                    our_jid,
                    msg.header.keys.iter().map(|g| &g.jid).collect::<Vec<_>>()
                );
                return None;
            }
        };
        let key_slot = match key_group.keys.iter().find(|k| k.rid == our_device_id) {
            Some(k) => k,
            None => {
                tracing::info!(
                    "[OMEMO decrypt] No key slot for our device {} (slots: {:?})",
                    our_device_id,
                    key_group.keys.iter().map(|k| k.rid).collect::<Vec<_>>()
                );
                return None;
            }
        };
        tracing::info!("[OMEMO decrypt] Found key slot: kex={}", key_slot.kex);
        tracing::info!(
            "[OMEMO decrypt raw] slot rid={} len={} data_hex={} iv_hex={}",
            key_slot.rid,
            key_slot.data.len(),
            hex(&key_slot.data),
            msg.header
                .iv
                .as_ref()
                .map(|v| hex(v))
                .unwrap_or_else(|| String::from("(none)")),
        );
        if let Some(payload) = &msg.payload {
            tracing::info!(
                "[OMEMO decrypt raw] payload_len={} payload_hex={}",
                payload.len(),
                hex(payload),
            );
        }

        let plaintext = if key_slot.kex {
            // PreKeySignalMessage: establish a new inbound session as Bob.
            let identity = signal_ratchet::keys::IdentityKeyPair::from_secret(
                self.account.identity_secret_key_bytes(),
            );
            let prekey_msg = match signal_ratchet::proto::PreKeySignalMessage::decode(
                &key_slot.data[1..],
            ) {
                Ok(m) => m,
                Err(e) => {
                    tracing::info!("[OMEMO decrypt] Failed to parse PreKeySignalMessage: {}", e);
                    return None;
                }
            };
            let signed_prekey_secret = match self
                .account
                .fallback_secret_key_by_id(prekey_msg.signed_pre_key_id)
            {
                Some(s) => s,
                None => {
                    tracing::info!(
                        "[OMEMO decrypt] No signed prekey secret for id {}",
                        prekey_msg.signed_pre_key_id
                    );
                    return None;
                }
            };
            let signed_prekey = signal_ratchet::keys::KeyPair::from_secret(signed_prekey_secret);
            let one_time_secret = match self.account.one_time_secret_key(prekey_msg.pre_key_id) {
                Some(s) => s,
                None => {
                    tracing::info!(
                        "[OMEMO decrypt] No one-time prekey secret for id {}",
                        prekey_msg.pre_key_id
                    );
                    return None;
                }
            };
            let one_time_prekey = signal_ratchet::keys::KeyPair::from_secret(one_time_secret);
            let (session, plaintext, _used_prekey_id) = match signal_ratchet::Session::new_bob(
                &identity,
                &signed_prekey,
                &one_time_prekey,
                &key_slot.data,
            ) {
                Ok(v) => v,
                Err(e) => {
                    tracing::info!("[OMEMO decrypt] new_bob failed: {}", e);
                    return None;
                }
            };
            self.legacy_sessions
                .entry(from_bare.to_string())
                .or_default()
                .insert(msg.header.sid, session);
            plaintext
        } else {
            let maybe_session = if let Some(m) = self.legacy_sessions.get_mut(&from_addr) {
                m.get_mut(&msg.header.sid)
            } else {
                self.legacy_sessions
                    .get_mut(from_bare)
                    .and_then(|m| m.get_mut(&msg.header.sid))
            };
            match maybe_session {
                Some(session) => match session.decrypt(&key_slot.data, false) {
                    Ok(pt) => pt,
                    Err(e) => {
                        tracing::info!("[OMEMO decrypt] legacy decrypt failed: {}", e);
                        return None;
                    }
                },
                None => {
                    tracing::info!(
                        "[OMEMO decrypt] No existing legacy session for {} or {} device {} (sessions_full={:?} sessions_bare={:?})",
                        from_addr,
                        from_bare,
                        msg.header.sid,
                        self.legacy_sessions
                            .get(&from_addr)
                            .map(|m| m.keys().collect::<Vec<_>>()),
                        self.legacy_sessions
                            .get(from_bare)
                            .map(|m| m.keys().collect::<Vec<_>>())
                    );
                    return None;
                }
            }
        };
        let payload = msg.payload.as_ref()?;
        if plaintext.len() == 32 {
            let iv = msg.header.iv.as_ref()?;
            if iv.len() != 12 {
                tracing::info!("[OMEMO decrypt] v0-gcm iv wrong length: {}", iv.len());
                return None;
            }
            let key: [u8; 16] = plaintext[..16].try_into().ok()?;
            let tag: [u8; 16] = plaintext[16..32].try_into().ok()?;
            let nonce: [u8; 12] = iv[..12].try_into().ok()?;
            let body = crypto::decrypt_payload_v0_conversations(payload, &key, &tag, &nonce)?;
            return String::from_utf8(body).ok();
        }

        if plaintext.len() == 48 {
            let key: [u8; 32] = plaintext[..32].try_into().ok()?;
            let hmac: [u8; 16] = plaintext[32..48].try_into().ok()?;
            let body = crypto::decrypt_payload_v0(payload, &key, &hmac)?;
            return String::from_utf8(body).ok();
        }

        tracing::info!(
            "[OMEMO decrypt] v0 plaintext wrong length: {}",
            plaintext.len()
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_self_chat_roundtrip() {
        let mut mgr = OmemoManager::generate(12345, Box::new(crate::store::MemoryStore::new()));
        let jid = "user@example.com";
        mgr.set_our_jid(jid);
        mgr.update_device_list(jid, vec![12345]);

        let encrypted = mgr.encrypt_message(jid, "hello legacy").expect("encrypt");
        let decrypted = mgr
            .decrypt_message(jid, &encrypted)
            .expect("decrypt");
        assert_eq!(decrypted, "hello legacy");
    }

    #[test]
    fn legacy_two_device_roundtrip() {
        let alice_jid = "alice@example.com";
        let bob_jid = "bob@example.com";

        let mut alice = OmemoManager::generate(11111, Box::new(crate::store::MemoryStore::new()));
        alice.set_our_jid(alice_jid);
        alice.update_device_list(bob_jid, vec![22222]);

        let mut bob = OmemoManager::generate(22222, Box::new(crate::store::MemoryStore::new()));
        bob.set_our_jid(bob_jid);

        let bob_bundle = bob.self_bundle().expect("bob self bundle");
        assert!(alice.create_session_from_bundle(bob_jid, 22222, &bob_bundle));

        // First message is a PreKeySignalMessage.
        let encrypted1 = alice
            .encrypt_message(bob_jid, "first legacy message")
            .expect("encrypt first");
        let decrypted1 = bob
            .decrypt_message(alice_jid, &encrypted1)
            .expect("decrypt first");
        assert_eq!(decrypted1, "first legacy message");

        // Second message is a normal SignalMessage (ratchet stepped).
        let encrypted2 = alice
            .encrypt_message(bob_jid, "second legacy message")
            .expect("encrypt second");
        assert!(!encrypted2.header.keys[0].keys[0].kex, "second msg should be normal");
        let decrypted2 = bob
            .decrypt_message(alice_jid, &encrypted2)
            .expect("decrypt second");
        assert_eq!(decrypted2, "second legacy message");
    }
}
