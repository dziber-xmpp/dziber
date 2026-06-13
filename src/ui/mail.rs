use iced::widget::{
    Button, Column, button, checkbox, column, container, row, scrollable, text, text_editor,
    text_input,
};
use iced::{Element, Length};

use crate::models::mail::{Email, Mailbox, MailFilter};
use crate::ui::app::Message;

#[derive(Debug, Clone)]
pub enum MailMessage {
    MailboxSelected(String),
    EmailSelected(String),
    ComposeClicked,
    SendClicked,
    CancelCompose,
    ToChanged(String),
    CcChanged(String),
    SubjectChanged(String),
    BodyChanged(String),
    MarkReadClicked(String, bool),
    DeleteClicked(String),
    FiltersClicked,
    MailboxViewClicked,
    FilterSelected(String),
    FilterNameChanged(String),
    FilterContentChanged(text_editor::Action),
    NewFilter,
    SaveFilter,
    DeleteFilter,
    ActivateFilter,
    CancelFilterEdit,
}

impl MailMessage {
    pub fn into_message(self) -> Message {
        Message::Mail(self)
    }
}

#[derive(Debug, Default)]
pub struct MailState {
    pub mailboxes: Vec<Mailbox>,
    pub emails: Vec<Email>,
    pub selected_mailbox: Option<String>,
    pub selected_email: Option<Email>,
    pub composing: bool,
    pub compose_to: String,
    pub compose_cc: String,
    pub compose_subject: String,
    pub compose_body: String,

    pub viewing_filters: bool,
    pub filters: Vec<MailFilter>,
    pub selected_filter: Option<MailFilter>,
    pub editing_filter: bool,
    pub filter_name: String,
    pub filter_content: text_editor::Content,
}

pub fn view(state: &MailState) -> Element<'_, Message> {
    if state.viewing_filters {
        filter_view(state)
    } else {
        mail_view(state)
    }
}

