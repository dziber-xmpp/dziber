use std::{str::FromStr, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{SinkExt, StreamExt, channel::mpsc, future::FutureExt};
use gloo_net::websocket::{Message as WsMessage, futures::WebSocket};
use minidom::Element;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use wasm_bindgen_futures::spawn_local;

use super::{
    model::{ChatMessage, Contact, Fetch, PendingOutgoing, Room, Shared, StreamState, bare_jid},
    store::BrowserStore,
    ui::render,
};
use crate::omemo::{
    NS_OMEMO_V0_BUNDLES, NS_OMEMO_V0_DEVICES, OmemoManager, build_bundle_element_v0,
    build_device_list_element_v0, build_message_stanza, device::Device, parse_bundle,
    parse_device_list, parse_encrypted_message,
};

const CLIENT: &str = "jabber:client";

trait ElementText {
    fn to_string(&self) -> String;
}

impl ElementText for Element {
    fn to_string(&self) -> String {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes).unwrap();
        String::from_utf8(bytes).unwrap()
    }
}

pub fn connect(shared: &Shared) {
    {
        let mut chat = shared.borrow_mut();
        if chat.connected || chat.connecting {
            return;
        }
        chat.connecting = true;
        chat.manual_disconnect = false;
        chat.error.clear();
    }
    render(shared);
    let shared = shared.clone();
    spawn_local(async move { run(shared).await });
}

pub fn disconnect(shared: &Shared) {
    {
        let mut chat = shared.borrow_mut();
        chat.manual_disconnect = true;
        chat.connected = false;
        chat.connecting = false;
        chat.outgoing = None;
        chat.omemo = None;
        chat.contacts.clear();
        chat.rooms.clear();
    }
    render(shared);
}

pub fn select(shared: &Shared, jid: String, room: bool) {
    shared.borrow_mut().selected = Some((jid.clone(), room));
    if !shared.borrow().history.contains(&jid) {
        history(shared, &jid, room);
    }
    render(shared);
}

pub fn join_room(shared: &Shared, jid: String, nick: String) {
    if jid.is_empty() || nick.is_empty() {
        return;
    }
    let presence = Element::builder("presence", CLIENT)
        .attr(name("to"), format!("{jid}/{nick}"))
        .append(Element::builder("x", "http://jabber.org/protocol/muc").build())
        .build();
    send(shared, presence.to_string());
    {
        let mut chat = shared.borrow_mut();
        if !chat.rooms.iter().any(|room| room.jid == jid) {
            chat.rooms.push(Room {
                jid: jid.clone(),
                name: Some(jid.split('@').next().unwrap_or(&jid).to_owned()),
                nick,
                joined: true,
            });
        }
    }
    select(shared, jid, true);
}

pub fn leave_room(shared: &Shared, jid: &str) {
    let room = shared
        .borrow()
        .rooms
        .iter()
        .find(|room| room.jid == jid)
        .cloned();
    if let Some(room) = room {
        send(
            shared,
            Element::builder("presence", CLIENT)
                .attr(name("to"), format!("{jid}/{}", room.nick))
                .attr(name("type"), "unavailable")
                .build()
                .to_string(),
        );
    }
    shared.borrow_mut().rooms.retain(|room| room.jid != jid);
    if shared
        .borrow()
        .selected
        .as_ref()
        .is_some_and(|(selected, _)| selected == jid)
    {
        shared.borrow_mut().selected = None;
    }
    render(shared);
}

pub fn send_message(shared: &Shared, body: String) {
    let body = body.trim().to_owned();
    if body.is_empty() {
        return;
    }
    let Some((to, room)) = shared.borrow().selected.clone() else {
        return;
    };
    if !shared.borrow().connected {
        return;
    }
    let id = shared.borrow_mut().id("message");
    if room {
        let stanza = Element::builder("message", CLIENT)
            .attr(name("to"), &to)
            .attr(name("type"), "groupchat")
            .attr(name("id"), &id)
            .append(
                Element::builder("body", CLIENT)
                    .append(body.clone())
                    .build(),
            )
            .build();
        send(shared, stanza.to_string());
        let nick = shared
            .borrow()
            .rooms
            .iter()
            .find(|item| item.jid == to)
            .map(|room| room.nick.clone())
            .unwrap_or_default();
        add_message(
            shared,
            to,
            ChatMessage {
                id,
                from: nick,
                body,
                timestamp: js_sys::Date::now(),
                incoming: false,
                room: true,
                encrypted: false,
                failed: false,
            },
        );
        return;
    }
    if shared.borrow().omemo_enabled(&to) && shared.borrow().omemo.is_some() {
        begin_omemo(shared, to, body, id);
    } else {
        send_plain(shared, to, body, id, false);
    }
}

