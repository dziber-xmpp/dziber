use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrustStatus {
    Trusted,
    Untrusted,
    Undecided,
}

/// Auto-accepts all keys
#[derive(Debug, Default, Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_store_is_empty() {
        let store = TrustStore::new();
        assert!(store.all_entries().is_empty());
    }

    #[test]
    fn set_and_all_entries() {
        let mut store = TrustStore::new();
        store.set("alice@example.com", 1, TrustStatus::Trusted);
        store.set("alice@example.com", 2, TrustStatus::Untrusted);
        store.set("bob@example.com", 1, TrustStatus::Undecided);

        let mut entries = store.all_entries();
        entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        assert_eq!(
            entries,
            vec![
                ("alice@example.com".to_string(), 1, TrustStatus::Trusted),
                ("alice@example.com".to_string(), 2, TrustStatus::Untrusted),
                ("bob@example.com".to_string(), 1, TrustStatus::Undecided),
            ]
        );
    }

    #[test]
    fn accept_all_marks_trusted() {
        let mut store = TrustStore::new();
        store.accept_all("alice@example.com", &[10, 20, 30]);
        let entries = store.all_entries();
        assert_eq!(entries.len(), 3);
        for (_, _, status) in &entries {
            assert_eq!(*status, TrustStatus::Trusted);
        }
    }

    #[test]
    fn set_overwrites_previous_status() {
        let mut store = TrustStore::new();
        store.set("alice@example.com", 1, TrustStatus::Trusted);
        store.set("alice@example.com", 1, TrustStatus::Untrusted);
        let entries = store.all_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].2, TrustStatus::Untrusted);
    }
}
