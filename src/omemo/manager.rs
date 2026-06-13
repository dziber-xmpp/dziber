use std::collections::{HashMap, HashSet};

use vodozemac::Curve25519PublicKey;
use vodozemac::olm::OlmMessage;
use vodozemac::olm::PreKeyMessage as VodoPreKeyMessage;

use crate::omemo::account::OmemoAccount;
use crate::omemo::bundle::Bundle;
use crate::omemo::crypto;
use crate::omemo::message::{EncryptedMessage, KeysGroup, MessageHeader, MessageKey};
use crate::omemo::session;
use crate::omemo::store;
use crate::omemo::trust::TrustStore;

/// Cached bundle info for peers (used during inbound session creation).
#[derive(Debug, Clone)]
pub struct CachedBundle {
    pub identity_key: Curve25519PublicKey,
}

pub struct OmemoManager {
    pub account: OmemoAccount,
    pub our_jid: Option<String>,
    pub sessions: HashMap<String, HashMap<u32, vodozemac::olm::Session>>,
    pub device_lists: HashMap<String, Vec<u32>>,
    pub bundle_cache: HashMap<String, HashMap<u32, CachedBundle>>,
    pub trust_store: TrustStore,
    /// JIDs for which only v0 bundles were found (send v0 messages to these).
    pub v0_jids: HashSet<String>,
    pickle_key: [u8; 32],
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

    pub fn generate(device_id: u32) -> Self {
        let pickle_key = store::load_or_generate_key();
        let mut account = OmemoAccount::generate(device_id);
        let _ = Self::ensure_account_material(&mut account);
        Self {
            account,
            our_jid: None,
            sessions: HashMap::new(),
            device_lists: HashMap::new(),
            bundle_cache: HashMap::new(),
            trust_store: TrustStore::new(),
            v0_jids: HashSet::new(),
            pickle_key,
        }
    }

