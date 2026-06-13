-- Revert the consolidated schema by dropping all tables.

DROP TABLE IF EXISTS filters;
DROP TABLE IF EXISTS tasks;
DROP TABLE IF EXISTS events;
DROP TABLE IF EXISTS calendars;
DROP TABLE IF EXISTS contacts;
DROP TABLE IF EXISTS addressbooks;
DROP TABLE IF EXISTS emails;
DROP TABLE IF EXISTS mailboxes;
DROP TABLE IF EXISTS calendar_accounts;
DROP TABLE IF EXISTS contacts_accounts;
DROP TABLE IF EXISTS mail_accounts;
DROP TABLE IF EXISTS omemo_key;
DROP TABLE IF EXISTS omemo_bundle_cache;
DROP TABLE IF EXISTS omemo_trust;
DROP TABLE IF EXISTS omemo_devices;
DROP TABLE IF EXISTS omemo_sessions;
DROP TABLE IF EXISTS omemo_account;
DROP TABLE IF EXISTS messages;