fn mail_view(state: &MailState) -> Element<'_, Message> {
    let sidebar = mailbox_list(state);
    let email_list = email_list_view(state);
    let detail = email_detail_view(state);

    row![
        container(sidebar).width(Length::Fixed(180.0)),
        container(email_list).width(Length::Fixed(250.0)),
        container(detail).width(Length::Fill),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn filter_view(state: &MailState) -> Element<'_, Message> {
    let sidebar = filter_list(state);
    let detail = filter_detail_view(state);

    row![
        container(sidebar).width(Length::Fixed(220.0)),
        container(detail).width(Length::Fill),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn mailbox_list(state: &MailState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    col = col.push(
        Button::new("Filters").on_press(MailMessage::FiltersClicked.into_message()),
    );

    for mb in &state.mailboxes {
        let selected = state.selected_mailbox.as_deref() == Some(mb.id.as_str());
        let label = if mb.unread_emails > 0 {
            format!("{} ({})", mb.name, mb.unread_emails)
        } else {
            mb.name.clone()
        };
        let btn = Button::new(text(label).size(13))
            .on_press(MailMessage::MailboxSelected(mb.id.clone()).into_message());
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

fn email_list_view(state: &MailState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    for email in &state.emails {
        let selected = state
            .selected_email
            .as_ref()
            .map(|e| e.id == email.id)
            .unwrap_or(false);
        let from = email
            .from
            .first()
            .map(|a| a.email.clone())
            .unwrap_or_default();
        let subject = if email.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            email.subject.clone()
        };
        let label = format!("{}\n{}", from, subject);
        let btn = Button::new(text(label).size(12))
            .on_press(MailMessage::EmailSelected(email.id.clone()).into_message());
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

fn email_detail_view(state: &MailState) -> Element<'_, Message> {
    if state.composing {
        return compose_view(state);
    }

    let mut col = Column::new().spacing(8).padding(8);
    col = col.push(
        Button::new("Compose")
            .on_press(MailMessage::ComposeClicked.into_message()),
    );

    if let Some(email) = &state.selected_email {
        let from = email
            .from
            .iter()
            .map(|a| a.email.clone())
            .collect::<Vec<_>>()
            .join(", ");
        col = col
            .push(text(format!("From: {}", from)).size(13))
            .push(text(format!("Subject: {}", email.subject)).size(14))
            .push(
                row![
                    Button::new(if email.is_read() { "Mark unread" } else { "Mark read" })
                        .on_press(MailMessage::MarkReadClicked(email.id.clone(), !email.is_read()).into_message()),
                    Button::new("Delete")
                        .on_press(MailMessage::DeleteClicked(email.id.clone()).into_message()),
                ]
                .spacing(8),
            );

        let body = email
            .body_text
            .as_deref()
            .or(email.preview.as_str().into())
            .unwrap_or("");
        col = col.push(text(body).size(12));
    } else {
        col = col.push(text("Select an email to read").size(12));
    }

    scrollable(col).into()
}

fn compose_view(state: &MailState) -> Element<'_, Message> {
    column![
        text("Compose Email").size(16),
        text_input("To", &state.compose_to)
            .on_input(|v| MailMessage::ToChanged(v).into_message()),
        text_input("Cc", &state.compose_cc)
            .on_input(|v| MailMessage::CcChanged(v).into_message()),
        text_input("Subject", &state.compose_subject)
            .on_input(|v| MailMessage::SubjectChanged(v).into_message()),
        text_input("Body", &state.compose_body)
            .on_input(|v| MailMessage::BodyChanged(v).into_message()),
        row![
            Button::new("Send").on_press(MailMessage::SendClicked.into_message()),
            Button::new("Cancel").on_press(MailMessage::CancelCompose.into_message()),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn filter_list(state: &MailState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(4).padding(8);

    col = col.push(
        Button::new("Back to mail").on_press(MailMessage::MailboxViewClicked.into_message()),
    );
    col = col.push(
        Button::new("New filter").on_press(MailMessage::NewFilter.into_message()),
    );

    for filter in &state.filters {
        let selected = state
            .selected_filter
            .as_ref()
            .map(|f| f.id == filter.id)
            .unwrap_or(false);
        let label = if filter.is_active {
            format!("{} (active)", filter.name)
        } else {
            filter.name.clone()
        };
        let btn = Button::new(text(label).size(13))
            .on_press(MailMessage::FilterSelected(filter.id.clone()).into_message());
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

fn filter_detail_view(state: &MailState) -> Element<'_, Message> {
    let mut col = Column::new().spacing(8).padding(8);

    if state.editing_filter {
        col = col
            .push(text("Filter name").size(12))
            .push(
                text_input("Filter name", &state.filter_name)
                    .on_input(|v| MailMessage::FilterNameChanged(v).into_message()),
            )
            .push(text("Sieve script").size(12))
            .push(
                scrollable(
                    text_editor(&state.filter_content)
                        .height(Length::Fill)
                        .on_action(|a| MailMessage::FilterContentChanged(a).into_message()),
                )
                .height(Length::Fixed(300.0)),
            )
            .push(
                row![
                    Button::new("Save").on_press(MailMessage::SaveFilter.into_message()),
                    Button::new("Cancel").on_press(MailMessage::CancelFilterEdit.into_message()),
                ]
                .spacing(8),
            );
    } else if let Some(filter) = &state.selected_filter {
        col = col
            .push(text(format!("Name: {}", filter.name)).size(14))
            .push(
                checkbox(filter.is_active)
                    .label("Active")
                    .on_toggle(|_| MailMessage::ActivateFilter.into_message()),
            )
            .push(
                scrollable(text(&filter.content).size(12))
                    .height(Length::Fixed(300.0)),
            )
            .push(
                row![
                    Button::new("Edit").on_press(MailMessage::NewFilter.into_message()),
                    Button::new("Delete").on_press(MailMessage::DeleteFilter.into_message()),
                ]
                .spacing(8),
            );
    } else {
        col = col.push(text("Select a filter or create a new one").size(12));
    }

    col.into()
}