pub fn history(shared: &Shared, jid: &str, room: bool) {
    if !shared.borrow().connected || shared.borrow().history_loading.contains(jid) {
        return;
    }
    let id = shared.borrow_mut().id("history");
    let archive = if room {
        jid.to_owned()
    } else {
        bare_jid(&shared.borrow().email)
    };
    let mut query = Element::builder("query", "urn:xmpp:mam:2").append(
        Element::builder("set", "http://jabber.org/protocol/rsm")
            .append(
                Element::builder("max", "http://jabber.org/protocol/rsm")
                    .append("50")
                    .build(),
            )
            .build(),
    );
    if !room {
        query = query.append(
            Element::builder("with", "urn:xmpp:mam:2")
                .append(jid)
                .build(),
        );
    }
    let iq = Element::builder("iq", CLIENT)
        .attr(name("type"), "set")
        .attr(name("id"), &id)
        .attr(name("to"), archive)
        .append(query)
        .build();
    shared.borrow_mut().fetches.insert(
        id,
        Fetch::History {
            jid: jid.to_owned(),
        },
    );
    shared.borrow_mut().history_loading.insert(jid.to_owned());
    send(shared, iq.to_string());
    render(shared);
}

async fn run(shared: Shared) {
    let url = {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .unwrap();
        document
            .get_element_by_id("chat-ws-url")
            .and_then(|element| {
                wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlInputElement>(element).ok()
            })
            .map(|input| input.value())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| shared.borrow().ws_url.clone())
    };
    let socket = match WebSocket::open_with_protocol(&url, "xmpp") {
        Ok(socket) => socket,
        Err(error) => {
            fail(&shared, format!("WebSocket connection failed: {error}"));
            return;
        }
    };
    let (mut write, mut read) = socket.split();
    let (sender, mut receiver) = mpsc::unbounded::<String>();
    shared.borrow_mut().outgoing = Some(sender);
    let email = shared.borrow().email.clone();
    let domain = email.split('@').nth(1).unwrap_or_default();
    if write
        .send(WsMessage::Text(open_stream(&email, domain)))
        .await
        .is_err()
    {
        fail(&shared, "Failed to open XMPP stream".to_owned());
        return;
    }
    loop {
        let incoming = read.next().fuse();
        let outgoing = receiver.next().fuse();
        futures::pin_mut!(incoming, outgoing);
        futures::select! {
            message = incoming => {
                match message {
                    Some(Ok(WsMessage::Text(frame))) => handle_frame(&shared, &frame),
                    Some(Ok(WsMessage::Bytes(_))) => fail(&shared, "Binary XMPP frames are not supported".to_owned()),
                    Some(Err(error)) => { fail(&shared, format!("XMPP WebSocket error: {error}")); break },
                    None => break,
                }
            }
            message = outgoing => {
                match message {
                    Some(text) => if write.send(WsMessage::Text(text)).await.is_err() {
                        fail(&shared, "Failed to send XMPP stanza".to_owned());
                        break;
                    },
                    None => break,
                }
            }
        }
        if shared.borrow().manual_disconnect {
            break;
        }
    }
    let was_manual = shared.borrow().manual_disconnect;
    {
        let mut chat = shared.borrow_mut();
        chat.connected = false;
        chat.connecting = false;
        chat.outgoing = None;
        chat.omemo = None;
        if !was_manual && chat.error.is_empty() {
            chat.error = "Disconnected".to_owned();
        }
    }
    render(&shared);
    if !was_manual {
        let shared = shared.clone();
        spawn_local(async move {
            gloo_timers::future::sleep(Duration::from_secs(2)).await;
            connect(&shared);
        });
    }
}

