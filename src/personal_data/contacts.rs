use calcard::vcard::{VCard, VCardProperty, VCardValue};
use serde_json::{Value, json};

use crate::models::account::ContactsAccount;
use crate::models::contact_card::{Addressbook, ContactCard};
use crate::personal_data::auth_header_for_account;
use crate::personal_data::dav::{DavClient, encode_account, extract_rel_path};
use crate::personal_data::jmap::JmapClient;

pub struct CardDavClient {
    dav: DavClient,
    account: ContactsAccount,
}

impl CardDavClient {
    pub fn new(account: &ContactsAccount) -> Self {
        let auth_header = auth_header_for_account(account);
        let dav = DavClient::new(account.server_url.clone(), auth_header);
        Self {
            dav,
            account: account.clone(),
        }
    }

    fn carddav_root(&self) -> String {
        let encoded = encode_account(&self.account.username);
        format!("/dav/card/{}/", encoded)
    }

    async fn root(&self) -> String {
        self.dav
            .discover_home_set("carddav")
            .await
            .unwrap_or_else(|| self.carddav_root())
    }

    fn account_id(&self) -> String {
        self.account.id.clone()
    }

    pub async fn list_addressbooks(&self) -> Result<Vec<Addressbook>, String> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop><D:resourcetype/><D:displayname/><D:getctag/></D:prop>
</D:propfind>"#;

        let root = self.root().await;
        let responses = self
            .dav
            .propfind(&root, body, "1")
            .await
            .map_err(|e| e.to_string())?;

        let account_id = self.account_id();
        let mut books = Vec::new();

        for resp in responses {
            let href = resp.href.clone();
            if href.ends_with('/') && resp.resource_type().iter().any(|t| t == "addressbook") {
                let rel = extract_rel_path(&href, "card");
                let name = resp.prop("displayname").unwrap_or("Addressbook").to_string();
                let ctag = resp.prop("getctag").map(|s| s.to_string());
                books.push(Addressbook {
                    id: rel.trim_end_matches('/').to_string(),
                    account_id: account_id.clone(),
                    href,
                    name,
                    ctag,
                });
            }
        }

        Ok(books)
    }

    pub async fn list_contacts(
        &self,
        addressbook: &Addressbook,
    ) -> Result<Vec<ContactCard>, String> {
        let path = if addressbook.href.starts_with('/') {
            addressbook.href.clone()
        } else {
            format!("/{}", addressbook.href)
        };

        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
  <D:prop><D:getetag/><C:address-data/></D:prop>
</D:propfind>"#;

        let responses = self
            .dav
            .propfind(&path, body, "1")
            .await
            .map_err(|e| e.to_string())?;

        let account_id = self.account_id();
        let mut contacts = Vec::new();

        for resp in responses {
            if resp.href.ends_with(".vcf")
                && let Some(vcard_text) = resp.prop("address-data")
                    && let Some(contact) = parse_vcard(
                        &account_id,
                        &addressbook.id,
                        &resp.href,
                        resp.prop("getetag").map(|s| s.to_string()),
                        vcard_text,
                    ) {
                        contacts.push(contact);
                    }
        }

        Ok(contacts)
    }

    pub async fn save_contact(&self, contact: &ContactCard) -> Result<(), String> {
        let vcard_text = serialize_vcard(contact);
        let path = if contact.href.starts_with('/') {
            contact.href.clone()
        } else {
            format!("/{}", contact.href)
        };

        let etag = contact.etag.as_deref();
        let (status, _) = self
            .dav
            .put(
                &path,
                vcard_text,
                "text/vcard; charset=utf-8",
                etag,
            )
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to save contact: HTTP {}", status));
        }

        Ok(())
    }

    pub async fn delete_contact(&self, contact: &ContactCard) -> Result<(), String> {
        let path = if contact.href.starts_with('/') {
            contact.href.clone()
        } else {
            format!("/{}", contact.href)
        };

        let etag = contact.etag.as_deref();
        let status = self
            .dav
            .delete(&path, etag)
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to delete contact: HTTP {}", status));
        }

        Ok(())
    }

}

fn first_text(values: &[VCardValue]) -> Option<String> {
    values.iter().find_map(|v| match v {
        VCardValue::Text(s) => Some(s.clone()),
        _ => None,
    })
}

