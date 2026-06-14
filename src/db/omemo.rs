use std::collections::HashMap;

use diesel::prelude::*;

use crate::db::establish_connection;
use crate::db::models::{
    DbOmemoAccount, DbOmemoBundleCache, DbOmemoDevice, DbOmemoKey, DbOmemoSession, DbOmemoTrust,
};
use crate::db::schema::{
    omemo_account, omemo_bundle_cache, omemo_devices, omemo_key, omemo_sessions, omemo_trust,
};
use dziber_omemo::OmemoStore;
use dziber_omemo::account::OmemoAccount;
use dziber_omemo::manager::CachedBundle;
use dziber_omemo::signal_ratchet;
use dziber_omemo::trust::{TrustStatus, TrustStore};

pub fn save_omemo_account(
    account: &OmemoAccount,
    key: &[u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let db_acc = DbOmemoAccount {
        id: 1,
        device_id: account.device_id as i32,
        pickle: account.pickle(key),
    };
    diesel::delete(omemo_account::table.filter(omemo_account::id.eq(1))).execute(&mut conn)?;
    diesel::insert_into(omemo_account::table)
        .values(&db_acc)
        .execute(&mut conn)?;
    Ok(())
}

pub fn save_omemo_pickle_key(key: &[u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let db_key = DbOmemoKey {
        id: 1,
        key: key.to_vec(),
    };
    diesel::delete(omemo_key::table.filter(omemo_key::id.eq(1))).execute(&mut conn)?;
    diesel::insert_into(omemo_key::table)
        .values(&db_key)
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_omemo_pickle_key() -> Result<Option<[u8; 32]>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let row: Option<DbOmemoKey> = omemo_key::table
        .filter(omemo_key::id.eq(1))
        .first(&mut conn)
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.key.len() != 32 {
        return Ok(None);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&row.key);
    Ok(Some(key))
}

pub fn load_omemo_account(
    key: &[u8; 32],
) -> Result<Option<OmemoAccount>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let result: Option<DbOmemoAccount> = omemo_account::table
        .filter(omemo_account::id.eq(1))
        .first(&mut conn)
        .optional()?;
    match result {
        Some(row) => {
            let mut account = OmemoAccount::unpickle(&row.pickle, key)
                .ok_or("Failed to unpickle OMEMO account")?;
            account.device_id = row.device_id as u32;
            Ok(Some(account))
        }
        None => Ok(None),
    }
}

pub fn save_omemo_legacy_sessions(
    sessions: &HashMap<String, HashMap<u32, signal_ratchet::Session>>,
    key: &[u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    diesel::delete(omemo_sessions::table).execute(&mut conn)?;
    for (jid_val, devices) in sessions {
        for (dev_id, sess) in devices {
            let db_sess = DbOmemoSession {
                jid: jid_val.clone(),
                device_id: *dev_id as i32,
                pickle: sess.pickle(key),
                created_at: chrono::Utc::now().naive_utc(),
            };
            diesel::insert_into(omemo_sessions::table)
                .values(&db_sess)
                .execute(&mut conn)?;
        }
    }
    Ok(())
}

pub fn load_omemo_legacy_sessions(
    key: &[u8; 32],
) -> Result<HashMap<String, HashMap<u32, signal_ratchet::Session>>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let results: Vec<DbOmemoSession> = omemo_sessions::table.load(&mut conn)?;
    let mut sessions: HashMap<String, HashMap<u32, signal_ratchet::Session>> = HashMap::new();
    for row in results {
        if let Ok(session) = signal_ratchet::Session::unpickle(&row.pickle, key) {
            sessions
                .entry(row.jid)
                .or_default()
                .insert(row.device_id as u32, session);
        }
    }
    Ok(sessions)
}

pub fn save_omemo_device_lists(
    device_lists: &HashMap<String, Vec<u32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    diesel::delete(omemo_devices::table).execute(&mut conn)?;
    for (jid_val, devices) in device_lists {
        for dev_id in devices {
            let db_dev = DbOmemoDevice {
                jid: jid_val.clone(),
                device_id: *dev_id as i32,
            };
            diesel::insert_into(omemo_devices::table)
                .values(&db_dev)
                .execute(&mut conn)?;
        }
    }
    Ok(())
}

pub fn load_omemo_device_lists() -> Result<HashMap<String, Vec<u32>>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let results: Vec<DbOmemoDevice> = omemo_devices::table.load(&mut conn)?;
    let mut lists: HashMap<String, Vec<u32>> = HashMap::new();
    for row in results {
        lists.entry(row.jid).or_default().push(row.device_id as u32);
    }
    Ok(lists)
}

