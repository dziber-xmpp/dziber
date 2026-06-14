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
use dziber_omemo::{NS_OMEMO_V0, NS_OMEMO_V0_BUNDLES, NS_OMEMO_V0_DEVICES, OmemoManager};
use dziber_omemo::bundle::{build_bundle_element_v0 as build_bundle_element, parse_bundle};
use dziber_omemo::device::{
    Device, build_device_list_element_v0 as build_device_list_element, parse_device_list,
};
use dziber_omemo::message::{
    build_message_stanza, parse_encrypted_message, EncryptedMessage as OmemoEncryptedMessage,
};


const NS_CARBONS: &str = "urn:xmpp:carbons:2";
const NS_FORWARD: &str = "urn:xmpp:forward:0";
const NS_RECEIPTS: &str = "urn:xmpp:receipts";
const NS_PING: &str = "urn:xmpp:ping";
const NS_CHATSTATES: &str = "http://jabber.org/protocol/chatstates";
const NS_MESSAGE_CORRECT: &str = "urn:xmpp:message-correct:0";
const NS_JINGLE: &str = "urn:xmpp:jingle:1";
const NS_JINGLE_RTP: &str = "urn:xmpp:jingle:apps:rtp:1";
const NS_JINGLE_ICE: &str = "urn:xmpp:jingle:transports:ice-udp:1";
const NS_JINGLE_RTP_INFO: &str = "urn:xmpp:jingle:apps:rtp:info:1";
const NS_JINGLE_DTLS: &str = "urn:xmpp:jingle:apps:dtls:0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatState {
    Active,
    Composing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCandidate {
    pub foundation: String,
    pub component: u32,
    pub protocol: String,
    pub priority: u64,
    pub ip: String,
    pub port: u16,
    pub typ: String,
}

