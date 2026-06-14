use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use minidom::Element;

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

/// Strip a leading `0x05` libsignal public-key prefix if present.
pub fn strip_signal_prefix(data: Vec<u8>) -> Vec<u8> {
    if data.len() == 33 && data.first() == Some(&0x05) {
        data.into_iter().skip(1).collect()
    } else {
        data
    }
}

/// Prefix a raw Curve25519 public key with the libsignal `0x05` marker.
pub fn add_signal_prefix(data: &[u8]) -> Vec<u8> {
    let mut result = vec![0x05];
    result.extend_from_slice(data);
    result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::OmemoAccount;
    use vodozemac::{Curve25519PublicKey, Curve25519SecretKey};

    fn sample_bundle() -> (OmemoAccount, Bundle) {
        let account = OmemoAccount::generate(11);
        let device_id = account.device_id;

        let ik = account.inner.curve25519_key().to_bytes().to_vec();

        let (spk_id, spk_secret) = account.fallback_secret_key_bytes().unwrap();
        let spk = Curve25519PublicKey::from(&Curve25519SecretKey::from_slice(&spk_secret))
            .to_bytes()
            .to_vec();
        let mut spk_for_sig = vec![0x05];
        spk_for_sig.extend_from_slice(&spk);
        let spks = account.xeddsa_sign(&spk_for_sig);

        let prekeys = account
            .all_stored_one_time_keys()
            .into_iter()
            .take(5)
            .map(|(id, pk)| (id, pk.to_bytes().to_vec()))
            .collect();

        let bundle = Bundle {
            device_id,
            spk_id,
            spk,
            spks,
            ik,
            prekeys,
        };
        (account, bundle)
    }

    #[test]
    fn signal_prefix_roundtrip() {
        let raw: [u8; 32] = rand::random();
        let prefixed = add_signal_prefix(&raw);
        assert_eq!(prefixed.len(), 33);
        assert_eq!(prefixed[0], 0x05);
        assert_eq!(&prefixed[1..], &raw[..]);
        let stripped = strip_signal_prefix(prefixed);
        assert_eq!(stripped, raw.to_vec());
    }

    #[test]
    fn strip_signal_prefix_non_33_unchanged() {
        let short = vec![1, 2, 3];
        assert_eq!(strip_signal_prefix(short.clone()), short);

        let wrong_prefix = vec![0u8; 33];
        assert_eq!(strip_signal_prefix(wrong_prefix.clone()), wrong_prefix);
    }

    #[test]
    fn build_and_parse_bundle_roundtrip() {
        let (_, bundle) = sample_bundle();
        let element = build_bundle_element_v0(&bundle);
        assert_eq!(element.name(), "bundle");
        assert_eq!(element.ns(), NS_OMEMO_V0);

        let mut parsed = parse_bundle(&element).unwrap();
        // parse_bundle clears device_id.
        parsed.device_id = bundle.device_id;
        assert_eq!(parsed, bundle);
    }

    #[test]
    fn parse_bundle_wrong_element_returns_none() {
        let element = Element::builder("bundle", "wrong:ns").build();
        assert!(parse_bundle(&element).is_none());
    }
}
