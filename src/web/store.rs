use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::omemo::{
    CachedBundle, OmemoAccount, OmemoStore, TrustStore, signal_ratchet::Session, trust::TrustStatus,
};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    pickle_key: Option<String>,
    device_id: u32,
    account: Option<String>,
    sessions: HashMap<String, HashMap<u32, String>>,
    device_lists: HashMap<String, Vec<u32>>,
    bundle_cache: HashMap<String, HashMap<u32, CachedBundle>>,
    trust: Vec<(String, u32, TrustStatus)>,
}

#[derive(Clone)]
pub struct BrowserStore {
    state: Arc<Mutex<State>>,
}

impl BrowserStore {
    pub fn new(snapshot: &str) -> Self {
        let state = serde_json::from_str(snapshot).unwrap_or_default();
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn snapshot(&self) -> String {
        serde_json::to_string(&*self.state.lock().unwrap()).unwrap_or_default()
    }

    fn error(message: impl Into<String>) -> Box<dyn std::error::Error> {
        Box::new(std::io::Error::other(message.into()))
    }
}

impl OmemoStore for BrowserStore {
    fn load_or_generate_pickle_key(&self) -> [u8; 32] {
        let mut state = self.state.lock().unwrap();
        if let Some(key) = state
            .pickle_key
            .as_deref()
            .and_then(|key| STANDARD.decode(key).ok())
            && let Ok(key) = <[u8; 32]>::try_from(key)
        {
            return key;
        }
        let mut key = [0; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
        state.pickle_key = Some(STANDARD.encode(key));
        key
    }

    fn load_account(
        &self,
        key: &[u8; 32],
    ) -> Result<Option<OmemoAccount>, Box<dyn std::error::Error>> {
        let state = self.state.lock().unwrap();
        let Some(account) = &state.account else {
            return Ok(None);
        };
        let bytes = STANDARD.decode(account)?;
        let mut account = OmemoAccount::unpickle(&bytes, key)
            .ok_or_else(|| Self::error("invalid OMEMO account"))?;
        account.device_id = state.device_id;
        Ok(Some(account))
    }

    fn save_account(
        &self,
        account: &OmemoAccount,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut state = self.state.lock().unwrap();
        state.device_id = account.device_id;
        state.account = Some(STANDARD.encode(account.pickle(key)));
        Ok(())
    }

    fn load_legacy_sessions(
        &self,
        key: &[u8; 32],
    ) -> Result<HashMap<String, HashMap<u32, Session>>, Box<dyn std::error::Error>> {
        let state = self.state.lock().unwrap();
        let mut result = HashMap::new();
        for (jid, devices) in &state.sessions {
            for (device, session) in devices {
                let bytes = STANDARD.decode(session)?;
                if let Ok(session) = Session::unpickle(&bytes, key) {
                    result
                        .entry(jid.clone())
                        .or_insert_with(HashMap::new)
                        .insert(*device, session);
                }
            }
        }
        Ok(result)
    }

    fn save_legacy_sessions(
        &self,
        sessions: &HashMap<String, HashMap<u32, Session>>,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut state = self.state.lock().unwrap();
        state.sessions = sessions
            .iter()
            .map(|(jid, devices)| {
                (
                    jid.clone(),
                    devices
                        .iter()
                        .map(|(device, session)| (*device, STANDARD.encode(session.pickle(key))))
                        .collect(),
                )
            })
            .collect();
        Ok(())
    }

    fn load_device_lists(&self) -> Result<HashMap<String, Vec<u32>>, Box<dyn std::error::Error>> {
        Ok(self.state.lock().unwrap().device_lists.clone())
    }

    fn save_device_lists(
        &self,
        lists: &HashMap<String, Vec<u32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.state.lock().unwrap().device_lists = lists.clone();
        Ok(())
    }

    fn load_bundle_cache(
        &self,
    ) -> Result<HashMap<String, HashMap<u32, CachedBundle>>, Box<dyn std::error::Error>> {
        Ok(self.state.lock().unwrap().bundle_cache.clone())
    }

    fn save_bundle_cache(
        &self,
        cache: &HashMap<String, HashMap<u32, CachedBundle>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.state.lock().unwrap().bundle_cache = cache.clone();
        Ok(())
    }

    fn load_trust_store(&self) -> Result<TrustStore, Box<dyn std::error::Error>> {
        let state = self.state.lock().unwrap();
        let mut store = TrustStore::new();
        for (jid, device, status) in &state.trust {
            store.set(jid, *device, status.clone());
        }
        Ok(store)
    }

    fn save_trust_store(&self, store: &TrustStore) -> Result<(), Box<dyn std::error::Error>> {
        self.state.lock().unwrap().trust = store.all_entries();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::omemo::OmemoManager;

    #[test]
    fn snapshot_restores_account_and_device_state() {
        let store = BrowserStore::new("");
        let mut manager = OmemoManager::load_or_generate(42, Box::new(store.clone()));
        manager.set_our_jid("alice@example.com");
        manager.update_device_list("bob@example.com", vec![7, 11]);
        manager.save().unwrap();

        let restored = BrowserStore::new(&store.snapshot());
        let manager = OmemoManager::load_or_generate(0, Box::new(restored));
        assert_eq!(manager.our_device_id(), 42);
        let mut devices = manager.device_lists.get("bob@example.com").unwrap().clone();
        devices.sort_unstable();
        assert_eq!(devices, vec![7, 11]);
        assert!(manager.self_bundle().is_some());
    }
}