fn first_component(values: &[VCardValue]) -> Option<Vec<String>> {
    values.iter().find_map(|v| match v {
        VCardValue::Component(parts) => Some(parts.clone()),
        _ => None,
    })
}

fn parse_vcard(
    account_id: &str,
    addressbook_id: &str,
    href: &str,
    etag: Option<String>,
    text: &str,
) -> Option<ContactCard> {
    let vcard = VCard::parse(text).ok()?;
    let mut contact = ContactCard {
        id: String::new(),
        account_id: account_id.to_string(),
        addressbook_id: addressbook_id.to_string(),
        href: href.to_string(),
        etag,
        uid: String::new(),
        display_name: String::new(),
        first_name: String::new(),
        last_name: String::new(),
        emails: Vec::new(),
        phones: Vec::new(),
        org: String::new(),
        note: String::new(),
        raw_vcard: text.to_string(),
    };

    for entry in &vcard.entries {
        match entry.name {
            VCardProperty::Uid => {
                if let Some(val) = first_text(&entry.values) {
                    contact.uid = val;
                }
            }
            VCardProperty::Fn => {
                if let Some(val) = first_text(&entry.values) {
                    contact.display_name = val;
                }
            }
            VCardProperty::N => {
                if let Some(parts) = first_component(&entry.values) {
                    contact.last_name = parts.first().cloned().unwrap_or_default();
                    contact.first_name = parts.get(1).cloned().unwrap_or_default();
                }
            }
            VCardProperty::Email => {
                if let Some(val) = first_text(&entry.values) {
                    contact.emails.push(val);
                }
            }
            VCardProperty::Tel => {
                if let Some(val) = first_text(&entry.values) {
                    contact.phones.push(val);
                }
            }
            VCardProperty::Org => {
                if let Some(parts) = first_component(&entry.values) {
                    contact.org = parts.join(" ");
                } else if let Some(val) = first_text(&entry.values) {
                    contact.org = val;
                }
            }
            VCardProperty::Note => {
                if let Some(val) = first_text(&entry.values) {
                    contact.note = val.replace("\\n", "\n").replace("\\,", ",");
                }
            }
            _ => {}
        }
    }

    if contact.uid.is_empty() {
        contact.uid = href.split('/').next_back().unwrap_or(href).to_string();
    }
    contact.id = contact.uid.clone();

    Some(contact)
}

fn serialize_vcard(contact: &ContactCard) -> String {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:3.0".to_string(),
        format!("UID:{}", contact.uid),
        format!("FN:{}", contact.display_name),
        format!(
            "N:{};{};;;",
            contact.last_name, contact.first_name
        ),
    ];

    for email in &contact.emails {
        lines.push(format!("EMAIL;TYPE=INTERNET:{}", email));
    }
    for phone in &contact.phones {
        lines.push(format!("TEL;TYPE=VOICE:{}", phone));
    }
    if !contact.org.is_empty() {
        lines.push(format!("ORG:{}", contact.org));
    }
    if !contact.note.is_empty() {
        lines.push(format!(
            "NOTE:{}",
            contact.note.replace('\n', "\\n").replace(',', "\\,")
        ));
    }

    lines.push("END:VCARD".to_string());
    lines.join("\r\n") + "\r\n"
}

pub fn import_vcards(text: &str, account_id: &str, addressbook_id: &str) -> Vec<ContactCard> {
    let mut contacts = Vec::new();
    let mut current = String::new();
    let mut in_card = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("BEGIN:VCARD") {
            in_card = true;
            current = "BEGIN:VCARD".to_string();
        } else if trimmed.eq_ignore_ascii_case("END:VCARD") {
            if in_card {
                current.push_str("\r\nEND:VCARD");
                in_card = false;
                if let Some(contact) = parse_vcard(account_id, addressbook_id, "", None, &current) {
                    contacts.push(contact);
                }
            }
        } else if in_card {
            current.push_str("\r\n");
            current.push_str(line);
        }
    }

    contacts
}

