//! Minimal protobuf encoder/decoder for the libsignal v3 messages used by
//! legacy OMEMO (v0).

use crate::signal_ratchet::Error;

const WIRE_TYPE_VARINT: u8 = 0;
const WIRE_TYPE_LEN_DELIM: u8 = 2;

fn encode_varint(value: u32) -> Vec<u8> {
    let mut v = value;
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

fn decode_varint(buf: &[u8], pos: &mut usize) -> Result<u32, Error> {
    let mut value = 0u32;
    let mut shift = 0;
    loop {
        if *pos >= buf.len() {
            return Err(Error::Proto("truncated varint".into()));
        }
        let b = buf[*pos];
        *pos += 1;
        value |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 28 {
            return Err(Error::Proto("varint overflow".into()));
        }
    }
}

fn encode_field_tag(field: u32, wire: u8) -> u8 {
    (((field & 0x0F) << 3) | (wire & 0x07) as u32) as u8
}

fn encode_bytes_field(field: u32, data: &[u8], out: &mut Vec<u8>) {
    out.push(encode_field_tag(field, WIRE_TYPE_LEN_DELIM));
    out.extend_from_slice(&encode_varint(data.len() as u32));
    out.extend_from_slice(data);
}

fn encode_uint32_field(field: u32, value: u32, out: &mut Vec<u8>) {
    out.push(encode_field_tag(field, WIRE_TYPE_VARINT));
    out.extend_from_slice(&encode_varint(value));
}

/// A decrypted/parsed SignalMessage (WhisperMessage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalMessage {
    pub ratchet_key: [u8; 32],
    pub counter: u32,
    pub previous_counter: u32,
    pub ciphertext: Vec<u8>,
}

impl SignalMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut ratchet_key = [0u8; 33];
        ratchet_key[0] = 0x05;
        ratchet_key[1..].copy_from_slice(&self.ratchet_key);
        encode_bytes_field(1, &ratchet_key, &mut out);
        encode_uint32_field(2, self.counter, &mut out);
        encode_uint32_field(3, self.previous_counter, &mut out);
        encode_bytes_field(4, &self.ciphertext, &mut out);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, Error> {
        let mut ratchet_key: Option<[u8; 32]> = None;
        let mut counter: Option<u32> = None;
        let mut previous_counter: Option<u32> = None;
        let mut ciphertext: Option<Vec<u8>> = None;

        let mut pos = 0;
        while pos < buf.len() {
            let tag = buf[pos];
            pos += 1;
            let field = (tag >> 3) as u32;
            let wire = tag & 0x07;

            match (field, wire) {
                (1, WIRE_TYPE_LEN_DELIM) => {
                    let len = decode_varint(buf, &mut pos)? as usize;
                    if pos + len > buf.len() || !(len == 32 || len == 33) {
                        return Err(Error::Proto("invalid ratchetKey length".into()));
                    }
                    let start = if len == 33 && buf[pos] == 0x05 { pos + 1 } else { pos };
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&buf[start..start + 32]);
                    pos += len;
                    ratchet_key = Some(key);
                }
                (2, WIRE_TYPE_VARINT) => {
                    counter = Some(decode_varint(buf, &mut pos)?);
                }
                (3, WIRE_TYPE_VARINT) => {
                    previous_counter = Some(decode_varint(buf, &mut pos)?);
                }
                (4, WIRE_TYPE_LEN_DELIM) => {
                    let len = decode_varint(buf, &mut pos)? as usize;
                    if pos + len > buf.len() {
                        return Err(Error::Proto("ciphertext truncated".into()));
                    }
                    ciphertext = Some(buf[pos..pos + len].to_vec());
                    pos += len;
                }
                _ => return Err(Error::Proto(format!("unexpected field {field} wire {wire}"))),
            }
        }

        Ok(SignalMessage {
            ratchet_key: ratchet_key.ok_or_else(|| Error::Proto("missing ratchetKey".into()))?,
            counter: counter.ok_or_else(|| Error::Proto("missing counter".into()))?,
            previous_counter: previous_counter.unwrap_or(0),
            ciphertext: ciphertext.ok_or_else(|| Error::Proto("missing ciphertext".into()))?,
        })
    }
}

/// A decrypted/parsed PreKeySignalMessage (PreKeyWhisperMessage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreKeySignalMessage {
    pub registration_id: u32,
    pub pre_key_id: u32,
    pub signed_pre_key_id: u32,
    pub base_key: [u8; 32],
    pub identity_key: [u8; 32],
    pub message: Vec<u8>, // serialized SignalMessage (version + protobuf + mac)
}