fn open_stream(email: &str, domain: &str) -> String {
    Element::builder("open", "urn:ietf:params:xml:ns:xmpp-framing")
        .attr(name("from"), email)
        .attr(name("to"), domain)
        .attr(name("version"), "1.0")
        .build()
        .to_string()
}

fn handle_frame(shared: &Shared, frame: &str) {
    let wrapped = format!("<root>{frame}</root>");
    let Ok(root) = Element::from_str(&wrapped) else {
        fail(shared, "Received invalid XML from XMPP server".to_owned());
        return;
    };
    for element in root.children() {
        handle_element(shared, element);
    }
    render(shared);
}

fn handle_element(shared: &Shared, element: &Element) {
    match element.name() {
        "open" => {}
        "features" => match shared.borrow().stream_state {
            StreamState::Authenticate => {
                let credentials = format!("\0{}\0{}", shared.borrow().email, shared.borrow().token);
                let auth = Element::builder("auth", "urn:ietf:params:xml:ns:xmpp-sasl")
                    .attr(name("mechanism"), "PLAIN")
                    .append(STANDARD.encode(credentials))
                    .build();
                send(shared, auth.to_string());
            }
            StreamState::Bind => {
                let id = shared.borrow_mut().id("bind");
                let iq = Element::builder("iq", CLIENT)
                    .attr(name("type"), "set")
                    .attr(name("id"), id)
                    .append(
                        Element::builder("bind", "urn:ietf:params:xml:ns:xmpp-bind")
                            .append(
                                Element::builder("resource", "urn:ietf:params:xml:ns:xmpp-bind")
                                    .append("freenit")
                                    .build(),
                            )
                            .build(),
                    )
                    .build();
                send(shared, iq.to_string());
            }
            StreamState::Online => {}
        },
        "success" => {
            shared.borrow_mut().stream_state = StreamState::Bind;
            let email = shared.borrow().email.clone();
            let domain = email.split('@').nth(1).unwrap_or_default().to_owned();
            send(shared, open_stream(&email, &domain));
        }
        "failure" => fail(shared, "XMPP authentication failed".to_owned()),
        "iq" => handle_iq(shared, element),
        "message" => handle_message(shared, element, None),
        "presence" => handle_presence(shared, element),
        "error" => fail(shared, format!("XMPP stream error: {}", element.text())),
        _ => {}
    }
}

fn handle_iq(shared: &Shared, iq: &Element) {
    if iq.attr("type") == Some("result")
        && iq
            .children()
            .any(|child| child.name() == "bind" && child.ns() == "urn:ietf:params:xml:ns:xmpp-bind")
    {
        online(shared, iq);
        return;
    }
    let id = iq.attr("id").unwrap_or_default().to_owned();
    let fetch = shared.borrow_mut().fetches.remove(&id);
    match fetch {
        Some(Fetch::Roster) => parse_roster(shared, iq),
        Some(Fetch::Bookmarks) => parse_bookmarks(shared, iq),
        Some(Fetch::History { jid }) => finish_history(shared, &jid),
        Some(Fetch::Devices { jid, publish }) => parse_devices(shared, iq, &jid, publish),
        Some(Fetch::Bundle { jid, device }) => parse_bundle_response(shared, iq, &jid, device),
        None => {}
    }
}

