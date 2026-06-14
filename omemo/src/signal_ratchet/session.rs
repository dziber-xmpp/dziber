use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand_core::{CryptoRng, RngCore};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::signal_ratchet::Error;
use crate::signal_ratchet::bundle::PreKeyBundle;
use crate::signal_ratchet::crypto::{
    aes_cbc_decrypt, aes_cbc_encrypt, compute_mac, derive_initial_root_and_chain,
    derive_message_keys, kdf_chain, kdf_root, x3dh_master_secret_alice, x3dh_master_secret_bob,
};
use crate::signal_ratchet::keys::{IdentityKeyPair, KeyPair, serialize_public_key};
use crate::signal_ratchet::proto::{PreKeySignalMessage, SignalMessage};

const MAX_FORWARD_JUMPS: u32 = 2000;
const CURRENT_VERSION: u8 = 3;
const VERSIONED_VERSION: u8 = (CURRENT_VERSION << 4) | CURRENT_VERSION;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct Chain {
    key: [u8; 32],
    index: u32,
    previous_counter: u32,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct PendingPreKey {
    pre_key_id: u32,
    signed_pre_key_id: u32,
    base_key: [u8; 32],
}

/// A set of derived message keys that have been skipped while waiting for an out-of-order message.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct MessageKeys {
    cipher_key: [u8; 32],
    mac_key: [u8; 32],
    iv: [u8; 16],
}

/// An established libsignal v3 session.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Session {
    session_version: u32,
    root_key: [u8; 32],
    our_ratchet_key: Option<KeyPair>,
    their_ratchet_key: Option<[u8; 32]>,
    sending_chain: Option<Chain>,
    sending_chain_remote_key: Option<[u8; 32]>,
    receiving_chains: Vec<([u8; 32], Chain)>,
    skipped_message_keys: Vec<([u8; 32], u32, MessageKeys)>,
    local_identity: [u8; 32],
    remote_identity: Option<[u8; 32]>,
    pending_pre_key: Option<PendingPreKey>,
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session")
            .field("session_version", &self.session_version)
            .field("local_identity", &hex::encode(self.local_identity))
            .field("remote_identity", &self.remote_identity.as_ref().map(hex::encode))
            .finish_non_exhaustive()
    }
}

/// The serialized form of a single encrypted message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiphertextMessage {
    Signal(Vec<u8>),
    PreKey(Vec<u8>),
}

impl CiphertextMessage {
    /// Return the serialized message bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            CiphertextMessage::Signal(v) | CiphertextMessage::PreKey(v) => v,
        }
    }

    /// Whether this is a PreKeySignalMessage.
    pub fn is_prekey(&self) -> bool {
        matches!(self, CiphertextMessage::PreKey(_))
    }
}

impl Session {
    /// Create a new outbound session as Alice (initiator).
    pub fn new_alice<R: RngCore + CryptoRng>(
        our_identity: &IdentityKeyPair,
        bundle: &PreKeyBundle,
        rng: &mut R,
    ) -> Result<Self, Error> {
        if !bundle.verify_signature() {
            return Err(Error::InvalidSignature);
        }

        let our_base_key = KeyPair::generate(rng);
        let master_secret = x3dh_master_secret_alice(
            our_identity,
            &our_base_key,
            &bundle.signed_pre_key_public,
            &bundle.identity_key,
            &bundle.pre_key_public,
        )
        .ok_or(Error::NonContributoryKey)?;

        let (root_key, receiver_chain_key) = derive_initial_root_and_chain(&master_secret);

        // The first receiving chain uses the peer's signed prekey as the ratchet key.
        let their_ratchet_key = bundle.signed_pre_key_public;
        let receiving_chains = vec![(
            their_ratchet_key,
            Chain {
                key: receiver_chain_key,
                index: 0,
                previous_counter: 0,
            },
        )];

        // Immediately advance the root to create our first sending chain.
        let our_ratchet_key = KeyPair::generate(rng);
        let dh = our_ratchet_key.diffie_hellman(&their_ratchet_key).ok_or(Error::NonContributoryKey)?;
        let (root_key, sender_chain_key) = kdf_root(&root_key, &dh);

        Ok(Session {
            session_version: CURRENT_VERSION as u32,
            root_key,
            our_ratchet_key: Some(our_ratchet_key.clone()),
            their_ratchet_key: Some(their_ratchet_key),
            sending_chain: Some(Chain {
                key: sender_chain_key,
                index: 0,
                previous_counter: 0,
            }),
            sending_chain_remote_key: Some(their_ratchet_key),
            receiving_chains,
            skipped_message_keys: Vec::new(),
            local_identity: *our_identity.public_key_bytes(),
            remote_identity: Some(bundle.identity_key),
            pending_pre_key: Some(PendingPreKey {
                pre_key_id: bundle.pre_key_id,
                signed_pre_key_id: bundle.signed_pre_key_id,
                base_key: *our_base_key.public_key_bytes(),
            }),
        })
    }

