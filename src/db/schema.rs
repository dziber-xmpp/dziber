// @generated automatically by Diesel CLI.

diesel::table! {
    addressbooks (id) {
        id -> Text,
        account_id -> Text,
        href -> Text,
        name -> Text,
        ctag -> Nullable<Text>,
    }
}

diesel::table! {
    calendar_accounts (id) {
        id -> Text,
        server_url -> Text,
        username -> Text,
        password -> Text,
        auth_mode -> Text,
        admin_user -> Nullable<Text>,
        admin_pass -> Nullable<Text>,
        last_sync -> Nullable<Timestamp>,
        calendar_protocol -> Text,
    }
}

diesel::table! {
    calendars (id) {
        id -> Text,
        account_id -> Text,
        href -> Text,
        name -> Text,
        color -> Nullable<Text>,
        ctag -> Nullable<Text>,
    }
}

diesel::table! {
    contacts (id) {
        id -> Text,
        account_id -> Text,
        addressbook_id -> Text,
        href -> Text,
        etag -> Nullable<Text>,
        uid -> Text,
        display_name -> Text,
        first_name -> Text,
        last_name -> Text,
        emails -> Text,
        phones -> Text,
        org -> Text,
        note -> Text,
        raw_vcard -> Text,
    }
}

diesel::table! {
    contacts_accounts (id) {
        id -> Text,
        server_url -> Text,
        username -> Text,
        password -> Text,
        auth_mode -> Text,
        admin_user -> Nullable<Text>,
        admin_pass -> Nullable<Text>,
        last_sync -> Nullable<Timestamp>,
        contacts_protocol -> Text,
    }
}

diesel::table! {
    emails (id) {
        id -> Text,
        account_id -> Text,
        thread_id -> Text,
        mailbox_ids -> Text,
        from_list -> Text,
        to_list -> Text,
        cc_list -> Text,
        bcc_list -> Text,
        subject -> Text,
        received_at -> Timestamp,
        preview -> Text,
        body_text -> Nullable<Text>,
        body_html -> Nullable<Text>,
        keywords -> Text,
        has_attachments -> Bool,
        size -> Integer,
    }
}

diesel::table! {
    events (id) {
        id -> Text,
        account_id -> Text,
        calendar_id -> Text,
        href -> Text,
        etag -> Nullable<Text>,
        uid -> Text,
        title -> Text,
        start -> Timestamp,
        end -> Timestamp,
        all_day -> Bool,
        description -> Text,
        location -> Text,
        status -> Text,
        raw_ics -> Text,
    }
}

diesel::table! {
    mail_accounts (id) {
        id -> Text,
        server_url -> Text,
        username -> Text,
        password -> Text,
        auth_mode -> Text,
        admin_user -> Nullable<Text>,
        admin_pass -> Nullable<Text>,
        last_sync -> Nullable<Timestamp>,
        mail_protocol -> Text,
        imap_server -> Nullable<Text>,
        imap_port -> Nullable<Integer>,
        smtp_server -> Nullable<Text>,
        smtp_port -> Nullable<Integer>,
        security -> Nullable<Text>,
        sieve_server -> Nullable<Text>,
        sieve_port -> Nullable<Integer>,
        sieve_security -> Nullable<Text>,
    }
}

diesel::table! {
    filters (id) {
        id -> Text,
        account_id -> Text,
        name -> Text,
        content -> Text,
        is_active -> Bool,
    }
}

diesel::table! {
    mailboxes (id) {
        id -> Text,
        account_id -> Text,
        name -> Text,
        role -> Nullable<Text>,
        sort_order -> Integer,
        total_emails -> Integer,
        unread_emails -> Integer,
    }
}

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
    omemo_bundle_cache (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
        identity_key -> Binary,
    }
}

diesel::table! {
    omemo_devices (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
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
    omemo_trust (jid, device_id) {
        jid -> Text,
        device_id -> Integer,
        status -> Text,
    }
}

diesel::table! {
    tasks (id) {
        id -> Text,
        account_id -> Text,
        calendar_id -> Text,
        href -> Text,
        etag -> Nullable<Text>,
        uid -> Text,
        title -> Text,
        due -> Nullable<Timestamp>,
        all_day -> Bool,
        description -> Text,
        location -> Text,
        status -> Text,
        priority -> Integer,
        percent_complete -> Integer,
        completed -> Nullable<Timestamp>,
        raw_ics -> Text,
    }
}

diesel::joinable!(addressbooks -> contacts_accounts (account_id));
diesel::joinable!(calendars -> calendar_accounts (account_id));
diesel::joinable!(contacts -> addressbooks (addressbook_id));
diesel::joinable!(contacts -> contacts_accounts (account_id));
diesel::joinable!(emails -> mail_accounts (account_id));
diesel::joinable!(filters -> mail_accounts (account_id));
diesel::joinable!(events -> calendar_accounts (account_id));
diesel::joinable!(events -> calendars (calendar_id));
diesel::joinable!(mailboxes -> mail_accounts (account_id));
diesel::joinable!(tasks -> calendar_accounts (account_id));
diesel::joinable!(tasks -> calendars (calendar_id));

diesel::allow_tables_to_appear_in_same_query!(
    addressbooks,
    calendar_accounts,
    calendars,
    contacts,
    contacts_accounts,
    emails,
    events,
    filters,
    mail_accounts,
    mailboxes,
    messages,
    omemo_account,
    omemo_bundle_cache,
    omemo_devices,
    omemo_key,
    omemo_sessions,
    omemo_trust,
    tasks,
);
