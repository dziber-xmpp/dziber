use iced::widget::{Button, Column, button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length};

use crate::models::contact_card::{Addressbook, ContactCard};
use crate::ui::app::Message;

#[derive(Debug, Clone)]
pub enum ContactsMessage {
    AddressbookSelected(String),
    ContactSelected(String),
    NewContact,
    SaveContact,
    DeleteContact,
    CancelEdit,
    DisplayNameChanged(String),
    FirstNameChanged(String),
    LastNameChanged(String),
    EmailChanged(String),
    PhoneChanged(String),
    OrgChanged(String),
    NoteChanged(String),
    ImportVcf,
    ExportVcf,
}

impl ContactsMessage {
    pub fn into_message(self) -> Message {
        Message::Contacts(self)
    }
}

#[derive(Debug, Default)]
pub struct ContactsState {
    pub addressbooks: Vec<Addressbook>,
    pub contacts: Vec<ContactCard>,
    pub selected_addressbook: Option<String>,
    pub selected_contact: Option<ContactCard>,
    pub editing: bool,
    pub edit_display_name: String,
    pub edit_first_name: String,
    pub edit_last_name: String,
    pub edit_email: String,
    pub edit_phone: String,
    pub edit_org: String,
    pub edit_note: String,
}

pub fn view(state: &ContactsState) -> Element<'_, Message> {
    let sidebar = addressbook_list(state);
    let contact_list = contact_list_view(state);
    let detail = contact_detail_view(state);

    row![
        container(sidebar).width(Length::Fixed(180.0)),
        container(contact_list).width(Length::Fixed(220.0)),
        container(detail).width(Length::Fill),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn addressbook_list(state: &ContactsState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    for book in &state.addressbooks {
        let selected = state.selected_addressbook.as_deref() == Some(book.id.as_str());
        let btn = Button::new(text(&book.name).size(13))
            .on_press(ContactsMessage::AddressbookSelected(book.id.clone()).into_message());
        let btn = if selected {
            btn.style(|theme: &iced::Theme, status| {
                let mut style = button::primary(theme, status);
                style.background = Some(iced::Background::Color(theme.palette().primary));
                style
            })
        } else {
            btn
        };
        col = col.push(btn);
    }

    scrollable(col).into()
}

fn contact_list_view(state: &ContactsState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    for contact in &state.contacts {
        let selected = state
            .selected_contact
            .as_ref()
            .map(|c| c.id == contact.id)
            .unwrap_or(false);
        let label = if contact.display_name.is_empty() {
            contact.uid.clone()
        } else {
            contact.display_name.clone()
        };
        let btn = Button::new(text(label).size(12))
            .on_press(ContactsMessage::ContactSelected(contact.id.clone()).into_message());
        let btn = if selected {
            btn.style(|theme: &iced::Theme, status| {
                let mut style = button::primary(theme, status);
                style.background = Some(iced::Background::Color(theme.palette().primary));
                style
            })
        } else {
            btn
        };
        col = col.push(btn);
    }

    scrollable(col).into()
}

fn contact_detail_view(state: &ContactsState) -> Element<'_, Message> {
    if state.editing {
        return edit_view(state);
    }

    let mut col = Column::new().spacing(8).padding(8);
    col = col.push(
        row![
            Button::new("New contact").on_press(ContactsMessage::NewContact.into_message()),
            Button::new("Import vCard").on_press(ContactsMessage::ImportVcf.into_message()),
            Button::new("Export vCard").on_press(ContactsMessage::ExportVcf.into_message()),
        ]
        .spacing(8),
    );

    if let Some(contact) = &state.selected_contact {
        col = col
            .push(text(&contact.display_name).size(16))
            .push(text(format!("Email: {}", contact.emails.join(", "))).size(12))
            .push(text(format!("Phone: {}", contact.phones.join(", "))).size(12))
            .push(text(format!("Organization: {}", contact.org)).size(12))
            .push(text(&contact.note).size(12))
            .push(
                row![
                    Button::new("Edit").on_press(ContactsMessage::NewContact.into_message()),
                    Button::new("Delete")
                        .on_press(ContactsMessage::DeleteContact.into_message()),
                ]
                .spacing(8),
            );
    } else {
        col = col.push(text("Select a contact").size(12));
    }

    scrollable(col).into()
}

fn edit_view(state: &ContactsState) -> Element<'_, Message> {
    column![
        text("Contact").size(16),
        text_input("Display name", &state.edit_display_name)
            .on_input(|v| ContactsMessage::DisplayNameChanged(v).into_message()),
        text_input("First name", &state.edit_first_name)
            .on_input(|v| ContactsMessage::FirstNameChanged(v).into_message()),
        text_input("Last name", &state.edit_last_name)
            .on_input(|v| ContactsMessage::LastNameChanged(v).into_message()),
        text_input("Email", &state.edit_email)
            .on_input(|v| ContactsMessage::EmailChanged(v).into_message()),
        text_input("Phone", &state.edit_phone)
            .on_input(|v| ContactsMessage::PhoneChanged(v).into_message()),
        text_input("Organization", &state.edit_org)
            .on_input(|v| ContactsMessage::OrgChanged(v).into_message()),
        text_input("Note", &state.edit_note)
            .on_input(|v| ContactsMessage::NoteChanged(v).into_message()),
        row![
            Button::new("Save").on_press(ContactsMessage::SaveContact.into_message()),
            Button::new("Cancel").on_press(ContactsMessage::CancelEdit.into_message()),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .padding(8)
    .into()
}
