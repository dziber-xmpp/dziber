CREATE TABLE messages (
    id TEXT NOT NULL PRIMARY KEY,
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

CREATE TABLE omemo_account (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    device_id INTEGER NOT NULL,
    pickle BLOB NOT NULL
);

CREATE TABLE omemo_sessions (
    jid TEXT NOT NULL,
    device_id INTEGER NOT NULL,
    pickle BLOB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
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

CREATE TABLE omemo_key (
    id INTEGER PRIMARY KEY NOT NULL,
    key BLOB NOT NULL
);

CREATE TABLE mail_accounts (
    id TEXT NOT NULL PRIMARY KEY,
    server_url TEXT NOT NULL,
    username TEXT NOT NULL,
    password TEXT NOT NULL,
    auth_mode TEXT NOT NULL,
    admin_user TEXT,
    admin_pass TEXT,
    last_sync TIMESTAMP,
    mail_protocol TEXT NOT NULL,
    imap_server TEXT,
    imap_port INTEGER,
    smtp_server TEXT,
    smtp_port INTEGER,
    security TEXT,
    sieve_server TEXT,
    sieve_port INTEGER,
    sieve_security TEXT
);

CREATE TABLE contacts_accounts (
    id TEXT NOT NULL PRIMARY KEY,
    server_url TEXT NOT NULL,
    username TEXT NOT NULL,
    password TEXT NOT NULL,
    auth_mode TEXT NOT NULL,
    admin_user TEXT,
    admin_pass TEXT,
    last_sync TIMESTAMP,
    contacts_protocol TEXT NOT NULL
);

CREATE TABLE calendar_accounts (
    id TEXT NOT NULL PRIMARY KEY,
    server_url TEXT NOT NULL,
    username TEXT NOT NULL,
    password TEXT NOT NULL,
    auth_mode TEXT NOT NULL,
    admin_user TEXT,
    admin_pass TEXT,
    last_sync TIMESTAMP,
    calendar_protocol TEXT NOT NULL
);

CREATE TABLE filters (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES mail_accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    content TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT 0,
    UNIQUE (account_id, name)
);

CREATE INDEX idx_filters_account ON filters(account_id);

CREATE TABLE mailboxes (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES mail_accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    total_emails INTEGER NOT NULL DEFAULT 0,
    unread_emails INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_mailboxes_account ON mailboxes(account_id);

CREATE TABLE emails (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES mail_accounts(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL,
    mailbox_ids TEXT NOT NULL,
    from_list TEXT NOT NULL,
    to_list TEXT NOT NULL,
    cc_list TEXT NOT NULL,
    bcc_list TEXT NOT NULL,
    subject TEXT NOT NULL,
    received_at TIMESTAMP NOT NULL,
    preview TEXT NOT NULL,
    body_text TEXT,
    body_html TEXT,
    keywords TEXT NOT NULL,
    has_attachments BOOLEAN NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_emails_account ON emails(account_id);
CREATE INDEX idx_emails_received_at ON emails(received_at);

CREATE TABLE addressbooks (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES contacts_accounts(id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    name TEXT NOT NULL,
    ctag TEXT
);

CREATE INDEX idx_addressbooks_account ON addressbooks(account_id);

CREATE TABLE contacts (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES contacts_accounts(id) ON DELETE CASCADE,
    addressbook_id TEXT NOT NULL REFERENCES addressbooks(id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    etag TEXT,
    uid TEXT NOT NULL,
    display_name TEXT NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    emails TEXT NOT NULL,
    phones TEXT NOT NULL,
    org TEXT NOT NULL,
    note TEXT NOT NULL,
    raw_vcard TEXT NOT NULL
);

CREATE INDEX idx_contacts_account ON contacts(account_id);
CREATE INDEX idx_contacts_addressbook ON contacts(addressbook_id);

CREATE TABLE calendars (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES calendar_accounts(id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    name TEXT NOT NULL,
    color TEXT,
    ctag TEXT
);

CREATE INDEX idx_calendars_account ON calendars(account_id);

CREATE TABLE events (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES calendar_accounts(id) ON DELETE CASCADE,
    calendar_id TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    etag TEXT,
    uid TEXT NOT NULL,
    title TEXT NOT NULL,
    start TIMESTAMP NOT NULL,
    end TIMESTAMP NOT NULL,
    all_day BOOLEAN NOT NULL DEFAULT 0,
    description TEXT NOT NULL,
    location TEXT NOT NULL,
    status TEXT NOT NULL,
    raw_ics TEXT NOT NULL
);

CREATE INDEX idx_events_account ON events(account_id);
CREATE INDEX idx_events_calendar ON events(calendar_id);
CREATE INDEX idx_events_start ON events(start);

CREATE TABLE tasks (
    id TEXT NOT NULL PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES calendar_accounts(id) ON DELETE CASCADE,
    calendar_id TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    etag TEXT,
    uid TEXT NOT NULL,
    title TEXT NOT NULL,
    due TIMESTAMP,
    all_day BOOLEAN NOT NULL DEFAULT 0,
    description TEXT NOT NULL,
    location TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    percent_complete INTEGER NOT NULL DEFAULT 0,
    completed TIMESTAMP,
    raw_ics TEXT NOT NULL
);

CREATE INDEX idx_tasks_account ON tasks(account_id);
CREATE INDEX idx_tasks_calendar ON tasks(calendar_id);
