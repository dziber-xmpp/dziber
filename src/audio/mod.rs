use maolan_engine::client::Client as MaolanClient;
use maolan_engine::message::{Action as MaolanAction, Message as MaolanMessage};
use tokio::sync::mpsc;

use crate::call::{IceCandidate as NetIceCandidate, MediaSession, candidate_to_addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCallState {
    Idle,
    Active,
}

#[derive(Debug)]
pub struct AudioEngine {
    state: AudioCallState,
    active_sid: Option<String>,
    peer_jid: Option<String>,
    remote_candidates: Vec<AudioIceCandidate>,
    maolan: Option<MaolanClient>,
    transport_connected: bool,
    hw_opened: bool,
    media_task: Option<tokio::task::JoinHandle<()>>,
    media_stop: Option<mpsc::Sender<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioIceCandidate {
    pub foundation: String,
    pub component: u32,
    pub protocol: String,
    pub priority: u64,
    pub ip: String,
    pub port: u16,
    pub typ: String,
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            state: AudioCallState::Idle,
            active_sid: None,
            peer_jid: None,
            remote_candidates: Vec::new(),
            maolan: None,
            transport_connected: false,
            hw_opened: false,
            media_task: None,
            media_stop: None,
        }
    }

    pub fn start_call(&mut self, peer_jid: &str, sid: &str) -> Result<(), String> {
        if self.state == AudioCallState::Active {
            return Ok(());
        }
        self.active_sid = Some(sid.to_string());
        self.peer_jid = Some(peer_jid.to_string());
        self.remote_candidates.clear();
        self.transport_connected = false;
        self.hw_opened = false;
        self.stop_media_task();
        self.maolan = Some(MaolanClient::default());
        self.state = AudioCallState::Active;
        Ok(())
    }

    pub fn stop_call(&mut self) {
        self.active_sid = None;
        self.peer_jid = None;
        self.remote_candidates.clear();
        self.transport_connected = false;
        self.hw_opened = false;
        self.stop_media_task();
        self.maolan = None;
        self.state = AudioCallState::Idle;
    }

    pub fn state(&self) -> AudioCallState {
        self.state
    }

    pub fn local_candidates(&self) -> Vec<AudioIceCandidate> {
        // Placeholder candidate set until full ICE gatherer integration lands.
        vec![AudioIceCandidate {
            foundation: "1".to_string(),
            component: 1,
            protocol: "udp".to_string(),
            priority: 2_130_706_431,
            ip: "127.0.0.1".to_string(),
            port: 5000,
            typ: "host".to_string(),
        }]
    }

    pub fn apply_remote_candidates(&mut self, candidates: Vec<AudioIceCandidate>) {
        self.remote_candidates.extend(candidates);
        if !self.remote_candidates.is_empty() {
            self.transport_connected = true;
            self.bind_media_streams();
            self.start_media_task();
        }
    }

    pub fn bind_media_streams(&mut self) {
        if self.state != AudioCallState::Active || self.hw_opened || !self.transport_connected {
            return;
        }
        let Some(client) = self.maolan.clone() else {
            return;
        };
        self.hw_opened = true;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                // Bring up real local capture/playback path through maolan-engine.
                let _ = client
                    .send(MaolanMessage::Request(MaolanAction::OpenAudioDevice {
                        device: "default".to_string(),
                        input_device: Some("default".to_string()),
                        sample_rate_hz: 48000,
                        bits: 16,
                        exclusive: false,
                        period_frames: 480,
                        nperiods: 2,
                        sync_mode: false,
                    }))
                    .await;
            });
        }
    }

    fn stop_media_task(&mut self) {
        if let Some(tx) = self.media_stop.take() {
            let _ = tx.try_send(());
        }
        if let Some(task) = self.media_task.take() {
            task.abort();
        }
    }

    fn start_media_task(&mut self) {
        if self.media_task.is_some() {
            return;
        }
        let Some(remote) = self.remote_candidates.first().cloned() else {
            return;
        };
        let Some(sid) = self.active_sid.clone() else {
            return;
        };
        let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
            return;
        };
        let Some(remote_addr) = candidate_to_addr(&NetIceCandidate {
            ip: remote.ip,
            port: remote.port,
        }) else {
            return;
        };

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        let task = handle.spawn(async move {
            let local_bind = "0.0.0.0:0"
                .parse()
                .expect("static local bind addr is valid");
            let mut session = match MediaSession::start(local_bind, remote_addr, sid.as_bytes()).await
            {
                Ok(s) => s,
                Err(_) => return,
            };

            loop {
                tokio::select! {
                    _ = stop_rx.recv() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                        let silence = [0u8; 320];
                        let _ = session.send_pcm_frame(&silence).await;
                    }
                    res = session.recv_frame() => {
                        if res.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        self.media_stop = Some(stop_tx);
        self.media_task = Some(task);
    }
}