    /// Create a new inbound session as Bob (responder) from a PreKeySignalMessage.
    ///
    /// Returns the decrypted plaintext and the ID of the one-time prekey that was used.
    pub fn new_bob(
        our_identity: &IdentityKeyPair,
        our_signed_pre_key: &KeyPair,
        our_one_time_pre_key: &KeyPair,
        message_bytes: &[u8],
    ) -> Result<(Self, Vec<u8>, u32), Error> {
        if message_bytes.is_empty() || message_bytes[0] != VERSIONED_VERSION {
            return Err(Error::UnsupportedVersion(message_bytes.first().copied().unwrap_or(0)));
        }
        let prekey_message = PreKeySignalMessage::decode(&message_bytes[1..])?;

        let master_secret = x3dh_master_secret_bob(
            our_identity,
            our_signed_pre_key,
            our_one_time_pre_key,
            &prekey_message.identity_key,
            &prekey_message.base_key,
        )
        .ok_or(Error::NonContributoryKey)?;

        let (root_key, sender_chain_key) = derive_initial_root_and_chain(&master_secret);

        let mut session = Session {
            session_version: CURRENT_VERSION as u32,
            root_key,
            our_ratchet_key: Some(our_signed_pre_key.clone()),
            their_ratchet_key: Some(prekey_message.base_key),
            sending_chain: Some(Chain {
                key: sender_chain_key,
                index: 0,
                previous_counter: 0,
            }),
            sending_chain_remote_key: Some(prekey_message.base_key),
            receiving_chains: Vec::new(),
            skipped_message_keys: Vec::new(),
            local_identity: *our_identity.public_key_bytes(),
            remote_identity: Some(prekey_message.identity_key),
            pending_pre_key: None,
        };

        let plaintext = session.decrypt_signal_message(&prekey_message.message)?;
        Ok((session, plaintext, prekey_message.pre_key_id))
    }

    /// Encrypt a plaintext payload.
    pub fn encrypt<R: RngCore + CryptoRng>(
        &mut self,
        plaintext: &[u8],
        rng: &mut R,
    ) -> Result<CiphertextMessage, Error> {
        self.ratchet_if_needed(rng)?;

        let sending_chain = self.sending_chain.as_mut().ok_or(Error::NoSendingChain)?;

        let (next_chain_key, message_key) = kdf_chain(&sending_chain.key);
        let counter = sending_chain.index;
        let previous_counter = sending_chain.previous_counter;
        sending_chain.key = next_chain_key;
        sending_chain.index = counter + 1;

        let ratchet_key = *self
            .our_ratchet_key
            .as_ref()
            .ok_or(Error::MissingLocalKey)?
            .public_key_bytes();

        let (cipher_key, mac_key, iv) = derive_message_keys(&message_key);
        let ciphertext = aes_cbc_encrypt(&cipher_key, &iv, plaintext);

        let proto_message = SignalMessage {
            ratchet_key,
            counter,
            previous_counter,
            ciphertext,
        };
        let proto_bytes = proto_message.encode();
        let mut body = Vec::with_capacity(1 + proto_bytes.len() + 8);
        body.push(VERSIONED_VERSION);
        body.extend_from_slice(&proto_bytes);

        let local_id = serialize_public_key(&self.local_identity);
        let remote_id = serialize_public_key(self.remote_identity.as_ref().ok_or(Error::MissingRemoteIdentity)?);
        let mac = compute_mac(&mac_key, &local_id, &remote_id, &body);
        body.extend_from_slice(&mac);

        if let Some(pending) = self.pending_pre_key.take() {
            let prekey_proto = PreKeySignalMessage {
                registration_id: 0, // caller can overwrite if desired
                pre_key_id: pending.pre_key_id,
                signed_pre_key_id: pending.signed_pre_key_id,
                base_key: pending.base_key,
                identity_key: self.local_identity,
                message: body,
            };
            let mut prekey_body = Vec::with_capacity(1 + prekey_proto.encode().len());
            prekey_body.push(VERSIONED_VERSION);
            prekey_body.extend_from_slice(&prekey_proto.encode());
            Ok(CiphertextMessage::PreKey(prekey_body))
        } else {
            Ok(CiphertextMessage::Signal(body))
        }
    }