fn online(shared: &Shared, iq: &Element) {
    let jid = iq
        .children()
        .find(|child| child.name() == "bind")
        .and_then(|bind| bind.get_child("jid", "urn:ietf:params:xml:ns:xmpp-bind"))
        .map(|jid| jid.text())
        .unwrap_or_else(|| shared.borrow().email.clone());
    {
        let mut chat = shared.borrow_mut();
        chat.bound_jid = jid.clone();
        chat.connected = true;
        chat.connecting = false;
        chat.stream_state = StreamState::Online;
        chat.error.clear();
        let store = BrowserStore::new(&chat.snapshot);
        let mut omemo = OmemoManager::load_or_generate(
            OmemoManager::generate_compatible_device_id(),
            Box::new(store.clone()),
        );
        omemo.set_our_jid(&bare_jid(&jid));
        chat.omemo_store = Some(store);
        chat.omemo = Some(omemo);
    }
    send(
        shared,
        Element::builder("presence", CLIENT).build().to_string(),
    );
    request_roster(shared);
    send(
        shared,
        Element::builder("iq", CLIENT)
            .attr(name("type"), "set")
            .append(Element::builder("enable", "urn:xmpp:carbons:2").build())
            .build()
            .to_string(),
    );
    request_bookmarks(shared);
    fetch_devices(shared, &bare_jid(&jid), true);
    persist(shared);
}

fn request_roster(shared: &Shared) {
    let id = shared.borrow_mut().id("roster");
    shared
        .borrow_mut()
        .fetches
        .insert(id.clone(), Fetch::Roster);
    send(
        shared,
        Element::builder("iq", CLIENT)
            .attr(name("type"), "get")
            .attr(name("id"), id)
            .append(Element::builder("query", "jabber:iq:roster").build())
            .build()
            .to_string(),
    );
}

fn parse_roster(shared: &Shared, iq: &Element) {
    let mut contacts: Vec<_> = iq
        .children()
        .filter(|child| child.name() == "query" && child.ns() == "jabber:iq:roster")
        .flat_map(|query| query.children().filter(|item| item.name() == "item"))
        .map(|item| Contact {
            jid: bare_jid(item.attr("jid").unwrap_or_default()),
            name: item.attr("name").map(str::to_owned),
            presence: "unavailable".to_owned(),
            status: String::new(),
        })
        .collect();
    let email = bare_jid(&shared.borrow().email);
    if !contacts.iter().any(|contact| contact.jid == email) {
        contacts.push(Contact {
            jid: email,
            name: Some("Me".to_owned()),
            presence: "available".to_owned(),
            status: String::new(),
        });
    }
    shared.borrow_mut().contacts = contacts;
}

fn request_bookmarks(shared: &Shared) {
    let id = shared.borrow_mut().id("bookmarks");
    shared
        .borrow_mut()
        .fetches
        .insert(id.clone(), Fetch::Bookmarks);
    send(
        shared,
        Element::builder("iq", CLIENT)
            .attr(name("type"), "get")
            .attr(name("id"), id)
            .append(
                Element::builder("query", "jabber:iq:private")
                    .append(Element::builder("storage", "storage:bookmarks").build())
                    .build(),
            )
            .build()
            .to_string(),
    );
}

fn parse_bookmarks(shared: &Shared, iq: &Element) {
    let rooms: Vec<_> = iq
        .children()
        .flat_map(|child| child.children())
        .flat_map(|child| child.children())
        .filter(|child| child.name() == "conference")
        .map(|room| Room {
            jid: room.attr("jid").unwrap_or_default().to_owned(),
            name: room.attr("name").map(str::to_owned),
            nick: room
                .get_child("nick", "storage:bookmarks")
                .map(|nick| nick.text())
                .unwrap_or_default(),
            joined: false,
        })
        .collect();
    for room in rooms {
        if !room.nick.is_empty() {
            join_room(shared, room.jid, room.nick)
        }
    }
}

fn finish_history(shared: &Shared, jid: &str) {
    shared.borrow_mut().history_loading.remove(jid);
    shared.borrow_mut().history.insert(jid.to_owned());
}

fn handle_presence(shared: &Shared, presence: &Element) {
    let from = bare_jid(presence.attr("from").unwrap_or_default());
    let status = presence
        .get_child("status", CLIENT)
        .map(|status| status.text())
        .unwrap_or_default();
    let show = presence
        .get_child("show", CLIENT)
        .map(|show| show.text())
        .unwrap_or_else(|| {
            if presence.attr("type") == Some("unavailable") {
                "unavailable".to_owned()
            } else {
                "available".to_owned()
            }
        });
    let mut chat = shared.borrow_mut();
    if let Some(contact) = chat.contacts.iter_mut().find(|contact| contact.jid == from) {
        contact.presence = show;
        contact.status = status;
    }
}

