use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use futures::FutureExt;
use futures::channel::mpsc;
use futures::sink::SinkExt;
use futures::stream::{Stream, StreamExt};
use iced::stream;
use rand::Rng;
use tokio_xmpp::minidom::Element;
use tokio_xmpp::{Client, Event as XmppEventRaw, Stanza};
use xmpp_parsers::iq::Iq;
use xmpp_parsers::jid::{BareJid, Jid};
use xmpp_parsers::message::{Lang, Message as XmppMessage, MessageType};
use xmpp_parsers::presence::{Presence as XmppPresence, Show as XmppShow, Type as PresenceType};
use xmpp_parsers::roster::Roster;
use xmpp_parsers::stanza_error::DefinedCondition;
use xmpp_parsers::vcard::VCard;

use chrono::{DateTime, Utc};

use crate::models::contact::{Contact, Presence, Show, Subscription};
use crate::models::message::{Direction, Message, MessageStatus};
use crate::omemo::OmemoManager;
use crate::omemo::bundle::{Bundle, build_bundle_element_v0 as build_bundle_element, parse_bundle};
use crate::omemo::device::{
    Device, build_device_list_element_v0 as build_device_list_element, parse_device_list,
};
use crate::omemo::message::{build_message_stanza, parse_encrypted_message};
use crate::omemo::{NS_OMEMO_V0, NS_OMEMO_V0_BUNDLES, NS_OMEMO_V0_DEVICES};
use vodozemac::{Curve25519PublicKey, Curve25519SecretKey};

const NS_CARBONS: &str = "urn:xmpp:carbons:2";
const NS_FORWARD: &str = "urn:xmpp:forward:0";

#[derive(Debug, Clone)]
pub enum XmppCommand {
    Connect {
        jid: String,
        password: String,
    },
    Disconnect,
    SendMessage {
        to: String,
        body: String,
        omemo: bool,
    },
    SendFile {
        to: String,
        path: String,
    },
    FetchDeviceList {
        jid: String,
    },
    FetchAvatar {
        jid: String,
    },
}

#[derive(Debug, Clone)]
pub enum XmppEvent {
    Ready(mpsc::Sender<XmppCommand>),
    Connected {
        jid: String,
    },
    Disconnected,
    ConnectionError(String),
    RosterItem(Contact),
    PresenceUpdate {
        jid: String,
        presence: Presence,
    },
    MessageReceived(Message),
    MessageSent {
        _id: String,
    },
    StatusChanged(String),
    BundleReceived,
    OmemoMessageReceived {
        from: String,
        body: String,
        direction: Direction,
    },
    AvatarReceived {
        jid: String,
        bytes: Vec<u8>,
    },
}

pub fn run_xmpp_worker() -> impl Stream<Item = XmppEvent> {
    stream::channel(100, async |mut output: mpsc::Sender<XmppEvent>| {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<XmppCommand>(100);
        let _ = output.send(XmppEvent::Ready(cmd_tx)).await;

        let mut client: Option<Client> = None;
        let mut omemo: Option<OmemoManager> = None;
        let mut pending_iqs: HashMap<String, PendingIq> = HashMap::new();
        let mut our_jid: Option<String> = None;
        let mut stream_healthy = true;

        loop {
            if let Some(ref mut c) = client {
                tokio::select! {
                    biased;

                    event = c.next() => {
                        match event {
                            Some(XmppEventRaw::Online { bound_jid, .. }) => {
                                stream_healthy = true;
                                let jid_str = bound_jid.to_string();
                                our_jid = Some(jid_str.clone());
                                let _ = output.send(XmppEvent::Connected { jid: jid_str.clone() }).await;
                                let _ = output.send(XmppEvent::StatusChanged("Online".to_string())).await;

                                // Initialize OMEMO
                                let mut mgr = OmemoManager::load_or_generate(rand::random());
                                mgr.set_our_jid(&bound_jid.to_bare().to_string());
                                let _device_id = mgr.our_device_id();

                                // Publish bundle to v0 and v0 (Conversations compat)
                                let pubsub_jid: Jid = bound_jid.to_bare().into();
                                let current_otk_count = mgr.account.all_stored_one_time_keys().len();
                                tracing::info!(
                                    "[OMEMO] Keeping existing one-time key set stable (count={})",
                                    current_otk_count
                                );
                                if let Some(bundle_iq) = build_bundle_iq(&mgr, &pubsub_jid) {
                                    let bundle_id = match &bundle_iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !bundle_id.is_empty() {
                                        pending_iqs.insert(bundle_id, PendingIq::BundlePublish { jid: pubsub_jid.to_string(), version: String::from("v0") });
                                    }
                                    tracing::info!("[OMEMO] Sending bundle publish (v0) for {}", pubsub_jid);
                                let _ = safe_send_stanza(c, bundle_iq.into(), "worker-loop", &mut stream_healthy).await;
                                } else {
                                    tracing::info!("[OMEMO] Bundle publish (v0) skipped: no fallback key");
                                }
                                if let Some(bundle_iq_v0) = build_bundle_iq_v0(&mgr, &pubsub_jid) {
                                    let bundle_id = match &bundle_iq_v0 {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !bundle_id.is_empty() {
                                        pending_iqs.insert(bundle_id, PendingIq::BundlePublish { jid: pubsub_jid.to_string(), version: String::from("v0") });
                                    }
                                    tracing::info!("[OMEMO] Sending bundle publish (v0) for {}", pubsub_jid);
                                    let _ = safe_send_stanza(c, bundle_iq_v0.into(), "worker-loop", &mut stream_healthy).await;
                                } else {
                                    tracing::info!("[OMEMO] Bundle publish (v0) skipped: no fallback key");
                                }
                                let _ = mgr.save();
                                omemo = Some(mgr);

                                // Fetch our own device list for multi-device / carbon support
                                let own_bare = bound_jid.to_bare().to_string();
                                let own_dev_iq = build_device_list_fetch_iq(&own_bare, OmemoVersion::V0);
                                let own_dev_id = match &own_dev_iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !own_dev_id.is_empty() {
                                    pending_iqs.insert(own_dev_id, PendingIq::DeviceList { jid: own_bare, version: OmemoVersion::V0, accumulated: Some(vec![]) });
                                }
                                let _ = safe_send_stanza(c, own_dev_iq.into(), "worker-loop", &mut stream_healthy).await;

                                // Send initial presence
                                let presence = make_presence();
                                let _ = safe_send_stanza(c, presence.into(), "worker-loop", &mut stream_healthy).await;

                                // Request roster
                                let roster_iq = Iq::from_get("roster-get", Roster { ver: None, items: vec![] });
                                tracing::info!("[ROSTER] Sending roster request");
                                let _ = safe_send_stanza(c, roster_iq.into(), "worker-loop", &mut stream_healthy).await;

                                // Enable Message Carbons (XEP-0280)
                                let carbons_iq = build_carbons_enable_iq();
                                let _ = safe_send_stanza(c, carbons_iq.into(), "worker-loop", &mut stream_healthy).await;

                                // Fetch recent MAM history (XEP-0313)
                                let mam_query = xmpp_parsers::mam::Query {
                                    queryid: Some(xmpp_parsers::mam::QueryId("mam-sync".to_string())),
                                    node: None,
                                    form: None,
                                    set: Some(xmpp_parsers::rsm::SetQuery {
                                        max: Some(50),
                                        after: None,
                                        before: None,
                                        index: None,
                                    }),
                                    flip_page: false,
                                };
                                let mam_iq = Iq::from_get("mam-query", mam_query);
                                tracing::info!("[MAM] Sending query: max=50");
                                let _ = safe_send_stanza(c, mam_iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppEventRaw::Stanza(stanza)) => {
                                handle_stanza(stanza, c, &mut output, &mut omemo, &mut pending_iqs, our_jid.as_deref(), &mut stream_healthy).await;
                            }
                            Some(XmppEventRaw::Disconnected(err)) => {
                                stream_healthy = false;
                                if let Some(ref mgr) = omemo {
                                    let _ = mgr.save();
                                }
                                let _ = output.send(XmppEvent::ConnectionError(err.to_string())).await;
                                let _ = output.send(XmppEvent::Disconnected).await;
                                client = None;
                                omemo = None;
                            }
                            None => {
                                stream_healthy = false;
                                if let Some(ref mgr) = omemo {
                                    let _ = mgr.save();
                                }
                                let _ = output.send(XmppEvent::Disconnected).await;
                                client = None;
                                omemo = None;
                            }
                        }
                    }

                    cmd = cmd_rx.next() => {
                        match cmd {
                            Some(XmppCommand::Connect { .. }) => {}
                            Some(XmppCommand::SendMessage { to, body, omemo: use_omemo }) => {
                                if let Some(ref mut mgr) = omemo
                                    && use_omemo
                                {
                                    match mgr.encrypt_message(&to, &body) {
                                        Some(encrypted) => {
                                            tracing::info!("[SEND] OMEMO encrypt ok for {}", to);
                                            let msg = build_message_stanza(
                                                &to,
                                                &encrypted,
                                                &uuid::Uuid::new_v4().to_string(),
                                            );
                                            if let Some(xml) = xmpp_message_to_xml(&msg) {
                                                tracing::info!("[SEND XML] {}", xml);
                                            }
                                            let _ = safe_send_stanza(
                                                c,
                                                msg.into(),
                                                "worker-loop",
                                                &mut stream_healthy,
                                            )
                                            .await;
                                            tracing::info!("[SEND] OMEMO stanza dispatched to {}", to);
                                            let _ = output
                                                .send(XmppEvent::MessageSent {
                                                    _id: uuid::Uuid::new_v4().to_string(),
                                                })
                                                .await;
                                            continue;
                                        }
                                        None => {
                                            tracing::info!("[SEND] OMEMO encrypt failed for {}", to);
                                        }
                                    }
                                }
                                let msg = make_message(&to, &body);
                                tracing::info!("[SEND] Plaintext stanza dispatched to {}", to);
                                let _ = safe_send_stanza(c, msg.into(), "worker-loop", &mut stream_healthy).await;
                                let _ = output.send(XmppEvent::MessageSent { _id: uuid::Uuid::new_v4().to_string() }).await;
                            }
                            Some(XmppCommand::SendFile { to, path }) => {
                                let file_path = PathBuf::from(path.clone());
                                let Some(filename) = file_path.file_name().and_then(|n| n.to_str()).map(ToOwned::to_owned) else {
                                    let _ = output.send(XmppEvent::StatusChanged("File send failed: invalid file path".to_string())).await;
                                    continue;
                                };
                                let metadata = match std::fs::metadata(&file_path) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!("File send failed: {}", e))).await;
                                        continue;
                                    }
                                };
                                let size = metadata.len();
                                let content_type = guess_content_type(&file_path);
                                let services = our_jid
                                    .as_deref()
                                    .map(candidate_upload_services)
                                    .unwrap_or_default();
                                if services.is_empty() {
                                    let _ = output.send(XmppEvent::StatusChanged("File send failed: not connected".to_string())).await;
                                    continue;
                                }
                                let request_id = format!("upload-slot-{}", uuid::Uuid::new_v4());
                                if !send_http_upload_slot_request(
                                    c,
                                    &request_id,
                                    &services[0],
                                    &filename,
                                    size,
                                    &content_type,
                                    &mut stream_healthy,
                                )
                                .await
                                {
                                    let _ = output.send(XmppEvent::StatusChanged("File send failed: could not request upload slot".to_string())).await;
                                    continue;
                                }
                                pending_iqs.insert(
                                    request_id,
                                    PendingIq::HttpUploadSlot {
                                        to,
                                        path: file_path,
                                        filename,
                                        size,
                                        content_type,
                                        services,
                                        service_idx: 0,
                                    },
                                );
                                let _ = output.send(XmppEvent::StatusChanged("Uploading file...".to_string())).await;
                            }
                            Some(XmppCommand::Disconnect) => {
                                if let Some(ref mgr) = omemo {
                                    let _ = mgr.save();
                                }
                                stream_healthy = false;
                                pending_iqs.clear();
                                let taken = client.take();
                                if let Some(c) = taken {
                                    let _ = c.send_end().await;
                                }
                                omemo = None;
                                let _ = output.send(XmppEvent::Disconnected).await;
                            }
                            Some(XmppCommand::FetchDeviceList { jid }) => {
                                tracing::info!("[OMEMO] FetchDeviceList command received for {}", jid);
                                let iq = build_device_list_fetch_iq(&jid, OmemoVersion::V0);
                                let id = match &iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !id.is_empty() {
                                    pending_iqs.insert(id, PendingIq::DeviceList { jid: jid.clone(), version: OmemoVersion::V0, accumulated: None });
                                }
                                tracing::info!("[OMEMO] Sending device list fetch IQ (v0) to {}", jid);
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppCommand::FetchAvatar { jid }) => {
                                tracing::info!("[AVATAR] FetchAvatar command received for {}", jid);
                                let iq = build_vcard_fetch_iq(&jid);
                                let id = match &iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !id.is_empty() {
                                    pending_iqs.insert(id, PendingIq::VCardAvatar { jid: jid.clone() });
                                }
                                tracing::info!("[AVATAR] Sending vCard IQ to {}", jid);
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            None => {
                                break;
                            }
                        }
                    }
                }
            } else {
                match cmd_rx.next().await {
                    Some(XmppCommand::SendMessage { .. }) => {}
                    Some(XmppCommand::SendFile { .. }) => {}
                    Some(XmppCommand::FetchDeviceList { .. }) => {}
                    Some(XmppCommand::FetchAvatar { .. }) => {}
                    Some(XmppCommand::Connect { jid, password }) => {
                        let _ = output
                            .send(XmppEvent::StatusChanged("Connecting...".to_string()))
                            .await;
                        let _ =
                            tokio_xmpp::rustls::crypto::ring::default_provider().install_default();

                        let connect_jid = match Jid::from_str(&jid) {
                            Ok(full) => full,
                            Err(_) => match BareJid::from_str(&jid) {
                                Ok(bare) => {
                                    let requested =
                                        format!("{}/{}", bare, random_dziber_resource());
                                    match Jid::from_str(&requested) {
                                        Ok(j) => j,
                                        Err(e) => {
                                            let _ = output
                                                .send(XmppEvent::ConnectionError(format!(
                                                    "Invalid JID: {}",
                                                    e
                                                )))
                                                .await;
                                            continue;
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = output
                                        .send(XmppEvent::ConnectionError(format!(
                                            "Invalid JID: {}",
                                            e
                                        )))
                                        .await;
                                    continue;
                                }
                            },
                        };

                        let new_client = Client::new(connect_jid, password);
                        client = Some(new_client);
                    }
                    Some(XmppCommand::Disconnect) => {}
                    None => break,
                }
            }
        }
    })
}

async fn safe_send_stanza(
    client: &mut Client,
    stanza: Stanza,
    context: &str,
    stream_healthy: &mut bool,
) -> bool {
    let send_future = client.send_stanza(stanza);
    if !*stream_healthy {
        return false;
    }
    match std::panic::AssertUnwindSafe(send_future)
        .catch_unwind()
        .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            *stream_healthy = false;
            tracing::info!("[XMPP] send_stanza failed in {}: {}", context, err);
            false
        }
        Err(_) => {
            *stream_healthy = false;
            tracing::info!(
                "[XMPP] send_stanza panicked in {} (stream closed/background worker crashed)",
                context
            );
            false
        }
    }
}

