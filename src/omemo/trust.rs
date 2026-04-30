use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    Trusted,
    Untrusted,
    Undecided,
}

/// Auto-accepts all keys
#[derive(Debug, Default)]
pub struct TrustStore {
    keys: HashMap<String, TrustStatus>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self {
            keys: HashMap::new(),
        }
    }

    pub fn set(&mut self, jid: &str, device_id: u32, status: TrustStatus) {
        let key = format!("{}:{}", jid, device_id);
        self.keys.insert(key, status);
    }

    pub fn accept_all(&mut self, jid: &str, device_ids: &[u32]) {
        for id in device_ids {
            self.set(jid, *id, TrustStatus::Trusted);
        }
    }

    pub fn all_entries(&self) -> Vec<(String, u32, TrustStatus)> {
        self.keys
            .iter()
            .filter_map(|(key, status)| {
                let mut parts = key.rsplitn(2, ':');
                let device_id = parts.next()?.parse().ok()?;
                let jid = parts.next()?.to_string();
                Some((jid, device_id, status.clone()))
            })
            .collect()
    }
}
