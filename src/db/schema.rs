diesel::table! {
    messages (id) {
        id -> Text,
        account_jid -> Text,
        contact_jid -> Text,
        from_jid -> Text,
        body -> Text,
        timestamp -> Timestamp,
        status -> Text,
        direction -> Text,
    }
}

diesel::table! {
    omemo_account (id) {
        id -> Integer,
        device_id -> Integer,
        pickle -> Binary,
    }
}

diesel::table! {
    omemo_key (id) {
        id -> Integer,
        key -> Binary,
    }
}

diesel::table! {
    omemo_sessions (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
        pickle -> Binary,
        created_at -> Timestamp,
    }
}

diesel::table! {
    omemo_devices (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
    }
}

diesel::table! {
    omemo_trust (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
        status -> Text,
    }
}

diesel::table! {
    omemo_bundle_cache (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
        identity_key -> Binary,
    }
}