    /// Whether the next encrypted message will be a PreKeySignalMessage.
    pub fn is_prekey(&self) -> bool {
        self.pending_pre_key.is_some()
    }

    /// Decrypt a payload.
    pub fn decrypt(&mut self, message_bytes: &[u8], is_prekey: bool) -> Result<Vec<u8>, Error> {
        if is_prekey {
            if message_bytes.is_empty() || message_bytes[0] != VERSIONED_VERSION {
                return Err(Error::UnsupportedVersion(message_bytes.first().copied().unwrap_or(0)));
            }
            let prekey_message = PreKeySignalMessage::decode(&message_bytes[1..])?;
            if self.remote_identity.is_none() {
                self.remote_identity = Some(prekey_message.identity_key);
            }
            self.decrypt_signal_message(&prekey_message.message)
        } else {
            self.decrypt_signal_message(message_bytes)
        }
    }

    fn decrypt_signal_message(
        &mut self,
        message_bytes: &[u8],
    ) -> Result<Vec<u8>, Error> {
        if message_bytes.len() < 1 + 8 {
            return Err(Error::InvalidMessage("message too short".into()));
        }
        let version = message_bytes[0];
        if version != VERSIONED_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let mac_offset = message_bytes.len() - 8;
        let signed_part = &message_bytes[..mac_offset];
        let their_mac = &message_bytes[mac_offset..];
        let proto_message = SignalMessage::decode(&signed_part[1..])?;

        self.maybe_step_receiving_ratchet(&proto_message)?;

        let chain = self
            .receiving_chains
            .iter_mut()
            .find(|(k, _)| k == &proto_message.ratchet_key)
            .map(|(_, c)| c)
            .ok_or_else(|| Error::InvalidMessage("unknown ratchet key".into()))?;

        let message_keys = if proto_message.counter < chain.index {
            {
                let pos = self
                    .skipped_message_keys
                    .iter()
                    .position(|(k, c, _)| k == &proto_message.ratchet_key && *c == proto_message.counter)
                    .ok_or_else(|| Error::DuplicateMessage)?;
                let (_, _, keys) = self.skipped_message_keys.remove(pos);
                keys
            }
        } else {
            Self::fill_message_keys(
                chain,
                proto_message.counter,
                &proto_message.ratchet_key,
                &mut self.skipped_message_keys,
            )?
        };

        let local_id = serialize_public_key(&self.local_identity);
        let remote_id = serialize_public_key(self.remote_identity.as_ref().ok_or(Error::MissingRemoteIdentity)?);
        let expected_mac = compute_mac(&message_keys.mac_key, &remote_id, &local_id, signed_part);
        if expected_mac.ct_eq(their_mac).unwrap_u8() == 0 {
            return Err(Error::BadMac);
        }

        let plaintext = aes_cbc_decrypt(&message_keys.cipher_key, &message_keys.iv, &proto_message.ciphertext)
            .ok_or(Error::DecryptionFailed)?;

        Ok(plaintext)
    }