fn handle_message(shared: &Shared, message: &Element, carbon: Option<&str>) {
    for child in message.children() {
        if child.name() == "forwarded" && carbon.is_none() {
            if let Some(message) = child.children().find(|child| child.name() == "message") {
                handle_message(shared, message, Some("history"));
            }
            return;
        }
        if matches!(child.name(), "received" | "sent") && child.ns() == "urn:xmpp:carbons:2" {
            if let Some(forwarded) = child.children().find(|child| child.name() == "forwarded")
                && let Some(message) = forwarded.children().find(|child| child.name() == "message")
            {
                handle_message(shared, message, Some(child.name()));
            }
            return;
        }
    }
    let room = message.attr("type") == Some("groupchat");
    let from = message.attr("from").unwrap_or_default();
    let to = message.attr("to").unwrap_or_default();
    let encrypted = parse_encrypted_message(message);
    let mut body = message.get_child("body", CLIENT).map(|body| body.text());
    let mut failed = false;
    if let Some(encrypted) = &encrypted {
        let sender = bare_jid(from);
        body = shared
            .borrow_mut()
            .omemo
            .as_mut()
            .and_then(|omemo| omemo.decrypt_message(&sender, encrypted));
        if body.is_none() {
            body = Some("[Unable to decrypt OMEMO message]".to_owned());
            failed = true;
        }
        persist(shared);
    }
    if body.is_none() && carbon.is_some() {
        return;
    }
    let Some(body) = body else { return };
    if body.starts_with("__call__:") {
        super::call::handle(shared, &bare_jid(from), &body);
        return;
    }
    let my_jid = bare_jid(&shared.borrow().email);
    let outgoing = bare_jid(from) == my_jid;
    let key = if room {
        bare_jid(from)
    } else if outgoing {
        bare_jid(to)
    } else {
        bare_jid(from)
    };
    add_message(
        shared,
        key,
        ChatMessage {
            id: message.attr("id").unwrap_or("received").to_owned(),
            from: if room {
                from.split('/').nth(1).unwrap_or(from).to_owned()
            } else {
                bare_jid(from)
            },
            body,
            timestamp: message
                .children()
                .find(|child| child.name() == "delay")
                .and_then(|delay| delay.attr("stamp"))
                .and_then(|stamp| {
                    let timestamp = js_sys::Date::parse(stamp);
                    timestamp.is_finite().then_some(timestamp)
                })
                .unwrap_or_else(js_sys::Date::now),
            incoming: !outgoing,
            room,
            encrypted: encrypted.is_some(),
            failed,
        },
    );
}

fn add_message(shared: &Shared, key: String, message: ChatMessage) {
    let mut chat = shared.borrow_mut();
    let messages = chat.messages.entry(key).or_default();
    if !messages.iter().any(|existing| existing.id == message.id) {
        messages.push(message);
        messages.sort_by(|left, right| left.timestamp.total_cmp(&right.timestamp));
    }
    drop(chat);
    render(shared);
}

pub fn send_call_signal(shared: &Shared, to: &str, signal: serde_json::Value) {
    let body = format!("__call__:{signal}");
    let stanza = Element::builder("message", CLIENT)
        .attr(name("to"), to)
        .append(Element::builder("body", CLIENT).append(body).build())
        .build();
    send(shared, stanza.to_string());
}

fn send_plain(shared: &Shared, to: String, body: String, id: String, encrypted: bool) {
    let stanza = Element::builder("message", CLIENT)
        .attr(name("to"), &to)
        .attr(name("id"), &id)
        .append(
            Element::builder("body", CLIENT)
                .append(body.clone())
                .build(),
        )
        .build();
    send(shared, stanza.to_string());
    add_message(
        shared,
        to,
        ChatMessage {
            id,
            from: bare_jid(&shared.borrow().email),
            body,
            timestamp: js_sys::Date::now(),
            incoming: false,
            room: false,
            encrypted,
            failed: false,
        },
    );
}

