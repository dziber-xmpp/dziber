CREATE TABLE omemo_account (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    device_id INTEGER NOT NULL,
    pickle BLOB NOT NULL
);

CREATE TABLE omemo_sessions (
    jid TEXT NOT NULL,
    device_id INTEGER NOT NULL,
    pickle BLOB NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (jid, device_id)
);

CREATE TABLE omemo_devices (
    jid TEXT NOT NULL,
    device_id INTEGER NOT NULL,
    PRIMARY KEY (jid, device_id)
);

CREATE TABLE omemo_trust (
    jid TEXT NOT NULL,
    device_id INTEGER NOT NULL,
    status TEXT NOT NULL,
    PRIMARY KEY (jid, device_id)
);

CREATE TABLE omemo_bundle_cache (
    jid TEXT NOT NULL,
    device_id INTEGER NOT NULL,
    identity_key BLOB NOT NULL,
    PRIMARY KEY (jid, device_id)
);
