pub mod account;
pub mod bundle;
pub mod crypto;
pub mod device;
pub mod manager;
pub mod message;
pub mod signal_ratchet;
pub mod store;
pub mod trust;

pub use account::OmemoAccount;
pub use bundle::{Bundle, build_bundle_element_v0, parse_bundle};
pub use device::{Device, build_device_list_element_v0, parse_device_list};
pub use manager::{CachedBundle, OmemoManager};
pub use message::{EncryptedMessage, build_message_stanza, parse_encrypted_message};
pub use store::{MemoryStore, OmemoStore};
pub use trust::{TrustStatus, TrustStore};

pub const NS_OMEMO_V0: &str = "eu.siacs.conversations.axolotl";
pub const NS_OMEMO_V0_DEVICES: &str = "eu.siacs.conversations.axolotl.devicelist";
pub const NS_OMEMO_V0_BUNDLES: &str = "eu.siacs.conversations.axolotl.bundles";

/// Helper to create an `NcName` from a string literal.
pub(crate) fn nc(s: &str) -> minidom::rxml::NcName {
    minidom::rxml::NcName::try_from(s).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_constants() {
        assert_eq!(NS_OMEMO_V0, "eu.siacs.conversations.axolotl");
        assert_eq!(NS_OMEMO_V0_DEVICES, "eu.siacs.conversations.axolotl.devicelist");
        assert_eq!(NS_OMEMO_V0_BUNDLES, "eu.siacs.conversations.axolotl.bundles");
    }

    #[test]
    fn nc_helper_creates_valid_name() {
        let name = nc("signedPreKeyId");
        assert_eq!(AsRef::<str>::as_ref(&name), "signedPreKeyId");
    }
}
