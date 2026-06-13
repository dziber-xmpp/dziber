use keyring::Entry;

pub const SERVICE_XMPP: &str = "dziber-xmpp";
pub const SERVICE_MAIL: &str = "dziber-mail";
pub const SERVICE_MAIL_ADMIN: &str = "dziber-mail-admin";
pub const SERVICE_CONTACTS: &str = "dziber-contacts";
pub const SERVICE_CONTACTS_ADMIN: &str = "dziber-contacts-admin";
pub const SERVICE_CALENDAR: &str = "dziber-calendar";
pub const SERVICE_CALENDAR_ADMIN: &str = "dziber-calendar-admin";

pub fn store_password(service: &str, account: &str, password: &str) -> Result<(), String> {
    Entry::new(service, account)
        .map_err(|e| format!("keyring entry failed: {}", e))?
        .set_password(password)
        .map_err(|e| format!("keyring store failed: {}", e))
}

pub fn get_password(service: &str, account: &str) -> Result<Option<String>, String> {
    match Entry::new(service, account)
        .map_err(|e| format!("keyring entry failed: {}", e))?
        .get_password()
    {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get failed: {}", e)),
    }
}

pub fn delete_password(service: &str, account: &str) -> Result<(), String> {
    match Entry::new(service, account)
        .map_err(|e| format!("keyring entry failed: {}", e))?
        .delete_credential()
    {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete failed: {}", e)),
    }
}
