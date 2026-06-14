use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use minidom::Element;

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

/// Build a `<message/>` stanza element for the legacy OMEMO payload.
///
/// The returned element is in the `jabber:client` namespace and can be
/// serialized directly or converted to a higher-level XMPP message type by
/// the caller.
pub fn build_message_stanza(
    to: &str,
    msg: &EncryptedMessage,
    id: &str,
    fallback_body: Option<&str>,
) -> Element {
    let encrypted_el = build_encrypted_element_v0(msg);

    let fallback = fallback_body.map(ToOwned::to_owned).unwrap_or_else(|| {
        "I sent you an OMEMO encrypted message but your client doesn’t seem to support that. Find more information on https://conversations.im/omemo".to_string()
    });

    let message = Element::builder("message", "jabber:client")
        .attr(nc("to"), to)
        .attr(nc("type"), "chat")
        .attr(nc("id"), id)
        .append(Element::builder("body", "jabber:client").append(fallback).build())
        .append(encrypted_el)
        .append(Element::builder("request", "urn:xmpp:receipts").build())
        .append(Element::builder("markable", "urn:xmpp:chat-markers:0").build())
        .append(Element::builder("store", "urn:xmpp:hints").build())
        .append(
            Element::builder("encryption", "urn:xmpp:eme:0")
                .attr(nc("name"), "OMEMO")
                .attr(nc("namespace"), NS_OMEMO_V0)
                .build(),
        )
        .build();

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_encrypted_message() -> EncryptedMessage {
        EncryptedMessage {
            is_v0: true,
            header: MessageHeader {
                sid: 42,
                keys: vec![KeysGroup {
                    jid: String::new(),
                    keys: vec![
                        MessageKey {
                            rid: 1,
                            kex: false,
                            data: vec![1, 2, 3],
                        },
                        MessageKey {
                            rid: 2,
                            kex: true,
                            data: vec![4, 5, 6],
                        },
                    ],
                }],
                iv: Some(vec![7, 8, 9]),
            },
            payload: Some(vec![10, 11, 12]),
        }
    }

    #[test]
    fn build_and_parse_encrypted_message_roundtrip() {
        let msg = sample_encrypted_message();
        let element = build_encrypted_element_v0(&msg);
        assert_eq!(element.name(), "encrypted");
        assert_eq!(element.ns(), NS_OMEMO_V0);

        let parsed = parse_encrypted_message(&element).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn parse_encrypted_message_wrapped_in_message() {
        let msg = sample_encrypted_message();
        let encrypted = build_encrypted_element_v0(&msg);
        let wrapper = Element::builder("message", "jabber:client")
            .append(encrypted)
            .build();
        let parsed = parse_encrypted_message(&wrapper).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn parse_encrypted_message_wrong_ns_returns_none() {
        let element = Element::builder("encrypted", "wrong:ns").build();
        assert!(parse_encrypted_message(&element).is_none());
    }

    #[test]
    fn build_message_stanza_structure() {
        let msg = sample_encrypted_message();
        let stanza = build_message_stanza("peer@example.com", &msg, "msg-1", None);
        assert_eq!(stanza.name(), "message");
        assert_eq!(stanza.ns(), "jabber:client");
        assert_eq!(stanza.attr("to"), Some("peer@example.com"));
        assert_eq!(stanza.attr("type"), Some("chat"));
        assert_eq!(stanza.attr("id"), Some("msg-1"));
        assert!(stanza.get_child("body", "jabber:client").is_some());
        assert!(stanza.get_child("encrypted", NS_OMEMO_V0).is_some());
    }
}
