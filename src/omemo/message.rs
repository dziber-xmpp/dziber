use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio_xmpp::minidom::Element;
use xmpp_parsers::jid::Jid;
use xmpp_parsers::message::{Lang, Message as XmppMessage, MessageType};

use super::{NS_OMEMO_V0, nc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageKey {
    pub rid: u32,
    pub kex: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysGroup {
    pub jid: String,
    pub keys: Vec<MessageKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHeader {
    pub sid: u32,
    pub keys: Vec<KeysGroup>,
    pub iv: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedMessage {
    pub is_v0: bool,
    pub header: MessageHeader,
    pub payload: Option<Vec<u8>>,
}

fn parse_key_element(key_el: &Element) -> Option<MessageKey> {
    let rid: u32 = key_el.attr("rid")?.parse().ok()?;
    let kex = key_el.attr("kex").is_some_and(|v| v == "true")
        || key_el.attr("prekey").is_some_and(|v| v == "true");
    let data = BASE64.decode(key_el.text()).ok()?;
    Some(MessageKey { rid, kex, data })
}

fn parse_encrypted_v0(encrypted_el: &Element) -> Option<EncryptedMessage> {
    let header_el = encrypted_el.get_child("header", NS_OMEMO_V0)?;
    let sid: u32 = header_el.attr("sid")?.parse().ok()?;

    let mut keys = Vec::new();
    for key_el in header_el.children() {
        if key_el.name() != "key" || key_el.ns() != NS_OMEMO_V0 {
            continue;
        }
        if let Some(key) = parse_key_element(key_el) {
            keys.push(key);
        }
    }

    let keys_groups = vec![KeysGroup {
        jid: String::new(),
        keys,
    }];

    let iv = header_el
        .get_child("iv", NS_OMEMO_V0)
        .and_then(|el| BASE64.decode(el.text()).ok());

    let header = MessageHeader {
        sid,
        keys: keys_groups,
        iv,
    };
    let payload = encrypted_el
        .get_child("payload", NS_OMEMO_V0)
        .and_then(|el| BASE64.decode(el.text()).ok());

    Some(EncryptedMessage {
        is_v0: true,
        header,
        payload,
    })
}

pub fn parse_encrypted_message(element: &Element) -> Option<EncryptedMessage> {
    if element.name() == "encrypted" && element.ns() == NS_OMEMO_V0 {
        return parse_encrypted_v0(element);
    }
    element
        .get_child("encrypted", NS_OMEMO_V0)
        .and_then(parse_encrypted_v0)
}

pub fn build_encrypted_element_v0(msg: &EncryptedMessage) -> Element {
    let mut header_el = Element::builder("header", NS_OMEMO_V0)
        .attr(nc("sid"), msg.header.sid.to_string())
        .build();

    for group in &msg.header.keys {
        for key in &group.keys {
            let mut key_builder =
                Element::builder("key", NS_OMEMO_V0).attr(nc("rid"), key.rid.to_string());
            if key.kex {
                key_builder = key_builder.attr(nc("prekey"), "true");
            }
            let key_el = key_builder.append(BASE64.encode(&key.data)).build();
            header_el.append_child(key_el);
        }
    }

    if let Some(ref iv) = msg.header.iv {
        let iv_el = Element::builder("iv", NS_OMEMO_V0)
            .append(BASE64.encode(iv))
            .build();
        header_el.append_child(iv_el);
    }

    let mut encrypted_builder = Element::builder("encrypted", NS_OMEMO_V0).append(header_el);

    if let Some(ref payload_bytes) = msg.payload {
        let payload_el = Element::builder("payload", NS_OMEMO_V0)
            .append(BASE64.encode(payload_bytes))
            .build();
        encrypted_builder = encrypted_builder.append(payload_el);
    }

    encrypted_builder.build()
}

pub fn build_message_stanza(to: &str, msg: &EncryptedMessage, id: &str) -> XmppMessage {
    let encrypted_el = build_encrypted_element_v0(msg);

    let to_jid = Jid::new(to).unwrap_or_else(|_| Jid::new("invalid@localhost").unwrap());
    let mut message = XmppMessage::new(Some(to_jid));
    message.type_ = MessageType::Chat;
    message.id = Some(xmpp_parsers::message::Id(id.to_string()));
    message.bodies.insert(
        Lang::default(),
        "I sent you an OMEMO encrypted message but your client doesn’t seem to support that. Find more information on https://conversations.im/omemo".to_string(),
    );
    message.payloads.push(encrypted_el);

    let markable_el = Element::builder("markable", "urn:xmpp:chat-markers:0").build();
    message.payloads.push(markable_el);

    let store_el = Element::builder("store", "urn:xmpp:hints").build();
    message.payloads.push(store_el);

    let eme_el = Element::builder("encryption", "urn:xmpp:eme:0")
        .attr(nc("name"), "OMEMO")
        .attr(nc("namespace"), NS_OMEMO_V0)
        .build();
    message.payloads.push(eme_el);

    message
}
