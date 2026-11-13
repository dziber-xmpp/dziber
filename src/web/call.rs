use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    MediaStream, MediaStreamConstraints, RtcConfiguration, RtcIceCandidateInit, RtcIceServer,
    RtcPeerConnection, RtcPeerConnectionIceEvent, RtcSdpType, RtcSessionDescriptionInit,
    RtcTrackEvent,
};

use super::{model::Shared, ui, xmpp};

pub struct Call {
    pub status: String,
    pub peer: String,
    peer_connection: Option<RtcPeerConnection>,
    local_stream: Option<MediaStream>,
}

impl Default for Call {
    fn default() -> Self {
        Self {
            status: "idle".to_owned(),
            peer: String::new(),
            peer_connection: None,
            local_stream: None,
        }
    }
}

pub fn start(shared: &Shared) {
    let peer = shared
        .borrow()
        .selected
        .as_ref()
        .and_then(|(jid, room)| (!room).then_some(jid.clone()));
    let Some(peer) = peer else { return };
    if shared.borrow().call.status != "idle" {
        return;
    }
    shared.borrow_mut().call.status = "outgoing".to_owned();
    shared.borrow_mut().call.peer = peer.clone();
    render(shared);
    let shared = shared.clone();
    spawn_local(async move {
        let Some(stream) = microphone(&shared).await else {
            return;
        };
        let peer_connection = create_peer_connection(&shared, &peer);
        peer_connection.add_stream(&stream);
        shared.borrow_mut().call.local_stream = Some(stream);
        let offer = JsFuture::from(peer_connection.create_offer()).await;
        let Ok(offer) = offer else {
            fail(&shared, "Failed to create audio offer");
            return;
        };
        let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        description.set_sdp(&sdp(&offer));
        if JsFuture::from(peer_connection.set_local_description(&description))
            .await
            .is_err()
        {
            fail(&shared, "Failed to set local audio description");
            return;
        }
        shared.borrow_mut().call.peer_connection = Some(peer_connection);
        xmpp::send_call_signal(
            &shared,
            &peer,
            json!({ "type": "offer", "sdp": description.get_sdp() }),
        );
    });
}

pub fn accept(shared: &Shared) {
    let peer = shared.borrow().call.peer.clone();
    let Some(peer_connection) = shared.borrow().call.peer_connection.clone() else {
        return;
    };
    let shared = shared.clone();
    spawn_local(async move {
        let Some(stream) = microphone(&shared).await else {
            return;
        };
        peer_connection.add_stream(&stream);
        shared.borrow_mut().call.local_stream = Some(stream);
        let answer = JsFuture::from(peer_connection.create_answer()).await;
        let Ok(answer) = answer else {
            fail(&shared, "Failed to create audio answer");
            return;
        };
        let description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        description.set_sdp(&sdp(&answer));
        if JsFuture::from(peer_connection.set_local_description(&description))
            .await
            .is_err()
        {
            fail(&shared, "Failed to set local audio description");
            return;
        }
        shared.borrow_mut().call.status = "connected".to_owned();
        xmpp::send_call_signal(
            &shared,
            &peer,
            json!({ "type": "answer", "sdp": description.get_sdp() }),
        );
        render(&shared);
    });
}

pub fn reject(shared: &Shared) {
    let peer = shared.borrow().call.peer.clone();
    if !peer.is_empty() {
        xmpp::send_call_signal(shared, &peer, json!({ "type": "reject" }));
    }
    cleanup(shared);
}

pub fn end(shared: &Shared) {
    let peer = shared.borrow().call.peer.clone();
    if !peer.is_empty() {
        xmpp::send_call_signal(shared, &peer, json!({ "type": "end" }));
    }
    cleanup(shared);
}

pub fn handle(shared: &Shared, from: &str, body: &str) {
    let Ok(signal) = serde_json::from_str::<Value>(body.trim_start_matches("__call__:")) else {
        return;
    };
    match signal
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "offer" => {
            if shared.borrow().call.status != "idle" {
                xmpp::send_call_signal(shared, from, json!({ "type": "reject" }));
                return;
            }
            {
                let mut chat = shared.borrow_mut();
                chat.call.status = "incoming".to_owned();
                chat.call.peer = from.to_owned();
                chat.call.peer_connection = Some(create_peer_connection(shared, from));
            }
            let peer_connection = shared.borrow().call.peer_connection.clone().unwrap();
            let shared = shared.clone();
            let sdp = signal
                .get("sdp")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            spawn_local(async move {
                let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
                description.set_sdp(&sdp);
                if JsFuture::from(peer_connection.set_remote_description(&description))
                    .await
                    .is_err()
                {
                    fail(&shared, "Failed to apply remote audio offer");
                }
                render(&shared);
            });
        }
        "answer" => {
            let peer_connection = shared.borrow().call.peer_connection.clone();
            let shared = shared.clone();
            let sdp = signal
                .get("sdp")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            spawn_local(async move {
                if let Some(peer_connection) = peer_connection {
                    let description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                    description.set_sdp(&sdp);
                    if JsFuture::from(peer_connection.set_remote_description(&description))
                        .await
                        .is_ok()
                    {
                        shared.borrow_mut().call.status = "connected".to_owned();
                        render(&shared);
                    }
                }
            });
        }
        "candidate" => {
            let Some(candidate) = signal.get("candidate") else {
                return;
            };
            if candidate.is_null() {
                return;
            }
            let init = RtcIceCandidateInit::new(
                candidate
                    .get("candidate")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            init.set_sdp_mid(candidate.get("sdpMid").and_then(Value::as_str));
            init.set_sdp_m_line_index(
                candidate
                    .get("sdpMLineIndex")
                    .and_then(Value::as_u64)
                    .map(|index| index as u16),
            );
            if let Some(peer_connection) = shared.borrow().call.peer_connection.clone() {
                spawn_local(async move {
                    let _ = JsFuture::from(
                        peer_connection
                            .add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init)),
                    )
                    .await;
                });
            }
        }
        "reject" | "end" => cleanup(shared),
        _ => {}
    }
    render(shared);
}

