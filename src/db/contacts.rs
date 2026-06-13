use diesel::prelude::*;

use crate::db::establish_connection;
use crate::db::models::{DbAddressbook, DbContact, DbContactsAccount};

use crate::models::account::{AuthMode, ContactsAccount, dav_or_jmap_from_string, dav_or_jmap_to_string};
use crate::models::contact_card::{Addressbook, ContactCard};

fn serialize_list(list: &[String]) -> String {
    list.join("\x1f")
}

fn deserialize_list(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\x1f').map(|x| x.to_string()).collect()
    }
}

pub fn save_account(account: &ContactsAccount) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::contacts_accounts;

    let mut conn = establish_connection();

    if !account.password.is_empty() {
        crate::secrets::store_password(
            crate::secrets::SERVICE_CONTACTS,
            &account.id,
            &account.password,
        )?;
    }

    let (auth, admin_user_val, admin_pass_val) = match &account.auth_mode {
        AuthMode::Basic => ("basic", None, None),
        AuthMode::StalwartImpersonation {
            admin_user: adm_user,
            admin_pass: adm_pass,
        } => {
            if !adm_pass.is_empty() {
                crate::secrets::store_password(
                    crate::secrets::SERVICE_CONTACTS_ADMIN,
                    &account.id,
                    adm_pass,
                )?;
            }
            ("stalwart", Some(adm_user.clone()), Some(String::new()))
        }
    };

    let db_account = DbContactsAccount {
        id: account.id.clone(),
        server_url: account.server_url.clone(),
        username: account.username.clone(),
        password: String::new(),
        auth_mode: auth.to_string(),
        admin_user: admin_user_val,
        admin_pass: admin_pass_val,
        last_sync: None,
        contacts_protocol: dav_or_jmap_to_string(&account.contacts_protocol),
    };

    diesel::replace_into(contacts_accounts::table)
        .values(&db_account)
        .execute(&mut conn)?;

    Ok(())
}

pub fn load_accounts() -> Result<Vec<ContactsAccount>, Box<dyn std::error::Error>> {
    use crate::db::schema::contacts_accounts;

    let mut conn = establish_connection();
    let results: Vec<DbContactsAccount> = contacts_accounts::table.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|a| {
            let password = if a.password.is_empty() {
                crate::secrets::get_password(crate::secrets::SERVICE_CONTACTS, &a.id)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            } else {
                a.password
            };

            let auth = match a.auth_mode.as_str() {
                "stalwart" => {
                    let admin_user = a.admin_user.unwrap_or_default();
                    let admin_pass = if a.admin_pass.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                        crate::secrets::get_password(crate::secrets::SERVICE_CONTACTS_ADMIN, &a.id)
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                    } else {
                        a.admin_pass.unwrap_or_default()
                    };
                    AuthMode::StalwartImpersonation { admin_user, admin_pass }
                }
                _ => AuthMode::Basic,
            };

            ContactsAccount {
                id: a.id,
                server_url: a.server_url,
                username: a.username,
                password,
                auth_mode: auth,
                contacts_protocol: dav_or_jmap_from_string(&a.contacts_protocol),
            }
        })
        .collect())
}

pub fn delete_account(account_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::contacts_accounts::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(contacts_accounts.filter(id.eq(account_id))).execute(&mut conn)?;

    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_CONTACTS, account_id);
    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_CONTACTS_ADMIN, account_id);

    Ok(())
}

pub fn save_addressbooks(
    acc_id: &str,
    items: &[Addressbook],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::addressbooks::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(addressbooks.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    let db_items: Vec<DbAddressbook> = items
        .iter()
        .map(|a| DbAddressbook {
            id: a.id.clone(),
            account_id: a.account_id.clone(),
            href: a.href.clone(),
            name: a.name.clone(),
            ctag: a.ctag.clone(),
        })
        .collect();

    diesel::insert_into(addressbooks)
        .values(&db_items)
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_addressbooks(acc_id: &str) -> Result<Vec<Addressbook>, Box<dyn std::error::Error>> {
    use crate::db::schema::addressbooks::dsl::*;

    let mut conn = establish_connection();
    let results: Vec<DbAddressbook> = addressbooks
        .filter(account_id.eq(acc_id))
        .order(name.asc())
        .load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|a| Addressbook {
            id: a.id,
            account_id: a.account_id,
            href: a.href,
            name: a.name,
            ctag: a.ctag,
        })
        .collect())
}

pub fn save_contacts(
    acc_id: &str,
    items: &[ContactCard],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::contacts::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(contacts.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    for item in items {
        let db_contact = DbContact {
            id: item.id.clone(),
            account_id: item.account_id.clone(),
            addressbook_id: item.addressbook_id.clone(),
            href: item.href.clone(),
            etag: item.etag.clone(),
            uid: item.uid.clone(),
            display_name: item.display_name.clone(),
            first_name: item.first_name.clone(),
            last_name: item.last_name.clone(),
            emails: serialize_list(&item.emails),
            phones: serialize_list(&item.phones),
            org: item.org.clone(),
            note: item.note.clone(),
            raw_vcard: item.raw_vcard.clone(),
        };

        diesel::replace_into(contacts)
            .values(&db_contact)
            .execute(&mut conn)?;
    }

    Ok(())
}

pub fn load_contacts(
    acc_id: &str,
    book_id: Option<&str>,
) -> Result<Vec<ContactCard>, Box<dyn std::error::Error>> {
    use crate::db::schema::contacts::dsl::*;

    let mut conn = establish_connection();
    let mut query = contacts
        .filter(account_id.eq(acc_id))
        .order(display_name.asc())
        .into_boxed();

    if let Some(b) = book_id {
        query = query.filter(addressbook_id.eq(b));
    }

    let results: Vec<DbContact> = query.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|c| ContactCard {
            id: c.id,
            account_id: c.account_id,
            addressbook_id: c.addressbook_id,
            href: c.href,
            etag: c.etag,
            uid: c.uid,
            display_name: c.display_name,
            first_name: c.first_name,
            last_name: c.last_name,
            emails: deserialize_list(&c.emails),
            phones: deserialize_list(&c.phones),
            org: c.org,
            note: c.note,
            raw_vcard: c.raw_vcard,
        })
        .collect())
}