fn random_dziber_resource() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let suffix: String = (0..10)
        .map(|_| {
            let idx = rng.gen_range(0..ALPHABET.len());
            ALPHABET[idx] as char
        })
        .collect();
    format!("dziber.{suffix}")
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OmemoVersion {
    V0,
}

impl OmemoVersion {
    fn next(self) -> Option<Self> {
        match self {
            Self::V0 => None,
        }
    }

    fn ns_devices(self) -> &'static str {
        match self {
            Self::V0 => NS_OMEMO_V0_DEVICES,
        }
    }

    fn ns_bundles(self) -> &'static str {
        match self {
            Self::V0 => NS_OMEMO_V0_BUNDLES,
        }
    }
}

#[derive(Debug)]
enum PendingIq {
    Bundle {
        jid: String,
        version: OmemoVersion,
    },
    BundleDevice {
        jid: String,
        device_id: u32,
    },
    DeviceList {
        jid: String,
        version: OmemoVersion,
        accumulated: Option<Vec<u32>>,
    },
    DeviceListPublish {
        jid: String,
        version: String,
    },
    BundlePublish {
        jid: String,
        version: String,
    },
    ConfigureNode {
        jid: String,
        node: String,
        devices: Vec<u32>,
        version: String,
    },
    PurgeV0DeviceList {
        jid: String,
        devices: Vec<u32>,
    },
    AvatarMetadata {
        jid: String,
    },
    AvatarData {
        jid: String,
    },
    VCardAvatar {
        jid: String,
    },
    HttpUploadSlot {
        to: String,
        path: PathBuf,
        filename: String,
        size: u64,
        content_type: String,
        services: Vec<String>,
        service_idx: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarbonType {
    Received,
    Sent,
}

fn extract_carbon_message(msg: &XmppMessage) -> Option<(CarbonType, XmppMessage)> {
    for payload in &msg.payloads {
        let carbon_type = if payload.name() == "received" && payload.ns() == NS_CARBONS {
            Some(CarbonType::Received)
        } else if payload.name() == "sent" && payload.ns() == NS_CARBONS {
            Some(CarbonType::Sent)
        } else {
            None
        };

        if let Some(ct) = carbon_type {
            for child in payload.children() {
                if child.name() == "forwarded" && child.ns() == NS_FORWARD {
                    for inner in child.children() {
                        if inner.name() == "message"
                            && let Ok(fwd_msg) = XmppMessage::try_from(inner.clone())
                        {
                            return Some((ct, fwd_msg));
                        }
                    }
                }
            }
        }
    }
    None
}

fn xmpp_message_to_xml(msg: &XmppMessage) -> Option<String> {
    let el: Element = msg.clone().into();
    let mut bytes = Vec::new();
    el.write_to(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn build_carbons_enable_iq() -> Iq {
    let enable = Element::builder("enable", NS_CARBONS).build();
    Iq::Set {
        id: String::from("carbons-enable"),
        from: None,
        to: None,
        payload: enable,
    }
}

async fn process_message(
    msg: XmppMessage,
    output: &mut mpsc::Sender<XmppEvent>,
    omemo: &mut Option<OmemoManager>,
    direction: Direction,
    from: Jid,
    timestamp: Option<DateTime<Utc>>,
    our_jid: Option<&str>,
) {
    fn hex(data: &[u8]) -> String {
        data.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join("")
    }

    let from_bare = from.to_bare().to_string();

    // Check for OMEMO payload (v0)
    let mut omemo_decrypted = None;
    for payload in &msg.payloads {
        if payload.name() == "encrypted"
            && (payload.ns() == NS_OMEMO_V0)
            && let Some(encrypted) = parse_encrypted_message(payload)
            && let Some(mgr) = omemo
        {
            tracing::info!(
                "[OMEMO raw] from={} ns={} sid={} groups={} payload_len={} iv_len={:?}",
                from_bare,
                payload.ns(),
                encrypted.header.sid,
                encrypted.header.keys.len(),
                encrypted.payload.as_ref().map(|p| p.len()).unwrap_or(0),
                encrypted.header.iv.as_ref().map(|v| v.len()),
            );
            for g in &encrypted.header.keys {
                tracing::info!("[OMEMO raw] group_jid={} key_count={}", g.jid, g.keys.len());
                for k in &g.keys {
                    tracing::info!(
                        "[OMEMO raw] key rid={} kex={} data_len={} data_hex={}",
                        k.rid,
                        k.kex,
                        k.data.len(),
                        hex(&k.data),
                    );
                }
            }
            if let Some(iv) = &encrypted.header.iv {
                tracing::info!("[OMEMO raw] iv_hex={}", hex(iv));
            }
            if let Some(payload_bytes) = &encrypted.payload {
                tracing::info!("[OMEMO raw] payload_hex={}", hex(payload_bytes));
            }
            // For sent carbons, use our JID for self-session lookup
            let session_jid = if direction == Direction::Outgoing {
                our_jid.unwrap_or(&from_bare)
            } else {
                &from_bare
            };
            if let Some(decrypted) = mgr.decrypt_message(session_jid, &encrypted) {
                omemo_decrypted = Some(decrypted);
                let _ = mgr.save();
                break;
            }
        }
    }

    if let Some(body) = omemo_decrypted {
        let _ = output
            .send(XmppEvent::OmemoMessageReceived {
                from: from_bare,
                body,
                direction,
            })
            .await;
        return;
    }

    // Sent carbon of an OMEMO message we couldn't decrypt (no self-session yet)
    let has_omemo_payload = msg
        .payloads
        .iter()
        .any(|p| p.name() == "encrypted" && (p.ns() == NS_OMEMO_V0));
    if !msg.bodies.contains_key("") && direction == Direction::Outgoing && has_omemo_payload {
        let placeholder = Message {
            id: msg
                .id
                .map(|id| id.0)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            from: from.to_string(),
            body: String::from("🔒 Encrypted message (sent from another device)"),
            timestamp: timestamp.unwrap_or_else(Utc::now),
            status: MessageStatus::Sent,
            direction: Direction::Outgoing,
        };
        let _ = output.send(XmppEvent::MessageReceived(placeholder)).await;
        return;
    }

    if let Some(body) = msg.bodies.get("") {
        let status = match direction {
            Direction::Outgoing => MessageStatus::Sent,
            Direction::Incoming => MessageStatus::Received,
        };
        let message = Message {
            id: msg
                .id
                .map(|id| id.0)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            from: from.to_string(),
            body: body.to_string(),
            timestamp: timestamp.unwrap_or_else(Utc::now),
            status,
            direction,
        };
        let _ = output.send(XmppEvent::MessageReceived(message)).await;
    }
}

async fn handle_stanza(
    stanza: Stanza,
    client: &mut Client,
    output: &mut mpsc::Sender<XmppEvent>,
    omemo: &mut Option<OmemoManager>,
    pending_iqs: &mut HashMap<String, PendingIq>,
    our_jid: Option<&str>,
    stream_healthy: &mut bool,
) {
    match stanza {
        Stanza::Message(mut msg) => {
            if msg.type_ == MessageType::Error {
                tracing::info!(
                    "[MSG ERROR] from={:?} to={:?} id={:?} payloads={}",
                    msg.from,
                    msg.to,
                    msg.id,
                    msg.payloads.len()
                );
                tracing::info!("[MSG ERROR RAW] {:?}", msg);
                if let Some(xml) = xmpp_message_to_xml(&msg) {
                    tracing::info!("[MSG ERROR XML] {}", xml);
                }
                return;
            }

            tracing::info!(
                "[MSG] from={:?} to={:?} type={:?} payloads={}",
                msg.from,
                msg.to,
                msg.type_,
                msg.payloads.len()
            );
            tracing::info!("[MSG RAW] {:?}", msg);
            if let Some(xml) = xmpp_message_to_xml(&msg) {
                tracing::info!("[MSG XML] {}", xml);
            }

            // Check for MAM archive results (XEP-0313)
            if let Ok(Some(mam_result)) = msg.extract_payload::<xmpp_parsers::mam::Result_>() {
                let archived_msg = mam_result.forwarded.message;
                let timestamp = mam_result
                    .forwarded
                    .delay
                    .map(|d| d.stamp.0.with_timezone(&Utc));
                let archived_from = archived_msg.from.as_ref().map(|j| j.to_bare().to_string());
                let archived_to = archived_msg.to.as_ref().map(|j| j.to_bare().to_string());
                tracing::info!(
                    "[MAM] result id={} archived_from={:?} archived_to={:?} our_jid={:?}",
                    mam_result.id,
                    archived_from,
                    archived_to,
                    our_jid
                );

                if let Some(our) = our_jid {
                    if archived_from.as_deref() == Some(our) {
                        // Outgoing message from archive
                        if let Some(to) = archived_msg.to.clone() {
                            tracing::info!("[MAM] treating as OUTGOING to {:?}", to);
                            process_message(
                                archived_msg,
                                output,
                                omemo,
                                Direction::Outgoing,
                                to,
                                timestamp,
                                our_jid,
                            )
                            .await;
                        }
                    } else {
                        // Incoming message from archive
                        if let Some(from) = archived_msg.from.clone() {
                            tracing::info!("[MAM] treating as INCOMING from {:?}", from);
                            process_message(
                                archived_msg,
                                output,
                                omemo,
                                Direction::Incoming,
                                from,
                                timestamp,
                                our_jid,
                            )
                            .await;
                        }
                    }
                } else {
                    if let Some(from) = archived_msg.from.clone() {
                        process_message(
                            archived_msg,
                            output,
                            omemo,
                            Direction::Incoming,
                            from,
                            timestamp,
                            our_jid,
                        )
                        .await;
                    }
                }
                return;
            }

            // Check for carbon copies (XEP-0280)
            if let Some((carbon_type, fwd_msg)) = extract_carbon_message(&msg) {
                tracing::info!(
                    "[CARBON] type={:?} fwd_from={:?} fwd_to={:?}",
                    carbon_type,
                    fwd_msg.from,
                    fwd_msg.to
                );
                match carbon_type {
                    CarbonType::Received => {
                        if let Some(from) = fwd_msg.from.clone() {
                            process_message(
                                fwd_msg,
                                output,
                                omemo,
                                Direction::Incoming,
                                from,
                                None,
                                our_jid,
                            )
                            .await;
                        }
                    }
                    CarbonType::Sent => {
                        if let Some(to) = fwd_msg.to.clone() {
                            process_message(
                                fwd_msg,
                                output,
                                omemo,
                                Direction::Outgoing,
                                to,
                                None,
                                our_jid,
                            )
                            .await;
                        }
                    }
                }
                return;
            }

            if let Some(from) = msg.from.clone() {
                tracing::info!(
                    "[LIVE] message from={:?} body={:?}",
                    from,
                    msg.bodies.get("")
                );
                process_message(msg, output, omemo, Direction::Incoming, from, None, our_jid).await;
            }
        }
        Stanza::Presence(presence) => {
            if let Some(from) = presence.from {
                let jid = from.to_string();
                let available = presence.type_ != PresenceType::Unavailable;
                let show = presence
                    .show
                    .map(|s| match s {
                        XmppShow::Away => Show::Away,
                        XmppShow::Chat => Show::Chat,
                        XmppShow::Dnd => Show::Dnd,
                        XmppShow::Xa => Show::Xa,
                    })
                    .unwrap_or(Show::None);
                let status = presence.statuses.get("").cloned();

                let pres = Presence {
                    show,
                    status,
                    available,
                };
                let _ = output
                    .send(XmppEvent::PresenceUpdate {
                        jid,
                        presence: pres,
                    })
                    .await;
            }
        }
        Stanza::Iq(iq) => {
            match iq {
                Iq::Result { id, payload, .. } => {
                    if let Some(pending) = pending_iqs.remove(&id) {
                        match pending {
                            PendingIq::HttpUploadSlot {
                                to,
                                path,
                                filename,
                                size: _size,
                                content_type,
                                services: _services,
                                service_idx: _service_idx,
                            } => {
                                let Some(element) = payload else {
                                    let _ = output.send(XmppEvent::StatusChanged("File send failed: upload slot response missing payload".to_string())).await;
                                    return;
                                };
                                let Some((put_url, get_url, headers)) =
                                    parse_http_upload_slot(&element)
                                else {
                                    let _ = output.send(XmppEvent::StatusChanged("File send failed: could not parse upload slot".to_string())).await;
                                    return;
                                };
                                match upload_file_to_slot(&put_url, &path, &content_type, &headers).await {
                                    Ok(()) => {
                                        let msg = make_file_message(&to, &filename, &get_url);
                                        let _ = safe_send_stanza(
                                            client,
                                            msg.into(),
                                            "http-upload-send-message",
                                            stream_healthy,
                                        )
                                        .await;
                                        let _ = output
                                            .send(XmppEvent::MessageSent {
                                                _id: uuid::Uuid::new_v4().to_string(),
                                            })
                                            .await;
                                        let _ = output.send(XmppEvent::StatusChanged("File sent".to_string())).await;
                                    }
                                    Err(err) => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!("File send failed: {}", err))).await;
                                    }
                                }
                            }
                            PendingIq::Bundle { jid, version } => {
                                let mut found_any = false;
                                let mut found_other = false;
                                let mut found_device_ids = Vec::new();
                                tracing::info!("[OMEMO] Bundle result for {} ({:?})", jid, version);
                                if let Some(element) = payload {
                                    tracing::info!("[OMEMO] Bundle result payload: {:?}", element);
                                    if let Ok(pubsub) =
                                        xmpp_parsers::pubsub::PubSub::try_from(element.clone())
                                    {
                                        if let xmpp_parsers::pubsub::PubSub::Items(items) = pubsub {
                                            tracing::info!(
                                                "[OMEMO] Bundle result for {} ({:?}): {} items",
                                                jid,
                                                version,
                                                items.items.len()
                                            );
                                            for item in items.items {
                                                let device_id = item
                                                    .id
                                                    .as_ref()
                                                    .and_then(|id| id.0.parse().ok())
                                                    .unwrap_or(0);
                                                if let Some(ref payload) = item.payload {
                                                    if let Some(bundle) = parse_bundle(payload) {
                                                        tracing::info!(
                                                            "[OMEMO] Parsed bundle for {} device {} (prekeys={})",
                                                            jid,
                                                            device_id,
                                                            bundle.prekeys.len()
                                                        );
                                                        if let Some(mgr) = omemo {
                                                            // Proactively create outbound sessions
                                                            // from fetched bundles so first send
                                                            // works before peer sends us OMEMO.
                                                            let _ = mgr.create_session_from_bundle(
                                                                &jid, device_id, &bundle,
                                                            );
                                                            if device_id != mgr.our_device_id() {
                                                                found_other = true;
                                                            }
                                                        }
                                                        let _ = output
                                                            .send(XmppEvent::BundleReceived)
                                                            .await;
                                                        found_any = true;
                                                        found_device_ids.push(device_id);
                                                    } else {
                                                        tracing::info!(
                                                            "[OMEMO] Failed to parse bundle for {} device {}",
                                                            jid,
                                                            device_id
                                                        );
                                                    }
                                                } else {
                                                    tracing::info!(
                                                        "[OMEMO] Bundle item for {} device {} has no payload",
                                                        jid,
                                                        device_id
                                                    );
                                                }
                                            }
                                        } else {
                                            tracing::info!(
                                                "[OMEMO] Bundle result for {} ({:?}) is not Items",
                                                jid,
                                                version
                                            );
                                        }
                                    } else {
                                        tracing::info!(
                                            "[OMEMO] Bundle result for {} ({:?}) failed to parse as PubSub",
                                            jid,
                                            version
                                        );
                                    }
                                } else {
                                    tracing::info!(
                                        "[OMEMO] Bundle result for {} ({:?}) has no payload",
                                        jid,
                                        version
                                    );
                                }
                                if let Some(mgr) = omemo {
                                    if version == OmemoVersion::V0 && found_any {
                                        // Prefer v0 whenever we have at least one valid v0 bundle.
                                        mgr.clear_v0_jid(&jid);
                                    }
                                    let _ = mgr.save();
                                }
                                // For v0, try per-device fetch for missing devices
                                if version == OmemoVersion::V0 {
                                    if let Some(mgr) = omemo
                                        && let Some(devices) = mgr.device_lists.get(&jid)
                                    {
                                        for device_id in devices {
                                            if *device_id == mgr.our_device_id() {
                                                continue;
                                            }
                                            if found_device_ids.contains(device_id) {
                                                continue;
                                            }
                                            let iq = build_bundle_fetch_iq_v0(&jid, *device_id);
                                            let id = match &iq {
                                                Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                    id.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            if !id.is_empty() {
                                                tracing::info!(
                                                    "[OMEMO] Sending v0 bundle fetch for {} device {}",
                                                    jid,
                                                    device_id
                                                );
                                                pending_iqs.insert(
                                                    id,
                                                    PendingIq::BundleDevice {
                                                        jid: jid.clone(),
                                                        device_id: *device_id,
                                                    },
                                                );
                                                let _ = safe_send_stanza(
                                                    client,
                                                    iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                    if !found_any {
                                        tracing::info!(
                                            "[OMEMO] No v0 bundles found for {}, trying v0 per-device",
                                            jid
                                        );
                                    } else {
                                        tracing::info!(
                                            "[OMEMO] Bundle v0 fetch complete for {}: found_any={} found_other={}",
                                            jid,
                                            found_any,
                                            found_other
                                        );
                                    }
                                } else {
                                    if !found_any {
                                        tracing::info!(
                                            "[OMEMO] No bundles found for {} in any version",
                                            jid
                                        );
                                    } else {
                                        tracing::info!(
                                            "[OMEMO] Bundle fetch complete for {}: found_any={} found_other={}",
                                            jid,
                                            found_any,
                                            found_other
                                        );
                                    }
                                }
                            }
                            PendingIq::BundleDevice { jid, device_id } => {
                                tracing::info!(
                                    "[OMEMO] Bundle result for {} device {} (v0)",
                                    jid,
                                    device_id
                                );
                                if let Some(element) = payload {
                                    if let Ok(pubsub) =
                                        xmpp_parsers::pubsub::PubSub::try_from(element.clone())
                                    {
                                        if let xmpp_parsers::pubsub::PubSub::Items(items) = pubsub {
                                            if let Some(item) = items.items.first() {
                                                if let Some(ref payload) = item.payload {
                                                    if let Some(bundle) = parse_bundle(payload) {
                                                        tracing::info!(
                                                            "[OMEMO] Parsed bundle for {} device {} (prekeys={})",
                                                            jid,
                                                            device_id,
                                                            bundle.prekeys.len()
                                                        );
                                                        if let Some(mgr) = omemo {
                                                            // Proactively create outbound sessions
                                                            // from fetched bundles so first send
                                                            // works before peer sends us OMEMO.
                                                            let _ = mgr.create_session_from_bundle(
                                                                &jid, device_id, &bundle,
                                                            );
                                                            if mgr.our_jid.as_deref()
                                                                != Some(jid.as_str())
                                                            {
                                                                mgr.mark_v0_jid(&jid);
                                                            }
                                                            let _ = mgr.save();
                                                        }
                                                        let _ = output
                                                            .send(XmppEvent::BundleReceived)
                                                            .await;
                                                    } else {
                                                        tracing::info!(
                                                            "[OMEMO] Failed to parse bundle for {} device {}",
                                                            jid,
                                                            device_id
                                                        );
                                                    }
                                                } else {
                                                    tracing::info!(
                                                        "[OMEMO] Bundle item for {} device {} has no payload",
                                                        jid,
                                                        device_id
                                                    );
                                                }
                                            } else {
                                                tracing::info!(
                                                    "[OMEMO] No items in v0 bundle response for {} device {}",
                                                    jid,
                                                    device_id
                                                );
                                            }
                                        } else {
                                            tracing::info!(
                                                "[OMEMO] Bundle result for {} device {} is not Items",
                                                jid,
                                                device_id
                                            );
                                        }
                                    } else {
                                        tracing::info!(
                                            "[OMEMO] Bundle result for {} device {} failed to parse as PubSub",
                                            jid,
                                            device_id
                                        );
                                    }
                                } else {
                                    tracing::info!(
                                        "[OMEMO] Bundle result for {} device {} has no payload",
                                        jid,
                                        device_id
                                    );
                                }
                            }
                            PendingIq::DeviceList {
                                jid,
                                version,
                                accumulated,
                            } => {
                                tracing::info!(
                                    "[OMEMO] DeviceList response for {} ({:?})",
                                    jid,
                                    version
                                );
                                let mut devices = Vec::new();
                                let mut item_count = 0;
                                if let Some(element) = payload
                                    && let Ok(pubsub) =
                                        xmpp_parsers::pubsub::PubSub::try_from(element.clone())
                                    && let xmpp_parsers::pubsub::PubSub::Items(items) = pubsub
                                {
                                    item_count = items.items.len();
                                    tracing::info!(
                                        "[OMEMO] Got {} pubsub items for {}",
                                        item_count,
                                        jid
                                    );
                                    for item in items.items {
                                        let item_id = item
                                            .id
                                            .as_ref()
                                            .map(|id| id.0.as_str())
                                            .unwrap_or("(no id)");
                                        if let Some(ref payload) = item.payload
                                            && let Some(device_list) = parse_device_list(payload)
                                        {
                                            let ids: Vec<u32> =
                                                device_list.devices.iter().map(|d| d.id).collect();
                                            tracing::info!(
                                                "[OMEMO] Device list item id={} for {}: {:?}",
                                                item_id,
                                                jid,
                                                ids
                                            );
                                            devices.extend(ids);
                                        } else {
                                            tracing::info!(
                                                "[OMEMO] Device list item id={} for {} has no parseable payload",
                                                item_id,
                                                jid
                                            );
                                        }
                                    }
                                }
                                let is_own_jid = accumulated.is_some();
                                let mut all_devices = accumulated.unwrap_or_default();
                                if !devices.is_empty() {
                                    tracing::info!(
                                        "[OMEMO] Parsed device list for {} ({:?}): {:?}",
                                        jid,
                                        version,
                                        devices
                                    );
                                    all_devices.extend(devices.iter().copied());
                                    all_devices.sort_unstable();
                                    all_devices.dedup();
                                }

                                if is_own_jid {
                                    // For own JID: always exhaust all versions and merge
                                    if let Some(next_version) = version.next() {
                                        tracing::info!(
                                            "[OMEMO] Fetching next version {:?} for own device list {}",
                                            next_version,
                                            jid
                                        );
                                        let iq = build_device_list_fetch_iq(&jid, next_version);
                                        let id = match &iq {
                                            Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                            _ => String::new(),
                                        };
                                        if !id.is_empty() {
                                            pending_iqs.insert(
                                                id,
                                                PendingIq::DeviceList {
                                                    jid: jid.clone(),
                                                    version: next_version,
                                                    accumulated: Some(all_devices),
                                                },
                                            );
                                        }
                                        let _ = safe_send_stanza(
                                            client,
                                            iq.into(),
                                            "stanza-handler",
                                            stream_healthy,
                                        )
                                        .await;
                                    } else {
                                        // All versions exhausted — publish merged list and fetch bundles
                                        if !all_devices.is_empty() {
                                            tracing::info!(
                                                "[OMEMO] Merged own device list for {}: {:?}",
                                                jid,
                                                all_devices
                                            );
                                        } else {
                                            tracing::info!(
                                                "[OMEMO] No own device list found for {} in any version",
                                                jid
                                            );
                                        }
                                        if let Some(mgr) = omemo {
                                            let our_device_id = mgr.our_device_id();
                                            if !all_devices.contains(&our_device_id) {
                                                all_devices.push(our_device_id);
                                                all_devices.sort_unstable();
                                                all_devices.dedup();
                                            }
                                            if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                                let merged_iq =
                                                    build_device_list_iq(&all_devices, &pubsub_jid);
                                                tracing::info!(
                                                    "[OMEMO] Publishing merged device list (v0) for {}: {:?}",
                                                    jid,
                                                    all_devices
                                                );
                                                let merged_id = match &merged_iq {
                                                    Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                        id.clone()
                                                    }
                                                    _ => String::new(),
                                                };
                                                if !merged_id.is_empty() {
                                                    pending_iqs.insert(
                                                        merged_id,
                                                        PendingIq::DeviceListPublish {
                                                            jid: jid.clone(),
                                                            version: String::from("v0"),
                                                        },
                                                    );
                                                }
                                                let _ = safe_send_stanza(
                                                    client,
                                                    merged_iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                                // If v0 has stale items, purge before publishing
                                                if version == OmemoVersion::V0 && item_count > 1 {
                                                    tracing::info!(
                                                        "[OMEMO] v0 device list for {} has {} items, purging before publish",
                                                        jid,
                                                        item_count
                                                    );
                                                    let purge_iq =
                                                        build_purge_v0_device_list_iq(&pubsub_jid);
                                                    let purge_id = match &purge_iq {
                                                        Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                            id.clone()
                                                        }
                                                        _ => String::new(),
                                                    };
                                                    if !purge_id.is_empty() {
                                                        pending_iqs.insert(
                                                            purge_id,
                                                            PendingIq::PurgeV0DeviceList {
                                                                jid: jid.clone(),
                                                                devices: all_devices.clone(),
                                                            },
                                                        );
                                                    }
                                                    let _ = safe_send_stanza(
                                                        client,
                                                        purge_iq.into(),
                                                        "stanza-handler",
                                                        stream_healthy,
                                                    )
                                                    .await;
                                                } else {
                                                    let v0_iq = build_device_list_iq_v0(
                                                        &all_devices,
                                                        &pubsub_jid,
                                                    );
                                                    tracing::info!(
                                                        "[OMEMO] Publishing merged device list (v0) for {}: {:?}",
                                                        jid,
                                                        all_devices
                                                    );
                                                    let v0_id = match &v0_iq {
                                                        Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                            id.clone()
                                                        }
                                                        _ => String::new(),
                                                    };
                                                    if !v0_id.is_empty() {
                                                        pending_iqs.insert(
                                                            v0_id,
                                                            PendingIq::DeviceListPublish {
                                                                jid: jid.clone(),
                                                                version: String::from("v0"),
                                                            },
                                                        );
                                                    }
                                                    let _ = safe_send_stanza(
                                                        client,
                                                        v0_iq.into(),
                                                        "stanza-handler",
                                                        stream_healthy,
                                                    )
                                                    .await;
                                                }
                                            }
                                            mgr.update_device_list(&jid, all_devices.clone());
                                            let iq = build_bundle_fetch_iq(&jid, OmemoVersion::V0);
                                            let bundle_id = match &iq {
                                                Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                    id.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            tracing::info!(
                                                "[OMEMO] Sending bundle fetch (auto) for {} (v0, id={})",
                                                jid,
                                                bundle_id
                                            );
                                            if !bundle_id.is_empty() {
                                                pending_iqs.insert(
                                                    bundle_id,
                                                    PendingIq::Bundle {
                                                        jid: jid.clone(),
                                                        version: OmemoVersion::V0,
                                                    },
                                                );
                                            }
                                            if !safe_send_stanza(
                                                client,
                                                iq.into(),
                                                "omemo-bundle-fetch",
                                                stream_healthy,
                                            )
                                            .await
                                            {
                                                tracing::info!(
                                                    "[OMEMO] Bundle fetch send FAILED for {}",
                                                    jid
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    // For other JIDs: current behavior — first non-empty wins
                                    if !all_devices.is_empty() {
                                        tracing::info!(
                                            "[OMEMO] Parsed device list for {}: {:?}",
                                            jid,
                                            all_devices
                                        );
                                        if let Some(mgr) = omemo {
                                            mgr.update_device_list(&jid, all_devices);
                                            let iq = build_bundle_fetch_iq(&jid, OmemoVersion::V0);
                                            let bundle_id = match &iq {
                                                Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                    id.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            tracing::info!(
                                                "[OMEMO] Sending bundle fetch (contact) for {} (v0, id={})",
                                                jid,
                                                bundle_id
                                            );
                                            if !bundle_id.is_empty() {
                                                pending_iqs.insert(
                                                    bundle_id,
                                                    PendingIq::Bundle {
                                                        jid: jid.clone(),
                                                        version: OmemoVersion::V0,
                                                    },
                                                );
                                            }
                                            if !safe_send_stanza(
                                                client,
                                                iq.into(),
                                                "omemo-bundle-fetch",
                                                stream_healthy,
                                            )
                                            .await
                                            {
                                                tracing::info!(
                                                    "[OMEMO] Bundle fetch send FAILED for {}",
                                                    jid
                                                );
                                            }
                                        }
                                    } else if let Some(next_version) = version.next() {
                                        tracing::info!(
                                            "[OMEMO] No device list for {} with {:?}, trying {:?}",
                                            jid,
                                            version,
                                            next_version
                                        );
                                        let iq = build_device_list_fetch_iq(&jid, next_version);
                                        let id = match &iq {
                                            Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                            _ => String::new(),
                                        };
                                        if !id.is_empty() {
                                            pending_iqs.insert(
                                                id,
                                                PendingIq::DeviceList {
                                                    jid: jid.clone(),
                                                    version: next_version,
                                                    accumulated: None,
                                                },
                                            );
                                        }
                                        tracing::info!(
                                            "[OMEMO] Sending device list fetch IQ ({:?}) to {}",
                                            next_version,
                                            jid
                                        );
                                        let _ = safe_send_stanza(
                                            client,
                                            iq.into(),
                                            "stanza-handler",
                                            stream_healthy,
                                        )
                                        .await;
                                    } else {
                                        tracing::info!(
                                            "[OMEMO] No device list found for {} in any version",
                                            jid
                                        );
                                    }
                                }
                            }
                            PendingIq::VCardAvatar { jid } => {
                                tracing::info!("[AVATAR] vCard response for {}", jid);
                                if let Some(element) = payload {
                                    if let Ok(vcard) = VCard::try_from(element.clone()) {
                                        if let Some(photo) = vcard.photo {
                                            tracing::info!(
                                                "[AVATAR] vCard photo found for {} type={}",
                                                jid,
                                                photo.type_.data
                                            );
                                            let _ = output
                                                .send(XmppEvent::AvatarReceived {
                                                    jid: jid.clone(),
                                                    bytes: photo.binval.data,
                                                })
                                                .await;
                                            return;
                                        } else {
                                            tracing::info!(
                                                "[AVATAR] vCard has no photo for {}, falling back to PEP",
                                                jid
                                            );
                                        }
                                    } else {
                                        tracing::info!(
                                            "[AVATAR] vCard parse failed for {}, falling back to PEP",
                                            jid
                                        );
                                    }
                                } else {
                                    tracing::info!(
                                        "[AVATAR] vCard response has no payload for {}, falling back to PEP",
                                        jid
                                    );
                                }
                                let iq = build_avatar_metadata_fetch_iq(&jid);
                                let meta_id = match &iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !meta_id.is_empty() {
                                    pending_iqs.insert(
                                        meta_id,
                                        PendingIq::AvatarMetadata { jid: jid.clone() },
                                    );
                                }
                                let _ = safe_send_stanza(
                                    client,
                                    iq.into(),
                                    "stanza-handler",
                                    stream_healthy,
                                )
                                .await;
                            }
                            PendingIq::AvatarMetadata { jid } => {
                                tracing::info!("[AVATAR] Metadata response for {}", jid);
                                let Some(element) = payload else {
                                    tracing::info!(
                                        "[AVATAR] Metadata response has no payload for {}",
                                        jid
                                    );
                                    return;
                                };
                                let Ok(pubsub) =
                                    xmpp_parsers::pubsub::PubSub::try_from(element.clone())
                                else {
                                    tracing::info!(
                                        "[AVATAR] Metadata response is not pubsub for {}",
                                        jid
                                    );
                                    return;
                                };
                                let xmpp_parsers::pubsub::PubSub::Items(items) = pubsub else {
                                    tracing::info!(
                                        "[AVATAR] Metadata pubsub is not Items for {}",
                                        jid
                                    );
                                    return;
                                };
                                tracing::info!(
                                    "[AVATAR] Metadata has {} items for {}",
                                    items.items.len(),
                                    jid
                                );
                                for item in items.items {
                                    let Some(payload_el) = item.payload else {
                                        tracing::info!(
                                            "[AVATAR] Metadata item has no payload for {}",
                                            jid
                                        );
                                        continue;
                                    };
                                    if payload_el.name() != "metadata"
                                        || payload_el.ns() != "urn:xmpp:avatar:metadata"
                                    {
                                        tracing::info!(
                                            "[AVATAR] Metadata item has wrong payload: name={} ns={} for {}",
                                            payload_el.name(),
                                            payload_el.ns(),
                                            jid
                                        );
                                        continue;
                                    }
                                    let mut found = false;
                                    for info in payload_el.children() {
                                        if info.name() == "info" {
                                            if let Some(hash) = info.attr("id") {
                                                tracing::info!(
                                                    "[AVATAR] Found avatar hash={} for {}, fetching data...",
                                                    hash,
                                                    jid
                                                );
                                                let iq = build_avatar_data_fetch_iq(&jid, hash);
                                                let data_id = match &iq {
                                                    Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                        id.clone()
                                                    }
                                                    _ => String::new(),
                                                };
                                                if !data_id.is_empty() {
                                                    pending_iqs.insert(
                                                        data_id,
                                                        PendingIq::AvatarData { jid: jid.clone() },
                                                    );
                                                }
                                                let _ = safe_send_stanza(
                                                    client,
                                                    iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                                found = true;
                                                break;
                                            } else {
                                                tracing::info!(
                                                    "[AVATAR] info element has no id attr for {}",
                                                    jid
                                                );
                                            }
                                        }
                                    }
                                    if !found {
                                        tracing::info!(
                                            "[AVATAR] No info elements found in metadata for {}",
                                            jid
                                        );
                                    }
                                }
                            }
                            PendingIq::AvatarData { jid } => {
                                tracing::info!("[AVATAR] Data response for {}", jid);
                                let Some(element) = payload else {
                                    tracing::info!(
                                        "[AVATAR] Data response has no payload for {}",
                                        jid
                                    );
                                    return;
                                };
                                let Ok(pubsub) =
                                    xmpp_parsers::pubsub::PubSub::try_from(element.clone())
                                else {
                                    tracing::info!(
                                        "[AVATAR] Data response is not pubsub for {}",
                                        jid
                                    );
                                    return;
                                };
                                let xmpp_parsers::pubsub::PubSub::Items(items) = pubsub else {
                                    tracing::info!("[AVATAR] Data pubsub is not Items for {}", jid);
                                    return;
                                };
                                tracing::info!(
                                    "[AVATAR] Data has {} items for {}",
                                    items.items.len(),
                                    jid
                                );
                                for item in items.items {
                                    let Some(payload_el) = item.payload else {
                                        tracing::info!(
                                            "[AVATAR] Data item has no payload for {}",
                                            jid
                                        );
                                        continue;
                                    };
                                    if payload_el.name() != "data"
                                        || payload_el.ns() != "urn:xmpp:avatar:data"
                                    {
                                        tracing::info!(
                                            "[AVATAR] Data item has wrong payload: name={} ns={} for {}",
                                            payload_el.name(),
                                            payload_el.ns(),
                                            jid
                                        );
                                        continue;
                                    }
                                    use base64::Engine;
                                    use base64::engine::general_purpose::STANDARD as BASE64;
                                    let text = payload_el.text();
                                    tracing::info!(
                                        "[AVATAR] Data text length={} for {}",
                                        text.len(),
                                        jid
                                    );
                                    if let Ok(bytes) = BASE64.decode(&text) {
                                        tracing::info!(
                                            "[AVATAR] Decoded {} bytes for {}",
                                            bytes.len(),
                                            jid
                                        );
                                        let _ = output
                                            .send(XmppEvent::AvatarReceived {
                                                jid: jid.clone(),
                                                bytes,
                                            })
                                            .await;
                                    } else {
                                        tracing::info!(
                                            "[AVATAR] Failed to decode base64 for {}",
                                            jid
                                        );
                                    }
                                }
                            }
                            PendingIq::DeviceListPublish { jid, version } => {
                                tracing::info!(
                                    "[OMEMO] Device list publish ({}) succeeded for {}",
                                    version,
                                    jid
                                );
                                if version.starts_with("v0") {
                                    tracing::info!(
                                        "[OMEMO] Re-fetching v0 device list to verify node state for {}",
                                        jid
                                    );
                                    let iq = build_device_list_fetch_iq(&jid, OmemoVersion::V0);
                                    let id = match &iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !id.is_empty() {
                                        pending_iqs.insert(
                                            id,
                                            PendingIq::DeviceList {
                                                jid: jid.clone(),
                                                version: OmemoVersion::V0,
                                                accumulated: None,
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
                            PendingIq::BundlePublish { jid, version } => {
                                tracing::info!(
                                    "[OMEMO] Bundle publish ({}) succeeded for {}",
                                    version,
                                    jid
                                );
                                if let Some(mgr) = omemo.as_mut() {
                                    mgr.account.inner.mark_keys_as_published();
                                    let _ = mgr.save();
                                    tracing::info!("[OMEMO] Marked one-time keys as published");
                                }
                            }
                            PendingIq::PurgeV0DeviceList { jid, devices } => {
                                tracing::info!(
                                    "[OMEMO] v0 device list node purged for {}, publishing fresh list: {:?}",
                                    jid,
                                    devices
                                );
                                if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                    let v0_iq = build_device_list_iq_v0(&devices, &pubsub_jid);
                                    let v0_id = match &v0_iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !v0_id.is_empty() {
                                        pending_iqs.insert(
                                            v0_id,
                                            PendingIq::DeviceListPublish {
                                                jid: jid.clone(),
                                                version: String::from("v0"),
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        v0_iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
                            PendingIq::ConfigureNode {
                                jid,
                                node,
                                devices,
                                version,
                            } => {
                                tracing::info!(
                                    "[OMEMO] Node configuration pushed for {} ({}), retrying publish",
                                    jid,
                                    node
                                );
                                if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                    let iq = if version.starts_with("v0") {
                                        build_device_list_iq_v0(&devices, &pubsub_jid)
                                    } else {
                                        build_device_list_iq(&devices, &pubsub_jid)
                                    };
                                    let pub_id = match &iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !pub_id.is_empty() {
                                        pending_iqs.insert(
                                            pub_id,
                                            PendingIq::DeviceListPublish {
                                                jid: jid.clone(),
                                                version: version.clone(),
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
                        }
                        return;
                    }

                    if let Some(element) = payload {
                        if let Ok(fin) = xmpp_parsers::mam::Fin::try_from(element.clone()) {
                            tracing::info!(
                                "[MAM] Fin complete={} first={:?} last={:?} count={:?}",
                                fin.complete,
                                fin.set.first.as_ref().map(|f| &f.item),
                                fin.set.last,
                                fin.set.count
                            );
                            let _ = output
                                .send(XmppEvent::StatusChanged(if fin.complete {
                                    "History sync complete".to_string()
                                } else {
                                    "Loading more history...".to_string()
                                }))
                                .await;
                            return;
                        }

                        tracing::info!(
                            "[ROSTER] IQ result element: name={} ns={}",
                            element.name(),
                            element.ns()
                        );
                        if element.name() == "query" && element.ns() == xmpp_parsers::ns::ROSTER {
                            tracing::info!("[ROSTER] Parsing roster query response");
                            if let Ok(roster) = Roster::try_from(element.clone()) {
                                tracing::info!("[ROSTER] Got {} items", roster.items.len());
                                for item in roster.items {
                                    tracing::info!(
                                        "[ROSTER] Item: jid={} name={:?}",
                                        item.jid,
                                        item.name
                                    );
                                    let subscription = match item.subscription {
                                        xmpp_parsers::roster::Subscription::None => {
                                            Subscription::None
                                        }
                                        xmpp_parsers::roster::Subscription::To => Subscription::To,
                                        xmpp_parsers::roster::Subscription::From => {
                                            Subscription::From
                                        }
                                        xmpp_parsers::roster::Subscription::Both => {
                                            Subscription::Both
                                        }
                                        xmpp_parsers::roster::Subscription::Remove => {
                                            Subscription::None
                                        }
                                    };
                                    let groups = item.groups.into_iter().map(|g| g.0).collect();
                                    let contact = Contact {
                                        jid: item.jid.to_string(),
                                        name: item.name,
                                        subscription,
                                        groups,
                                        presence: Presence::default(),
                                    };
                                    let _ = output.send(XmppEvent::RosterItem(contact)).await;
                                }
                            }
                        }
                    }
                }
                Iq::Set { payload, .. } => {
                    tracing::info!(
                        "[ROSTER] IQ set element: name={} ns={}",
                        payload.name(),
                        payload.ns()
                    );
                    if payload.name() == "query" && payload.ns() == xmpp_parsers::ns::ROSTER {
                        tracing::info!("[ROSTER] Parsing roster push");
                        if let Ok(roster) = Roster::try_from(payload.clone()) {
                            tracing::info!("[ROSTER] Push got {} items", roster.items.len());
                            for item in roster.items {
                                let subscription = match item.subscription {
                                    xmpp_parsers::roster::Subscription::None => Subscription::None,
                                    xmpp_parsers::roster::Subscription::To => Subscription::To,
                                    xmpp_parsers::roster::Subscription::From => Subscription::From,
                                    xmpp_parsers::roster::Subscription::Both => Subscription::Both,
                                    xmpp_parsers::roster::Subscription::Remove => {
                                        Subscription::None
                                    }
                                };
                                let groups = item.groups.into_iter().map(|g| g.0).collect();
                                let contact = Contact {
                                    jid: item.jid.to_string(),
                                    name: item.name,
                                    subscription,
                                    groups,
                                    presence: Presence::default(),
                                };
                                let _ = output.send(XmppEvent::RosterItem(contact)).await;
                            }
                        }
                    }
                }
                Iq::Error { id, error, .. } => {
                    tracing::info!("[IQ] Error id={} error={:?}", id, error);
                    if let Some(pending) = pending_iqs.remove(&id) {
                        tracing::info!("[IQ] Pending IQ {} failed", id);
                        match pending {
                            PendingIq::HttpUploadSlot {
                                to,
                                path,
                                filename,
                                size,
                                content_type,
                                services,
                                service_idx,
                            } => {
                                let next_idx = service_idx + 1;
                                if next_idx < services.len() {
                                    let request_id = format!("upload-slot-{}", uuid::Uuid::new_v4());
                                    if send_http_upload_slot_request(
                                        client,
                                        &request_id,
                                        &services[next_idx],
                                        &filename,
                                        size,
                                        &content_type,
                                        stream_healthy,
                                    )
                                    .await
                                    {
                                        pending_iqs.insert(
                                            request_id,
                                            PendingIq::HttpUploadSlot {
                                                to,
                                                path,
                                                filename,
                                                size,
                                                content_type,
                                                services,
                                                service_idx: next_idx,
                                            },
                                        );
                                        return;
                                    }
                                }
                                let _ = output.send(XmppEvent::StatusChanged("File send failed: upload service unavailable".to_string())).await;
                            }
                            PendingIq::VCardAvatar { jid } => {
                                tracing::info!(
                                    "[AVATAR] vCard error for {}, falling back to PEP",
                                    jid
                                );
                                let iq = build_avatar_metadata_fetch_iq(&jid);
                                let meta_id = match &iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !meta_id.is_empty() {
                                    pending_iqs.insert(meta_id, PendingIq::AvatarMetadata { jid });
                                }
                                let _ = safe_send_stanza(
                                    client,
                                    iq.into(),
                                    "stanza-handler",
                                    stream_healthy,
                                )
                                .await;
                            }
                            PendingIq::DeviceList {
                                jid,
                                version,
                                accumulated,
                            } if error.defined_condition == DefinedCondition::ItemNotFound => {
                                if let Some(next_version) = version.next() {
                                    tracing::info!(
                                        "[OMEMO] DeviceList not found for {} with {:?}, trying {:?}",
                                        jid,
                                        version,
                                        next_version
                                    );
                                    let iq = build_device_list_fetch_iq(&jid, next_version);
                                    let new_id = match &iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !new_id.is_empty() {
                                        pending_iqs.insert(
                                            new_id,
                                            PendingIq::DeviceList {
                                                jid: jid.clone(),
                                                version: next_version,
                                                accumulated,
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                } else {
                                    tracing::info!(
                                        "[OMEMO] DeviceList not found for {} in any version",
                                        jid
                                    );
                                    if accumulated.is_some() {
                                        // Own JID: publish initial list to v0 and v0
                                        if let Some(mgr) = omemo {
                                            let our_device_id = mgr.our_device_id();
                                            if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                                let iq = build_device_list_iq(
                                                    &[our_device_id],
                                                    &pubsub_jid,
                                                );
                                                tracing::info!(
                                                    "[OMEMO] Publishing initial device list (v0) for {}: [{}]",
                                                    jid,
                                                    our_device_id
                                                );
                                                let iq_id = match &iq {
                                                    Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                        id.clone()
                                                    }
                                                    _ => String::new(),
                                                };
                                                if !iq_id.is_empty() {
                                                    pending_iqs.insert(
                                                        iq_id,
                                                        PendingIq::DeviceListPublish {
                                                            jid: jid.clone(),
                                                            version: String::from("v0"),
                                                        },
                                                    );
                                                }
                                                let _ = safe_send_stanza(
                                                    client,
                                                    iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                                let v0_iq = build_device_list_iq_v0(
                                                    &[our_device_id],
                                                    &pubsub_jid,
                                                );
                                                tracing::info!(
                                                    "[OMEMO] Publishing initial device list (v0) for {}: [{}]",
                                                    jid,
                                                    our_device_id
                                                );
                                                let v0_id = match &v0_iq {
                                                    Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                        id.clone()
                                                    }
                                                    _ => String::new(),
                                                };
                                                if !v0_id.is_empty() {
                                                    pending_iqs.insert(
                                                        v0_id,
                                                        PendingIq::DeviceListPublish {
                                                            jid: jid.clone(),
                                                            version: String::from("v0"),
                                                        },
                                                    );
                                                }
                                                let _ = safe_send_stanza(
                                                    client,
                                                    v0_iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                }
                            }
                            PendingIq::Bundle { jid, version } => {
                                if version == OmemoVersion::V0 {
                                    tracing::info!(
                                        "[OMEMO] Bundle fetch error for {} (v0): {:?}, trying v0 per-device",
                                        jid,
                                        error.defined_condition
                                    );
                                    if let Some(mgr) = omemo
                                        && let Some(devices) = mgr.device_lists.get(&jid)
                                    {
                                        for device_id in devices {
                                            if *device_id == mgr.our_device_id() {
                                                continue;
                                            }
                                            let iq = build_bundle_fetch_iq_v0(&jid, *device_id);
                                            let id = match &iq {
                                                Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                    id.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            if !id.is_empty() {
                                                tracing::info!(
                                                    "[OMEMO] Sending v0 bundle fetch for {} device {}",
                                                    jid,
                                                    device_id
                                                );
                                                pending_iqs.insert(
                                                    id,
                                                    PendingIq::BundleDevice {
                                                        jid: jid.clone(),
                                                        device_id: *device_id,
                                                    },
                                                );
                                                let _ = safe_send_stanza(
                                                    client,
                                                    iq.into(),
                                                    "stanza-handler",
                                                    stream_healthy,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                } else {
                                    tracing::info!(
                                        "[OMEMO] Bundles not found for {} in any version (last error: {:?})",
                                        jid,
                                        error.defined_condition
                                    );
                                }
                            }
                            PendingIq::BundleDevice { jid, device_id } => {
                                tracing::info!(
                                    "[OMEMO] Bundle fetch error for {} device {} (v0): {:?}",
                                    jid,
                                    device_id,
                                    error.defined_condition
                                );
                            }
                            PendingIq::DeviceListPublish { jid, version } => {
                                tracing::info!(
                                    "[OMEMO] Device list publish ({}) FAILED for {}: {:?}",
                                    version,
                                    jid,
                                    error
                                );
                                let is_retry = version.ends_with("-retry");
                                if !is_retry {
                                    let is_precondition = error.other.as_ref().is_some_and(|el| {
                                        el.name() == "precondition-not-met"
                                            && el.ns() == "http://jabber.org/protocol/pubsub#errors"
                                    });
                                    if is_precondition
                                        || error.defined_condition == DefinedCondition::Conflict
                                    {
                                        tracing::info!(
                                            "[OMEMO] Pushing node config for {} device list ({}), then retrying",
                                            jid,
                                            version
                                        );
                                        let node = String::from(NS_OMEMO_V0_DEVICES);
                                        if let Ok(pubsub_jid) = Jid::from_str(&jid)
                                            && let Some(mgr) = omemo
                                        {
                                            let devices = mgr
                                                .device_lists
                                                .get(&jid)
                                                .cloned()
                                                .unwrap_or_else(|| vec![mgr.our_device_id()]);
                                            let config_iq =
                                                build_configure_node_iq(&node, &pubsub_jid);
                                            let config_id = match &config_iq {
                                                Iq::Get { id, .. } | Iq::Set { id, .. } => {
                                                    id.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            if !config_id.is_empty() {
                                                pending_iqs.insert(
                                                    config_id,
                                                    PendingIq::ConfigureNode {
                                                        jid: jid.clone(),
                                                        node,
                                                        devices,
                                                        version: format!("{}-retry", version),
                                                    },
                                                );
                                            }
                                            let _ = safe_send_stanza(
                                                client,
                                                config_iq.into(),
                                                "stanza-handler",
                                                stream_healthy,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                            PendingIq::BundlePublish { jid, version } => {
                                tracing::info!(
                                    "[OMEMO] Bundle publish ({}) FAILED for {}: {:?}",
                                    version,
                                    jid,
                                    error
                                );
                            }
                            PendingIq::PurgeV0DeviceList { jid, devices } => {
                                tracing::info!(
                                    "[OMEMO] v0 device list purge FAILED for {}: {:?}, publishing anyway",
                                    jid,
                                    error.defined_condition
                                );
                                if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                    let v0_iq = build_device_list_iq_v0(&devices, &pubsub_jid);
                                    let v0_id = match &v0_iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !v0_id.is_empty() {
                                        pending_iqs.insert(
                                            v0_id,
                                            PendingIq::DeviceListPublish {
                                                jid: jid.clone(),
                                                version: String::from("v0"),
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        v0_iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
                            PendingIq::ConfigureNode {
                                jid,
                                node,
                                devices,
                                version,
                            } => {
                                tracing::info!(
                                    "[OMEMO] Node configuration push failed for {} ({}): {:?}, retrying publish anyway",
                                    jid,
                                    node,
                                    error.defined_condition
                                );
                                if let Ok(pubsub_jid) = Jid::from_str(&jid) {
                                    let iq = if version.starts_with("v0") {
                                        build_device_list_iq_v0(&devices, &pubsub_jid)
                                    } else {
                                        build_device_list_iq(&devices, &pubsub_jid)
                                    };
                                    let pub_id = match &iq {
                                        Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                        _ => String::new(),
                                    };
                                    if !pub_id.is_empty() {
                                        pending_iqs.insert(
                                            pub_id,
                                            PendingIq::DeviceListPublish {
                                                jid: jid.clone(),
                                                version: version.clone(),
                                            },
                                        );
                                    }
                                    let _ = safe_send_stanza(
                                        client,
                                        iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn make_presence() -> XmppPresence {
    let mut presence = XmppPresence::new(PresenceType::None);
    presence.show = Some(XmppShow::Chat);
    presence
        .statuses
        .insert(Lang::default(), String::from("Dziber XMPP"));
    presence
}

fn make_message(to: &str, body: &str) -> XmppMessage {
    let to_jid = Jid::from_str(to).unwrap_or_else(|_| Jid::from_str("invalid@localhost").unwrap());
    let mut message = XmppMessage::new(Some(to_jid));
    message.type_ = MessageType::Chat;
    message.bodies.insert(Lang::default(), body.to_owned());
    message
}

fn make_file_message(to: &str, filename: &str, url: &str) -> XmppMessage {
    make_message(to, &format!("📎 {}\n{}", filename, url))
}

fn guess_content_type(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        Some(ext) => match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg".to_string(),
            "png" => "image/png".to_string(),
            "gif" => "image/gif".to_string(),
            "webp" => "image/webp".to_string(),
            "pdf" => "application/pdf".to_string(),
            "txt" => "text/plain".to_string(),
            "mp3" => "audio/mpeg".to_string(),
            "wav" => "audio/wav".to_string(),
            "mp4" => "video/mp4".to_string(),
            _ => "application/octet-stream".to_string(),
        },
        None => "application/octet-stream".to_string(),
    }
}

fn candidate_upload_services(our_jid: &str) -> Vec<String> {
    let bare = our_jid.split('/').next().unwrap_or(our_jid);
    let domain = bare.split('@').nth(1).unwrap_or(bare);
    vec![format!("upload.{}", domain), domain.to_string()]
}

async fn send_http_upload_slot_request(
    client: &mut Client,
    request_id: &str,
    service: &str,
    filename: &str,
    size: u64,
    content_type: &str,
    stream_healthy: &mut bool,
) -> bool {
    let request = Element::builder("request", "urn:xmpp:http:upload:0")
        .attr("filename".try_into().expect("valid attr"), filename)
        .attr("size".try_into().expect("valid attr"), size.to_string())
        .attr(
            "content-type".try_into().expect("valid attr"),
            content_type,
        )
        .append(Element::builder("message", "urn:xmpp:http:upload:purpose:0"))
        .build();
    let iq = Iq::Get {
        id: request_id.to_string(),
        from: None,
        to: Jid::from_str(service).ok(),
        payload: request,
    };
    safe_send_stanza(client, iq.into(), "http-upload-slot", stream_healthy).await
}

fn parse_http_upload_slot(payload: &Element) -> Option<(String, String, Vec<(String, String)>)> {
    if payload.name() != "slot" || payload.ns() != "urn:xmpp:http:upload:0" {
        return None;
    }
    let put = payload.get_child("put", "urn:xmpp:http:upload:0")?;
    let get = payload.get_child("get", "urn:xmpp:http:upload:0")?;
    let put_url = put.attr("url")?.to_string();
    let get_url = get.attr("url")?.to_string();
    let mut headers = Vec::new();
    for child in put.children() {
        if child.name() == "header" && child.ns() == "urn:xmpp:http:upload:0"
            && let Some(name) = child.attr("name")
        {
            headers.push((name.to_string(), child.text()));
        }
    }
    Some((put_url, get_url, headers))
}

async fn upload_file_to_slot(
    put_url: &str,
    path: &Path,
    content_type: &str,
    headers: &[(String, String)],
) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("read failed: {}", e))?;
    let client = reqwest::Client::new();
    let mut req = client.put(put_url).header(reqwest::header::CONTENT_TYPE, content_type);
    for (name, value) in headers {
        let allowed = matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "cookie" | "expires"
        );
        if !allowed {
            continue;
        }
        req = req.header(name, value);
    }
    let res = req
        .body(data)
        .send()
        .await
        .map_err(|e| format!("PUT failed: {}", e))?;
    if res.status().is_success() {
        Ok(())
    } else {
        Err(format!("PUT failed: HTTP {}", res.status()))
    }
}

fn build_device_list_iq(device_ids: &[u32], to: &Jid) -> Iq {
    let devices: Vec<Device> = device_ids
        .iter()
        .map(|id| Device {
            id: *id,
            label: None,
            labelsig: None,
        })
        .collect();
    let device_list_el = build_device_list_element(&devices);
    let item = xmpp_parsers::pubsub::pubsub::Item {
        id: Some(xmpp_parsers::pubsub::ItemId(String::from("current"))),
        publisher: None,
        payload: Some(device_list_el),
    };
    let publish = xmpp_parsers::pubsub::pubsub::Publish {
        node: xmpp_parsers::pubsub::NodeName(String::from(NS_OMEMO_V0_DEVICES)),
        items: vec![item],
    };
    let options = xmpp_parsers::pubsub::pubsub::PublishOptions {
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::new(
                    "FORM_TYPE",
                    xmpp_parsers::data_forms::FieldType::Hidden,
                )
                .with_value("http://jabber.org/protocol/pubsub#publish-options"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#max_items", "1"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#access_model", "open"),
            ],
        }),
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Publish {
        publish,
        publish_options: Some(options),
    };
    let mut iq = Iq::from_set(format!("pub-devices-{}", to), pubsub);
    *iq.to_mut() = Some(to.clone());
    iq
}

fn build_device_list_iq_v0(device_ids: &[u32], to: &Jid) -> Iq {
    let devices: Vec<Device> = device_ids
        .iter()
        .map(|id| Device {
            id: *id,
            label: None,
            labelsig: None,
        })
        .collect();
    let device_list_el = build_device_list_element(&devices);
    let item = xmpp_parsers::pubsub::pubsub::Item {
        id: Some(xmpp_parsers::pubsub::ItemId(String::from("current"))),
        publisher: None,
        payload: Some(device_list_el),
    };
    let publish = xmpp_parsers::pubsub::pubsub::Publish {
        node: xmpp_parsers::pubsub::NodeName(String::from(NS_OMEMO_V0_DEVICES)),
        items: vec![item],
    };
    let options = xmpp_parsers::pubsub::pubsub::PublishOptions {
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::new(
                    "FORM_TYPE",
                    xmpp_parsers::data_forms::FieldType::Hidden,
                )
                .with_value("http://jabber.org/protocol/pubsub#publish-options"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#access_model", "open"),
            ],
        }),
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Publish {
        publish,
        publish_options: Some(options),
    };
    let mut iq = Iq::from_set(format!("pub-devices-v0-{}", to), pubsub);
    *iq.to_mut() = Some(to.clone());
    iq
}

fn build_configure_node_iq(node: &str, to: &Jid) -> Iq {
    let payload = xmpp_parsers::pubsub::owner::Payload::Configure {
        node: Some(xmpp_parsers::pubsub::NodeName(String::from(node))),
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::new(
                    "FORM_TYPE",
                    xmpp_parsers::data_forms::FieldType::Hidden,
                )
                .with_value("http://jabber.org/protocol/pubsub#node_config"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#max_items", "1"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#access_model", "open"),
            ],
        }),
    };
    let owner = xmpp_parsers::pubsub::owner::Owner { payload };
    let mut iq = Iq::from_set(format!("configure-{node}-{}", to), owner);
    *iq.to_mut() = Some(to.clone());
    iq
}

fn build_purge_v0_device_list_iq(to: &Jid) -> Iq {
    let payload = xmpp_parsers::pubsub::owner::Payload::Purge {
        node: xmpp_parsers::pubsub::NodeName(String::from(NS_OMEMO_V0_DEVICES)),
    };
    let owner = xmpp_parsers::pubsub::owner::Owner { payload };
    let mut iq = Iq::from_set(format!("purge-devices-v0-{}", to), owner);
    *iq.to_mut() = Some(to.clone());
    iq
}

fn build_bundle_iq(mgr: &OmemoManager, to: &Jid) -> Option<Iq> {
    let (spk_id, spk, spks, ik, prekeys) = build_bundle_material(mgr)?;

    let bundle = Bundle {
        device_id: mgr.our_device_id(),
        spk_id,
        spk,
        spks,
        ik,
        prekeys,
    };
    let bundle_el = build_bundle_element(&bundle);
    let item = xmpp_parsers::pubsub::pubsub::Item {
        id: Some(xmpp_parsers::pubsub::ItemId(
            mgr.our_device_id().to_string(),
        )),
        publisher: None,
        payload: Some(bundle_el),
    };
    let publish = xmpp_parsers::pubsub::pubsub::Publish {
        node: xmpp_parsers::pubsub::NodeName(String::from(NS_OMEMO_V0_BUNDLES)),
        items: vec![item],
    };
    let options = xmpp_parsers::pubsub::pubsub::PublishOptions {
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::text_single("pubsub#max_items", "max"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#access_model", "open"),
            ],
        }),
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Publish {
        publish,
        publish_options: Some(options),
    };
    let mut iq = Iq::from_set("pub-bundle", pubsub);
    *iq.to_mut() = Some(to.clone());
    Some(iq)
}

fn build_bundle_iq_v0(mgr: &OmemoManager, to: &Jid) -> Option<Iq> {
    let (spk_id, spk, spks, ik, prekeys) = build_bundle_material(mgr)?;

    let bundle = Bundle {
        device_id: mgr.our_device_id(),
        spk_id,
        spk,
        spks,
        ik,
        prekeys,
    };
    let bundle_el = build_bundle_element(&bundle);
    let item = xmpp_parsers::pubsub::pubsub::Item {
        id: Some(xmpp_parsers::pubsub::ItemId(String::from("current"))),
        publisher: None,
        payload: Some(bundle_el),
    };
    let publish = xmpp_parsers::pubsub::pubsub::Publish {
        node: xmpp_parsers::pubsub::NodeName(format!(
            "{}:{}",
            NS_OMEMO_V0_BUNDLES,
            mgr.our_device_id()
        )),
        items: vec![item],
    };
    let options = xmpp_parsers::pubsub::pubsub::PublishOptions {
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::new(
                    "FORM_TYPE",
                    xmpp_parsers::data_forms::FieldType::Hidden,
                )
                .with_value("http://jabber.org/protocol/pubsub#publish-options"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#max_items", "max"),
                xmpp_parsers::data_forms::Field::text_single("pubsub#access_model", "open"),
            ],
        }),
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Publish {
        publish,
        publish_options: Some(options),
    };
    let mut iq = Iq::from_set(format!("pub-bundle-v0-{}", to), pubsub);
    *iq.to_mut() = Some(to.clone());
    Some(iq)
}

type BundleMaterial = (u32, Vec<u8>, Vec<u8>, Vec<u8>, Vec<(u32, Vec<u8>)>);

fn build_bundle_material(mgr: &OmemoManager) -> Option<BundleMaterial> {
    // Signed pre-key is derived from fallback secret and keeps its actual key id.
    // Reusing a fixed id across rotations can cause peers to cache stale SPKs.
    let (fk_id, fk_secret) = mgr.account.fallback_secret_key_bytes()?;
    let spk_secret = Curve25519SecretKey::from_slice(&fk_secret);
    let spk_pub = Curve25519PublicKey::from(&spk_secret);
    let spk = spk_pub.to_bytes().to_vec();
    let spk_id: u32 = fk_id;
    // Signal/Conversations expects the signed-prekey signature over the
    // serialized Curve25519 public key (0x05 prefix + 32-byte key).
    let mut spk_for_sig = Vec::with_capacity(33);
    spk_for_sig.push(0x05);
    spk_for_sig.extend_from_slice(&spk_pub.to_bytes());
    let spks = mgr.account.xeddsa_sign(&spk_for_sig);

    let ik = mgr.account.inner.curve25519_key().to_bytes().to_vec();

    let mut keyed = mgr.account.all_stored_one_time_secret_keys();
    keyed.sort_by_key(|(id, _)| *id);
    let prekeys: Vec<(u32, Vec<u8>)> = keyed
        .into_iter()
        .take(100)
        .map(|(orig_id, sk)| {
            let sec = Curve25519SecretKey::from_slice(&sk);
            let pk = Curve25519PublicKey::from(&sec);
            (orig_id, pk.to_bytes().to_vec())
        })
        .collect();

    Some((spk_id, spk, spks, ik, prekeys))
}

fn build_bundle_fetch_iq(jid: &str, version: OmemoVersion) -> Iq {
    let items = xmpp_parsers::pubsub::pubsub::Items {
        node: xmpp_parsers::pubsub::NodeName(String::from(version.ns_bundles())),
        max_items: None,
        subid: None,
        items: vec![],
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Items(items);
    let mut iq = Iq::from_get(format!("bundle-{jid}-{:?}", version), pubsub);
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}

fn build_bundle_fetch_iq_v0(jid: &str, device_id: u32) -> Iq {
    let node = format!("{}:{}", NS_OMEMO_V0_BUNDLES, device_id);
    let items = xmpp_parsers::pubsub::pubsub::Items {
        node: xmpp_parsers::pubsub::NodeName(node),
        max_items: None,
        subid: None,
        items: vec![],
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Items(items);
    let mut iq = Iq::from_get(format!("bundle-{}-v0-{}", jid, device_id), pubsub);
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}

fn build_device_list_fetch_iq(jid: &str, version: OmemoVersion) -> Iq {
    let items = xmpp_parsers::pubsub::pubsub::Items {
        node: xmpp_parsers::pubsub::NodeName(String::from(version.ns_devices())),
        max_items: None,
        subid: None,
        items: vec![],
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Items(items);
    let mut iq = Iq::from_get(format!("devicelist-{jid}-{:?}", version), pubsub);
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}

fn build_vcard_fetch_iq(jid: &str) -> Iq {
    let mut iq = Iq::from_get(format!("vcard-{jid}"), xmpp_parsers::vcard::VCardQuery);
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}

fn build_avatar_metadata_fetch_iq(jid: &str) -> Iq {
    let items = xmpp_parsers::pubsub::pubsub::Items {
        node: xmpp_parsers::pubsub::NodeName(String::from("urn:xmpp:avatar:metadata")),
        max_items: Some(1),
        subid: None,
        items: vec![],
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Items(items);
    let mut iq = Iq::from_get(format!("avatar-meta-{jid}"), pubsub);
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}

fn build_avatar_data_fetch_iq(jid: &str, hash: &str) -> Iq {
    let items = xmpp_parsers::pubsub::pubsub::Items {
        node: xmpp_parsers::pubsub::NodeName(String::from("urn:xmpp:avatar:data")),
        max_items: Some(1),
        subid: None,
        items: vec![],
    };
    let pubsub = xmpp_parsers::pubsub::PubSub::Items(items);
    let mut iq = Iq::from_get(
        format!("fetch-avatar-data-{}", &hash[..8.min(hash.len())]),
        pubsub,
    );
    if let Ok(target) = Jid::from_str(jid) {
        *iq.to_mut() = Some(target);
    }
    iq
}