fn begin_omemo(shared: &Shared, to: String, body: String, id: String) {
    let devices = shared
        .borrow()
        .omemo
        .as_ref()
        .and_then(|omemo| omemo.device_lists.get(&to).cloned())
        .unwrap_or_default();
    shared.borrow_mut().pending_outgoing.insert(
        to.clone(),
        PendingOutgoing {
            to: to.clone(),
            body,
            id,
            needed: devices.iter().copied().collect(),
        },
    );
    if devices.is_empty() {
        fetch_devices(shared, &to, false);
    } else {
        for device in devices {
            fetch_bundle(shared, &to, device)
        }
    }
}

fn fetch_devices(shared: &Shared, jid: &str, publish: bool) {
    let id = shared.borrow_mut().id("devices");
    shared.borrow_mut().fetches.insert(
        id.clone(),
        Fetch::Devices {
            jid: jid.to_owned(),
            publish,
        },
    );
    let item = Element::builder("item", NS_OMEMO_V0_DEVICES)
        .attr(name("id"), "current")
        .build();
    send(
        shared,
        pubsub_iq(&id, jid, "get", NS_OMEMO_V0_DEVICES, item, false).to_string(),
    );
}

fn parse_devices(shared: &Shared, iq: &Element, jid: &str, publish: bool) {
    let devices = iq
        .children()
        .flat_map(|child| child.children())
        .flat_map(|child| child.children())
        .flat_map(|child| child.children())
        .find_map(parse_device_list)
        .map(|list| {
            list.devices
                .into_iter()
                .map(|device| device.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut devices = devices;
    if let Some(omemo) = shared.borrow_mut().omemo.as_mut() {
        if publish {
            let own = omemo.our_device_id();
            if !devices.contains(&own) {
                devices.push(own)
            }
        }
        omemo.update_device_list(jid, devices.clone());
    }
    if publish {
        publish_devices(shared, jid, &devices);
        publish_bundle(shared, jid);
    } else if devices.is_empty() {
        if let Some(pending) = shared.borrow_mut().pending_outgoing.remove(jid) {
            send_plain(shared, pending.to, pending.body, pending.id, false);
        }
    } else {
        if let Some(pending) = shared.borrow_mut().pending_outgoing.get_mut(jid) {
            pending.needed = devices.iter().copied().collect();
        }
        for device in devices {
            fetch_bundle(shared, jid, device)
        }
    }
    persist(shared);
}

fn publish_devices(shared: &Shared, jid: &str, devices: &[u32]) {
    let id = shared.borrow_mut().id("publish-devices");
    let devices: Vec<_> = devices
        .iter()
        .map(|id| Device {
            id: *id,
            label: None,
            labelsig: None,
        })
        .collect();
    let item = Element::builder("item", NS_OMEMO_V0_DEVICES)
        .attr(name("id"), "current")
        .append(build_device_list_element_v0(&devices))
        .build();
    send(
        shared,
        pubsub_iq(&id, jid, "set", NS_OMEMO_V0_DEVICES, item, true).to_string(),
    );
}

fn publish_bundle(shared: &Shared, jid: &str) {
    let Some(bundle) = shared
        .borrow()
        .omemo
        .as_ref()
        .and_then(|omemo| omemo.self_bundle())
    else {
        return;
    };
    let id = shared.borrow_mut().id("publish-bundle");
    let device = bundle.device_id;
    let item = Element::builder("item", NS_OMEMO_V0_BUNDLES)
        .attr(name("id"), device.to_string())
        .append(build_bundle_element_v0(&bundle))
        .build();
    let node = format!("{NS_OMEMO_V0_BUNDLES}:{device}");
    send(
        shared,
        pubsub_iq(&id, jid, "set", &node, item, true).to_string(),
    );
}

fn fetch_bundle(shared: &Shared, jid: &str, device: u32) {
    let known = shared.borrow().omemo.as_ref().is_some_and(|omemo| {
        omemo
            .legacy_sessions
            .get(jid)
            .is_some_and(|sessions| sessions.contains_key(&device))
    });
    if known {
        complete_bundle(shared, jid, device);
        return;
    }
    let id = shared.borrow_mut().id("bundle");
    shared.borrow_mut().fetches.insert(
        id.clone(),
        Fetch::Bundle {
            jid: jid.to_owned(),
            device,
        },
    );
    let node = format!("{NS_OMEMO_V0_BUNDLES}:{device}");
    let item = Element::builder("item", NS_OMEMO_V0_BUNDLES)
        .attr(name("id"), device.to_string())
        .build();
    send(
        shared,
        pubsub_iq(&id, jid, "get", &node, item, false).to_string(),
    );
}

fn parse_bundle_response(shared: &Shared, iq: &Element, jid: &str, device: u32) {
    let bundle = iq
        .children()
        .flat_map(|child| child.children())
        .flat_map(|child| child.children())
        .flat_map(|child| child.children())
        .find_map(parse_bundle);
    if let Some(bundle) = bundle {
        shared
            .borrow_mut()
            .omemo
            .as_mut()
            .map(|omemo| omemo.create_session_from_bundle(jid, device, &bundle));
    }
    complete_bundle(shared, jid, device);
    persist(shared);
}

fn complete_bundle(shared: &Shared, jid: &str, device: u32) {
    let ready = if let Some(pending) = shared.borrow_mut().pending_outgoing.get_mut(jid) {
        pending.needed.remove(&device);
        pending.needed.is_empty()
    } else {
        false
    };
    if ready {
        let pending = shared.borrow_mut().pending_outgoing.remove(jid).unwrap();
        encrypt_and_send(shared, pending);
    }
}

fn encrypt_and_send(shared: &Shared, pending: PendingOutgoing) {
    let encrypted = shared
        .borrow_mut()
        .omemo
        .as_mut()
        .and_then(|omemo| omemo.encrypt_message(&pending.to, &pending.body));
    if let Some(encrypted) = encrypted {
        let stanza = build_message_stanza(&pending.to, &encrypted, &pending.id, None);
        send(shared, stanza.to_string());
        add_message(
            shared,
            pending.to,
            ChatMessage {
                id: pending.id,
                from: bare_jid(&shared.borrow().email),
                body: pending.body,
                timestamp: js_sys::Date::now(),
                incoming: false,
                room: false,
                encrypted: true,
                failed: false,
            },
        );
        persist(shared);
    } else {
        send_plain(shared, pending.to, pending.body, pending.id, false);
    }
}

fn pubsub_iq(id: &str, jid: &str, kind: &str, node: &str, item: Element, publish: bool) -> Element {
    let action = if publish { "publish" } else { "items" };
    let mut content = Element::builder(action, "http://jabber.org/protocol/pubsub")
        .attr(name("node"), node)
        .build();
    content.append_child(item);
    let pubsub = Element::builder("pubsub", "http://jabber.org/protocol/pubsub")
        .append(content)
        .build();
    Element::builder("iq", CLIENT)
        .attr(name("id"), id)
        .attr(name("type"), kind)
        .attr(name("to"), jid)
        .append(pubsub)
        .build()
}

fn persist(shared: &Shared) {
    let (snapshot, url) = {
        let chat = shared.borrow_mut();
        if let Some(omemo) = &chat.omemo {
            let _ = omemo.save();
        }
        let Some(store) = &chat.omemo_store else {
            return;
        };
        (store.snapshot(), chat.save_url.clone())
    };
    spawn_local(async move {
        let body = format!(
            "bundle={}",
            utf8_percent_encode(&snapshot, NON_ALPHANUMERIC)
        );
        let result = gloo_net::http::Request::post(&url)
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .body(body)
            .unwrap()
            .send()
            .await;
        if let Ok(response) = result
            && !response.ok()
        {
            web_sys::console::error_1(&"Failed to persist OMEMO state".into());
        }
    });
}

fn send(shared: &Shared, stanza: String) {
    if let Some(outgoing) = &shared.borrow().outgoing {
        let _ = outgoing.unbounded_send(stanza);
    }
}

fn fail(shared: &Shared, error: String) {
    shared.borrow_mut().error = error;
    shared.borrow_mut().connecting = false;
    render(shared);
}

fn name(value: &str) -> minidom::rxml::NcName {
    minidom::rxml::NcName::try_from(value).unwrap()
}