pub fn export_vcards(contacts: &[ContactCard]) -> String {
    contacts
        .iter()
        .map(serialize_vcard)
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub struct JmapContactsClient {
    jmap: JmapClient,
}

impl JmapContactsClient {
    pub fn new(account: &ContactsAccount) -> Self {
        Self {
            jmap: JmapClient::new(account),
        }
    }

    pub async fn list_addressbooks(&self) -> Result<Vec<Addressbook>, String> {
        let account_id = self.jmap.account_id.clone();
        let response = self
            .jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:contacts",
                ],
                vec![json!([
                    "AddressBook/get",
                    {
                        "accountId": account_id,
                        "ids": null
                    },
                    "0"
                ])],
            )
            .await?;

        let mut books = Vec::new();
        if let Some(args) = self.jmap.extract_response(&response, "AddressBook/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        books.push(Addressbook {
                            id: id.to_string(),
                            account_id: self.jmap.account_id.clone(),
                            href: String::new(),
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Addressbook")
                                .to_string(),
                            ctag: None,
                        });
                    }
                }
            }
        Ok(books)
    }

    pub async fn list_contacts(
        &self,
        addressbook: &Addressbook,
    ) -> Result<Vec<ContactCard>, String> {
        let account_id = self.jmap.account_id.clone();
        let response = self
            .jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:contacts",
                ],
                vec![json!([
                    "ContactCard/get",
                    {
                        "accountId": account_id,
                        "ids": null,
                        "properties": [
                            "id", "addressBookIds", "name", "emails", "phones",
                            "organizations", "notes", "uid"
                        ]
                    },
                    "0"
                ])],
            )
            .await?;

        let mut contacts = Vec::new();
        if let Some(args) = self.jmap.extract_response(&response, "ContactCard/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(contact) = parse_jmap_contact(&self.jmap.account_id, item)
                        && contact.addressbook_id == addressbook.id {
                            contacts.push(contact);
                        }
                }
            }
        Ok(contacts)
    }

    pub async fn save_contact(&self, contact: &ContactCard) -> Result<(), String> {
        let account_id = self.jmap.account_id.clone();
        let payload = contact_to_jmap_json(contact);
        let method_call = if contact.id.is_empty() || contact.id.starts_with('~') {
            let c_id = "new-contact";
            json!([
                "ContactCard/set",
                {
                    "accountId": account_id,
                    "create": {
                        c_id: payload
                    }
                },
                "0"
            ])
        } else {
            json!([
                "ContactCard/set",
                {
                    "accountId": account_id,
                    "update": {
                        contact.id.clone(): payload
                    }
                },
                "0"
            ])
        };

        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:contacts",
                ],
                vec![method_call],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_contact(&self, contact: &ContactCard) -> Result<(), String> {
        if contact.id.is_empty() {
            return Ok(());
        }
        let account_id = self.jmap.account_id.clone();
        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:contacts",
                ],
                vec![json!([
                    "ContactCard/set",
                    {
                        "accountId": account_id,
                        "destroy": [contact.id.clone()]
                    },
                    "0"
                ])],
            )
            .await?;
        Ok(())
    }

}