    fn maybe_step_receiving_ratchet(&mut self, message: &SignalMessage) -> Result<(), Error> {
        if self.receiving_chains.iter().any(|(k, _)| k == &message.ratchet_key) {
            return Ok(());
        }

        let our_ratchet_key = self.our_ratchet_key.as_ref().ok_or(Error::MissingLocalKey)?;
        let dh = our_ratchet_key
            .diffie_hellman(&message.ratchet_key)
            .ok_or(Error::NonContributoryKey)?;

        // If we have an existing receiving chain, fill in skipped message keys up to
        // previousCounter and then drop it.
        if let Some(their_old_key) = self.their_ratchet_key {
            if let Some(pos) = self.receiving_chains.iter().position(|(k, _)| k == &their_old_key) {
                let (_, mut old_chain) = self.receiving_chains.remove(pos);
                Self::fill_message_keys(
                    &mut old_chain,
                    message.previous_counter,
                    &their_old_key,
                    &mut self.skipped_message_keys,
                )
                .ok();
            }
        }

        let (new_root, new_chain_key) = kdf_root(&self.root_key, &dh);
        self.root_key = new_root;
        self.receiving_chains.push((
            message.ratchet_key,
            Chain {
                key: new_chain_key,
                index: 0,
                previous_counter: 0,
            },
        ));
        self.their_ratchet_key = Some(message.ratchet_key);
        Ok(())
    }

    fn ratchet_if_needed<R: RngCore + CryptoRng>(&mut self, rng: &mut R) -> Result<(), Error> {
        let needs_ratchet = match (&self.sending_chain_remote_key, self.their_ratchet_key) {
            (Some(send_remote), Some(current_remote)) => send_remote != &current_remote,
            (None, Some(_)) => true,
            _ => false,
        };

        if !needs_ratchet {
            return Ok(());
        }

        let their_ratchet_key = self.their_ratchet_key.ok_or(Error::MissingRemoteKey)?;
        let new_ratchet_key = KeyPair::generate(rng);
        let dh = new_ratchet_key
            .diffie_hellman(&their_ratchet_key)
            .ok_or(Error::NonContributoryKey)?;

        let previous_counter = self
            .sending_chain
            .as_ref()
            .map(|c| c.index)
            .unwrap_or(0);

        let (new_root, new_chain_key) = kdf_root(&self.root_key, &dh);
        self.root_key = new_root;
        self.our_ratchet_key = Some(new_ratchet_key);
        self.sending_chain = Some(Chain {
            key: new_chain_key,
            index: 0,
            previous_counter,
        });
        self.sending_chain_remote_key = Some(their_ratchet_key);
        Ok(())
    }

    fn fill_message_keys(
        chain: &mut Chain,
        target_counter: u32,
        ratchet_key: &[u8; 32],
        skipped: &mut Vec<([u8; 32], u32, MessageKeys)>,
    ) -> Result<MessageKeys, Error> {
        if target_counter < chain.index {
            return Err(Error::DuplicateMessage);
        }
        if target_counter - chain.index > MAX_FORWARD_JUMPS {
            return Err(Error::MessageGapTooLarge(target_counter - chain.index));
        }

        while chain.index < target_counter {
            let (next_chain_key, message_key) = kdf_chain(&chain.key);
            let (cipher_key, mac_key, iv) = derive_message_keys(&message_key);
            skipped.push((*ratchet_key, chain.index, MessageKeys { cipher_key, mac_key, iv }));
            chain.key = next_chain_key;
            chain.index += 1;
        }

        let (next_chain_key, message_key) = kdf_chain(&chain.key);
        let keys = {
            let (cipher_key, mac_key, iv) = derive_message_keys(&message_key);
            MessageKeys { cipher_key, mac_key, iv }
        };
        chain.key = next_chain_key;
        chain.index += 1;
        Ok(keys)
    }

    /// True if this session still has an unacknowledged prekey message pending.
    pub fn has_pending_pre_key(&self) -> bool {
        self.pending_pre_key.is_some()
    }

