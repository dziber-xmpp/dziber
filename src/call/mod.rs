use std::collections::BTreeMap;
use std::net::SocketAddr;

use aes_gcm::aead::{Aead, generic_array::GenericArray};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use sha2::{Digest, Sha256};
use tokio::net::UdpSocket;

#[derive(Debug, Clone)]
pub struct IceCandidate {
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

impl RtpPacket {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.payload.len());
        out.push(0x80);
        out.push(self.payload_type & 0x7f);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 12 {
            return None;
        }
        let payload_type = bytes[1] & 0x7f;
        let sequence = u16::from_be_bytes([bytes[2], bytes[3]]);
        let timestamp = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let ssrc = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let payload = bytes[12..].to_vec();
        Some(Self {
            payload_type,
            sequence,
            timestamp,
            ssrc,
            payload,
        })
    }
}

#[derive(Debug)]
pub struct JitterBuffer {
    next_seq: Option<u16>,
    queued: BTreeMap<u16, RtpPacket>,
    max_depth: usize,
}

impl JitterBuffer {
    pub fn new(max_depth: usize) -> Self {
        Self {
            next_seq: None,
            queued: BTreeMap::new(),
            max_depth,
        }
    }

    pub fn push(&mut self, pkt: RtpPacket) {
        self.queued.insert(pkt.sequence, pkt);
        if self.queued.len() > self.max_depth
            && let Some(first) = self.queued.keys().next().copied()
        {
            self.queued.remove(&first);
        }
    }

    pub fn pop_ready(&mut self) -> Option<RtpPacket> {
        let next = match self.next_seq {
            Some(v) => v,
            None => {
                let first = *self.queued.keys().next()?;
                self.next_seq = Some(first);
                first
            }
        };
        let pkt = self.queued.remove(&next)?;
        self.next_seq = Some(next.wrapping_add(1));
        Some(pkt)
    }
}

#[derive(Clone)]
pub struct SrtpContext {
    cipher: Aes256Gcm,
    salt: [u8; 12],
}

impl SrtpContext {
    pub fn new(shared_secret: &[u8]) -> Self {
        let digest = Sha256::digest(shared_secret);
        let key = &digest[..32];
        let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
        let mut salt = [0u8; 12];
        salt.copy_from_slice(&digest[..12]);
        Self { cipher, salt }
    }

    fn nonce_for(&self, seq: u16) -> GenericArray<u8, <Aes256Gcm as aes_gcm::aead::AeadCore>::NonceSize> {
        let mut n = self.salt;
        n[10] ^= (seq >> 8) as u8;
        n[11] ^= (seq & 0xff) as u8;
        *Nonce::from_slice(&n)
    }

    pub fn protect(&self, pkt: &RtpPacket) -> Option<Vec<u8>> {
        let pt = pkt.to_bytes();
        self.cipher.encrypt(&self.nonce_for(pkt.sequence), pt.as_ref()).ok()
    }

    pub fn unprotect(&self, seq_hint: u16, data: &[u8]) -> Option<RtpPacket> {
        let pt = self
            .cipher
            .decrypt(&self.nonce_for(seq_hint), data)
            .ok()?;
        RtpPacket::from_bytes(&pt)
    }
}

pub struct MediaSession {
    socket: UdpSocket,
    remote: SocketAddr,
    srtp: SrtpContext,
    jitter: JitterBuffer,
    seq: u16,
    ts: u32,
    ssrc: u32,
}

impl MediaSession {
    pub async fn start(
        local_bind: SocketAddr,
        remote: SocketAddr,
        shared_secret: &[u8],
    ) -> Result<Self, String> {
        let socket = UdpSocket::bind(local_bind)
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        Ok(Self {
            socket,
            remote,
            srtp: SrtpContext::new(shared_secret),
            jitter: JitterBuffer::new(128),
            seq: 1,
            ts: 1,
            ssrc: rand::random(),
        })
    }

    pub async fn send_pcm_frame(&mut self, pcm: &[u8]) -> Result<(), String> {
        let pkt = RtpPacket {
            payload_type: 111,
            sequence: self.seq,
            timestamp: self.ts,
            ssrc: self.ssrc,
            payload: pcm.to_vec(),
        };
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(960);
        let protected = self
            .srtp
            .protect(&pkt)
            .ok_or_else(|| "srtp protect failed".to_string())?;
        self.socket
            .send_to(&protected, self.remote)
            .await
            .map_err(|e| format!("send failed: {}", e))?;
        Ok(())
    }

    pub async fn recv_frame(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut buf = [0u8; 2048];
        let (n, _) = self
            .socket
            .recv_from(&mut buf)
            .await
            .map_err(|e| format!("recv failed: {}", e))?;
        let data = &buf[..n];
        let pkt = self
            .srtp
            .unprotect(self.seq, data)
            .ok_or_else(|| "srtp unprotect failed".to_string())?;
        self.jitter.push(pkt);
        Ok(self.jitter.pop_ready().map(|p| p.payload))
    }
}

pub fn candidate_to_addr(c: &IceCandidate) -> Option<SocketAddr> {
    format!("{}:{}", c.ip, c.port).parse().ok()
}
