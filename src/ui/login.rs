use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Element, Length};

use super::app::Message;

pub fn view<'a>(
    jid: &'a str,
    password: &'a str,
    error: &'a Option<String>,
    status: &'a str,
) -> Element<'a, Message> {
    let title = text("Dziber XMPP").size(32).align_x(Alignment::Center);

    let subtitle = text("Connect to your XMPP account")
        .size(14)
        .align_x(Alignment::Center);

    let jid_input = text_input("user@example.com", jid)
        .on_input(Message::JidChanged)
        .on_submit(Message::LoginClicked)
        .padding(10)
        .width(Length::Fill);

    let password_input = text_input("Password", password)
        .secure(true)
        .on_input(Message::PasswordChanged)
        .on_submit(Message::LoginClicked)
        .padding(10)
        .width(Length::Fill);

    let login_button = button("Connect")
        .on_press(Message::LoginClicked)
        .padding(10);

    let mut content = column![
        title,
        subtitle,
        Space::new().height(20),
        jid_input,
        password_input,
    ]
    .spacing(10)
    .padding(40)
    .max_width(400)
    .align_x(Alignment::Center);

    if !status.is_empty() {
        content = content.push(text(status).size(12));
    }

    if let Some(err) = error {
        content = content.push(text(err).size(12));
    }

    content = content.push(Space::new().height(10));
    content = content.push(row![Space::new().width(Length::Fill), login_button].spacing(10));

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}
