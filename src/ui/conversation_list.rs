use std::collections::HashMap;

use iced::widget::{Column, column, container, image, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding, Theme};

use crate::models::conversation::Conversation;

use super::app::Message;

pub fn view<'a>(
    conversations: &'a [Conversation],
    avatar_handles: &'a HashMap<String, iced::widget::image::Handle>,
    selected: Option<usize>,
) -> Element<'a, Message> {
    let header = container(text("Conversations").size(18))
        .padding(Padding::new(10.0))
        .width(Length::Fill);

    let items: Element<Message> = if conversations.is_empty() {
        container(
            text("No conversations yet")
                .size(12)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(20)
        .into()
    } else {
        let mut list = Column::new().spacing(2);
        for (idx, conv) in conversations.iter().enumerate() {
            let is_selected = selected == Some(idx);
            let name = conv.display_name().to_string();
            let last_msg = conv
                .last_message()
                .map(|m| {
                    let preview: String = m.body.chars().take(40).collect();
                    if m.body.len() > 40 {
                        format!("{}...", preview)
                    } else {
                        preview
                    }
                })
                .unwrap_or_else(|| "No messages".to_string());

            let name_row = if conv.unread_count > 0 {
                row![
                    text(name).size(14),
                    text(format!("({})", conv.unread_count)).size(12),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
            } else {
                row![text(name).size(14)]
            };

            let avatar: Element<Message> = match avatar_handles.get(&conv.contact_jid) {
                Some(handle) => image(handle.clone())
                    .width(Length::Fixed(32.0))
                    .height(Length::Fixed(32.0))
                    .border_radius(16.0)
                    .content_fit(iced::ContentFit::Cover)
                    .into(),
                None => {
                    let initial = conv
                        .display_name()
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string();
                    container(text(initial).size(14))
                        .width(Length::Fixed(32.0))
                        .height(Length::Fixed(32.0))
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center)
                        .style(|theme: &Theme| {
                            container::background(theme.extended_palette().primary.strong.color)
                        })
                        .into()
                }
            };

            let item = container(
                row![
                    avatar,
                    column![name_row, text(last_msg).size(11),]
                        .spacing(2)
                        .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(8)
            .width(Length::Fill)
            .style(move |theme: &Theme| {
                if is_selected {
                    container::background(theme.palette().primary)
                } else {
                    container::transparent(theme)
                }
            });

            let clickable =
                iced::widget::mouse_area(item).on_press(Message::ConversationSelected(idx));

            list = list.push(clickable);
        }
        scrollable(list).into()
    };

    column![header, items]
        .spacing(4)
        .width(Length::Fixed(280.0))
        .height(Length::Fill)
        .into()
}
