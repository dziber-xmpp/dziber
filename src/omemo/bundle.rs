use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio_xmpp::minidom::Element;

use super::{NS_OMEMO_V0, nc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub device_id: u32,
    pub spk_id: u32,
    pub spk: Vec<u8>,
    pub spks: Vec<u8>,
    pub ik: Vec<u8>,
    pub prekeys: Vec<(u32, Vec<u8>)>,
}

fn strip_signal_prefix(data: Vec<u8>) -> Vec<u8> {
    if data.len() == 33 && data.first() == Some(&0x05) {
        data.into_iter().skip(1).collect()
    } else {
        data
    }
}

pub fn parse_bundle(element: &Element) -> Option<Bundle> {
    if element.name() != "bundle" || element.ns() != NS_OMEMO_V0 {
        return None;
    }

    let spk_el = element.get_child("signedPreKeyPublic", NS_OMEMO_V0)?;
    let spk_id: u32 = spk_el.attr("signedPreKeyId")?.parse().ok()?;
    let spk = strip_signal_prefix(BASE64.decode(spk_el.text()).ok()?);

    let spks_el = element.get_child("signedPreKeySignature", NS_OMEMO_V0)?;
    let spks = BASE64.decode(spks_el.text()).ok()?;

    let ik_el = element.get_child("identityKey", NS_OMEMO_V0)?;
    let ik = strip_signal_prefix(BASE64.decode(ik_el.text()).ok()?);

    let prekeys_el = element.get_child("prekeys", NS_OMEMO_V0)?;
    let mut prekeys = Vec::new();
    for pk_el in prekeys_el.children() {
        if pk_el.name() != "preKeyPublic" || pk_el.ns() != NS_OMEMO_V0 {
            continue;
        }
        let id: u32 = pk_el.attr("preKeyId")?.parse().ok()?;
        let data = strip_signal_prefix(BASE64.decode(pk_el.text()).ok()?);
        prekeys.push((id, data));
    }

    Some(Bundle {
        device_id: 0,
        spk_id,
        spk,
        spks,
        ik,
        prekeys,
    })
}

pub fn add_signal_prefix(data: &[u8]) -> Vec<u8> {
    let mut result = vec![0x05];
    result.extend_from_slice(data);
    result
}

pub fn build_bundle_element_v0(bundle: &Bundle) -> Element {
    let spk_el = Element::builder("signedPreKeyPublic", NS_OMEMO_V0)
        .attr(nc("signedPreKeyId"), bundle.spk_id.to_string())
        .append(BASE64.encode(add_signal_prefix(&bundle.spk)))
        .build();

    let spks_el = Element::builder("signedPreKeySignature", NS_OMEMO_V0)
        .append(BASE64.encode(&bundle.spks))
        .build();

    let ik_el = Element::builder("identityKey", NS_OMEMO_V0)
        .append(BASE64.encode(add_signal_prefix(&bundle.ik)))
        .build();

    let mut prekeys_el = Element::builder("prekeys", NS_OMEMO_V0).build();
    for (id, data) in &bundle.prekeys {
        let pk_el = Element::builder("preKeyPublic", NS_OMEMO_V0)
            .attr(nc("preKeyId"), id.to_string())
            .append(BASE64.encode(add_signal_prefix(data)))
            .build();
        prekeys_el.append_child(pk_el);
    }

    Element::builder("bundle", NS_OMEMO_V0)
        .append(spk_el)
        .append(spks_el)
        .append(ik_el)
        .append(prekeys_el)
        .build()
}