    pub fn load_or_generate(device_id: u32) -> Self {
        let pickle_key = store::load_or_generate_key();
        if let Ok(Some(mut account)) = crate::db::omemo::load_omemo_account(&pickle_key) {
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
                let _ = crate::db::omemo::save_omemo_account(&account, &pickle_key);
            }
            if account.device_id == 0 || account.device_id > Self::MAX_COMPAT_DEVICE_ID {
                tracing::info!(
                    "[OMEMO] Incompatible device id {} detected; regenerating OMEMO state with compatible 31-bit device id",
                    account.device_id
                );
                return Self::generate(Self::generate_compatible_device_id());
            }
            let sessions = crate::db::omemo::load_omemo_sessions(&pickle_key).unwrap_or_default();
            let device_lists = crate::db::omemo::load_omemo_device_lists().unwrap_or_default();
            let bundle_cache = crate::db::omemo::load_omemo_bundle_cache().unwrap_or_default();
            let trust_store = crate::db::omemo::load_omemo_trust_store().unwrap_or_default();
            return Self {
                account,
                our_jid: None,
                sessions,
                device_lists,
                bundle_cache,
                trust_store,
                v0_jids: HashSet::new(),
                pickle_key,
            };
        }
        let compatible_device_id = if device_id == 0 || device_id > Self::MAX_COMPAT_DEVICE_ID {
            Self::generate_compatible_device_id()
        } else {
            device_id
        };
        let mgr = Self::generate(compatible_device_id);
        let _ = crate::db::omemo::save_omemo_account(&mgr.account, &mgr.pickle_key);
        mgr
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        crate::db::omemo::save_omemo_account(&self.account, &self.pickle_key)?;
        crate::db::omemo::save_omemo_sessions(&self.sessions, &self.pickle_key)?;
        crate::db::omemo::save_omemo_device_lists(&self.device_lists)?;
        crate::db::omemo::save_omemo_bundle_cache(&self.bundle_cache)?;
        crate::db::omemo::save_omemo_trust_store(&self.trust_store)?;
        Ok(())
    }

    pub fn our_device_id(&self) -> u32 {
        self.account.device_id
    }

    pub fn set_our_jid(&mut self, jid: &str) {
        self.our_jid = Some(jid.to_string());
    }

    pub fn update_device_list(&mut self, jid: &str, devices: Vec<u32>) {
        self.device_lists.insert(jid.to_string(), devices.clone());
        self.trust_store.accept_all(jid, &devices);
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
        let identity_key = Curve25519PublicKey::from(ik_bytes);
        self.bundle_cache
            .entry(jid.to_string())
            .or_default()
            .insert(device_id, CachedBundle { identity_key });
    }

    pub fn create_session_from_bundle(
        &mut self,
        jid: &str,
        device_id: u32,
        bundle: &Bundle,
    ) -> bool {
        let had_vodo = self
            .sessions
            .get(jid)
            .and_then(|m| m.get(&device_id))
            .is_some();
        if had_vodo {
            return true;
        }

        let ik_bytes: [u8; 32] = match bundle.ik.as_slice().try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let their_identity = Curve25519PublicKey::from(ik_bytes);

        let (_, otk) = match bundle.prekeys.first() {
            Some((id, data)) => (*id, data),
            None => return false,
        };
        let otk_bytes: [u8; 32] = match otk.as_slice().try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let their_otk = Curve25519PublicKey::from(otk_bytes);

        let session =
            session::create_outbound_session(&self.account.inner, their_identity, their_otk);
        self.sessions
            .entry(jid.to_string())
            .or_default()
            .insert(device_id, session);
        self.cache_bundle(jid, device_id, bundle);
        true
    }

    pub fn encrypt_message(&mut self, to: &str, body: &str) -> Option<EncryptedMessage> {
        let our_device_id = self.our_device_id();
        let our_jid = self.our_jid.as_ref()?.clone();
        let use_v0 = true;
        let is_self_chat = to == our_jid;
        let mut devices = self.device_lists.get(to).cloned().unwrap_or_default();
        if devices.is_empty() {
            if let Some(sess) = self.sessions.get(to) {
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
                if let Some(sessions) = self.sessions.get_mut(to) {
                    sessions.remove(device_id);
                }
                let ik = self.account.inner.curve25519_key().to_bytes().to_vec();
                let (spk_id, spk_pub) =
                    match self.account.all_stored_fallback_keys().into_iter().next() {
                        Some(v) => v,
                        None => continue,
                    };
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
                    continue;
                }
                let bundle = Bundle {
                    device_id: our_device_id,
                    spk_id,
                    spk,
                    spks,
                    ik,
                    prekeys,
                };
                let _ = self.create_session_from_bundle(to, *device_id, &bundle);
            }
            let session = match self.sessions.get_mut(to).and_then(|m| m.get_mut(device_id)) {
                Some(s) => s,
                None => {
                    tracing::info!(
                        "[OMEMO encrypt] No session for recipient {} device {}",
                        to,
                        device_id
                    );
                    continue;
                }
            };
            let olm_msg = session::encrypt(session, &plaintext);
            let kex = matches!(olm_msg, OlmMessage::PreKey(_));
            let data = match olm_msg {
                OlmMessage::Normal(ref m) => m.to_bytes(),
                OlmMessage::PreKey(ref m) => m.to_bytes(),
            };
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
                    .sessions
                    .get_mut(&our_jid)
                    .and_then(|m| m.get_mut(device_id))
                {
                    Some(s) => s,
                    None => continue,
                };
                let olm_msg = session::encrypt(session, &plaintext);
                let kex = matches!(olm_msg, OlmMessage::PreKey(_));
                let data = match olm_msg {
                    OlmMessage::Normal(ref m) => m.to_bytes(),
                    OlmMessage::PreKey(ref m) => m.to_bytes(),
                };
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

        fn first_bytes_hex(data: &[u8], n: usize) -> String {
            data.iter()
                .take(n)
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join("")
        }

        fn decode_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
            let mut shift = 0u32;
            let mut out = 0u64;
            while *pos < data.len() && shift <= 63 {
                let b = data[*pos];
                *pos += 1;
                out |= u64::from(b & 0x7f) << shift;
                if (b & 0x80) == 0 {
                    return Some(out);
                }
                shift += 7;
            }
            None
        }

        fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
            loop {
                let mut b = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    b |= 0x80;
                }
                out.push(b);
                if v == 0 {
                    break;
                }
            }
        }

        fn normalize_signal_inner_message(
            data: Vec<u8>,
            strip_field1_prefix: bool,
        ) -> Option<Vec<u8>> {
            if data.is_empty() {
                return None;
            }
            let mut ver = data[0];
            if matches!(ver, 0x22 | 0x33 | 0x44) {
                ver &= 0x0f;
            }

            let mut pos = 1usize;
            let mut out = Vec::with_capacity(data.len());
            out.push(ver);

            while pos < data.len() {
                let tag = decode_varint(&data, &mut pos)?;
                let field = (tag >> 3) as u32;
                let wire = (tag & 0x07) as u8;
                match wire {
                    0 => {
                        let v = decode_varint(&data, &mut pos)?;
                        encode_varint(tag, &mut out);
                        encode_varint(v, &mut out);
                    }
                    1 => {
                        if pos + 8 > data.len() {
                            return None;
                        }
                        encode_varint(tag, &mut out);
                        out.extend_from_slice(&data[pos..pos + 8]);
                        pos += 8;
                    }
                    2 => {
                        let len = usize::try_from(decode_varint(&data, &mut pos)?).ok()?;
                        if pos + len > data.len() {
                            return None;
                        }
                        let mut bytes = data[pos..pos + len].to_vec();
                        pos += len;
                        if strip_field1_prefix
                            && field == 1
                            && bytes.len() == 33
                            && bytes[0] == 0x05
                        {
                            bytes = bytes[1..].to_vec();
                        }
                        encode_varint(tag, &mut out);
                        encode_varint(bytes.len() as u64, &mut out);
                        out.extend_from_slice(&bytes);
                    }
                    5 => {
                        if pos + 4 > data.len() {
                            return None;
                        }
                        encode_varint(tag, &mut out);
                        out.extend_from_slice(&data[pos..pos + 4]);
                        pos += 4;
                    }
                    _ => {
                        let mut fallback = data;
                        if !fallback.is_empty() {
                            fallback[0] = ver;
                        }
                        return Some(fallback);
                    }
                }
            }

            Some(out)
        }

        fn strip_signal_inner_ratchet_prefix(mut data: Vec<u8>) -> Vec<u8> {
            if data.len() >= 36 && data[1] == 0x0a && data[2] == 0x21 && data[3] == 0x05 {
                data[2] = 0x20;
                data.remove(3);
            }
            data
        }

        fn swap_vodo_prekey_base_identity(data: &[u8]) -> Option<Vec<u8>> {
            if data.len() < 2 {
                return None;
            }
            let version = data[0];
            let mut pos = 1usize;
            let mut one = None::<Vec<u8>>;
            let mut base = None::<Vec<u8>>;
            let mut ident = None::<Vec<u8>>;
            let mut msg = None::<Vec<u8>>;
            while pos < data.len() {
                let tag = decode_varint(data, &mut pos)?;
                let field = (tag >> 3) as u32;
                let wire = (tag & 0x07) as u8;
                if wire != 2 {
                    return None;
                }
                let ln = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
                if pos + ln > data.len() {
                    return None;
                }
                let bytes = data[pos..pos + ln].to_vec();
                pos += ln;
                match field {
                    1 => one = Some(bytes),
                    2 => base = Some(bytes),
                    3 => ident = Some(bytes),
                    4 => msg = Some(bytes),
                    _ => {}
                }
            }
            let one = one?;
            let base = base?;
            let ident = ident?;
            let msg = msg?;
            let mut out = Vec::with_capacity(data.len());
            out.push(version);
            out.push((1 << 3) | 2);
            encode_varint(one.len() as u64, &mut out);
            out.extend_from_slice(&one);
            out.push((2 << 3) | 2);
            encode_varint(ident.len() as u64, &mut out);
            out.extend_from_slice(&ident);
            out.push((3 << 3) | 2);
            encode_varint(base.len() as u64, &mut out);
            out.extend_from_slice(&base);
            out.push((4 << 3) | 2);
            encode_varint(msg.len() as u64, &mut out);
            out.extend_from_slice(&msg);
            Some(out)
        }

        fn rewrite_signalish_normal_for_vodo(data: &[u8]) -> Option<Vec<u8>> {
            // Input shape seen in logs:
            // 0x33 + protobuf fields:
            //   1: ratchet key (33 bytes, usually 0x05-prefixed)
            //   2: counter (varint)
            //   3: previous counter (varint)
            //   4: ciphertext+mac (bytes, mac(8) in tail)
            // vodo expects:
            //   version 0x03/0x04 + protobuf fields 1,2,4 where
            //   field1 ratchet key is 32 bytes and MAC is trailing 8 bytes.
            if data.len() < 12 {
                return None;
            }
            let mut pos = 1usize;
            let mut ratchet = None::<Vec<u8>>;
            let mut counter = None::<u64>;
            let mut ctext_with_mac = None::<Vec<u8>>;
            while pos < data.len() {
                let tag = decode_varint(data, &mut pos)?;
                let field = (tag >> 3) as u32;
                let wire = (tag & 0x07) as u8;
                match (field, wire) {
                    (1, 2) | (4, 2) => {
                        let len = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
                        if pos + len > data.len() {
                            return None;
                        }
                        let bytes = data[pos..pos + len].to_vec();
                        pos += len;
                        if field == 1 {
                            ratchet = Some(bytes);
                        } else {
                            ctext_with_mac = Some(bytes);
                        }
                    }
                    (2, 0) => counter = Some(decode_varint(data, &mut pos)?),
                    (_, 0) => {
                        let _ = decode_varint(data, &mut pos)?;
                    }
                    (_, 2) => {
                        let len = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
                        if pos + len > data.len() {
                            return None;
                        }
                        pos += len;
                    }
                    _ => return None,
                }
            }
            let mut ratchet = ratchet?;
            if ratchet.len() == 33 && ratchet[0] == 0x05 {
                ratchet = ratchet[1..].to_vec();
            }
            if ratchet.len() != 32 {
                return None;
            }
            let mut cwm = ctext_with_mac?;
            if cwm.len() <= 8 {
                return None;
            }
            let mac = cwm.split_off(cwm.len() - 8);
            let counter = counter.unwrap_or(0);
            let mut out = Vec::with_capacity(data.len());
            // nibble-normalize 0x33 -> 0x03 for truncated-MAC format
            let ver = if matches!(data[0], 0x22 | 0x33 | 0x44) {
                data[0] & 0x0f
            } else {
                data[0]
            };
            out.push(ver);
            out.push((1 << 3) | 2);
            encode_varint(ratchet.len() as u64, &mut out);
            out.extend_from_slice(&ratchet);
            out.push(2 << 3);
            encode_varint(counter, &mut out);
            out.push((4 << 3) | 2);
            encode_varint(cwm.len() as u64, &mut out);
            out.extend_from_slice(&cwm);
            out.extend_from_slice(&mac);
            Some(out)
        }

        fn rewrite_signalish_normal_for_vodo_strict(data: &[u8]) -> Option<Vec<u8>> {
            if data.len() < 50 || data.first().copied()? != 0x33 {
                return None;
            }
            if data.get(1).copied()? != 0x0a || data.get(2).copied()? != 0x21 {
                return None;
            }
            let mut pos = 3usize;
            if pos + 33 > data.len() {
                return None;
            }
            let mut ratchet = data[pos..pos + 33].to_vec();
            pos += 33;
            if ratchet.first().copied() == Some(0x05) && ratchet.len() == 33 {
                ratchet = ratchet[1..].to_vec();
            }
            if ratchet.len() != 32 {
                return None;
            }
            if data.get(pos).copied()? != 0x10 {
                return None;
            }
            pos += 1;
            let counter = decode_varint(data, &mut pos)?;
            if data.get(pos).copied()? != 0x18 {
                return None;
            }
            pos += 1;
            let _prev = decode_varint(data, &mut pos)?;
            if data.get(pos).copied()? != 0x22 {
                return None;
            }
            pos += 1;
            let clen = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
            if pos + clen > data.len() || clen <= 8 {
                return None;
            }
            let mut cwm = data[pos..pos + clen].to_vec();
            let mac = cwm.split_off(cwm.len() - 8);
            let mut out = Vec::with_capacity(data.len());
            out.push(0x03);
            out.push(0x0a);
            out.push(0x20);
            out.extend_from_slice(&ratchet);
            out.push(0x10);
            encode_varint(counter, &mut out);
            out.push(0x22);
            encode_varint(cwm.len() as u64, &mut out);
            out.extend_from_slice(&cwm);
            out.extend_from_slice(&mac);
            Some(out)
        }

        fn signal_to_vodo_prekey_bytes(
            data: &[u8],
            local_otk_by_id: &HashMap<u32, Curve25519PublicKey>,
            strip_inner_field1_prefix: bool,
            strip_base_identity_prefix: bool,
            prefix_one_time_key: bool,
            normalize_inner_version: bool,
        ) -> Option<Vec<u8>> {
            if data.len() < 2 {
                return None;
            }

            let mut msg_version = data[0];
            if matches!(msg_version, 0x22 | 0x33 | 0x44) {
                msg_version &= 0x0f;
            }
            if msg_version != 0x03 && msg_version != 0x04 {
                return None;
            }

            let mut pos = 1usize;
            let mut prekey_id: Option<u32> = None;
            let mut base_key: Option<Vec<u8>> = None;
            let mut identity_key: Option<Vec<u8>> = None;
            let mut inner_msg: Option<Vec<u8>> = None;

            while pos < data.len() {
                let tag = decode_varint(data, &mut pos)?;
                let field = (tag >> 3) as u32;
                let wire = (tag & 0x07) as u8;
                match (field, wire) {
                    // Signal-style prekey id (varint).
                    (1, 0) => {
                        let v = decode_varint(data, &mut pos)?;
                        if let Ok(id) = u32::try_from(v) {
                            prekey_id = Some(id);
                        }
                    }
                    // Base key / identity key / inner message as length-delimited.
                    (2, 2) | (3, 2) | (4, 2) => {
                        let len = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
                        if pos + len > data.len() {
                            return None;
                        }
                        let bytes = data[pos..pos + len].to_vec();
                        pos += len;
                        match field {
                            2 => base_key = Some(bytes),
                            3 => identity_key = Some(bytes),
                            4 => inner_msg = Some(bytes),
                            _ => {}
                        }
                    }
                    // Skip unknown varint field.
                    (_, 0) => {
                        let _ = decode_varint(data, &mut pos)?;
                    }
                    // Skip unknown length-delimited field.
                    (_, 2) => {
                        let len = usize::try_from(decode_varint(data, &mut pos)?).ok()?;
                        if pos + len > data.len() {
                            return None;
                        }
                        pos += len;
                    }
                    _ => return None,
                }
            }

            let prekey_id = prekey_id?;
            let mut one_time_key = local_otk_by_id.get(&prekey_id)?.to_bytes().to_vec();
            if prefix_one_time_key {
                let mut p = Vec::with_capacity(33);
                p.push(0x05);
                p.extend_from_slice(&one_time_key);
                one_time_key = p;
            }
            let mut base_key = base_key?;
            let mut identity_key = identity_key?;
            // Signal protobuf often carries 0x05-prefixed keys (33 bytes).
            if strip_base_identity_prefix && base_key.len() == 33 && base_key[0] == 0x05 {
                base_key = base_key[1..].to_vec();
            }
            if strip_base_identity_prefix && identity_key.len() == 33 && identity_key[0] == 0x05 {
                identity_key = identity_key[1..].to_vec();
            }
            // Normalize to the shape accepted by vodozemac prekey parser.
            let inner_raw = strip_signal_inner_ratchet_prefix(inner_msg?);
            let mut inner_msg =
                normalize_signal_inner_message(inner_raw.clone(), strip_inner_field1_prefix)
                    .unwrap_or(inner_raw);
            if normalize_inner_version
                && let Some(first) = inner_msg.first_mut()
                && matches!(*first, 0x22 | 0x33 | 0x44)
            {
                *first &= 0x0f;
            }

            tracing::info!(
                "[OMEMO rewrite] prekey_id={} one_time_key_len={} base_key_len={} identity_key_len={} inner_len={} inner_first8={}",
                prekey_id,
                one_time_key.len(),
                base_key.len(),
                identity_key.len(),
                inner_msg.len(),
                first_bytes_hex(&inner_msg, 8)
            );

            // Re-encode using vodozemac expected shape:
            // 1: one_time_key bytes, 2: base_key bytes, 3: identity_key bytes, 4: message bytes
            let mut out = Vec::with_capacity(data.len());
            out.push(msg_version);
            out.push((1 << 3) | 2);
            encode_varint(one_time_key.len() as u64, &mut out);
            out.extend_from_slice(&one_time_key);
            out.push((2 << 3) | 2);
            encode_varint(base_key.len() as u64, &mut out);
            out.extend_from_slice(&base_key);
            out.push((3 << 3) | 2);
            encode_varint(identity_key.len() as u64, &mut out);
            out.extend_from_slice(&identity_key);
            out.push((4 << 3) | 2);
            encode_varint(inner_msg.len() as u64, &mut out);
            out.extend_from_slice(&inner_msg);
            Some(out)
        }

        let normalized_key_data = key_slot.data.clone();
        let olm_msg = if key_slot.kex {
            None
        } else {
            let mut parsed = vodozemac::olm::Message::from_bytes(&normalized_key_data).ok();
            if parsed.is_none()
                && let Some(rewritten) = rewrite_signalish_normal_for_vodo(&normalized_key_data)
            {
                tracing::info!(
                    "[OMEMO rewrite] signal-normal rewritten len={} first16={}",
                    rewritten.len(),
                    first_bytes_hex(&rewritten, 16)
                );
                parsed = vodozemac::olm::Message::from_bytes(&rewritten).ok();
                if parsed.is_none() {
                    tracing::info!("[OMEMO rewrite] rewritten candidate still failed vodo parse");
                }
            } else if parsed.is_none() {
                tracing::info!("[OMEMO rewrite] signal-normal rewrite not applicable");
                if let Some(rewritten) =
                    rewrite_signalish_normal_for_vodo_strict(&normalized_key_data)
                {
                    tracing::info!(
                        "[OMEMO rewrite] strict signal-normal rewritten len={} first16={}",
                        rewritten.len(),
                        first_bytes_hex(&rewritten, 16)
                    );
                    parsed = vodozemac::olm::Message::from_bytes(&rewritten).ok();
                    if parsed.is_none() {
                        tracing::info!(
                            "[OMEMO rewrite] strict rewritten candidate still failed vodo parse"
                        );
                    }
                }
            }
            match parsed {
                Some(m) => Some(OlmMessage::Normal(m)),
                None => match vodozemac::olm::Message::from_bytes(&normalized_key_data) {
                    Ok(m) => Some(OlmMessage::Normal(m)),
                    Err(e1) => {
                        if normalized_key_data.len() > 1 {
                            match vodozemac::olm::Message::from_bytes(&normalized_key_data[1..]) {
                                Ok(m) => {
                                    tracing::info!(
                                        "[OMEMO decrypt] Parsed OlmMessage after stripping 1-byte prefix"
                                    );
                                    Some(OlmMessage::Normal(m))
                                }
                                Err(e2) => {
                                    match std::str::from_utf8(&normalized_key_data)
                                        .ok()
                                        .and_then(|s| vodozemac::olm::Message::from_base64(s).ok())
                                    {
                                        Some(m) => {
                                            tracing::info!(
                                                "[OMEMO decrypt] Parsed OlmMessage from base64 text payload"
                                            );
                                            Some(OlmMessage::Normal(m))
                                        }
                                        None => {
                                            tracing::info!(
                                                "[OMEMO decrypt] Failed to parse OlmMessage: {}; retry_without_prefix: {}; len={} first16={} normalized_first16={}",
                                                e1,
                                                e2,
                                                key_slot.data.len(),
                                                first_bytes_hex(&key_slot.data, 16),
                                                first_bytes_hex(&normalized_key_data, 16)
                                            );
                                            return None;
                                        }
                                    }
                                }
                            }
                        } else {
                            match std::str::from_utf8(&normalized_key_data)
                                .ok()
                                .and_then(|s| vodozemac::olm::Message::from_base64(s).ok())
                            {
                                Some(m) => {
                                    tracing::info!(
                                        "[OMEMO decrypt] Parsed OlmMessage from base64 text payload"
                                    );
                                    Some(OlmMessage::Normal(m))
                                }
                                None => {
                                    tracing::info!(
                                        "[OMEMO decrypt] Failed to parse OlmMessage: {}; len={} first16={} normalized_first16={}",
                                        e1,
                                        key_slot.data.len(),
                                        first_bytes_hex(&key_slot.data, 16),
                                        first_bytes_hex(&normalized_key_data, 16)
                                    );
                                    return None;
                                }
                            }
                        }
                    }
                },
            }
        };

        let plaintext = if key_slot.kex {
            let local_otk_by_id: HashMap<u32, Curve25519PublicKey> = self
                .account
                .all_stored_one_time_keys()
                .into_iter()
                .collect();
            let mut candidates = Vec::new();
            // Minimal normalization path: keep protobuf shape untouched, only
            // normalize packed prekey version nibble at byte 0.
            if !key_slot.data.is_empty() {
                let mut v = key_slot.data.clone();
                if matches!(v[0], 0x22 | 0x33 | 0x44) {
                    v[0] &= 0x0f;
                }
                candidates.push(v);
            }
            for strip_inner in [true, false] {
                for strip_bi in [true, false] {
                    for prefix_otk in [false, true] {
                        for normalize_inner_version in [false, true] {
                            if let Some(vodo) = signal_to_vodo_prekey_bytes(
                                &key_slot.data,
                                &local_otk_by_id,
                                strip_inner,
                                strip_bi,
                                prefix_otk,
                                normalize_inner_version,
                            ) {
                                candidates.push(vodo.clone());
                                if let Some(swapped) = swap_vodo_prekey_base_identity(&vodo) {
                                    candidates.push(swapped);
                                }
                            }
                        }
                    }
                }
            }
            if candidates.is_empty() {
                candidates.push(key_slot.data.clone());
            }
            let mut parsed_any = false;
            let mut first_err: Option<String> = None;
            for raw in candidates {
                let Ok(prekey_msg) = VodoPreKeyMessage::from_bytes(&raw) else {
                    continue;
                };
                parsed_any = true;
                let sender_ik = prekey_msg.identity_key();
                let mut temp_account =
                    vodozemac::olm::Account::from_pickle(self.account.inner.pickle());
                let inbound = match session::create_inbound_session(
                    &mut temp_account,
                    sender_ik,
                    &prekey_msg,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e.to_string());
                        }
                        continue;
                    }
                };
                self.account.inner = temp_account;
                let plaintext = inbound.plaintext;
                self.sessions
                    .entry(from_bare.to_string())
                    .or_default()
                    .insert(msg.header.sid, inbound.session);
                return String::from_utf8(if plaintext.len() == 32 {
                    let payload = msg.payload.as_ref()?;
                    let iv = msg.header.iv.as_ref()?;
                    if iv.len() != 12 {
                        return None;
                    }
                    let key: [u8; 16] = plaintext[..16].try_into().ok()?;
                    let tag: [u8; 16] = plaintext[16..32].try_into().ok()?;
                    let nonce: [u8; 12] = iv[..12].try_into().ok()?;
                    crypto::decrypt_payload_v0_conversations(payload, &key, &tag, &nonce)?
                } else if plaintext.len() == 48 {
                    let payload = msg.payload.as_ref()?;
                    let key: [u8; 32] = plaintext[..32].try_into().ok()?;
                    let hmac: [u8; 16] = plaintext[32..48].try_into().ok()?;
                    crypto::decrypt_payload_v0(payload, &key, &hmac)?
                } else {
                    return None;
                })
                .ok();
            }
            if !parsed_any {
                tracing::info!("[OMEMO decrypt] vodo PreKeyMessage parse failed");
            } else {
                tracing::info!(
                    "[OMEMO decrypt] vodo inbound prekey session failed for all candidates{}",
                    first_err
                        .as_ref()
                        .map(|e| format!("; first_error={}", e))
                        .unwrap_or_default()
                );
            }
            return None;
        } else {
            let maybe_session = if let Some(m) = self.sessions.get_mut(&from_addr) {
                m.get_mut(&msg.header.sid)
            } else {
                self.sessions
                    .get_mut(from_bare)
                    .and_then(|m| m.get_mut(&msg.header.sid))
            };
            match maybe_session {
                Some(session) => match session::decrypt(session, olm_msg.as_ref()?) {
                    Ok(pt) => pt,
                    Err(e) => {
                        tracing::info!("[OMEMO decrypt] Olm decrypt failed: {}", e);
                        return None;
                    }
                },
                None => {
                    tracing::info!(
                        "[OMEMO decrypt] No existing session for {} or {} device {} (sessions_full={:?} sessions_bare={:?})",
                        from_addr,
                        from_bare,
                        msg.header.sid,
                        self.sessions
                            .get(&from_addr)
                            .map(|m| m.keys().collect::<Vec<_>>()),
                        self.sessions
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