pub fn save_omemo_trust_store(trust_store: &TrustStore) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    diesel::delete(omemo_trust::table).execute(&mut conn)?;
    for (jid_val, dev_id, status) in trust_store.all_entries() {
        let status_str = match status {
            TrustStatus::Trusted => "trusted",
            TrustStatus::Untrusted => "untrusted",
            TrustStatus::Undecided => "undecided",
        };
        let db_trust = DbOmemoTrust {
            jid: jid_val,
            device_id: dev_id as i32,
            status: status_str.to_string(),
        };
        diesel::insert_into(omemo_trust::table)
            .values(&db_trust)
            .execute(&mut conn)?;
    }
    Ok(())
}

pub fn load_omemo_trust_store() -> Result<TrustStore, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let results: Vec<DbOmemoTrust> = omemo_trust::table.load(&mut conn)?;
    let mut store = TrustStore::new();
    for row in results {
        let status = match row.status.as_str() {
            "trusted" => TrustStatus::Trusted,
            "untrusted" => TrustStatus::Untrusted,
            _ => TrustStatus::Undecided,
        };
        store.set(&row.jid, row.device_id as u32, status);
    }
    Ok(store)
}

pub fn save_omemo_bundle_cache(
    bundle_cache: &HashMap<String, HashMap<u32, CachedBundle>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    diesel::delete(omemo_bundle_cache::table).execute(&mut conn)?;
    for (jid_val, devices) in bundle_cache {
        for (dev_id, cache) in devices {
            let db_cache = DbOmemoBundleCache {
                jid: jid_val.clone(),
                device_id: *dev_id as i32,
                identity_key: cache.identity_key.to_vec(),
            };
            diesel::insert_into(omemo_bundle_cache::table)
                .values(&db_cache)
                .execute(&mut conn)?;
        }
    }
    Ok(())
}

pub fn load_omemo_bundle_cache()
-> Result<HashMap<String, HashMap<u32, CachedBundle>>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let results: Vec<DbOmemoBundleCache> = omemo_bundle_cache::table.load(&mut conn)?;
    let mut cache: HashMap<String, HashMap<u32, CachedBundle>> = HashMap::new();
    for row in results {
        if let Ok(ik_bytes) = <&[u8] as TryInto<[u8; 32]>>::try_into(row.identity_key.as_slice()) {
            cache
                .entry(row.jid)
                .or_default()
                .insert(row.device_id as u32, CachedBundle { identity_key: ik_bytes });
        }
    }
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use diesel::prelude::*;
    use std::collections::HashMap;

    use crate::db::test_helpers::{connection, with_test_db};
    use dziber_omemo::account::OmemoAccount;
    use dziber_omemo::signal_ratchet;
    use dziber_omemo::trust::{TrustStatus, TrustStore};

    fn sample_key() -> [u8; 32] {
        [0xAB; 32]
    }

    #[test]
    fn save_and_load_omemo_pickle_key() {
        let _guard = with_test_db();
        let key = sample_key();
        super::save_omemo_pickle_key(&key).unwrap();
        let loaded = super::load_omemo_pickle_key().unwrap();
        assert_eq!(loaded, Some(key));
    }

    #[test]
    fn load_omemo_pickle_key_wrong_length_returns_none() {
        let _guard = with_test_db();
        let mut conn = connection();
        diesel::insert_into(crate::db::schema::omemo_key::table)
            .values(&crate::db::models::DbOmemoKey {
                id: 1,
                key: vec![1, 2, 3],
            })
            .execute(&mut conn)
            .unwrap();

        let loaded = super::load_omemo_pickle_key().unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn save_and_load_omemo_device_lists() {
        let _guard = with_test_db();
        let mut lists = HashMap::new();
        lists.insert("alice@example.com".to_string(), vec![1, 2, 3]);
        lists.insert("bob@example.com".to_string(), vec![42]);

        super::save_omemo_device_lists(&lists).unwrap();
        let loaded = super::load_omemo_device_lists().unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["alice@example.com"], vec![1, 2, 3]);
        assert_eq!(loaded["bob@example.com"], vec![42]);
    }

    #[test]
    fn save_omemo_device_lists_replaces_previous() {
        let _guard = with_test_db();
        let mut first = HashMap::new();
        first.insert("alice@example.com".to_string(), vec![1]);
        super::save_omemo_device_lists(&first).unwrap();

        let mut second = HashMap::new();
        second.insert("bob@example.com".to_string(), vec![7, 8]);
        super::save_omemo_device_lists(&second).unwrap();

        let loaded = super::load_omemo_device_lists().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded.contains_key("alice@example.com"));
        assert_eq!(loaded["bob@example.com"], vec![7, 8]);
    }

    #[test]
    fn save_and_load_omemo_trust_store() {
        let _guard = with_test_db();
        let mut store = TrustStore::new();
        store.set("alice@example.com", 1, TrustStatus::Trusted);
        store.set("alice@example.com", 2, TrustStatus::Untrusted);
        store.set("bob@example.com", 1, TrustStatus::Undecided);

        super::save_omemo_trust_store(&store).unwrap();
        let loaded = super::load_omemo_trust_store().unwrap();

        let mut entries = loaded.all_entries();
        entries.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));

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
    fn save_and_load_omemo_account() {
        let _guard = with_test_db();
        let key = sample_key();
        let original = OmemoAccount::generate(12345);
        super::save_omemo_account(&original, &key).unwrap();

        let loaded = super::load_omemo_account(&key).unwrap().expect("account missing");
        assert_eq!(loaded.device_id, 12345);
        assert_eq!(
            loaded.inner.curve25519_key().to_bytes(),
            original.inner.curve25519_key().to_bytes()
        );
    }

    #[test]
    fn save_and_load_omemo_legacy_sessions() {
        use rand::thread_rng;

        let _guard = with_test_db();
        let key = sample_key();
        let mut rng = thread_rng();
        let alice_identity = signal_ratchet::IdentityKeyPair::generate(&mut rng);
        let bob_identity = signal_ratchet::IdentityKeyPair::generate(&mut rng);
        let bob_signed_pre_key =
            signal_ratchet::SignedPreKey::generate(1, &bob_identity, &mut rng);
        let bob_one_time_pre_key = signal_ratchet::PreKey::generate(7, &mut rng);
        let bundle = signal_ratchet::PreKeyBundle {
            registration_id: 1234,
            device_id: 2,
            signed_pre_key_id: bob_signed_pre_key.id,
            signed_pre_key_public: *bob_signed_pre_key.key_pair.public_key_bytes(),
            signed_pre_key_signature: bob_signed_pre_key.signature,
            identity_key: *bob_identity.public_key_bytes(),
            pre_key_id: bob_one_time_pre_key.id,
            pre_key_public: *bob_one_time_pre_key.key_pair.public_key_bytes(),
        };
        let session =
            signal_ratchet::Session::new_alice(&alice_identity, &bundle, &mut rng).unwrap();

        let mut sessions: HashMap<String, HashMap<u32, signal_ratchet::Session>> =
            HashMap::new();
        sessions
            .entry("bob@example.com".to_string())
            .or_default()
            .insert(2, session);
        super::save_omemo_legacy_sessions(&sessions, &key).unwrap();

        let loaded = super::load_omemo_legacy_sessions(&key).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded["bob@example.com"].contains_key(&2));
    }
}

