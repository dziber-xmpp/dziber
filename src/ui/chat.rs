use std::collections::HashMap;

use iced::widget::{
    Column, Space, button, column, container, row, scrollable, text, text_editor, text_input,
};
use iced::{Alignment, Background, Color, Element, Length, Padding, Theme};
use iced::widget::text::Wrapping;

use crate::models::conversation::Conversation;
use crate::models::message::{Direction, MessageStatus};

use super::app::Message;

pub const CHAT_SCROLL_ID: &str = "chat_messages_scroll";

fn body_width_px(body: &str) -> u32 {
    let longest_line = body.lines().map(|l| l.chars().count()).max().unwrap_or(1) as u32;
    // Approximate glyph width at size 13; clamp so short messages stay narrow
    // and long messages wrap instead of clipping.
    (longest_line.saturating_mul(8) + 16).clamp(72, 520)
}

fn parse_file_message(body: &str) -> Option<(String, String)> {
    let mut lines = body.lines();
    let first = lines.next()?.trim();
    let second = lines.next()?.trim();
    if lines.next().is_some() {
        return None;
    }
    let name = first.strip_prefix("📎 ")?.trim();
    if name.is_empty() {
        return None;
    }
    if !(second.starts_with("http://")
        || second.starts_with("https://")
        || second.starts_with("aesgcm://"))
    {
        return None;
    }
    Some((name.to_string(), second.to_string()))
}

pub fn view<'a>(
    conversation: Option<&'a Conversation>,
    draft: &'a str,
    chat_message_bodies: &'a HashMap<String, text_editor::Content>,
    avatar_handles: &'a HashMap<String, iced::widget::image::Handle>,
    active_call_with: Option<&str>,
) -> Element<'a, Message> {
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

    let in_call = active_call_with.is_some_and(|jid| jid == conv.contact_jid);
    let call_button = if in_call {
        button("Hang Up").on_press(Message::EndCallClicked).padding(8)
    } else {
        button("Call").on_press(Message::StartCallClicked).padding(8)
    };

    let header = container(
        row![
            text(conv.display_name()).size(16),
            Space::new().width(Length::Fill),
            call_button,
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
            let group_sender = if !is_outgoing {
                msg.from
                    .strip_prefix(&(conv.contact_jid.clone() + "/"))
                    .map(std::string::ToString::to_string)
            } else {
                None
            };
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

            let body_editor: Element<Message> = if msg.direction == Direction::Incoming {
                if let Some((filename, url)) = parse_file_message(&msg.body) {
                    button(text(filename.clone()).size(13))
                        .on_press(Message::DownloadFileClicked {
                            url,
                            filename: filename.clone(),
                        })
                        .padding(Padding {
                            top: 2.0,
                            right: 4.0,
                            bottom: 2.0,
                            left: 4.0,
                        })
                        .into()
                } else if let Some(content) = chat_message_bodies.get(&msg.id) {
                    let width = body_width_px(&msg.body);
                    text_editor(content)
                        .on_action({
                            let id = msg.id.clone();
                            move |action| Message::ChatMessageBodyAction {
                                message_id: id.clone(),
                                action,
                            }
                        })
                        .style(|theme, status| {
                            let mut style = text_editor::default(theme, status);
                            style.background = Background::Color(Color::TRANSPARENT);
                            style.border.width = 0.0;
                            style
                        })
                        .width(width)
                        .size(13)
                        .wrapping(Wrapping::Word)
                        .min_height(18)
                        .padding(Padding {
                            top: 1.0,
                            right: 2.0,
                            bottom: 1.0,
                            left: 2.0,
                        })
                        .height(Length::Shrink)
                        .into()
                } else {
                    text(&msg.body).size(13).into()
                }
            } else if let Some(content) = chat_message_bodies.get(&msg.id) {
                let width = body_width_px(&msg.body);
                text_editor(content)
                    .on_action({
                        let id = msg.id.clone();
                        move |action| Message::ChatMessageBodyAction {
                            message_id: id.clone(),
                            action,
                        }
                    })
                    .style(|theme, status| {
                        let mut style = text_editor::default(theme, status);
                        style.background = Background::Color(Color::TRANSPARENT);
                        style.border.width = 0.0;
                        style
                    })
                    .width(width)
                    .size(13)
                    .wrapping(Wrapping::Word)
                    .min_height(18)
                    .padding(Padding {
                        top: 1.0,
                        right: 2.0,
                        bottom: 1.0,
                        left: 2.0,
                    })
                    .height(Length::Shrink)
                    .into()
            } else {
                text(&msg.body).size(13).into()
            };

            let bubble_content = if let Some(sender) = &group_sender {
                column![text(sender.clone()).size(10), body_editor, meta_row].spacing(2)
            } else {
                column![body_editor, meta_row].spacing(2)
            };

            let bubble = container(bubble_content)
                .padding(8)
                .width(Length::Shrink)
                .style(move |theme: &Theme| {
                    if is_outgoing {
                        container::background(theme.extended_palette().primary.strong.color)
                    } else {
                        container::background(theme.extended_palette().background.strong.color)
                    }
                });

            let sender_avatar: Option<Element<Message>> = group_sender.as_ref().map(|sender| {
                let sender_bare = sender.split('/').next().unwrap_or(sender);
                match avatar_handles.get(sender_bare) {
                    Some(handle) => iced::widget::image(handle.clone())
                        .width(Length::Fixed(24.0))
                        .height(Length::Fixed(24.0))
                        .border_radius(12.0)
                        .content_fit(iced::ContentFit::Cover)
                        .into(),
                    None => {
                        let initial = sender
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .to_string();
                        container(text(initial).size(11))
                            .width(Length::Fixed(24.0))
                            .height(Length::Fixed(24.0))
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center)
                            .style(|theme: &Theme| {
                                container::background(theme.extended_palette().primary.strong.color)
                            })
                            .into()
                    }
                }
            });

            let align = if is_outgoing {
                row![Space::new().width(Length::Fill), bubble]
            } else if let Some(avatar) = sender_avatar {
                row![avatar, bubble, Space::new().width(Length::Fill)]
                    .spacing(6)
                    .align_y(Alignment::Start)
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
    let send_file_button = button("Send File")
        .on_press(Message::SendFileClicked)
        .padding(10);

    let input_row = row![input, send_button, send_file_button]
        .spacing(8)
        .align_y(Alignment::Center);

    column![header, messages, input_row]
        .spacing(4)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