    /// Serialize the session to raw bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.session_version.to_le_bytes());
        out.extend_from_slice(&self.root_key);
        write_option_keypair(&mut out, self.our_ratchet_key.as_ref());
        write_option_array32(&mut out, self.their_ratchet_key.as_ref());
        write_option_chain(&mut out, self.sending_chain.as_ref());
        write_option_array32(&mut out, self.sending_chain_remote_key.as_ref());
        out.extend_from_slice(&(self.receiving_chains.len() as u32).to_le_bytes());
        for (k, chain) in &self.receiving_chains {
            out.extend_from_slice(k);
            write_chain(&mut out, chain);
        }
        out.extend_from_slice(&(self.skipped_message_keys.len() as u32).to_le_bytes());
        for (k, index, keys) in &self.skipped_message_keys {
            out.extend_from_slice(k);
            out.extend_from_slice(&index.to_le_bytes());
            out.extend_from_slice(&keys.cipher_key);
            out.extend_from_slice(&keys.mac_key);
            out.extend_from_slice(&keys.iv);
        }
        out.extend_from_slice(&self.local_identity);
        write_option_array32(&mut out, self.remote_identity.as_ref());
        write_option_pending_pre_key(&mut out, self.pending_pre_key.as_ref());
        out
    }

    /// Deserialize a session from raw bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut pos = 0;
        let session_version = read_u32(bytes, &mut pos)?;
        let root_key = read_array32(bytes, &mut pos)?;
        let our_ratchet_key = read_option_keypair(bytes, &mut pos)?;
        let their_ratchet_key = read_option_array32(bytes, &mut pos)?;
        let sending_chain = read_option_chain(bytes, &mut pos)?;
        let sending_chain_remote_key = read_option_array32(bytes, &mut pos)?;
        let receiving_chain_count = read_u32(bytes, &mut pos)? as usize;
        let mut receiving_chains = Vec::with_capacity(receiving_chain_count);
        for _ in 0..receiving_chain_count {
            let k = read_array32(bytes, &mut pos)?;
            let chain = read_chain(bytes, &mut pos)?;
            receiving_chains.push((k, chain));
        }
        let skipped_count = read_u32(bytes, &mut pos)? as usize;
        let mut skipped_message_keys = Vec::with_capacity(skipped_count);
        for _ in 0..skipped_count {
            let k = read_array32(bytes, &mut pos)?;
            let index = read_u32(bytes, &mut pos)?;
            let cipher_key = read_array32(bytes, &mut pos)?;
            let mac_key = read_array32(bytes, &mut pos)?;
            let iv = read_array16(bytes, &mut pos)?;
            skipped_message_keys.push((k, index, MessageKeys { cipher_key, mac_key, iv }));
        }
        let local_identity = read_array32(bytes, &mut pos)?;
        let remote_identity = read_option_array32(bytes, &mut pos)?;
        let pending_pre_key = read_option_pending_pre_key(bytes, &mut pos)?;
        Ok(Session {
            session_version,
            root_key,
            our_ratchet_key,
            their_ratchet_key,
            sending_chain,
            sending_chain_remote_key,
            receiving_chains,
            skipped_message_keys,
            local_identity,
            remote_identity,
            pending_pre_key,
        })
    }

    /// Encrypt the serialized session with the given 256-bit key.
    pub fn pickle(&self, key: &[u8; 32]) -> Vec<u8> {
        let plaintext = self.to_bytes();
        let cipher = Aes256Gcm::new_from_slice(key).expect("valid AES-256 key length");
        let nonce_bytes = rand::random::<[u8; 12]>();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).expect("encryption");
        let mut out = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.append(&mut ciphertext);
        out
    }

    /// Decrypt and deserialize a pickled session.
    pub fn unpickle(bytes: &[u8], key: &[u8; 32]) -> Result<Self, Error> {
        if bytes.len() < 12 + 16 {
            return Err(Error::InvalidPickle);
        }
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::InvalidPickle)?;
        let nonce = Nonce::from_slice(&bytes[..12]);
        let plaintext = cipher
            .decrypt(nonce, &bytes[12..])
            .map_err(|_| Error::InvalidPickle)?;
        Self::from_bytes(&plaintext)
    }
}

fn write_option_keypair(out: &mut Vec<u8>, kp: Option<&KeyPair>) {
    if let Some(kp) = kp {
        out.push(1);
        out.extend_from_slice(kp.secret_key_bytes());
    } else {
        out.push(0);
    }
}

fn write_option_array32(out: &mut Vec<u8>, arr: Option<&[u8; 32]>) {
    if let Some(arr) = arr {
        out.push(1);
        out.extend_from_slice(arr);
    } else {
        out.push(0);
    }
}

