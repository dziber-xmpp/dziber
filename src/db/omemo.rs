use std::collections::HashMap;

use diesel::prelude::*;

use crate::db::establish_connection;
use crate::db::models::{
    DbOmemoAccount, DbOmemoBundleCache, DbOmemoDevice, DbOmemoKey, DbOmemoSession, DbOmemoTrust,
};
use crate::db::schema::{
    omemo_account, omemo_bundle_cache, omemo_devices, omemo_key, omemo_sessions, omemo_trust,
};
use crate::omemo::account::OmemoAccount;
use crate::omemo::manager::CachedBundle;
use crate::omemo::session;
use crate::omemo::trust::{TrustStatus, TrustStore};

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

pub fn save_omemo_sessions(
    sessions: &HashMap<String, HashMap<u32, vodozemac::olm::Session>>,
    key: &[u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    diesel::delete(omemo_sessions::table).execute(&mut conn)?;
    for (jid_val, devices) in sessions {
        for (dev_id, sess) in devices {
            let db_sess = DbOmemoSession {
                jid: jid_val.clone(),
                device_id: *dev_id as i32,
                pickle: session::pickle_session(sess, key),
                created_at: chrono::Utc::now().naive_utc(),
            };
            diesel::insert_into(omemo_sessions::table)
                .values(&db_sess)
                .execute(&mut conn)?;
        }
    }
    Ok(())
}

pub fn load_omemo_sessions(
    key: &[u8; 32],
) -> Result<HashMap<String, HashMap<u32, vodozemac::olm::Session>>, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let results: Vec<DbOmemoSession> = omemo_sessions::table.load(&mut conn)?;
    let mut sessions: HashMap<String, HashMap<u32, vodozemac::olm::Session>> = HashMap::new();
    for row in results {
        if let Some(session) = session::unpickle_session(&row.pickle, key) {
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
                identity_key: cache.identity_key.to_bytes().to_vec(),
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
            let identity_key = vodozemac::Curve25519PublicKey::from(ik_bytes);
            cache
                .entry(row.jid)
                .or_default()
                .insert(row.device_id as u32, CachedBundle { identity_key });
        }
    }
    Ok(cache)
}
