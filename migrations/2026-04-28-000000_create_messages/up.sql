CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    account_jid TEXT NOT NULL,
    contact_jid TEXT NOT NULL,
    from_jid TEXT NOT NULL,
    body TEXT NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    status TEXT NOT NULL,
    direction TEXT NOT NULL
);

CREATE INDEX idx_messages_account_contact ON messages (account_jid, contact_jid);
CREATE INDEX idx_messages_timestamp ON messages (timestamp);
