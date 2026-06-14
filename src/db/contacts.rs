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


#[cfg(test)]
mod tests {
    use diesel::prelude::*;

    use crate::db::models::{DbAddressbook, DbContactsAccount};
    use crate::db::schema::{addressbooks, contacts_accounts};
    use crate::db::test_helpers::{connection, with_test_db};
    use crate::models::contact_card::{Addressbook, ContactCard};

    use super::{deserialize_list, serialize_list};

    fn insert_contacts_account(account_id: &str) {
        let mut conn = connection();
        diesel::insert_into(contacts_accounts::table)
            .values(&DbContactsAccount {
                id: account_id.to_string(),
                server_url: "https://contacts.example.com".to_string(),
                username: "user".to_string(),
                password: String::new(),
                auth_mode: "basic".to_string(),
                admin_user: None,
                admin_pass: None,
                last_sync: None,
                contacts_protocol: "dav".to_string(),
            })
            .execute(&mut conn)
            .unwrap();
    }

    fn insert_addressbook(id: &str, account_id: &str) {
        let mut conn = connection();
        diesel::insert_into(addressbooks::table)
            .values(&DbAddressbook {
                id: id.to_string(),
                account_id: account_id.to_string(),
                href: format!("/addressbooks/{}/", id),
                name: format!("Book {}", id),
                ctag: Some("1".to_string()),
            })
            .execute(&mut conn)
            .unwrap();
    }

    fn sample_addressbook(id: &str, account_id: &str, name: &str) -> Addressbook {
        Addressbook {
            id: id.to_string(),
            account_id: account_id.to_string(),
            href: format!("/addressbooks/{}/", id),
            name: name.to_string(),
            ctag: Some("1".to_string()),
        }
    }

    fn sample_contact(id: &str, account_id: &str, book_id: &str, name: &str) -> ContactCard {
        ContactCard {
            id: id.to_string(),
            account_id: account_id.to_string(),
            addressbook_id: book_id.to_string(),
            href: format!("/contacts/{}", id),
            etag: Some("etag".to_string()),
            uid: format!("uid-{}", id),
            display_name: name.to_string(),
            first_name: "First".to_string(),
            last_name: "Last".to_string(),
            emails: vec![format!("{}@example.com", id), "other@example.com".to_string()],
            phones: vec!["+123".to_string()],
            org: "Org".to_string(),
            note: "Note".to_string(),
            raw_vcard: "VCARD".to_string(),
        }
    }

    #[test]
    fn serialize_list_roundtrip() {
        let cases = [
            Vec::<String>::new(),
            vec!["one".to_string()],
            vec!["a".to_string(), "b".to_string()],
        ];
        for original in &cases {
            assert_eq!(deserialize_list(&serialize_list(original)), *original);
        }
    }

    #[test]
    fn save_and_load_addressbooks() {
        let _guard = with_test_db();
        insert_contacts_account("contacts-acc-1");

        let books = vec![
            sample_addressbook("b2", "contacts-acc-1", "Work"),
            sample_addressbook("b1", "contacts-acc-1", "Personal"),
        ];
        super::save_addressbooks("contacts-acc-1", &books).unwrap();
        let loaded = super::load_addressbooks("contacts-acc-1").unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Personal");
        assert_eq!(loaded[1].name, "Work");
        assert_eq!(loaded, vec![
            sample_addressbook("b1", "contacts-acc-1", "Personal"),
            sample_addressbook("b2", "contacts-acc-1", "Work"),
        ]);
    }

    #[test]
    fn save_and_load_contacts() {
        let _guard = with_test_db();
        insert_contacts_account("contacts-acc-1");
        insert_addressbook("book-1", "contacts-acc-1");

        let contacts = vec![
            sample_contact("c1", "contacts-acc-1", "book-1", "Alice"),
            sample_contact("c2", "contacts-acc-1", "book-1", "Bob"),
        ];
        super::save_contacts("contacts-acc-1", &contacts).unwrap();
        let loaded = super::load_contacts("contacts-acc-1", None).unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].display_name, "Alice");
        assert_eq!(loaded[1].display_name, "Bob");
        assert_eq!(loaded, contacts);
    }

    #[test]
    fn load_contacts_by_addressbook() {
        let _guard = with_test_db();
        insert_contacts_account("contacts-acc-1");
        insert_addressbook("book-1", "contacts-acc-1");
        insert_addressbook("book-2", "contacts-acc-1");

        let contact_a = sample_contact("c1", "contacts-acc-1", "book-1", "Alice");
        let contact_b = sample_contact("c2", "contacts-acc-1", "book-2", "Bob");
        super::save_contacts("contacts-acc-1", &[contact_a, contact_b]).unwrap();

        let loaded = super::load_contacts("contacts-acc-1", Some("book-1")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "c1");
    }
}
