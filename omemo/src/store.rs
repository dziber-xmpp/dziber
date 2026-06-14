use std::collections::HashMap;
use std::sync::Mutex;

use crate::account::OmemoAccount;
use crate::manager::CachedBundle;
use crate::signal_ratchet;
use crate::trust::TrustStore;

/// Persistence interface for OMEMO state.
///
/// The `dziber` application provides an implementation backed by its SQLite
/// database; test code can use [`MemoryStore`].
pub trait OmemoStore: Send + Sync {
    /// Load or generate the 32-byte symmetric key used to encrypt pickles.
    fn load_or_generate_pickle_key(&self) -> [u8; 32];

    /// Load the pickled account, if one exists.
    fn load_account(
        &self,
        key: &[u8; 32],
    ) -> Result<Option<OmemoAccount>, Box<dyn std::error::Error>>;

    /// Persist the pickled account.
    fn save_account(
        &self,
        account: &OmemoAccount,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Load legacy `signal_ratchet` sessions.
    fn load_legacy_sessions(
        &self,
        key: &[u8; 32],
    ) -> Result<HashMap<String, HashMap<u32, signal_ratchet::Session>>, Box<dyn std::error::Error>>;

    /// Persist legacy `signal_ratchet` sessions.
    fn save_legacy_sessions(
        &self,
        sessions: &HashMap<String, HashMap<u32, signal_ratchet::Session>>,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Load cached device lists.
    fn load_device_lists(
        &self,
    ) -> Result<HashMap<String, Vec<u32>>, Box<dyn std::error::Error>>;

    /// Persist cached device lists.
    fn save_device_lists(
        &self,
        lists: &HashMap<String, Vec<u32>>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Load cached prekey bundles.
    fn load_bundle_cache(
        &self,
    ) -> Result<HashMap<String, HashMap<u32, CachedBundle>>, Box<dyn std::error::Error>>;

    /// Persist cached prekey bundles.
    fn save_bundle_cache(
        &self,
        cache: &HashMap<String, HashMap<u32, CachedBundle>>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Load the trust store.
    fn load_trust_store(&self) -> Result<TrustStore, Box<dyn std::error::Error>>;

    /// Persist the trust store.
    fn save_trust_store(
        &self,
        store: &TrustStore,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

/// In-memory [`OmemoStore`] implementation for tests.
#[derive(Default)]
pub struct MemoryStore {
    pickle_key: Mutex<Option<[u8; 32]>>,
    account: Mutex<Option<(Vec<u8>, [u8; 32])>>,
    legacy_sessions: Mutex<HashMap<String, HashMap<u32, signal_ratchet::Session>>>,
    device_lists: Mutex<HashMap<String, Vec<u32>>>,
    bundle_cache: Mutex<HashMap<String, HashMap<u32, CachedBundle>>>,
    trust_store: Mutex<TrustStore>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl OmemoStore for MemoryStore {
    fn load_or_generate_pickle_key(&self) -> [u8; 32] {
        let mut key = self.pickle_key.lock().unwrap();
        if let Some(k) = *key {
            return k;
        }
        let k = generate_key();
        *key = Some(k);
        k
    }

    fn load_account(
        &self,
        key: &[u8; 32],
    ) -> Result<Option<OmemoAccount>, Box<dyn std::error::Error>> {
        let account = self.account.lock().unwrap();
        Ok(match account.as_ref() {
            Some((pickle, stored_key)) if key == stored_key => {
                OmemoAccount::unpickle(pickle, key)
            }
            _ => None,
        })
    }

    fn save_account(
        &self,
        account: &OmemoAccount,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        *self.account.lock().unwrap() = Some((account.pickle(key), *key));
        Ok(())
    }

    fn load_legacy_sessions(
        &self,
        _key: &[u8; 32],
    ) -> Result<HashMap<String, HashMap<u32, signal_ratchet::Session>>, Box<dyn std::error::Error>>
    {
        Ok(self.legacy_sessions.lock().unwrap().clone())
    }

    fn save_legacy_sessions(
        &self,
        sessions: &HashMap<String, HashMap<u32, signal_ratchet::Session>>,
        _key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        *self.legacy_sessions.lock().unwrap() = sessions.clone();
        Ok(())
    }

    fn load_device_lists(
        &self,
    ) -> Result<HashMap<String, Vec<u32>>, Box<dyn std::error::Error>> {
        Ok(self.device_lists.lock().unwrap().clone())
    }

    fn save_device_lists(
        &self,
        lists: &HashMap<String, Vec<u32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        *self.device_lists.lock().unwrap() = lists.clone();
        Ok(())
    }

    fn load_bundle_cache(
        &self,
    ) -> Result<HashMap<String, HashMap<u32, CachedBundle>>, Box<dyn std::error::Error>> {
        Ok(self.bundle_cache.lock().unwrap().clone())
    }

    fn save_bundle_cache(
        &self,
        cache: &HashMap<String, HashMap<u32, CachedBundle>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        *self.bundle_cache.lock().unwrap() = cache.clone();
        Ok(())
    }

    fn load_trust_store(&self) -> Result<TrustStore, Box<dyn std::error::Error>> {
        Ok(self.trust_store.lock().unwrap().clone())
    }

    fn save_trust_store(
        &self,
        store: &TrustStore,
    ) -> Result<(), Box<dyn std::error::Error>> {
        *self.trust_store.lock().unwrap() = store.clone();
        Ok(())
    }
}

fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
    key
}