#[derive(Debug, Clone)]
pub enum XmppCommand {
    Connect {
        jid: String,
        password: String,
    },
    Disconnect,
    SendMessage {
        id: String,
        to: String,
        body: String,
        omemo: bool,
    },
    SendMessageCorrection {
        id: String,
        to: String,
        replace_id: String,
        body: String,
        omemo: bool,
    },
    SendChatState {
        to: String,
        state: ChatState,
    },
    InitiateCall {
        to: String,
    },
    AcceptCall {
        with: String,
        sid: String,
    },
    RejectCall {
        with: String,
        sid: String,
        reason: CallRejectReason,
    },
    EndCall {
        with: String,
        sid: String,
    },
    SendTransportInfo {
        with: String,
        sid: String,
        candidates: Vec<IceCandidate>,
    },
    SendTransportInfoEnd {
        with: String,
        sid: String,
    },
    SendCallRinging {
        with: String,
        sid: String,
    },
    SendFile {
        to: String,
        path: String,
        omemo: bool,
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
    MessageDelivered {
        id: String,
    },
    MessageCorrected {
        from: String,
        target_id: String,
        body: String,
    },
    IncomingCall {
        from: String,
        sid: String,
    },
    CallAccepted {
        with: String,
        sid: String,
    },
    CallRinging {
        with: String,
        sid: String,
    },
    CallTransportInfo {
        with: String,
        sid: String,
        candidates: Vec<IceCandidate>,
    },
    CallEnded {
        with: String,
        sid: String,
        reason: String,
    },
    StatusChanged(String),
    BundleReceived {
        jid: String,
        device_id: u32,
    },
    OmemoReady {
        device_id: u32,
    },
    DeviceListUpdated {
        jid: String,
        devices: Vec<u32>,
    },
    OmemoMessageReceived {
        id: String,
        from: String,
        body: String,
        direction: Direction,
        replace_id: Option<String>,
    },
    AvatarReceived {
        jid: String,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRejectReason {
    Decline,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JingleSessionState {
    PendingOutgoing,
    PendingIncoming,
    Active,
    Terminating,
}

#[derive(Debug, Clone)]
struct JingleSession {
    peer_bare: String,
    state: JingleSessionState,
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
        let mut jingle_sessions: HashMap<String, JingleSession> = HashMap::new();

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
                                let mut mgr = OmemoManager::load_or_generate(
                                    rand::random(),
                                    Box::new(crate::db::omemo::DziberOmemoStore),
                                );
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
                                let device_id = mgr.our_device_id();
                                let _ = output
                                    .send(XmppEvent::OmemoReady { device_id })
                                    .await;
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

                                // Fetch bookmarked rooms (XEP-0048 via private storage).
                                let bookmarks_iq = build_bookmarks_fetch_iq();
                                let bookmarks_id = match &bookmarks_iq {
                                    Iq::Get { id, .. } | Iq::Set { id, .. } => id.clone(),
                                    _ => String::new(),
                                };
                                if !bookmarks_id.is_empty() {
                                    pending_iqs.insert(bookmarks_id, PendingIq::Bookmarks);
                                }
                                let _ = safe_send_stanza(
                                    c,
                                    bookmarks_iq.into(),
                                    "worker-loop",
                                    &mut stream_healthy,
                                )
                                .await;

                                // Fetch recent direct-message history (XEP-0313).
                                let mam_iq = build_mam_query_iq(
                                    "mam-query",
                                    "mam-sync",
                                    None,
                                    Some(String::new()),
                                );
                                tracing::info!("[MAM] Sending query: max=50 (mam:2)");
                                let _ = safe_send_stanza(c, mam_iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppEventRaw::Stanza(stanza)) => {
                                let mut sessions = SessionState {
                                    pending_iqs: &mut pending_iqs,
                                    jingle_sessions: &mut jingle_sessions,
                                };
                                handle_stanza(
                                    stanza,
                                    c,
                                    &mut output,
                                    &mut omemo,
                                    &mut sessions,
                                    our_jid.as_deref(),
                                    &mut stream_healthy,
                                ).await;
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
                                jingle_sessions.clear();
                            }
                            None => {
                                stream_healthy = false;
                                if let Some(ref mgr) = omemo {
                                    let _ = mgr.save();
                                }
                                let _ = output.send(XmppEvent::Disconnected).await;
                                client = None;
                                omemo = None;
                                jingle_sessions.clear();
                            }
                        }
                    }

                    cmd = cmd_rx.next() => {
                        match cmd {
                            Some(XmppCommand::Connect { .. }) => {}
                            Some(XmppCommand::SendMessage { id, to, body, omemo: use_omemo }) => {
                                if let Some(ref mut mgr) = omemo
                                    && use_omemo
                                {
                                    match mgr.encrypt_message(&to, &body) {
                                        Some(encrypted) => {
                                            tracing::info!("[SEND] OMEMO encrypt ok for {}", to);
                                            let msg = build_omemo_xmpp_message(
                                                &to,
                                                &encrypted,
                                                &id,
                                                None,
                                            );
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
                                                    _id: id,
                                                })
                                                .await;
                                            continue;
                                        }
                                        None => {
                                            tracing::info!("[SEND] OMEMO encrypt failed for {}", to);
                                        }
                                    }
                                }
                                let msg = make_message(&to, &body, &id);
                                tracing::info!("[SEND] Plaintext stanza dispatched to {}", to);
                                let _ = safe_send_stanza(c, msg.into(), "worker-loop", &mut stream_healthy).await;
                                let _ = output.send(XmppEvent::MessageSent { _id: id }).await;
                            }
                            Some(XmppCommand::SendMessageCorrection {
                                id,
                                to,
                                replace_id,
                                body,
                                omemo: use_omemo,
                            }) => {
                                if let Some(ref mut mgr) = omemo
                                    && use_omemo
                                {
                                    match mgr.encrypt_message(&to, &body) {
                                        Some(encrypted) => {
                                            tracing::info!(
                                                "[SEND] OMEMO correction encrypt ok for {}",
                                                to
                                            );
                                            let mut msg = build_omemo_xmpp_message(
                                                &to,
                                                &encrypted,
                                                &id,
                                                Some(&body),
                                            );
                                            msg.payloads.push(
                                                Element::builder("replace", NS_MESSAGE_CORRECT)
                                                    .attr(
                                                        "id".try_into().expect("valid attr"),
                                                        replace_id.clone(),
                                                    )
                                                    .build(),
                                            );
                                            let _ = safe_send_stanza(
                                                c,
                                                msg.into(),
                                                "worker-loop",
                                                &mut stream_healthy,
                                            )
                                            .await;
                                            tracing::info!(
                                                "[SEND] OMEMO correction stanza dispatched to {}",
                                                to
                                            );
                                            let _ = output
                                                .send(XmppEvent::MessageSent { _id: id })
                                                .await;
                                            continue;
                                        }
                                        None => {
                                            tracing::info!(
                                                "[SEND] OMEMO correction encrypt failed for {}",
                                                to
                                            );
                                        }
                                    }
                                }
                                let msg = make_correction_message(&to, &body, &id, &replace_id);
                                let _ = safe_send_stanza(
                                    c,
                                    msg.into(),
                                    "worker-loop",
                                    &mut stream_healthy,
                                )
                                .await;
                                let _ = output.send(XmppEvent::MessageSent { _id: id }).await;
                            }
                            Some(XmppCommand::SendChatState { to, state }) => {
                                let msg = make_chatstate_message(&to, state);
                                let _ = safe_send_stanza(
                                    c,
                                    msg.into(),
                                    "worker-loop",
                                    &mut stream_healthy,
                                )
                                .await;
                            }
                            Some(XmppCommand::InitiateCall { to }) => {
                                let sid = uuid::Uuid::new_v4().to_string();
                                let bare = to.split('/').next().unwrap_or(&to).to_string();
                                jingle_sessions.insert(
                                    sid.clone(),
                                    JingleSession {
                                        peer_bare: bare,
                                        state: JingleSessionState::PendingOutgoing,
                                    },
                                );
                                let iq = build_jingle_session_initiate_iq(&to, &sid, our_jid.as_deref());
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                                let _ = output.send(XmppEvent::StatusChanged(format!("Calling {to}..."))).await;
                            }
                            Some(XmppCommand::AcceptCall { with, sid }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                match jingle_sessions.get_mut(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == with_bare
                                            && sess.state == JingleSessionState::PendingIncoming =>
                                    {
                                        sess.state = JingleSessionState::Active;
                                    }
                                    _ => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!(
                                            "Ignoring AcceptCall: unknown or invalid session sid={}",
                                            sid
                                        ))).await;
                                        continue;
                                    }
                                }
                                let iq = build_jingle_session_accept_iq(&with, &sid, our_jid.as_deref());
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                                let _ = output.send(XmppEvent::CallAccepted { with, sid }).await;
                            }
                            Some(XmppCommand::RejectCall { with, sid, reason }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                if let Some(sess) = jingle_sessions.get(&sid)
                                    && sess.peer_bare != with_bare
                                {
                                    let _ = output.send(XmppEvent::StatusChanged(format!(
                                        "Ignoring RejectCall: SID peer mismatch sid={}",
                                        sid
                                    ))).await;
                                    continue;
                                }
                                if let Some(sess) = jingle_sessions.get_mut(&sid) {
                                    sess.state = JingleSessionState::Terminating;
                                }
                                let iq = build_jingle_session_reject_iq(&with, &sid, our_jid.as_deref(), reason);
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                                jingle_sessions.remove(&sid);
                                let _ = output.send(XmppEvent::CallEnded {
                                    with,
                                    sid,
                                    reason: String::from("local-reject"),
                                }).await;
                            }
                            Some(XmppCommand::EndCall { with, sid }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                match jingle_sessions.get_mut(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == with_bare
                                            && matches!(
                                                sess.state,
                                                JingleSessionState::Active
                                                    | JingleSessionState::PendingOutgoing
                                                    | JingleSessionState::PendingIncoming
                                            ) =>
                                    {
                                        sess.state = JingleSessionState::Terminating;
                                    }
                                    _ => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!(
                                            "Ignoring EndCall: unknown or invalid session sid={}",
                                            sid
                                        ))).await;
                                        continue;
                                    }
                                }
                                let iq = build_jingle_session_terminate_iq(&with, &sid, our_jid.as_deref());
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                                jingle_sessions.remove(&sid);
                                let _ = output.send(XmppEvent::CallEnded {
                                    with,
                                    sid,
                                    reason: String::from("success"),
                                }).await;
                            }
                            Some(XmppCommand::SendTransportInfo {
                                with,
                                sid,
                                candidates,
                            }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                match jingle_sessions.get(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == with_bare
                                            && matches!(
                                                sess.state,
                                                JingleSessionState::Active
                                                    | JingleSessionState::PendingOutgoing
                                                    | JingleSessionState::PendingIncoming
                                            ) => {}
                                    _ => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!(
                                            "Ignoring transport-info: unknown or invalid session sid={}",
                                            sid
                                        ))).await;
                                        continue;
                                    }
                                }
                                let iq = build_jingle_transport_info_iq(
                                    &with,
                                    &sid,
                                    our_jid.as_deref(),
                                    &candidates,
                                );
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppCommand::SendTransportInfoEnd { with, sid }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                match jingle_sessions.get(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == with_bare
                                            && matches!(
                                                sess.state,
                                                JingleSessionState::Active
                                                    | JingleSessionState::PendingOutgoing
                                                    | JingleSessionState::PendingIncoming
                                            ) => {}
                                    _ => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!(
                                            "Ignoring end-of-candidates: unknown or invalid session sid={}",
                                            sid
                                        ))).await;
                                        continue;
                                    }
                                }
                                let iq = build_jingle_transport_info_end_iq(&with, &sid, our_jid.as_deref());
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppCommand::SendCallRinging { with, sid }) => {
                                let with_bare = with.split('/').next().unwrap_or(&with).to_string();
                                match jingle_sessions.get(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == with_bare
                                            && sess.state == JingleSessionState::PendingIncoming => {}
                                    _ => {
                                        let _ = output.send(XmppEvent::StatusChanged(format!(
                                            "Ignoring ringing: unknown or invalid session sid={}",
                                            sid
                                        ))).await;
                                        continue;
                                    }
                                }
                                let iq = build_jingle_session_info_ringing_iq(&with, &sid, our_jid.as_deref());
                                let _ = safe_send_stanza(c, iq.into(), "worker-loop", &mut stream_healthy).await;
                            }
                            Some(XmppCommand::SendFile { to, path, omemo }) => {
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
                                        omemo,
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
                    Some(XmppCommand::SendMessageCorrection { .. }) => {}
                    Some(XmppCommand::SendChatState { .. }) => {}
                    Some(XmppCommand::SendFile { .. }) => {}
                    Some(XmppCommand::InitiateCall { .. }) => {}
                    Some(XmppCommand::AcceptCall { .. }) => {}
                    Some(XmppCommand::RejectCall { .. }) => {}
                    Some(XmppCommand::EndCall { .. }) => {}
                    Some(XmppCommand::SendTransportInfo { .. }) => {}
                    Some(XmppCommand::SendTransportInfoEnd { .. }) => {}
                    Some(XmppCommand::SendCallRinging { .. }) => {}
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
    Bookmarks,
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
        omemo: bool,
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

fn element_to_xml(el: &Element) -> Option<String> {
    let mut bytes = Vec::new();
    el.write_to(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn xmpp_message_to_xml(msg: &XmppMessage) -> Option<String> {
    let el: Element = msg.clone().into();
    element_to_xml(&el)
}

fn build_omemo_xmpp_message(
    to: &str,
    encrypted: &OmemoEncryptedMessage,
    id: &str,
    fallback_body: Option<&str>,
) -> XmppMessage {
    let element = build_message_stanza(to, encrypted, id, fallback_body);
    if let Some(xml) = element_to_xml(&element) {
        tracing::info!("[SEND XML] {}", xml);
    }
    element.try_into().unwrap_or_else(|_| {
        tracing::warn!(
            "[SEND] invalid JID in OMEMO message ({}), falling back to invalid@localhost",
            to
        );
        let fallback = build_message_stanza("invalid@localhost", encrypted, id, fallback_body);
        fallback
            .try_into()
            .expect("fallback OMEMO message element is a valid XMPP message")
    })
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

fn build_bookmarks_fetch_iq() -> Iq {
    let storage = Element::builder("storage", "storage:bookmarks").build();
    let query = Element::builder("query", "jabber:iq:private")
        .append(storage)
        .build();
    Iq::Get {
        id: String::from("bookmarks-get"),
        from: None,
        to: None,
        payload: query,
    }
}

fn build_jingle_session_initiate_iq(to: &str, sid: &str, our_jid: Option<&str>) -> Iq {
    let initiator = our_jid.unwrap_or_default().to_string();
    let content = Element::builder("content", NS_JINGLE)
        .attr("creator".try_into().expect("valid attr"), "initiator")
        .attr("name".try_into().expect("valid attr"), "audio")
        .append(
            Element::builder("description", NS_JINGLE_RTP)
                .attr("media".try_into().expect("valid attr"), "audio")
                .append(
                    Element::builder("payload-type", NS_JINGLE_RTP)
                        .attr("id".try_into().expect("valid attr"), "111")
                        .attr("name".try_into().expect("valid attr"), "opus")
                        .attr("clockrate".try_into().expect("valid attr"), "48000")
                        .attr("channels".try_into().expect("valid attr"), "2")
                        .build(),
                )
                .build(),
        )
        .append(
            Element::builder("transport", NS_JINGLE_ICE).append(
                Element::builder("fingerprint", NS_JINGLE_DTLS)
                    .attr("hash".try_into().expect("valid attr"), "sha-256")
                    .attr("setup".try_into().expect("valid attr"), "actpass")
                    .append("00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00")
                    .build(),
            ),
        )
        .build();
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr(
            "action".try_into().expect("valid attr"),
            "session-initiate",
        )
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr("initiator".try_into().expect("valid attr"), initiator)
        .append(content)
        .build();
    Iq::Set {
        id: format!("jingle-init-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_session_accept_iq(to: &str, sid: &str, our_jid: Option<&str>) -> Iq {
    let responder = our_jid.unwrap_or_default().to_string();
    let content = Element::builder("content", NS_JINGLE)
        .attr("creator".try_into().expect("valid attr"), "initiator")
        .attr("name".try_into().expect("valid attr"), "audio")
        .append(
            Element::builder("description", NS_JINGLE_RTP)
                .attr("media".try_into().expect("valid attr"), "audio")
                .append(
                    Element::builder("payload-type", NS_JINGLE_RTP)
                        .attr("id".try_into().expect("valid attr"), "111")
                        .attr("name".try_into().expect("valid attr"), "opus")
                        .attr("clockrate".try_into().expect("valid attr"), "48000")
                        .attr("channels".try_into().expect("valid attr"), "2")
                        .build(),
                )
                .build(),
        )
        .append(
            Element::builder("transport", NS_JINGLE_ICE).append(
                Element::builder("fingerprint", NS_JINGLE_DTLS)
                    .attr("hash".try_into().expect("valid attr"), "sha-256")
                    .attr("setup".try_into().expect("valid attr"), "active")
                    .append("00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00")
                    .build(),
            ),
        )
        .build();
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr("action".try_into().expect("valid attr"), "session-accept")
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr("responder".try_into().expect("valid attr"), responder)
        .append(content)
        .build();
    Iq::Set {
        id: format!("jingle-accept-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_session_terminate_iq(to: &str, sid: &str, our_jid: Option<&str>) -> Iq {
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr(
            "action".try_into().expect("valid attr"),
            "session-terminate",
        )
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr(
            "initiator".try_into().expect("valid attr"),
            our_jid.unwrap_or_default().to_string(),
        )
        .append(
            Element::builder("reason", NS_JINGLE)
                .append(Element::builder("success", NS_JINGLE))
                .build(),
        )
        .build();
    Iq::Set {
        id: format!("jingle-term-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_session_reject_iq(
    to: &str,
    sid: &str,
    our_jid: Option<&str>,
    reason: CallRejectReason,
) -> Iq {
    let reason_el = match reason {
        CallRejectReason::Decline => Element::builder("decline", NS_JINGLE).build(),
        CallRejectReason::Busy => Element::builder("busy", NS_JINGLE).build(),
    };
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr(
            "action".try_into().expect("valid attr"),
            "session-terminate",
        )
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr(
            "initiator".try_into().expect("valid attr"),
            our_jid.unwrap_or_default().to_string(),
        )
        .append(
            Element::builder("reason", NS_JINGLE)
                .append(reason_el)
                .build(),
        )
        .build();
    Iq::Set {
        id: format!("jingle-reject-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_session_info_ringing_iq(to: &str, sid: &str, our_jid: Option<&str>) -> Iq {
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr("action".try_into().expect("valid attr"), "session-info")
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr(
            "initiator".try_into().expect("valid attr"),
            our_jid.unwrap_or_default().to_string(),
        )
        .append(Element::builder("ringing", NS_JINGLE_RTP_INFO))
        .build();
    Iq::Set {
        id: format!("jingle-info-ringing-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_transport_info_iq(
    to: &str,
    sid: &str,
    our_jid: Option<&str>,
    candidates: &[IceCandidate],
) -> Iq {
    let mut transport = Element::builder("transport", NS_JINGLE_ICE)
        .attr("ufrag".try_into().expect("valid attr"), "dziber")
        .attr("pwd".try_into().expect("valid attr"), "dziberpwd")
        .build();
    for c in candidates {
        let cand = Element::builder("candidate", NS_JINGLE_ICE)
            .attr("foundation".try_into().expect("valid attr"), c.foundation.clone())
            .attr(
                "component".try_into().expect("valid attr"),
                c.component.to_string(),
            )
            .attr("protocol".try_into().expect("valid attr"), c.protocol.clone())
            .attr(
                "priority".try_into().expect("valid attr"),
                c.priority.to_string(),
            )
            .attr("ip".try_into().expect("valid attr"), c.ip.clone())
            .attr("port".try_into().expect("valid attr"), c.port.to_string())
            .attr("type".try_into().expect("valid attr"), c.typ.clone())
            .build();
        transport.append_child(cand);
    }
    let content = Element::builder("content", NS_JINGLE)
        .attr("creator".try_into().expect("valid attr"), "initiator")
        .attr("name".try_into().expect("valid attr"), "audio")
        .append(transport)
        .build();
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr("action".try_into().expect("valid attr"), "transport-info")
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr(
            "initiator".try_into().expect("valid attr"),
            our_jid.unwrap_or_default().to_string(),
        )
        .append(content)
        .build();
    Iq::Set {
        id: format!("jingle-transport-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn build_jingle_transport_info_end_iq(to: &str, sid: &str, our_jid: Option<&str>) -> Iq {
    let content = Element::builder("content", NS_JINGLE)
        .attr("creator".try_into().expect("valid attr"), "initiator")
        .attr("name".try_into().expect("valid attr"), "audio")
        .append(
            Element::builder("transport", NS_JINGLE_ICE)
                .append(Element::builder("end-of-candidates", NS_JINGLE_ICE))
                .build(),
        )
        .build();
    let jingle = Element::builder("jingle", NS_JINGLE)
        .attr("action".try_into().expect("valid attr"), "transport-info")
        .attr("sid".try_into().expect("valid attr"), sid.to_string())
        .attr(
            "initiator".try_into().expect("valid attr"),
            our_jid.unwrap_or_default().to_string(),
        )
        .append(content)
        .build();
    Iq::Set {
        id: format!("jingle-transport-end-{}", sid),
        from: None,
        to: Jid::from_str(to).ok(),
        payload: jingle,
    }
}

fn parse_jingle_candidates(jingle: &Element) -> Vec<IceCandidate> {
    let mut out = Vec::new();
    for content in jingle.children() {
        if content.name() != "content" || content.ns() != NS_JINGLE {
            continue;
        }
        for transport in content.children() {
            if transport.name() != "transport" || transport.ns() != NS_JINGLE_ICE {
                continue;
            }
            for cand in transport.children() {
                if cand.name() != "candidate" || cand.ns() != NS_JINGLE_ICE {
                    continue;
                }
                let Some(foundation) = cand.attr("foundation").map(str::to_string) else {
                    continue;
                };
                let Some(component) = cand.attr("component").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                let Some(priority) = cand.attr("priority").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                let Some(port) = cand.attr("port").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                let Some(protocol) = cand.attr("protocol").map(str::to_string) else {
                    continue;
                };
                let Some(ip) = cand.attr("ip").map(str::to_string) else {
                    continue;
                };
                let Some(typ) = cand.attr("type").map(str::to_string) else {
                    continue;
                };
                out.push(IceCandidate {
                    foundation,
                    component,
                    protocol,
                    priority,
                    ip,
                    port,
                    typ,
                });
            }
        }
    }
    out
}

fn parse_jingle_terminate_reason(jingle: &Element) -> Option<String> {
    for child in jingle.children() {
        if child.name() != "reason" || child.ns() != NS_JINGLE {
            continue;
        }
        if let Some(r) = child.children().next() {
            return Some(r.name().to_string());
        }
    }
    None
}

fn build_iq_jingle_error_reply(to: Option<Jid>, id: String, kind: &str) -> Iq {
    let condition = match kind {
        "out-of-order" => "unexpected-request",
        "unknown-session" => "item-not-found",
        "tie-break" => "conflict",
        _ => "bad-request",
    };
    let mut error = Element::builder("error", "jabber:client")
        .attr("type".try_into().expect("valid attr"), "cancel")
        .append(Element::builder(condition, "urn:ietf:params:xml:ns:xmpp-stanzas"))
        .build();
    error.append_child(Element::builder(kind, "urn:xmpp:jingle:errors:1").build());
    Iq::Error {
        id,
        from: None,
        to,
        payload: None,
        error: xmpp_parsers::stanza_error::StanzaError {
            type_: xmpp_parsers::stanza_error::ErrorType::Cancel,
            by: None,
            defined_condition: match condition {
                "item-not-found" => DefinedCondition::ItemNotFound,
                "conflict" => DefinedCondition::Conflict,
                "unexpected-request" => DefinedCondition::UnexpectedRequest,
                _ => DefinedCondition::BadRequest,
            },
            texts: std::collections::BTreeMap::new(),
            other: Some(error),
        },
    }
}

fn build_mam_query_iq(
    id: impl Into<String>,
    queryid: impl Into<String>,
    to: Option<Jid>,
    before: Option<String>,
) -> Iq {
    let mam_query = xmpp_parsers::mam::Query {
        queryid: Some(xmpp_parsers::mam::QueryId(queryid.into())),
        node: None,
        form: Some(xmpp_parsers::data_forms::DataForm {
            type_: xmpp_parsers::data_forms::DataFormType::Submit,
            title: None,
            instructions: None,
            fields: vec![
                xmpp_parsers::data_forms::Field::new(
                    "FORM_TYPE",
                    xmpp_parsers::data_forms::FieldType::Hidden,
                )
                .with_value("urn:xmpp:mam:2"),
            ],
        }),
        set: Some(xmpp_parsers::rsm::SetQuery {
            max: Some(50),
            after: None,
            // Empty string requests the most recent page; non-empty paginates older pages.
            before,
            index: None,
        }),
        flip_page: false,
    };
    let mut iq = Iq::from_set(id.into(), mam_query);
    *iq.to_mut() = to;
    iq
}

async fn process_message(
    msg: XmppMessage,
    output: &mut mpsc::Sender<XmppEvent>,
    omemo: &mut Option<OmemoManager>,
    direction: Direction,
    from: Jid,
    archive: Option<(Option<DateTime<Utc>>, String)>,
    our_jid: Option<&str>,
) {
    let (timestamp, archive_id) = match archive {
        Some((t, id)) => (t, Some(id)),
        None => (None, None),
    };

    fn hex(data: &[u8]) -> String {
        data.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join("")
    }

    let from_bare = from.to_bare().to_string();

    // Prefer protocol-level stable IDs for dedup across reconnect/history sync.
    let stanza_id = msg
        .payloads
        .iter()
        .find(|p| p.name() == "stanza-id" && p.ns() == "urn:xmpp:sid:0")
        .and_then(|p| p.attr("id"))
        .map(std::string::ToString::to_string);
    let stable_id = || {
        msg.id
            .as_ref()
            .map(|id| id.0.clone())
            .or_else(|| stanza_id.clone())
            .or_else(|| archive_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    };

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
                our_jid
                    .map(|j| j.split('/').next().unwrap_or(j))
                    .unwrap_or(&from_bare)
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
        let stable_id = stable_id();
        let replace_id = msg.payloads.iter().find_map(|p| {
            if p.name() == "replace" && p.ns() == NS_MESSAGE_CORRECT {
                p.attr("id").map(|v| v.to_string())
            } else {
                None
            }
        });
        let _ = output
            .send(XmppEvent::OmemoMessageReceived {
                id: stable_id,
                from: from_bare,
                body,
                direction,
                replace_id,
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
            id: stable_id(),
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
            id: stable_id(),
            from: from.to_string(),
            body: body.to_string(),
            timestamp: timestamp.unwrap_or_else(Utc::now),
            status,
            direction,
        };
        let _ = output.send(XmppEvent::MessageReceived(message)).await;
    }
}

struct SessionState<'a> {
    pending_iqs: &'a mut HashMap<String, PendingIq>,
    jingle_sessions: &'a mut HashMap<String, JingleSession>,
}

async fn handle_stanza(
    stanza: Stanza,
    client: &mut Client,
    output: &mut mpsc::Sender<XmppEvent>,
    omemo: &mut Option<OmemoManager>,
    sessions: &mut SessionState<'_>,
    our_jid: Option<&str>,
    stream_healthy: &mut bool,
) {
    let SessionState {
        pending_iqs,
        jingle_sessions,
    } = sessions;

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

            // Handle live PEP device-list updates (own or contact).
            if let Some(event) = msg.payloads.iter().find(|p| {
                p.name() == "event" && p.ns() == "http://jabber.org/protocol/pubsub#event"
            }) {
                if let Some(items) = event.get_child("items", "http://jabber.org/protocol/pubsub#event")
                {
                    if items
                        .attr("node")
                        .map(|n| n.contains("devicelist"))
                        .unwrap_or(false)
                    {
                        if let Some(item) =
                            items.get_child("item", "http://jabber.org/protocol/pubsub#event")
                        {
                            if let Some(list_el) = item.get_child("list", NS_OMEMO_V0) {
                                if let Some(device_list) = parse_device_list(list_el) {
                                    let devices: Vec<u32> =
                                        device_list.devices.iter().map(|d| d.id).collect();
                                    // PEP device-list notifications come from the account that
                                    // owns the list, so use msg.from (not msg.to).
                                    let list_jid = msg
                                        .from
                                        .as_ref()
                                        .map(|j| j.to_bare().to_string())
                                        .or_else(|| our_jid.map(|j| j.split('/').next().unwrap_or(j).to_string()))
                                        .unwrap_or_default();
                                    tracing::info!(
                                        "[OMEMO] Live device list update for {}: {:?}",
                                        list_jid,
                                        devices
                                    );
                                    if let Some(mgr) = omemo {
                                        mgr.update_device_list(&list_jid, devices.clone());
                                        let _ = output
                                            .send(XmppEvent::DeviceListUpdated {
                                                jid: list_jid,
                                                devices: devices.clone(),
                                            })
                                            .await;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(received_id) = extract_received_receipt_id(&msg) {
                let _ = output
                    .send(XmppEvent::MessageDelivered { id: received_id })
                    .await;
                return;
            }

            maybe_send_delivery_receipt(client, &msg, stream_healthy).await;

            if let Some(state) = extract_chat_state(&msg) {
                tracing::info!("[CHATSTATE] from={:?} state={}", msg.from, state);
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
                    let our_bare = our.split('/').next().unwrap_or(our);
                    if archived_from.as_deref() == Some(our_bare) {
                        // Outgoing message from archive
                        if let Some(to) = archived_msg.to.clone() {
                            tracing::info!("[MAM] treating as OUTGOING to {:?}", to);
                            process_message(
                                archived_msg,
                                output,
                                omemo,
                                Direction::Outgoing,
                                to,
                                Some((timestamp, mam_result.id.to_string())),
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
                                Some((timestamp, mam_result.id.to_string())),
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
                            Some((timestamp, mam_result.id.to_string())),
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
                if let Some(target_id) = extract_replace_id(&msg)
                    && let Some(body) = msg.bodies.get("")
                {
                    let _ = output
                        .send(XmppEvent::MessageCorrected {
                            from: from.to_bare().to_string(),
                            target_id,
                            body: body.to_string(),
                        })
                        .await;
                    return;
                }
                tracing::info!(
                    "[LIVE] message from={:?} body={:?}",
                    from,
                    msg.bodies.get("")
                );
                process_message(
                    msg,
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
                Iq::Result {
                    id,
                    from,
                    payload,
                    ..
                } => {
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
                                omemo: use_omemo,
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
                                        let file_body = format!("📎 {}\n{}", filename, get_url);
                                        let sent_msg = if let Some(mgr) = omemo
                                            && use_omemo
                                        {
                                            match mgr.encrypt_message(&to, &file_body) {
                                                Some(encrypted) => {
                                                    tracing::info!("[SEND] OMEMO file encrypt ok for {}", to);
                                                    build_omemo_xmpp_message(
                                                        &to,
                                                        &encrypted,
                                                        &uuid::Uuid::new_v4().to_string(),
                                                        Some(&file_body),
                                                    )
                                                }
                                                None => {
                                                    tracing::info!("[SEND] OMEMO file encrypt failed for {}; sending plaintext", to);
                                                    make_file_message(&to, &filename, &get_url)
                                                }
                                            }
                                        } else {
                                            make_file_message(&to, &filename, &get_url)
                                        };
                                        let _ = safe_send_stanza(
                                            client,
                                            sent_msg.into(),
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
                                                // v0 bundles use item id "current", which would
                                                // parse as 0. We can't know the real device id here,
                                                // so let the per-device fetch create the session.
                                                if device_id == 0 {
                                                    tracing::info!(
                                                        "[OMEMO] Generic bundle response for {} has item id '{}' (device id 0); skipping, will use per-device fetch",
                                                        jid,
                                                        item.id.as_ref().map(|id| id.0.as_str()).unwrap_or("(none)")
                                                    );
                                                    continue;
                                                }
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
                                                            .send(XmppEvent::BundleReceived {
                                                                jid: jid.clone(),
                                                                device_id,
                                                            })
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
                                                            .send(XmppEvent::BundleReceived {
                                                                jid: jid.clone(),
                                                                device_id,
                                                            })
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
                                            let _ = output
                                                .send(XmppEvent::DeviceListUpdated {
                                                    jid: jid.clone(),
                                                    devices: all_devices.clone(),
                                                })
                                                .await;
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
                                            mgr.update_device_list(&jid, all_devices.clone());
                                            let _ = output
                                                .send(XmppEvent::DeviceListUpdated {
                                                    jid: jid.clone(),
                                                    devices: all_devices.clone(),
                                                })
                                                .await;
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
                            PendingIq::Bookmarks => {
                                let Some(element) = payload else {
                                    tracing::info!("[BOOKMARKS] Empty private storage response");
                                    return;
                                };
                                if element.name() != "query"
                                    || element.ns() != "jabber:iq:private"
                                {
                                    tracing::info!(
                                        "[BOOKMARKS] Unexpected payload: name={} ns={}",
                                        element.name(),
                                        element.ns()
                                    );
                                    return;
                                }
                                let mut count = 0usize;
                                for storage in element.children() {
                                    if storage.name() != "storage"
                                        || storage.ns() != "storage:bookmarks"
                                    {
                                        continue;
                                    }
                                    for conf in storage.children() {
                                        if conf.name() != "conference" {
                                            continue;
                                        }
                                        let Some(jid) = conf.attr("jid") else {
                                            continue;
                                        };
                                        let name = conf
                                            .attr("name")
                                            .map(std::string::ToString::to_string);
                                        let contact = Contact {
                                            jid: jid.to_string(),
                                            name,
                                            subscription: Subscription::None,
                                            groups: vec!["Rooms".to_string()],
                                            presence: Presence::default(),
                                        };
                                        let _ = output.send(XmppEvent::RosterItem(contact)).await;
                                        if let Some(join_presence) =
                                            build_muc_join_presence(jid, &default_muc_nick(our_jid))
                                        {
                                            tracing::info!("[MUC] Joining room {}", jid);
                                            let _ = safe_send_stanza(
                                                client,
                                                join_presence.into(),
                                                "stanza-handler",
                                                stream_healthy,
                                            )
                                            .await;
                                        } else {
                                            tracing::info!(
                                                "[MUC] Failed to build join presence for room {}",
                                                jid
                                            );
                                        }
                                        if let Ok(room_jid) = Jid::from_str(jid) {
                                            let queryid =
                                                format!("mam-room-{}", jid.replace('@', "_"));
                                            let iqid =
                                                format!("mam-room-query-{}", jid.replace('@', "_"));
                                            let room_mam_iq = build_mam_query_iq(
                                                iqid,
                                                queryid,
                                                Some(room_jid),
                                                Some(String::new()),
                                            );
                                            tracing::info!(
                                                "[MAM] Sending room query for {}",
                                                jid
                                            );
                                            let _ = safe_send_stanza(
                                                client,
                                                room_mam_iq.into(),
                                                "stanza-handler",
                                                stream_healthy,
                                            )
                                            .await;
                                        }
                                        count += 1;
                                    }
                                }
                                tracing::info!("[BOOKMARKS] Loaded {} rooms", count);
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
                            if !fin.complete
                                && let Some(first) = fin.set.first.as_ref().map(|f| f.item.clone())
                            {
                                if id == "mam-query" {
                                    let next_iq = build_mam_query_iq(
                                        "mam-query",
                                        "mam-sync",
                                        None,
                                        Some(first),
                                    );
                                    tracing::info!("[MAM] Paginating older direct-history page");
                                    let _ = safe_send_stanza(
                                        client,
                                        next_iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                } else if id.starts_with("mam-room-query-") {
                                    let room_to = from.clone();
                                    let room_queryid = id.replacen("mam-room-query-", "mam-room-", 1);
                                    let next_iq =
                                        build_mam_query_iq(id.clone(), room_queryid, room_to, Some(first));
                                    tracing::info!(
                                        "[MAM] Paginating older room-history page for id={}",
                                        id
                                    );
                                    let _ = safe_send_stanza(
                                        client,
                                        next_iq.into(),
                                        "stanza-handler",
                                        stream_healthy,
                                    )
                                    .await;
                                }
                            }
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
                Iq::Set {
                    id, from, payload, ..
                } => {
                    if payload.name() == "jingle" && payload.ns() == NS_JINGLE {
                        let action = payload.attr("action").unwrap_or_default().to_string();
                        let sid = payload.attr("sid").unwrap_or_default().to_string();
                        let from_bare = from
                            .as_ref()
                            .map(|j| j.to_bare().to_string())
                            .unwrap_or_default();
                        let from_jid = from.clone();

                        let invalid = |kind: &str| build_iq_jingle_error_reply(from_jid.clone(), id.clone(), kind);

                        match action.as_str() {
                            "session-initiate" => {
                                // Tie-break: duplicate SID from same peer or SID collision.
                                if let Some(existing) = jingle_sessions.get(&sid)
                                    && existing.peer_bare != from_bare
                                {
                                    let _ = safe_send_stanza(
                                        client,
                                        invalid("tie-break").into(),
                                        "jingle-invalid",
                                        stream_healthy,
                                    ).await;
                                    return;
                                }
                                jingle_sessions.insert(
                                    sid.clone(),
                                    JingleSession {
                                        peer_bare: from_bare.clone(),
                                        state: JingleSessionState::PendingIncoming,
                                    },
                                );
                                let _ = output
                                    .send(XmppEvent::IncomingCall {
                                        from: from_bare,
                                        sid,
                                    })
                                    .await;
                            }
                            "session-accept" => {
                                match jingle_sessions.get_mut(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == from_bare
                                            && sess.state == JingleSessionState::PendingOutgoing =>
                                    {
                                        sess.state = JingleSessionState::Active;
                                    }
                                    Some(_) => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("out-of-order").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                    None => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("unknown-session").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                }
                                let _ = output
                                    .send(XmppEvent::CallAccepted {
                                        with: from_bare,
                                        sid,
                                    })
                                    .await;
                            }
                            "session-info" => {
                                match jingle_sessions.get(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == from_bare
                                            && matches!(
                                                sess.state,
                                                JingleSessionState::PendingOutgoing
                                                    | JingleSessionState::PendingIncoming
                                                    | JingleSessionState::Active
                                            ) => {}
                                    Some(_) => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("out-of-order").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                    None => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("unknown-session").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                }
                                if payload
                                    .get_child("ringing", NS_JINGLE_RTP_INFO)
                                    .is_some()
                                {
                                    let _ = output
                                        .send(XmppEvent::CallRinging {
                                            with: from_bare,
                                            sid,
                                        })
                                        .await;
                                }
                            }
                            "session-terminate" => {
                                if let Some(sess) = jingle_sessions.get_mut(&sid) {
                                    sess.state = JingleSessionState::Terminating;
                                }
                                let reason = parse_jingle_terminate_reason(&payload)
                                    .unwrap_or_else(|| String::from("unknown"));
                                jingle_sessions.remove(&sid);
                                let _ = output
                                    .send(XmppEvent::CallEnded {
                                        with: from_bare,
                                        sid,
                                        reason,
                                    })
                                    .await;
                            }
                            "transport-info" => {
                                match jingle_sessions.get(&sid) {
                                    Some(sess)
                                        if sess.peer_bare == from_bare
                                            && matches!(
                                                sess.state,
                                                JingleSessionState::PendingOutgoing
                                                    | JingleSessionState::PendingIncoming
                                                    | JingleSessionState::Active
                                            ) => {}
                                    Some(_) => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("out-of-order").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                    None => {
                                        let _ = safe_send_stanza(
                                            client,
                                            invalid("unknown-session").into(),
                                            "jingle-invalid",
                                            stream_healthy,
                                        ).await;
                                        return;
                                    }
                                }
                                let candidates = parse_jingle_candidates(&payload);
                                let _ = output
                                    .send(XmppEvent::CallTransportInfo {
                                        with: from_bare,
                                        sid,
                                        candidates,
                                    })
                                    .await;
                            }
                            _ => {}
                        }
                    }
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
                    let result = Iq::Result {
                        id,
                        from: None,
                        to: from,
                        payload: None,
                    };
                    let _ =
                        safe_send_stanza(client, result.into(), "iq-set-ack", stream_healthy).await;
                }
                Iq::Get {
                    id,
                    from,
                    payload,
                    ..
                } => {
                    let is_ping = payload.name() == "ping" && payload.ns() == NS_PING;
                    let result = Iq::Result {
                        id,
                        from: None,
                        to: from,
                        payload: None,
                    };
                    let _ =
                        safe_send_stanza(client, result.into(), "iq-get-reply", stream_healthy).await;
                    if is_ping {
                        tracing::info!("[PING] Replied to XEP-0199 ping");
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
                                omemo,
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
                                                omemo,
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

fn make_message(to: &str, body: &str, id: &str) -> XmppMessage {
    let to_jid = Jid::from_str(to).unwrap_or_else(|_| Jid::from_str("invalid@localhost").unwrap());
    let mut message = XmppMessage::new(Some(to_jid));
    message.id = Some(xmpp_parsers::message::Id(id.to_string()));
    if looks_like_room_jid(to) {
        message.type_ = MessageType::Groupchat;
    } else {
        message.type_ = MessageType::Chat;
        message
            .payloads
            .push(Element::builder("request", NS_RECEIPTS).build());
    }
    message.bodies.insert(Lang::default(), body.to_owned());
    message
}

fn make_file_message(to: &str, filename: &str, url: &str) -> XmppMessage {
    make_message(
        to,
        &format!("📎 {}\n{}", filename, url),
        &uuid::Uuid::new_v4().to_string(),
    )
}

fn make_correction_message(to: &str, body: &str, id: &str, replace_id: &str) -> XmppMessage {
    let mut message = make_message(to, body, id);
    message
        .payloads
        .push(Element::builder("replace", NS_MESSAGE_CORRECT).attr(
            "id".try_into().expect("valid attr"),
            replace_id.to_string(),
        ).build());
    message
}

fn make_chatstate_message(to: &str, state: ChatState) -> XmppMessage {
    let to_jid = Jid::from_str(to).unwrap_or_else(|_| Jid::from_str("invalid@localhost").unwrap());
    let mut message = XmppMessage::new(Some(to_jid));
    message.type_ = MessageType::Chat;
    let name = match state {
        ChatState::Active => "active",
        ChatState::Composing => "composing",
    };
    message.payloads.push(Element::builder(name, NS_CHATSTATES).build());
    message
}

fn extract_received_receipt_id(msg: &XmppMessage) -> Option<String> {
    for payload in &msg.payloads {
        if payload.name() == "received"
            && payload.ns() == NS_RECEIPTS
            && let Some(id) = payload.attr("id")
        {
            return Some(id.to_string());
        }
    }
    None
}

fn extract_replace_id(msg: &XmppMessage) -> Option<String> {
    for payload in &msg.payloads {
        if payload.name() == "replace"
            && payload.ns() == NS_MESSAGE_CORRECT
            && let Some(id) = payload.attr("id")
        {
            return Some(id.to_string());
        }
    }
    None
}

fn extract_chat_state(msg: &XmppMessage) -> Option<&'static str> {
    for payload in &msg.payloads {
        if payload.ns() != NS_CHATSTATES {
            continue;
        }
        match payload.name() {
            "active" => return Some("active"),
            "composing" => return Some("composing"),
            "paused" => return Some("paused"),
            "inactive" => return Some("inactive"),
            "gone" => return Some("gone"),
            _ => {}
        }
    }
    None
}

async fn maybe_send_delivery_receipt(
    client: &mut Client,
    msg: &XmppMessage,
    stream_healthy: &mut bool,
) {
    if msg.type_ != MessageType::Chat {
        return;
    }
    let Some(from) = msg.from.clone() else {
        return;
    };
    let Some(stanza_id) = msg.id.as_ref().map(|id| id.0.clone()) else {
        return;
    };
    let requested = msg
        .payloads
        .iter()
        .any(|p| p.name() == "request" && p.ns() == NS_RECEIPTS);
    if !requested {
        return;
    }

    let mut receipt = XmppMessage::new(Some(from));
    receipt.type_ = MessageType::Chat;
    receipt
        .payloads
        .push(
            Element::builder("received", NS_RECEIPTS)
                .attr("id".try_into().expect("valid attr"), stanza_id)
                .build(),
        );
    let _ = safe_send_stanza(client, receipt.into(), "message-receipt", stream_healthy).await;
}

fn looks_like_room_jid(jid: &str) -> bool {
    let bare = jid.split('/').next().unwrap_or(jid);
    bare.contains("@conference.") || bare.contains("@muc.")
}

fn default_muc_nick(our_jid: Option<&str>) -> String {
    let bare = our_jid
        .unwrap_or("dziber")
        .split('/')
        .next()
        .unwrap_or("dziber");
    bare.split('@').next().unwrap_or("dziber").to_string()
}

fn build_muc_join_presence(room_jid: &str, nick: &str) -> Option<XmppPresence> {
    let room = room_jid.split('/').next().unwrap_or(room_jid);
    let full = format!("{}/{}", room, nick);
    let to_jid = Jid::from_str(&full).ok()?;

    let mut presence = XmppPresence::new(PresenceType::None);
    presence.to = Some(to_jid);
    let muc_x = Element::builder("x", "http://jabber.org/protocol/muc").build();
    presence.payloads.push(muc_x);
    Some(presence)
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

type HttpUploadSlot = (String, String, Vec<(String, String)>);

fn parse_http_upload_slot(payload: &Element) -> Option<HttpUploadSlot> {
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
    let bundle = mgr.self_bundle()?;
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
    let bundle = mgr.self_bundle()?;
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn el_to_string(el: &Element) -> String {
        let mut bytes = Vec::new();
        el.write_to(&mut bytes).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    fn iq_set_payload(iq: &Iq) -> &Element {
        match iq {
            Iq::Set { payload, .. } | Iq::Get { payload, .. } => payload,
            _ => panic!("expected Get/Set IQ"),
        }
    }

    #[test]
    fn random_dziber_resource_format() {
        let r = random_dziber_resource();
        assert!(r.starts_with("dziber."));
        assert_eq!(r.len(), 7 + 10);
        assert!(r[7..].chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn extract_carbon_message_received() {
        let inner = make_message("peer@example.com", "hello carbon", "mid");
        let inner_el: Element = inner.into();
        let forwarded = Element::builder("forwarded", NS_FORWARD)
            .append(inner_el)
            .build();
        let received = Element::builder("received", NS_CARBONS)
            .append(forwarded)
            .build();
        let mut msg = XmppMessage::new(None);
        msg.payloads.push(received);

        let (ct, fwd) = extract_carbon_message(&msg).unwrap();
        assert_eq!(ct, CarbonType::Received);
        assert_eq!(fwd.bodies.get("").map(|s| s.as_str()), Some("hello carbon"));
    }

    #[test]
    fn extract_carbon_message_sent() {
        let inner = make_message("peer@example.com", "sent msg", "mid");
        let inner_el: Element = inner.into();
        let forwarded = Element::builder("forwarded", NS_FORWARD)
            .append(inner_el)
            .build();
        let sent = Element::builder("sent", NS_CARBONS)
            .append(forwarded)
            .build();
        let mut msg = XmppMessage::new(None);
        msg.payloads.push(sent);

        let (ct, fwd) = extract_carbon_message(&msg).unwrap();
        assert_eq!(ct, CarbonType::Sent);
        assert_eq!(fwd.bodies.get("").map(|s| s.as_str()), Some("sent msg"));
    }

    #[test]
    fn xmpp_message_to_xml_contains_body() {
        let msg = make_message("peer@example.com", "hi there", "mid");
        let xml = xmpp_message_to_xml(&msg).unwrap();
        assert!(xml.contains("<message"));
        assert!(xml.contains("hi there"));
    }

    #[test]
    fn build_carbons_enable_iq_structure() {
        let iq = build_carbons_enable_iq();
        let Iq::Set { id, to, payload, .. } = iq else {
            panic!("expected Set IQ");
        };
        assert_eq!(id, "carbons-enable");
        assert!(to.is_none());
        assert_eq!(payload.name(), "enable");
        assert_eq!(payload.ns(), NS_CARBONS);
    }

    #[test]
    fn build_bookmarks_fetch_iq_structure() {
        let iq = build_bookmarks_fetch_iq();
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert_eq!(id, "bookmarks-get");
        assert!(to.is_none());
        assert_eq!(payload.name(), "query");
        assert_eq!(payload.ns(), "jabber:iq:private");
        let storage = payload.get_child("storage", "storage:bookmarks");
        assert!(storage.is_some());
    }

    #[test]
    fn build_jingle_session_initiate_iq_structure() {
        let iq = build_jingle_session_initiate_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
        );
        let payload = iq_set_payload(&iq);
        assert_eq!(payload.name(), "jingle");
        assert_eq!(payload.ns(), NS_JINGLE);
        assert_eq!(payload.attr("action"), Some("session-initiate"));
        assert_eq!(payload.attr("sid"), Some("sid1"));
        assert_eq!(payload.attr("initiator"), Some("me@example.com/dziber"));
    }

    #[test]
    fn build_jingle_session_accept_iq_structure() {
        let iq = build_jingle_session_accept_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
        );
        let payload = iq_set_payload(&iq);
        assert_eq!(payload.attr("action"), Some("session-accept"));
        assert_eq!(payload.attr("sid"), Some("sid1"));
        assert_eq!(payload.attr("responder"), Some("me@example.com/dziber"));
    }

    #[test]
    fn build_jingle_session_terminate_iq_structure() {
        let iq = build_jingle_session_terminate_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
        );
        let payload = iq_set_payload(&iq);
        assert_eq!(payload.attr("action"), Some("session-terminate"));
        let reason = payload.get_child("reason", NS_JINGLE).unwrap();
        assert!(reason.get_child("success", NS_JINGLE).is_some());
    }

    #[test]
    fn build_jingle_session_reject_iq_decline() {
        let iq = build_jingle_session_reject_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
            CallRejectReason::Decline,
        );
        let payload = iq_set_payload(&iq);
        let reason = payload.get_child("reason", NS_JINGLE).unwrap();
        assert!(reason.get_child("decline", NS_JINGLE).is_some());
    }

    #[test]
    fn build_jingle_session_reject_iq_busy() {
        let iq = build_jingle_session_reject_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
            CallRejectReason::Busy,
        );
        let payload = iq_set_payload(&iq);
        let reason = payload.get_child("reason", NS_JINGLE).unwrap();
        assert!(reason.get_child("busy", NS_JINGLE).is_some());
    }

    #[test]
    fn build_jingle_session_info_ringing_iq_structure() {
        let iq = build_jingle_session_info_ringing_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
        );
        let payload = iq_set_payload(&iq);
        assert_eq!(payload.attr("action"), Some("session-info"));
        assert!(payload.get_child("ringing", NS_JINGLE_RTP_INFO).is_some());
    }

    #[test]
    fn parse_jingle_candidates_roundtrip() {
        let candidates = vec![IceCandidate {
            foundation: "1".to_string(),
            component: 1,
            protocol: "udp".to_string(),
            priority: 2_130_706_431,
            ip: "192.0.2.1".to_string(),
            port: 5000,
            typ: "host".to_string(),
        }];
        let iq = build_jingle_transport_info_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
            &candidates,
        );
        let payload = iq_set_payload(&iq);
        let parsed = parse_jingle_candidates(payload);
        assert_eq!(parsed.len(), 1);
        let c = &parsed[0];
        assert_eq!(c.foundation, "1");
        assert_eq!(c.component, 1);
        assert_eq!(c.protocol, "udp");
        assert_eq!(c.priority, 2_130_706_431);
        assert_eq!(c.ip, "192.0.2.1");
        assert_eq!(c.port, 5000);
        assert_eq!(c.typ, "host");
    }

    #[test]
    fn build_jingle_transport_info_end_iq_structure() {
        let iq = build_jingle_transport_info_end_iq(
            "peer@example.com/resource",
            "sid1",
            Some("me@example.com/dziber"),
        );
        let payload = iq_set_payload(&iq);
        let content = payload.get_child("content", NS_JINGLE).unwrap();
        let transport = content.get_child("transport", NS_JINGLE_ICE).unwrap();
        assert!(transport.get_child("end-of-candidates", NS_JINGLE_ICE).is_some());
    }

    #[test]
    fn parse_jingle_terminate_reason_success() {
        let jingle = Element::builder("jingle", NS_JINGLE)
            .append(
                Element::builder("reason", NS_JINGLE)
                    .append(Element::builder("success", NS_JINGLE).build())
                    .build(),
            )
            .build();
        assert_eq!(parse_jingle_terminate_reason(&jingle), Some("success".to_string()));
    }

    #[test]
    fn parse_jingle_terminate_reason_none() {
        let jingle = Element::builder("jingle", NS_JINGLE).build();
        assert_eq!(parse_jingle_terminate_reason(&jingle), None);
    }

    #[test]
    fn build_iq_jingle_error_reply_out_of_order() {
        let to = Jid::from_str("peer@example.com").ok();
        let iq = build_iq_jingle_error_reply(to.clone(), "id1".to_string(), "out-of-order");
        let Iq::Error { id, to: t, error, .. } = iq else {
            panic!("expected Error IQ");
        };
        assert_eq!(id, "id1");
        assert_eq!(t, to);
        assert_eq!(error.defined_condition, DefinedCondition::UnexpectedRequest);
        let other = error.other.as_ref().unwrap();
        assert_eq!(other.name(), "error");
        assert!(other
            .get_child("out-of-order", "urn:xmpp:jingle:errors:1")
            .is_some());
    }

    #[test]
    fn build_iq_jingle_error_reply_unknown_session() {
        let iq = build_iq_jingle_error_reply(None, "id2".to_string(), "unknown-session");
        let Iq::Error { error, .. } = iq else {
            panic!("expected Error IQ");
        };
        assert_eq!(error.defined_condition, DefinedCondition::ItemNotFound);
        let other = error.other.as_ref().unwrap();
        assert_eq!(other.name(), "error");
        assert!(other
            .get_child("unknown-session", "urn:xmpp:jingle:errors:1")
            .is_some());
    }

    #[test]
    fn build_iq_jingle_error_reply_default() {
        let iq = build_iq_jingle_error_reply(None, "id3".to_string(), "unknown-kind");
        let Iq::Error { error, .. } = iq else {
            panic!("expected Error IQ");
        };
        assert_eq!(error.defined_condition, DefinedCondition::BadRequest);
    }

    #[test]
    fn build_mam_query_iq_structure() {
        let to = Jid::from_str("archive@example.com").ok();
        let iq = build_mam_query_iq("mam-1", "q1", to.clone(), Some("before-id".to_string()));
        let Iq::Set { id, to: t, payload, .. } = iq else {
            panic!("expected Set IQ");
        };
        assert_eq!(id, "mam-1");
        assert_eq!(t, to);
        assert_eq!(payload.name(), "query");
        let xml = el_to_string(&payload);
        assert!(xml.contains("urn:xmpp:mam:2"));
        assert!(xml.contains("before-id"));
    }

    #[test]
    fn make_presence_sets_status_and_show() {
        let p = make_presence();
        assert_eq!(p.type_, PresenceType::None);
        assert_eq!(p.show, Some(XmppShow::Chat));
        assert_eq!(
            p.statuses.get(&Lang::default()),
            Some(&"Dziber XMPP".to_string())
        );
    }

    #[test]
    fn make_message_chat_requests_receipt() {
        let m = make_message("user@example.com", "hello", "mid");
        assert_eq!(m.type_, MessageType::Chat);
        assert_eq!(m.bodies.get(""), Some(&"hello".to_string()));
        assert!(m.payloads.iter().any(|p| p.name() == "request" && p.ns() == NS_RECEIPTS));
    }

    #[test]
    fn make_message_groupchat_no_receipt() {
        let m = make_message("room@conference.example.com", "hello", "mid");
        assert_eq!(m.type_, MessageType::Groupchat);
        assert!(!m.payloads.iter().any(|p| p.name() == "request"));
    }

    #[test]
    fn make_file_message_body_contains_filename_and_url() {
        let m = make_file_message("user@example.com", "photo.png", "https://get.example.com/x");
        let body = m.bodies.get("").unwrap();
        assert!(body.contains("photo.png"));
        assert!(body.contains("https://get.example.com/x"));
    }

    #[test]
    fn make_correction_message_includes_replace_payload() {
        let m = make_correction_message("user@example.com", "fixed", "mid", "old-mid");
        assert!(m.payloads.iter().any(|p| {
            p.name() == "replace"
                && p.ns() == NS_MESSAGE_CORRECT
                && p.attr("id") == Some("old-mid")
        }));
        assert_eq!(m.bodies.get(""), Some(&"fixed".to_string()));
    }

    #[test]
    fn make_chatstate_message_active_and_composing() {
        let active = make_chatstate_message("user@example.com", ChatState::Active);
        assert!(active.payloads.iter().any(|p| p.name() == "active" && p.ns() == NS_CHATSTATES));

        let composing = make_chatstate_message("user@example.com", ChatState::Composing);
        assert!(composing
            .payloads
            .iter()
            .any(|p| p.name() == "composing" && p.ns() == NS_CHATSTATES));
    }

    #[test]
    fn extract_received_receipt_id_finds_id() {
        let mut m = XmppMessage::new(None);
        m.payloads.push(
            Element::builder("received", NS_RECEIPTS)
                .attr("id".try_into().expect("valid attr"), "receipt-id")
                .build(),
        );
        assert_eq!(extract_received_receipt_id(&m), Some("receipt-id".to_string()));
    }

    #[test]
    fn extract_replace_id_finds_id() {
        let mut m = XmppMessage::new(None);
        m.payloads.push(
            Element::builder("replace", NS_MESSAGE_CORRECT)
                .attr("id".try_into().expect("valid attr"), "replace-id")
                .build(),
        );
        assert_eq!(extract_replace_id(&m), Some("replace-id".to_string()));
    }

    #[test]
    fn extract_chat_state_returns_known_states() {
        for (name, expected) in [
            ("active", Some("active")),
            ("composing", Some("composing")),
            ("paused", Some("paused")),
            ("unknown", None),
        ] {
            let mut m = XmppMessage::new(None);
            m.payloads.push(Element::builder(name, NS_CHATSTATES).build());
            assert_eq!(extract_chat_state(&m), expected);
        }
    }

    #[test]
    fn looks_like_room_jid_detects_muc() {
        assert!(looks_like_room_jid("room@conference.example.com"));
        assert!(looks_like_room_jid("room@muc.example.com/nick"));
        assert!(!looks_like_room_jid("user@example.com"));
        assert!(!looks_like_room_jid("user@example.com/resource"));
    }

    #[test]
    fn default_muc_nick_derives_from_jid() {
        assert_eq!(default_muc_nick(Some("user@example.com/dziber")), "user");
        assert_eq!(default_muc_nick(None), "dziber");
    }

    #[test]
    fn build_muc_join_presence_structure() {
        let p = build_muc_join_presence("room@conference.example.com", "nick").unwrap();
        assert_eq!(p.to.as_ref().unwrap().to_string(), "room@conference.example.com/nick");
        assert!(p.payloads.iter().any(|pl| {
            pl.name() == "x" && pl.ns() == "http://jabber.org/protocol/muc"
        }));
    }

    #[test]
    fn guess_content_type_known_and_default() {
        assert_eq!(guess_content_type(Path::new("image.PNG")), "image/png");
        assert_eq!(guess_content_type(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(guess_content_type(Path::new("unknown.xyz")), "application/octet-stream");
        assert_eq!(guess_content_type(Path::new("no_extension")), "application/octet-stream");
    }

    #[test]
    fn candidate_upload_services_prefers_upload_subdomain() {
        let services = candidate_upload_services("user@example.com/resource");
        assert_eq!(services, vec!["upload.example.com", "example.com"]);
    }

    #[test]
    fn parse_http_upload_slot_extracts_urls_and_headers() {
        let slot = Element::builder("slot", "urn:xmpp:http:upload:0")
            .append(
                Element::builder("put", "urn:xmpp:http:upload:0")
                    .attr("url".try_into().expect("valid attr"), "https://put.example.com/x")
                    .append(
                        Element::builder("header", "urn:xmpp:http:upload:0")
                            .attr("name".try_into().expect("valid attr"), "Authorization")
                            .append("Bearer token")
                            .build(),
                    )
                    .build(),
            )
            .append(
                Element::builder("get", "urn:xmpp:http:upload:0")
                    .attr("url".try_into().expect("valid attr"), "https://get.example.com/x")
                    .build(),
            )
            .build();
        let (put, get, headers) = parse_http_upload_slot(&slot).unwrap();
        assert_eq!(put, "https://put.example.com/x");
        assert_eq!(get, "https://get.example.com/x");
        assert_eq!(headers, vec![("Authorization".to_string(), "Bearer token".to_string())]);
    }

    #[test]
    fn build_device_list_iq_is_pubsub_publish() {
        let to = Jid::from_str("user@example.com").unwrap();
        let iq = build_device_list_iq(&[1, 2, 3], &to);
        let Iq::Set { id, to: t, payload, .. } = iq else {
            panic!("expected Set IQ");
        };
        assert!(id.starts_with("pub-devices-"));
        assert_eq!(t.as_ref(), Some(&to));
        assert_eq!(payload.name(), "pubsub");
        assert_eq!(payload.ns(), "http://jabber.org/protocol/pubsub");
    }

    #[test]
    fn build_device_list_iq_v0_is_pubsub_publish() {
        let to = Jid::from_str("user@example.com").unwrap();
        let iq = build_device_list_iq_v0(&[4, 5], &to);
        let Iq::Set { id, payload, .. } = iq else {
            panic!("expected Set IQ");
        };
        assert!(id.starts_with("pub-devices-v0-"));
        assert_eq!(payload.name(), "pubsub");
    }

    #[test]
    fn build_device_list_fetch_iq_targets_jid() {
        let iq = build_device_list_fetch_iq("user@example.com", OmemoVersion::V0);
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert!(id.contains("V0"));
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        assert_eq!(payload.name(), "pubsub");
    }

    #[test]
    fn build_bundle_fetch_iq_targets_jid() {
        let iq = build_bundle_fetch_iq("user@example.com", OmemoVersion::V0);
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert!(id.contains("V0"));
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        assert_eq!(payload.name(), "pubsub");
    }

    #[test]
    fn build_bundle_fetch_iq_v0_includes_device_id() {
        let iq = build_bundle_fetch_iq_v0("user@example.com", 42);
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert!(id.contains("42"));
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        let items = payload.get_child("items", "http://jabber.org/protocol/pubsub").unwrap();
        assert!(items.attr("node").unwrap().contains("42"));
    }

    #[test]
    fn build_vcard_fetch_iq_targets_jid() {
        let iq = build_vcard_fetch_iq("user@example.com");
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert_eq!(id, "vcard-user@example.com");
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        assert_eq!(payload.name(), "vCard");
    }

    #[test]
    fn build_avatar_metadata_fetch_iq_targets_jid() {
        let iq = build_avatar_metadata_fetch_iq("user@example.com");
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert_eq!(id, "avatar-meta-user@example.com");
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        let items = payload.get_child("items", "http://jabber.org/protocol/pubsub").unwrap();
        assert_eq!(items.attr("node"), Some("urn:xmpp:avatar:metadata"));
    }

    #[test]
    fn build_avatar_data_fetch_iq_uses_hash_prefix() {
        let iq = build_avatar_data_fetch_iq("user@example.com", "abcdef1234567890");
        let Iq::Get { id, to, payload, .. } = iq else {
            panic!("expected Get IQ");
        };
        assert_eq!(id, "fetch-avatar-data-abcdef12");
        assert_eq!(to.as_ref().unwrap().to_string(), "user@example.com");
        let items = payload.get_child("items", "http://jabber.org/protocol/pubsub").unwrap();
        assert_eq!(items.attr("node"), Some("urn:xmpp:avatar:data"));
    }
}