/// SQLite-backed [`OmemoStore`] implementation used by the main dziber app.
#[derive(Debug, Clone, Copy, Default)]
pub struct DziberOmemoStore;

impl OmemoStore for DziberOmemoStore {
    fn load_or_generate_pickle_key(&self) -> [u8; 32] {
        if let Ok(Some(key)) = load_omemo_pickle_key() {
            return key;
        }
        let key = generate_key();
        let _ = save_omemo_pickle_key(&key);
        key
    }

    fn load_account(
        &self,
        key: &[u8; 32],
    ) -> Result<Option<OmemoAccount>, Box<dyn std::error::Error>> {
        load_omemo_account(key)
    }

    fn save_account(
        &self,
        account: &OmemoAccount,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_omemo_account(account, key)
    }

    fn load_legacy_sessions(
        &self,
        key: &[u8; 32],
    ) -> Result<HashMap<String, HashMap<u32, signal_ratchet::Session>>, Box<dyn std::error::Error>>
    {
        load_omemo_legacy_sessions(key)
    }

    fn save_legacy_sessions(
        &self,
        sessions: &HashMap<String, HashMap<u32, signal_ratchet::Session>>,
        key: &[u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_omemo_legacy_sessions(sessions, key)
    }

    fn load_device_lists(
        &self,
    ) -> Result<HashMap<String, Vec<u32>>, Box<dyn std::error::Error>> {
        load_omemo_device_lists()
    }

    fn save_device_lists(
        &self,
        lists: &HashMap<String, Vec<u32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_omemo_device_lists(lists)
    }

    fn load_bundle_cache(
        &self,
    ) -> Result<HashMap<String, HashMap<u32, CachedBundle>>, Box<dyn std::error::Error>> {
        load_omemo_bundle_cache()
    }

    fn save_bundle_cache(
        &self,
        cache: &HashMap<String, HashMap<u32, CachedBundle>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_omemo_bundle_cache(cache)
    }

    fn load_trust_store(&self) -> Result<TrustStore, Box<dyn std::error::Error>> {
        load_omemo_trust_store()
    }

    fn save_trust_store(
        &self,
        store: &TrustStore,
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_omemo_trust_store(store)
    }
}

fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
    key
}
