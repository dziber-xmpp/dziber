/// Load or generate the 32-byte pickle encryption key.
pub fn load_or_generate_key() -> [u8; 32] {
    if let Ok(Some(key)) = crate::db::omemo::load_omemo_pickle_key() {
        return key;
    }

    let key = generate_key();
    let _ = crate::db::omemo::save_omemo_pickle_key(&key);
    key
}

fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
    key
}