fn write_chain(out: &mut Vec<u8>, chain: &Chain) {
    out.extend_from_slice(&chain.key);
    out.extend_from_slice(&chain.index.to_le_bytes());
    out.extend_from_slice(&chain.previous_counter.to_le_bytes());
}

fn write_option_chain(out: &mut Vec<u8>, chain: Option<&Chain>) {
    if let Some(chain) = chain {
        out.push(1);
        write_chain(out, chain);
    } else {
        out.push(0);
    }
}

fn write_option_pending_pre_key(out: &mut Vec<u8>, pending: Option<&PendingPreKey>) {
    if let Some(pending) = pending {
        out.push(1);
        out.extend_from_slice(&pending.pre_key_id.to_le_bytes());
        out.extend_from_slice(&pending.signed_pre_key_id.to_le_bytes());
        out.extend_from_slice(&pending.base_key);
    } else {
        out.push(0);
    }
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, Error> {
    if *pos + 4 > bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(&bytes[*pos..*pos + 4]);
    *pos += 4;
    Ok(u32::from_le_bytes(arr))
}

fn read_array32(bytes: &[u8], pos: &mut usize) -> Result<[u8; 32], Error> {
    if *pos + 32 > bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[*pos..*pos + 32]);
    *pos += 32;
    Ok(arr)
}

fn read_array16(bytes: &[u8], pos: &mut usize) -> Result<[u8; 16], Error> {
    if *pos + 16 > bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes[*pos..*pos + 16]);
    *pos += 16;
    Ok(arr)
}

fn read_option_keypair(bytes: &[u8], pos: &mut usize) -> Result<Option<KeyPair>, Error> {
    if *pos >= bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let present = bytes[*pos];
    *pos += 1;
    if present == 0 {
        Ok(None)
    } else {
        let secret = read_array32(bytes, pos)?;
        Ok(Some(KeyPair::from_secret(secret)))
    }
}

fn read_option_array32(bytes: &[u8], pos: &mut usize) -> Result<Option<[u8; 32]>, Error> {
    if *pos >= bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let present = bytes[*pos];
    *pos += 1;
    if present == 0 {
        Ok(None)
    } else {
        Ok(Some(read_array32(bytes, pos)?))
    }
}

fn read_chain(bytes: &[u8], pos: &mut usize) -> Result<Chain, Error> {
    let key = read_array32(bytes, pos)?;
    let index = read_u32(bytes, pos)?;
    let previous_counter = read_u32(bytes, pos)?;
    Ok(Chain {
        key,
        index,
        previous_counter,
    })
}

fn read_option_chain(bytes: &[u8], pos: &mut usize) -> Result<Option<Chain>, Error> {
    if *pos >= bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let present = bytes[*pos];
    *pos += 1;
    if present == 0 {
        Ok(None)
    } else {
        Ok(Some(read_chain(bytes, pos)?))
    }
}

