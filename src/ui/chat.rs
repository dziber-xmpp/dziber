use iced::widget::{Column, Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length, Padding, Theme};

use crate::models::conversation::Conversation;
use crate::models::message::{Direction, MessageStatus};

use super::app::Message;

pub const CHAT_SCROLL_ID: &str = "chat_messages_scroll";

pub fn view<'a>(conversation: Option<&'a Conversation>, draft: &'a str) -> Element<'a, Message> {
    let Some(conv) = conversation else {
        return container(
            text("Select a conversation")
                .size(14)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into();
    };

    let header = container(
        row![
            text(conv.display_name()).size(16),
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center),
    )
    .padding(Padding::new(10.0))
    .width(Length::Fill);

    let messages: Element<Message> = if conv.messages.is_empty() {
        container(text("No messages yet").size(12).align_x(Alignment::Center))
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .padding(20)
            .into()
    } else {
        let mut list = Column::new().spacing(8).padding(10);
        for msg in &conv.messages {
            let is_outgoing = msg.direction == Direction::Outgoing;
            let status_icon = match msg.status {
                MessageStatus::Pending => "⏳",
                MessageStatus::Sent => "✓",
                MessageStatus::Delivered => "✓✓",
                MessageStatus::Received => "",
                MessageStatus::Error => "⚠",
            };

            let meta_row = if is_outgoing {
                row![
                    text(format!("{}", msg.timestamp.format("%H:%M"))).size(9),
                    text(status_icon).size(9),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
            } else {
                row![text(format!("{}", msg.timestamp.format("%H:%M"))).size(9),]
            };

            let bubble = container(column![text(&msg.body).size(13), meta_row,].spacing(2))
                .padding(8)
                .style(move |theme: &Theme| {
                    if is_outgoing {
                        container::background(theme.extended_palette().primary.strong.color)
                    } else {
                        container::background(theme.extended_palette().background.strong.color)
                    }
                });

            let align = if is_outgoing {
                row![Space::new().width(Length::Fill), bubble]
            } else {
                row![bubble, Space::new().width(Length::Fill)]
            };

            list = list.push(align);
        }
        scrollable(list)
            .id(CHAT_SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    let input = text_input("Type a message...", draft)
        .on_input(Message::DraftChanged)
        .on_submit(Message::SendMessageClicked)
        .padding(10)
        .width(Length::Fill);

    let send_button = button("Send")
        .on_press(Message::SendMessageClicked)
        .padding(10);

    let input_row = row![input, send_button]
        .spacing(8)
        .align_y(Alignment::Center);

    column![header, messages, input_row]
        .spacing(4)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
