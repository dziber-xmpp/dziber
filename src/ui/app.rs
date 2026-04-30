use std::collections::HashMap;
use std::path::PathBuf;

use futures::channel::mpsc;
use futures::sink::SinkExt;
use iced::widget::{Column, Space, button, image, row, text};
use iced::{Alignment, Element, Length, Subscription, Task, Theme, exit, stream, window};

use crate::models::account::{Account, ConnectionStatus};
use crate::models::contact::Contact;
use crate::models::conversation::Conversation;
use crate::models::message::{Direction, Message as ChatMessage};
use crate::ui::chat;
use crate::ui::conversation_list;
use crate::ui::login;
use crate::xmpp::{XmppCommand, XmppEvent};

fn avatar_cache_path(jid: &str) -> Option<PathBuf> {
    let safe_jid = jid.replace('/', "_");
    dirs::cache_dir().map(|p| p.join("dziber").join(safe_jid))
}

fn load_cached_avatar(jid: &str) -> Option<Vec<u8>> {
    let path = avatar_cache_path(jid)?;
    std::fs::read(path).ok()
}

fn save_cached_avatar(jid: &str, bytes: &[u8]) {
    let Some(path) = avatar_cache_path(jid) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, bytes);
}

fn sort_conversations(state: &mut AppState) {
    let selected_jid = state
        .selected_conversation
        .and_then(|idx| state.conversations.get(idx))
        .map(|c| c.contact_jid.clone());

    state.conversations.sort_by(|a, b| {
        let a_time = a
            .last_message()
            .map(|m| m.timestamp)
            .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);
        let b_time = b
            .last_message()
            .map(|m| m.timestamp)
            .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    if let Some(jid) = selected_jid {
        state.selected_conversation = state
            .conversations
            .iter()
            .position(|c| c.contact_jid == jid);
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    // UI events
    JidChanged(String),
    PasswordChanged(String),
    LoginClicked,
    LogoutClicked,
    ConversationSelected(usize),
    DraftChanged(String),
    SendMessageClicked,
    ToggleOmemo,
    ToggleOmemoQr,
    WindowOpened(window::Id),
    WindowCloseRequested(window::Id),
    TrayShowRequested,
    TrayQuitRequested,
    ConfirmQuitDiscard,
    ConfirmQuitCancel,

    // XMPP events
    XmppEvent(XmppEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug)]
pub struct AppState {
    pub screen: Screen,
    pub account: Option<Account>,
    pub jid_input: String,
    pub password_input: String,
    pub login_error: Option<String>,
    pub connection_status: String,

    pub xmpp_sender: Option<mpsc::Sender<XmppCommand>>,

    pub contacts: HashMap<String, Contact>,
    pub conversations: Vec<Conversation>,
    pub selected_conversation: Option<usize>,
    pub draft: String,
    pub omemo_enabled: bool,
    pub show_omemo_qr: bool,
    pub omemo_qr_uri: Option<String>,
    pub omemo_qr_handle: Option<iced::widget::image::Handle>,
    pub avatar_handles: HashMap<String, iced::widget::image::Handle>,
    pub main_window_id: Option<window::Id>,
    pub window_hidden_to_tray: bool,
    pub show_unsaved_quit_confirm: bool,
}

impl Default for AppState {
    fn default() -> Self {
        let mut state = Self {
            screen: Screen::Login,
            account: None,
            jid_input: String::new(),
            password_input: String::new(),
            login_error: None,
            connection_status: String::new(),
            xmpp_sender: None,
            contacts: HashMap::new(),
            conversations: Vec::new(),
            selected_conversation: None,
            draft: String::new(),
            omemo_enabled: true,
            show_omemo_qr: false,
            omemo_qr_uri: None,
            omemo_qr_handle: None,
            avatar_handles: HashMap::new(),
            main_window_id: None,
            window_hidden_to_tray: false,
            show_unsaved_quit_confirm: false,
        };

        if let Ok(config) = load_config() {
            state.jid_input = config.jid.clone();
            state.password_input = config.password.clone();
            state.account = Some(config);
        }

        state
    }
}

pub fn boot() -> (AppState, Task<Message>) {
    (AppState::default(), Task::none())
}

pub fn update(state: &mut AppState, message: Message) -> Task<Message> {
    let has_unsaved_changes = |state: &AppState| !state.draft.trim().is_empty();

    match message {
        Message::JidChanged(jid) => {
            state.jid_input = jid;
            state.login_error = None;
            Task::none()
        }
        Message::PasswordChanged(password) => {
            state.password_input = password;
            state.login_error = None;
            Task::none()
        }
        Message::LoginClicked => {
            if state.jid_input.is_empty() || state.password_input.is_empty() {
                state.login_error = Some("JID and password are required".to_string());
                return Task::none();
            }

            if let Some(ref mut sender) = state.xmpp_sender {
                let jid = state.jid_input.clone();
                let password = state.password_input.clone();
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender.send(XmppCommand::Connect { jid, password }).await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }

            state.login_error = Some("XMPP worker not ready".to_string());
            Task::none()
        }
        Message::LogoutClicked => {
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender.send(XmppCommand::Disconnect).await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::ConversationSelected(idx) => {
            state.selected_conversation = Some(idx);
            let selected_jid = state.conversations.get(idx).map(|c| c.contact_jid.clone());
            if let Some(conv) = state.conversations.get_mut(idx) {
                conv.mark_read();
            }
            if let Some(jid) = selected_jid
                && let Some(ref mut sender) = state.xmpp_sender
            {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::FetchAvatar { jid: jid.clone() })
                            .await;
                        let _ = sender.send(XmppCommand::FetchDeviceList { jid }).await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::DraftChanged(text) => {
            state.draft = text;
            Task::none()
        }
        Message::SendMessageClicked => {
            let body = state.draft.trim();
            if body.is_empty() {
                return Task::none();
            }

            let Some(idx) = state.selected_conversation else {
                return Task::none();
            };
            let Some(conv) = state.conversations.get(idx) else {
                return Task::none();
            };
            let to = conv.contact_jid.clone();
            let body = body.to_string();
            let omemo = state.omemo_enabled;
            let account_jid = state
                .account
                .as_ref()
                .map(|a| a.jid.clone())
                .unwrap_or_default();

            let msg = ChatMessage::new(account_jid.clone(), body.clone(), Direction::Outgoing);
            let msg_id = msg.id.clone();

            if let Err(e) = crate::db::save_message(&msg, &account_jid, &to) {
                eprintln!("Failed to save message: {}", e);
            }

            if let Some(conv) = state.conversations.get_mut(idx) {
                conv.add_message(msg);
            }
            state.draft.clear();
            sort_conversations(state);

            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::SendMessage { to, body, omemo })
                            .await;
                        msg_id
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }

            Task::none()
        }
        Message::ToggleOmemo => {
            state.omemo_enabled = !state.omemo_enabled;
            Task::none()
        }
        Message::ToggleOmemoQr => {
            state.show_omemo_qr = !state.show_omemo_qr;
            if state.show_omemo_qr
                && state.omemo_qr_handle.is_none()
                && let Some(jid) = state.account.as_ref().map(|a| a.jid.clone())
                && let Some(uri) = crate::ui::omemo_qr::build_share_uri(&jid)
                && let Some((w, h, rgba)) = crate::ui::omemo_qr::build_qr_rgba(&uri)
            {
                state.omemo_qr_uri = Some(uri);
                state.omemo_qr_handle = Some(iced::widget::image::Handle::from_rgba(w, h, rgba));
            }
            Task::none()
        }
        Message::WindowOpened(id) => {
            if state.main_window_id.is_none() {
                state.main_window_id = Some(id);
            }
            Task::none()
        }
        Message::WindowCloseRequested(id) => {
            state.main_window_id = Some(id);
            state.window_hidden_to_tray = true;
            window::set_mode(id, window::Mode::Hidden)
        }
        Message::TrayShowRequested => {
            if let Some(id) = state.main_window_id {
                state.window_hidden_to_tray = false;
                return Task::batch([
                    window::set_mode(id, window::Mode::Windowed),
                    window::gain_focus(id),
                ]);
            }
            Task::none()
        }
        Message::TrayQuitRequested => {
            if has_unsaved_changes(state) {
                state.show_unsaved_quit_confirm = true;
                Task::none()
            } else {
                exit()
            }
        }
        Message::ConfirmQuitDiscard => exit(),
        Message::ConfirmQuitCancel => {
            state.show_unsaved_quit_confirm = false;
            Task::none()
        }
        Message::XmppEvent(event) => match event {
            XmppEvent::Ready(sender) => {
                state.xmpp_sender = Some(sender);
                if let Err(e) = crate::db::run_migrations() {
                    eprintln!("Database migration failed: {}", e);
                }
                if let Some(account) = &state.account {
                    let jid = account.jid.clone();
                    let password = account.password.clone();
                    if let Some(ref mut s) = state.xmpp_sender {
                        let mut s = s.clone();
                        return Task::perform(
                            async move {
                                let _ = s.send(XmppCommand::Connect { jid, password }).await;
                            },
                            |_| Message::JidChanged(String::new()),
                        )
                        .discard();
                    }
                }
                Task::none()
            }
            XmppEvent::Connected { jid } => {
                state.screen = Screen::Main;
                state.connection_status = format!("Connected as {}", jid);
                if let Some(ref mut account) = state.account {
                    account.status = ConnectionStatus::Online;
                    let _ = save_config(account);
                } else {
                    let account = Account::new(jid.clone(), state.password_input.clone());
                    let _ = save_config(&account);
                    state.account = Some(account);
                }

                // Load message history from database
                state.conversations.clear();
                match crate::db::load_messages(&jid) {
                    Ok(msgs) => {
                        eprintln!("[UI] Loaded {} messages from local DB", msgs.len());
                        let account_jid = jid.clone();
                        for (contact_jid, msg) in msgs {
                            if let Some(conv) = state
                                .conversations
                                .iter_mut()
                                .find(|c| c.contact_jid == contact_jid)
                            {
                                conv.messages.push(msg);
                            } else {
                                let mut conv =
                                    Conversation::new(contact_jid.clone(), account_jid.clone());
                                conv.name = state
                                    .contacts
                                    .get(&contact_jid)
                                    .map(|c| c.display_name().to_string());
                                conv.messages.push(msg);
                                state.conversations.push(conv);
                            }
                        }
                        // Recompute unread counts from loaded messages
                        for conv in &mut state.conversations {
                            conv.unread_count = conv
                                .messages
                                .iter()
                                .filter(|m| m.direction == Direction::Incoming)
                                .count();
                        }
                        sort_conversations(state);
                    }
                    Err(e) => eprintln!("[UI] Failed to load history: {}", e),
                }

                Task::none()
            }
            XmppEvent::Disconnected => {
                state.connection_status = "Disconnected".to_string();
                if let Some(ref mut account) = state.account {
                    account.status = ConnectionStatus::Offline;
                }
                state.screen = Screen::Login;
                state.contacts.clear();
                state.selected_conversation = None;
                state.omemo_enabled = false;
                state.show_omemo_qr = false;
                state.omemo_qr_uri = None;
                state.omemo_qr_handle = None;
                state.avatar_handles.clear();
                Task::none()
            }
            XmppEvent::ConnectionError(err) => {
                state.connection_status = format!("Error: {}", err);
                if state.screen == Screen::Login {
                    state.login_error = Some(err);
                }
                if let Some(ref mut account) = state.account {
                    account.status = ConnectionStatus::Error(state.connection_status.clone());
                }
                Task::none()
            }
            XmppEvent::StatusChanged(status) => {
                state.connection_status = status;
                Task::none()
            }
            XmppEvent::RosterItem(contact) => {
                let jid = contact.jid.clone();
                let display_name = contact.display_name().to_string();
                eprintln!("[UI] RosterItem: jid={} name={:?}", jid, display_name);
                state.contacts.insert(jid.clone(), contact);
                if let Some(bytes) = load_cached_avatar(&jid) {
                    let handle = iced::widget::image::Handle::from_bytes(bytes);
                    state.avatar_handles.insert(jid.clone(), handle);
                }

                if !state.conversations.iter().any(|c| c.contact_jid == jid) {
                    let account_jid = state
                        .account
                        .as_ref()
                        .map(|a| a.jid.clone())
                        .unwrap_or_default();
                    let mut conv = Conversation::new(jid.clone(), account_jid);
                    conv.name = Some(display_name);
                    state.conversations.push(conv);
                    sort_conversations(state);
                }

                Task::none()
            }
            XmppEvent::PresenceUpdate { jid, presence } => {
                if let Some(contact) = state.contacts.get_mut(&jid) {
                    contact.presence = presence;
                }
                Task::none()
            }
            XmppEvent::MessageReceived(msg) => {
                let from_bare = msg.from.split('/').next().unwrap_or(&msg.from).to_string();
                let account_jid = state
                    .account
                    .as_ref()
                    .map(|a| a.jid.clone())
                    .unwrap_or_default();

                if let Err(e) = crate::db::save_message(&msg, &account_jid, &from_bare) {
                    eprintln!("Failed to save message: {}", e);
                }

                let conv_idx = state
                    .conversations
                    .iter()
                    .position(|c| c.contact_jid == from_bare);

                match conv_idx {
                    Some(idx) => {
                        state.conversations[idx].add_message(msg);
                    }
                    None => {
                        let mut conv = Conversation::new(from_bare.clone(), account_jid);
                        conv.name = state
                            .contacts
                            .get(&from_bare)
                            .map(|c| c.display_name().to_string());
                        conv.add_message(msg);
                        state.conversations.push(conv);
                    }
                }

                sort_conversations(state);

                Task::none()
            }
            XmppEvent::MessageSent { .. } => Task::none(),
            XmppEvent::OmemoMessageReceived {
                from,
                body,
                direction,
            } => {
                let account_jid = state
                    .account
                    .as_ref()
                    .map(|a| a.jid.clone())
                    .unwrap_or_default();
                let msg = ChatMessage::new(from.clone(), body, direction);

                if let Err(e) = crate::db::save_message(&msg, &account_jid, &from) {
                    eprintln!("Failed to save message: {}", e);
                }

                let conv_idx = state
                    .conversations
                    .iter()
                    .position(|c| c.contact_jid == from);
                match conv_idx {
                    Some(idx) => {
                        state.conversations[idx].add_message(msg);
                    }
                    None => {
                        let mut conv = Conversation::new(from.clone(), account_jid);
                        conv.name = state
                            .contacts
                            .get(&from)
                            .map(|c| c.display_name().to_string());
                        conv.add_message(msg);
                        state.conversations.push(conv);
                    }
                }

                sort_conversations(state);
                Task::none()
            }
            XmppEvent::BundleReceived => Task::none(),
            XmppEvent::AvatarReceived { jid, bytes } => {
                eprintln!("[UI] AvatarReceived: jid={} bytes={}", jid, bytes.len());
                save_cached_avatar(&jid, &bytes);
                let handle = iced::widget::image::Handle::from_bytes(bytes);
                state.avatar_handles.insert(jid, handle);
                Task::none()
            }
        },
    }
}

pub fn view(state: &AppState) -> Element<'_, Message> {
    if state.show_unsaved_quit_confirm {
        return Column::new()
            .padding(16)
            .spacing(12)
            .push(text("Unsaved draft detected"))
            .push(text("Discard your current message draft and quit Dziber?").size(13))
            .push(
                row![
                    button("Discard & Quit")
                        .on_press(Message::ConfirmQuitDiscard)
                        .padding(8),
                    button("Cancel")
                        .on_press(Message::ConfirmQuitCancel)
                        .padding(8),
                ]
                .spacing(8),
            )
            .into();
    }

    match state.screen {
        Screen::Login => login::view(
            &state.jid_input,
            &state.password_input,
            &state.login_error,
            &state.connection_status,
        ),
        Screen::Main => {
            let sidebar = conversation_list::view(
                &state.conversations,
                &state.avatar_handles,
                state.selected_conversation,
            );

            let selected_conv = state
                .selected_conversation
                .and_then(|idx| state.conversations.get(idx));
            let chat_view = chat::view(selected_conv, &state.draft);

            let content = row![sidebar, chat_view].spacing(0);

            let omemo_button = button(if state.omemo_enabled {
                "🔒 OMEMO ON"
            } else {
                "🔓 OMEMO OFF"
            })
            .on_press(Message::ToggleOmemo);

            let qr_button = button(if state.show_omemo_qr {
                "Hide OMEMO QR"
            } else {
                "Show OMEMO QR"
            })
            .on_press(Message::ToggleOmemoQr);

            let toolbar = row![
                text("Dziber").size(14),
                Space::new().width(Length::Fill),
                qr_button,
                omemo_button,
                text(&state.connection_status).size(11),
                button("Disconnect")
                    .on_press(Message::LogoutClicked)
                    .padding(4),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .padding(8);

            let mut root = Column::new().push(toolbar);

            if state.show_omemo_qr {
                let mut qr_col = Column::new()
                    .spacing(6)
                    .padding(8)
                    .push(text("Scan this in Conversations > Scan QR Code"));
                if let Some(handle) = &state.omemo_qr_handle {
                    qr_col = qr_col.push(
                        image(handle.clone())
                            .width(Length::Fixed(320.0))
                            .height(Length::Fixed(320.0)),
                    );
                }
                if let Some(uri) = &state.omemo_qr_uri {
                    qr_col = qr_col.push(text(uri).size(11));
                }
                root = root.push(qr_col);
            }

            root.push(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }
}

pub fn subscription(_state: &AppState) -> Subscription<Message> {
    let xmpp_sub = Subscription::run(crate::xmpp::run_xmpp_worker).map(Message::XmppEvent);
    let close_sub = window::close_requests().map(Message::WindowCloseRequested);
    let open_sub = window::open_events().map(Message::WindowOpened);
    let tray_sub = Subscription::run(|| {
        stream::channel(
            64,
            |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            loop {
                while let Some(event) = crate::tray::try_recv_event() {
                    let msg = match event {
                        crate::tray::TrayEvent::ShowRequested => Message::TrayShowRequested,
                        crate::tray::TrayEvent::QuitRequested => Message::TrayQuitRequested,
                    };
                    let _ = output.send(msg).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        },
        )
    });

    Subscription::batch([xmpp_sub, close_sub, open_sub, tray_sub])
}

pub fn theme(_state: &AppState) -> Theme {
    Theme::Dark
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ConfigFile {
    account: Account,
}

fn config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("dziber").join("config.json"))
}

fn load_config() -> Result<Account, Box<dyn std::error::Error>> {
    let path = config_path().ok_or("No config directory")?;
    let contents = std::fs::read_to_string(path)?;
    let config: ConfigFile = serde_json::from_str(&contents)?;
    Ok(config.account)
}

fn save_config(account: &Account) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path().ok_or("No config directory")?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let config = ConfigFile {
        account: account.clone(),
    };
    let contents = serde_json::to_string_pretty(&config)?;
    std::fs::write(path, contents)?;
    Ok(())
}