fn parse_jmap_contact(account_id: &str, item: &Value) -> Option<ContactCard> {
    let id = item.get("id")?.as_str()?.to_string();
    let addressbook_id = item
        .get("addressBookIds")
        .and_then(|v| v.as_object())?
        .keys()
        .next()?
        .clone();

    let name_obj = item.get("name")?;
    let components = name_obj.get("components")?.as_array()?;
    let mut first_name = String::new();
    let mut last_name = String::new();
    let mut display_name = name_obj
        .get("full")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    for comp in components {
        if let (Some(kind), Some(value)) = (
            comp.get("kind").and_then(|v| v.as_str()),
            comp.get("value").and_then(|v| v.as_str()),
        ) {
            match kind {
                "given" => first_name = value.to_string(),
                "surname" => last_name = value.to_string(),
                _ => {}
            }
        }
    }

    if display_name.is_empty() {
        display_name = format!("{} {}", first_name, last_name)
            .trim()
            .to_string();
    }

    let emails = item
        .get("emails")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.values()
                .filter_map(|e| e.get("address").and_then(|a| a.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let phones = item
        .get("phones")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.values()
                .filter_map(|p| {
                    p.get("phone")
                        .or_else(|| p.get("value"))
                        .and_then(|a| a.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    let org = item
        .get("organizations")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.values().next())
        .and_then(|o| o.get("name").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let note = item
        .get("notes")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.values().next())
        .and_then(|o| o.get("note").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let uid = item
        .get("uid")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();

    Some(ContactCard {
        id,
        account_id: account_id.to_string(),
        addressbook_id,
        href: String::new(),
        etag: None,
        uid,
        display_name,
        first_name,
        last_name,
        emails,
        phones,
        org,
        note,
        raw_vcard: String::new(),
    })
}

fn contact_to_jmap_json(contact: &ContactCard) -> Value {
    let mut emails = serde_json::Map::new();
    for (i, email) in contact.emails.iter().enumerate() {
        emails.insert(i.to_string(), json!({ "address": email }));
    }

    let mut phones = serde_json::Map::new();
    for (i, phone) in contact.phones.iter().enumerate() {
        phones.insert(i.to_string(), json!({ "phone": phone }));
    }

    json!({
        "addressBookIds": {
            contact.addressbook_id.clone(): true
        },
        "name": {
            "full": contact.display_name.clone(),
            "components": [
                { "kind": "surname", "value": contact.last_name.clone() },
                { "kind": "given", "value": contact.first_name.clone() }
            ]
        },
        "emails": emails,
        "phones": phones,
        "organizations": {
            "0": { "name": contact.org.clone() }
        },
        "notes": {
            "0": { "note": contact.note.clone() }
        }
    })
}

pub enum ContactsClient {
    Dav(CardDavClient),
    Jmap(JmapContactsClient),
}

impl ContactsClient {
    pub fn new(account: &ContactsAccount) -> Self {
        match account.contacts_protocol {
            crate::models::account::DavOrJmap::Jmap => Self::Jmap(JmapContactsClient::new(account)),
            _ => Self::Dav(CardDavClient::new(account)),
        }
    }

    pub async fn list_addressbooks(&self) -> Result<Vec<Addressbook>, String> {
        match self {
            Self::Dav(c) => c.list_addressbooks().await,
            Self::Jmap(c) => c.list_addressbooks().await,
        }
    }

    pub async fn list_contacts(
        &self,
        addressbook: &Addressbook,
    ) -> Result<Vec<ContactCard>, String> {
        match self {
            Self::Dav(c) => c.list_contacts(addressbook).await,
            Self::Jmap(c) => c.list_contacts(addressbook).await,
        }
    }

    pub async fn save_contact(&self, contact: &ContactCard) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.save_contact(contact).await,
            Self::Jmap(c) => c.save_contact(contact).await,
        }
    }

    pub async fn delete_contact(&self, contact: &ContactCard) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.delete_contact(contact).await,
            Self::Jmap(c) => c.delete_contact(contact).await,
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn sample_contact() -> ContactCard {
        ContactCard {
            id: "c1".to_string(),
            account_id: "a".to_string(),
            addressbook_id: "ab1".to_string(),
            href: String::new(),
            etag: None,
            uid: "c1".to_string(),
            display_name: "Alice Smith".to_string(),
            first_name: "Alice".to_string(),
            last_name: "Smith".to_string(),
            emails: vec!["alice@example.com".to_string()],
            phones: vec!["+123".to_string()],
            org: "Example".to_string(),
            note: "Friend".to_string(),
            raw_vcard: String::new(),
        }
    }

    #[test]
    fn vcard_export_import_roundtrip() {
        let contact = sample_contact();
        let vcf = export_vcards(std::slice::from_ref(&contact));

        assert!(vcf.contains("BEGIN:VCARD"));
        assert!(vcf.contains("Alice Smith"));
        assert!(vcf.contains("alice@example.com"));

        let imported = import_vcards(&vcf, "a", "ab1");
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].uid, contact.uid);
        assert_eq!(imported[0].display_name, contact.display_name);
        assert_eq!(imported[0].emails, contact.emails);
        assert_eq!(imported[0].phones, contact.phones);
        assert_eq!(imported[0].org, contact.org);
    }

    #[test]
    fn vcard_import_multiple_cards() {
        let vcf = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nFN:One\r\nN:One;;;\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nUID:c2\r\nFN:Two\r\nN:Two;;;\r\nEND:VCARD\r\n";
        let imported = import_vcards(vcf, "a", "ab1");
        assert_eq!(imported.len(), 2);
        assert_eq!(imported[0].display_name, "One");
        assert_eq!(imported[1].display_name, "Two");
    }
}