fn read_option_pending_pre_key(
    bytes: &[u8],
    pos: &mut usize,
) -> Result<Option<PendingPreKey>, Error> {
    if *pos >= bytes.len() {
        return Err(Error::InvalidPickle);
    }
    let present = bytes[*pos];
    *pos += 1;
    if present == 0 {
        Ok(None)
    } else {
        let pre_key_id = read_u32(bytes, pos)?;
        let signed_pre_key_id = read_u32(bytes, pos)?;
        let base_key = read_array32(bytes, pos)?;
        Ok(Some(PendingPreKey {
            pre_key_id,
            signed_pre_key_id,
            base_key,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use crate::signal_ratchet::keys::{PreKey, SignedPreKey};

    fn make_bundle(
        identity: &IdentityKeyPair,
        signed_pre_key: &SignedPreKey,
        one_time_pre_key: &PreKey,
    ) -> PreKeyBundle {
        PreKeyBundle {
            registration_id: 1234,
            device_id: 1,
            signed_pre_key_id: signed_pre_key.id,
            signed_pre_key_public: *signed_pre_key.key_pair.public_key_bytes(),
            signed_pre_key_signature: signed_pre_key.signature,
            identity_key: *identity.public_key_bytes(),
            pre_key_id: one_time_pre_key.id,
            pre_key_public: *one_time_pre_key.key_pair.public_key_bytes(),
        }
    }

    #[test]
    fn alice_bob_roundtrip() {
        let mut rng = thread_rng();
        let alice_identity = IdentityKeyPair::generate(&mut rng);
        let bob_identity = IdentityKeyPair::generate(&mut rng);
        let bob_signed_pre_key = SignedPreKey::generate(1, &bob_identity, &mut rng);
        let bob_one_time_pre_key = PreKey::generate(99, &mut rng);
        let bundle = make_bundle(&bob_identity, &bob_signed_pre_key, &bob_one_time_pre_key);

        let mut alice = Session::new_alice(&alice_identity, &bundle, &mut rng).unwrap();

        let first = alice.encrypt(b"hello bob", &mut rng).unwrap();
        let first_bytes = match first {
            CiphertextMessage::PreKey(b) => b,
            _ => panic!("first message must be prekey"),
        };

        let (mut bob, plaintext, used_prekey_id) = Session::new_bob(
            &bob_identity,
            &bob_signed_pre_key.key_pair,
            &bob_one_time_pre_key.key_pair,
            &first_bytes,
        )
        .unwrap();
        assert_eq!(plaintext, b"hello bob");
        assert_eq!(used_prekey_id, 99);

        let reply = bob.encrypt(b"hi alice", &mut rng).unwrap();
        let reply_bytes = match reply {
            CiphertextMessage::Signal(b) => b,
            _ => panic!("reply should be a normal signal message"),
        };

        let decrypted = alice.decrypt(&reply_bytes, false).unwrap();
        assert_eq!(decrypted, b"hi alice");

        let second = alice.encrypt(b"second", &mut rng).unwrap();
        let second_bytes = match second {
            CiphertextMessage::Signal(b) => b,
            _ => panic!("second message should be normal"),
        };
        let decrypted_second = bob.decrypt(&second_bytes, false).unwrap();
        assert_eq!(decrypted_second, b"second");
    }

    #[test]
    fn pickle_roundtrip() {
        let mut rng = thread_rng();
        let alice_identity = IdentityKeyPair::generate(&mut rng);
        let bob_identity = IdentityKeyPair::generate(&mut rng);
        let bob_signed_pre_key = SignedPreKey::generate(1, &bob_identity, &mut rng);
        let bob_one_time_pre_key = PreKey::generate(7, &mut rng);
        let bundle = make_bundle(&bob_identity, &bob_signed_pre_key, &bob_one_time_pre_key);

        let mut alice = Session::new_alice(&alice_identity, &bundle, &mut rng).unwrap();
        let _ = alice.encrypt(b"before pickle", &mut rng).unwrap();

        let key = [0xABu8; 32];
        let pickled = alice.pickle(&key);
        let restored = Session::unpickle(&pickled, &key).unwrap();

        // The restored session should have the same public state and be able to decrypt.
        assert_eq!(
            restored.local_identity,
            alice.local_identity
        );
        assert_eq!(
            restored.remote_identity,
            alice.remote_identity
        );
    }

    #[test]
    fn out_of_order_message() {
        let mut rng = thread_rng();
        let alice_identity = IdentityKeyPair::generate(&mut rng);
        let bob_identity = IdentityKeyPair::generate(&mut rng);
        let bob_signed_pre_key = SignedPreKey::generate(1, &bob_identity, &mut rng);
        let bob_one_time_pre_key = PreKey::generate(7, &mut rng);
        let bundle = make_bundle(&bob_identity, &bob_signed_pre_key, &bob_one_time_pre_key);

        let mut alice = Session::new_alice(&alice_identity, &bundle, &mut rng).unwrap();
        let first = match alice.encrypt(b"1", &mut rng).unwrap() {
            CiphertextMessage::PreKey(b) => b,
            _ => panic!("expected prekey"),
        };
        let second = match alice.encrypt(b"2", &mut rng).unwrap() {
            CiphertextMessage::Signal(b) => b,
            _ => panic!("expected signal"),
        };

        let (mut bob, _, _) = Session::new_bob(
            &bob_identity,
            &bob_signed_pre_key.key_pair,
            &bob_one_time_pre_key.key_pair,
            &first,
        )
        .unwrap();

        let dec_second = bob.decrypt(&second, false).unwrap();
        assert_eq!(dec_second, b"2");
    }
}
