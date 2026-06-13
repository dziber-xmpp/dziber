pub mod account;
pub mod bundle;
pub mod crypto;
pub mod device;
pub mod manager;
pub mod message;
pub mod session;
pub mod store;
pub mod trust;

pub use manager::OmemoManager;

pub const NS_OMEMO_V0: &str = "eu.siacs.conversations.axolotl";
pub const NS_OMEMO_V0_DEVICES: &str = "eu.siacs.conversations.axolotl.devicelist";
pub const NS_OMEMO_V0_BUNDLES: &str = "eu.siacs.conversations.axolotl.bundles";

/// Helper to create an `NcName` from a string literal.
pub(crate) fn nc(s: &str) -> tokio_xmpp::minidom::rxml::NcName {
    tokio_xmpp::minidom::rxml::NcName::try_from(s).unwrap()
}