impl PreKeySignalMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        encode_uint32_field(1, self.pre_key_id, &mut out);
        let mut base_key = [0u8; 33];
        base_key[0] = 0x05;
        base_key[1..].copy_from_slice(&self.base_key);
        encode_bytes_field(2, &base_key, &mut out);
        let mut identity_key = [0u8; 33];
        identity_key[0] = 0x05;
        identity_key[1..].copy_from_slice(&self.identity_key);
        encode_bytes_field(3, &identity_key, &mut out);
        encode_bytes_field(4, &self.message, &mut out);
        encode_uint32_field(5, self.registration_id, &mut out);
        encode_uint32_field(6, self.signed_pre_key_id, &mut out);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, Error> {
        let mut registration_id: Option<u32> = None;
        let mut pre_key_id: Option<u32> = None;
        let mut signed_pre_key_id: Option<u32> = None;
        let mut base_key: Option<[u8; 32]> = None;
        let mut identity_key: Option<[u8; 32]> = None;
        let mut message: Option<Vec<u8>> = None;

        let mut pos = 0;
        while pos < buf.len() {
            let tag = buf[pos];
            pos += 1;
            let field = (tag >> 3) as u32;
            let wire = tag & 0x07;

            match (field, wire) {
                (1, WIRE_TYPE_VARINT) => {
                    pre_key_id = Some(decode_varint(buf, &mut pos)?);
                }
                (2, WIRE_TYPE_LEN_DELIM) => {
                    let len = decode_varint(buf, &mut pos)? as usize;
                    if pos + len > buf.len() || !(len == 32 || len == 33) {
                        return Err(Error::Proto("invalid baseKey length".into()));
                    }
                    let start = if len == 33 && buf[pos] == 0x05 { pos + 1 } else { pos };
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&buf[start..start + 32]);
                    pos += len;
                    base_key = Some(key);
                }
                (3, WIRE_TYPE_LEN_DELIM) => {
                    let len = decode_varint(buf, &mut pos)? as usize;
                    if pos + len > buf.len() || !(len == 32 || len == 33) {
                        return Err(Error::Proto("invalid identityKey length".into()));
                    }
                    let start = if len == 33 && buf[pos] == 0x05 { pos + 1 } else { pos };
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&buf[start..start + 32]);
                    pos += len;
                    identity_key = Some(key);
                }
                (4, WIRE_TYPE_LEN_DELIM) => {
                    let len = decode_varint(buf, &mut pos)? as usize;
                    if pos + len > buf.len() {
                        return Err(Error::Proto("message truncated".into()));
                    }
                    message = Some(buf[pos..pos + len].to_vec());
                    pos += len;
                }
                (5, WIRE_TYPE_VARINT) => {
                    registration_id = Some(decode_varint(buf, &mut pos)?);
                }
                (6, WIRE_TYPE_VARINT) => {
                    signed_pre_key_id = Some(decode_varint(buf, &mut pos)?);
                }
                _ => return Err(Error::Proto(format!("unexpected field {field} wire {wire}"))),
            }
        }

        Ok(PreKeySignalMessage {
            registration_id: registration_id.ok_or_else(|| Error::Proto("missing registrationId".into()))?,
            pre_key_id: pre_key_id.ok_or_else(|| Error::Proto("missing preKeyId".into()))?,
            signed_pre_key_id: signed_pre_key_id.ok_or_else(|| Error::Proto("missing signedPreKeyId".into()))?,
            base_key: base_key.ok_or_else(|| Error::Proto("missing baseKey".into()))?,
            identity_key: identity_key.ok_or_else(|| Error::Proto("missing identityKey".into()))?,
            message: message.ok_or_else(|| Error::Proto("missing message".into()))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_message_roundtrip() {
        let msg = SignalMessage {
            ratchet_key: [1u8; 32],
            counter: 7,
            previous_counter: 3,
            ciphertext: vec![9, 8, 7],
        };
        let encoded = msg.encode();
        let decoded = SignalMessage::decode(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn pre_key_signal_message_roundtrip() {
        let msg = PreKeySignalMessage {
            registration_id: 1234,
            pre_key_id: 99,
            signed_pre_key_id: 5,
            base_key: [2u8; 32],
            identity_key: [3u8; 32],
            message: vec![0x33, 1, 2, 3, 4, 5, 6, 7, 8],
        };
        let encoded = msg.encode();
        let decoded = PreKeySignalMessage::decode(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }
}