pub fn render(shared: &Shared) {
    let call = &shared.borrow().call;
    set_hidden("chat-call", call.status == "idle");
    text("chat-call-peer", &call.peer);
    text("chat-call-status", &call.status);
    set_hidden("chat-call-accept", call.status != "incoming");
    set_hidden("chat-call-reject", call.status != "incoming");
    set_hidden("chat-call-end", call.status == "incoming");
    if let Some(stream) = &call.local_stream {
        audio("chat-local-audio").set_src_object(Some(stream));
    }
    if let Some(selected) = &shared.borrow().selected {
        set_hidden("chat-call-start", selected.1);
    } else {
        set_hidden("chat-call-start", true);
    }
}

fn create_peer_connection(shared: &Shared, peer: &str) -> RtcPeerConnection {
    let ice = RtcIceServer::new();
    ice.set_urls(&JsValue::from_str("stun:stun.l.google.com:19302"));
    let servers = js_sys::Array::new();
    servers.push(&ice);
    let configuration = RtcConfiguration::new();
    configuration.set_ice_servers(&servers);
    let peer_connection = RtcPeerConnection::new_with_configuration(&configuration).unwrap();

    let signal_shared = shared.clone();
    let signal_peer = peer.to_owned();
    let candidate = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
        move |event: RtcPeerConnectionIceEvent| {
            if let Some(candidate) = event.candidate() {
                xmpp::send_call_signal(
                    &signal_shared,
                    &signal_peer,
                    json!({
                        "type": "candidate",
                        "candidate": {
                            "candidate": candidate.candidate(),
                            "sdpMid": candidate.sdp_mid(),
                            "sdpMLineIndex": candidate.sdp_m_line_index(),
                        }
                    }),
                );
            }
        },
    );
    peer_connection.set_onicecandidate(Some(candidate.as_ref().unchecked_ref()));
    candidate.forget();

    let track_shared = shared.clone();
    let track = Closure::<dyn FnMut(RtcTrackEvent)>::new(move |event: RtcTrackEvent| {
        if let Some(stream) = event.streams().get(0).dyn_into::<MediaStream>().ok() {
            audio("chat-remote-audio").set_src_object(Some(&stream));
        }
        track_shared.borrow_mut().call.status = "connected".to_owned();
        render(&track_shared);
    });
    peer_connection.set_ontrack(Some(track.as_ref().unchecked_ref()));
    track.forget();
    peer_connection
}

async fn microphone(shared: &Shared) -> Option<MediaStream> {
    let constraints = MediaStreamConstraints::new();
    constraints.set_audio_bool(true);
    constraints.set_video_bool(false);
    let Ok(devices) = web_sys::window().unwrap().navigator().media_devices() else {
        fail(shared, "Microphone access is unavailable");
        return None;
    };
    let promise = devices.get_user_media_with_constraints(&constraints);
    match promise {
        Ok(promise) => match JsFuture::from(promise).await {
            Ok(stream) => stream.dyn_into::<MediaStream>().ok(),
            Err(_) => {
                fail(shared, "Microphone access failed");
                None
            }
        },
        Err(_) => {
            fail(shared, "Microphone access is unavailable");
            None
        }
    }
}

fn cleanup(shared: &Shared) {
    let mut chat = shared.borrow_mut();
    if let Some(peer_connection) = chat.call.peer_connection.take() {
        peer_connection.close();
    }
    if let Some(stream) = chat.call.local_stream.take() {
        stream.get_tracks().for_each(&mut |track, _, _| {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        });
    }
    chat.call.status = "idle".to_owned();
    chat.call.peer.clear();
    audio("chat-local-audio").set_src_object(None);
    audio("chat-remote-audio").set_src_object(None);
    drop(chat);
    render(shared);
}

fn fail(shared: &Shared, message: &str) {
    shared.borrow_mut().error = message.to_owned();
    cleanup(shared);
    ui::render(shared);
}

fn sdp(description: &JsValue) -> String {
    js_sys::Reflect::get(description, &JsValue::from_str("sdp"))
        .ok()
        .and_then(|sdp| sdp.as_string())
        .unwrap_or_default()
}

fn text(id: &str, value: &str) {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id(id)
        .unwrap()
        .set_text_content(Some(value));
}

fn set_hidden(id: &str, hidden: bool) {
    let element = web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id(id)
        .unwrap();
    if hidden {
        element.set_attribute("hidden", "").unwrap();
    } else {
        element.remove_attribute("hidden").unwrap();
    }
}

fn audio(id: &str) -> web_sys::HtmlAudioElement {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id(id)
        .unwrap()
        .dyn_into()
        .unwrap()
}
