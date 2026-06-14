use std::collections::HashMap;
use std::path::PathBuf;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use futures::channel::mpsc;
use futures::sink::SinkExt;
use chrono::Datelike;
use iced::widget::operation;
use iced::widget::{Column, Space, button, container, image, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task, Theme, stream, window};

use crate::audio::{AudioCallState, AudioEngine};
use crate::models::account::{Account, CalendarAccount, ConnectionStatus, ContactsAccount, MailAccount};
use crate::models::contact::Contact;
use crate::models::contact_card::ContactCard;
use crate::models::conversation::Conversation;
use crate::models::event::CalendarEvent;
use crate::models::mail::MailFilter;
use crate::models::message::{Direction, Message as ChatMessage};
use crate::models::task::CalendarTask;
use crate::ui::calendar::{self, CalendarMessage, CalendarState};
use crate::ui::chat;
use crate::ui::contacts::{self, ContactsMessage, ContactsState};
use crate::ui::conversation_list;
use crate::ui::login;
use crate::ui::mail::{self, MailMessage, MailState};
use crate::ui::settings::{self, SettingsMessage, SettingsState};
use crate::xmpp::{CallRejectReason, ChatState, IceCandidate, XmppCommand, XmppEvent};

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

fn omemo_pref_for(account: Option<&Account>, jid: &str) -> bool {
    account
        .and_then(|a| a.omemo_prefs.get(jid).copied())
        .unwrap_or(true)
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
    SendFileClicked,
    StartCallClicked,
    EndCallClicked,
    AcceptIncomingCallClicked,
    DeclineIncomingCallClicked,
    DownloadFileClicked {
        url: String,
        filename: String,
    },
    FileDownloadFinished(Result<String, String>),
    ToggleOmemo,
    ToggleOmemoQr,
    WindowOpened(window::Id),
    WindowCloseRequested(window::Id),
    TrayShowRequested,
    TrayQuitRequested,
    ConfirmQuitDiscard,
    ConfirmQuitCancel,
    ChatMessageBodyAction {
        message_id: String,
        action: text_editor::Action,
    },

    // Personal data (mail, contacts, calendar)
    Settings(SettingsMessage),
    SavePersonalDataConfig,
    ClearPersonalDataConfig,
    Mail(MailMessage),
    Contacts(ContactsMessage),
    Calendar(CalendarMessage),
    TabSelected(Tab),
    SyncPersonalData,
    PersonalDataEvent(crate::personal_data::PersonalDataEvent),

    // XMPP events
    XmppEvent(XmppEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Chat,
    Mail,
    Contacts,
    Calendar,
    Settings,
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
    pub chat_message_bodies: HashMap<String, text_editor::Content>,
    pub audio_engine: AudioEngine,
    pub active_call_with: Option<String>,
    pub active_call_sid: Option<String>,
    pub pending_incoming_call: Option<(String, String)>,

    pub current_tab: Tab,
    pub settings_state: SettingsState,
    pub mail_state: MailState,
    pub contacts_state: ContactsState,
    pub calendar_state: CalendarState,
    pub mail_account: Option<MailAccount>,
    pub contacts_account: Option<ContactsAccount>,
    pub calendar_account: Option<CalendarAccount>,
    pub personal_data_status: String,
}

fn refresh_chat_message_bodies(state: &mut AppState) {
    let Some(idx) = state.selected_conversation else {
        state.chat_message_bodies.clear();
        return;
    };
    let Some(conv) = state.conversations.get(idx) else {
        state.chat_message_bodies.clear();
        return;
    };

    let mut next = HashMap::with_capacity(conv.messages.len());
    for msg in &conv.messages {
        let content = state
            .chat_message_bodies
            .remove(&msg.id)
            .unwrap_or_else(|| text_editor::Content::with_text(&msg.body));
        next.insert(msg.id.clone(), content);
    }
    state.chat_message_bodies = next;
}

fn total_unread_count(state: &AppState) -> u32 {
    let jabber: usize = state.conversations.iter().map(|c| c.unread_count).sum();
    let mail: usize = state
        .mail_state
        .mailboxes
        .iter()
        .map(|mb| mb.unread_emails as usize)
        .sum();
    jabber.saturating_add(mail).min(u32::MAX as usize) as u32
}

fn update_tray_unread(state: &AppState) {
    crate::tray::set_unread_count(total_unread_count(state));
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
            window_hidden_to_tray: true,
            show_unsaved_quit_confirm: false,
            chat_message_bodies: HashMap::new(),
            audio_engine: AudioEngine::new(),
            active_call_with: None,
            active_call_sid: None,
            pending_incoming_call: None,
            current_tab: Tab::Chat,
            settings_state: SettingsState::default(),
            mail_state: MailState::default(),
            contacts_state: ContactsState::default(),
            calendar_state: CalendarState::default(),
            mail_account: None,
            contacts_account: None,
            calendar_account: None,
            personal_data_status: String::new(),
        };

        if let Ok(config) = load_config() {
            state.jid_input = config.jid.clone();
            state.password_input = config.password.clone();
            if let Some(personal) = config.personal_data.as_ref() {
                let _ = migrate_personal_data_config(personal);
            }
            state.account = Some(config);
        }

        load_personal_data_accounts(&mut state);
        load_personal_data_from_db(&mut state);

        state
    }
}

fn load_personal_data_accounts(state: &mut AppState) {
    state.mail_account = crate::db::mail::load_accounts().ok().and_then(|v| v.into_iter().next());
    state.contacts_account = crate::db::contacts::load_accounts()
        .ok()
        .and_then(|v| v.into_iter().next());
    state.calendar_account = crate::db::calendar::load_accounts()
        .ok()
        .and_then(|v| v.into_iter().next());
    state.settings_state = SettingsState::from_configs(
        state.mail_account.as_ref(),
        state.contacts_account.as_ref(),
        state.calendar_account.as_ref(),
    );
}

fn migrate_personal_data_config(
    personal: &crate::models::account::PersonalDataConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::models::account::{CalendarAccount, ContactsAccount, MailAccount};

    let id = uuid::Uuid::new_v4().to_string();
    let mail = MailAccount {
        id: id.clone(),
        server_url: personal.server_url.clone(),
        username: personal.username.clone(),
        password: personal.password.clone(),
        auth_mode: personal.auth_mode.clone(),
        mail_protocol: personal.mail_protocol.clone(),
        sieve_config: None,
    };
    let contacts = ContactsAccount {
        id: id.clone(),
        server_url: personal.server_url.clone(),
        username: personal.username.clone(),
        password: personal.password.clone(),
        auth_mode: personal.auth_mode.clone(),
        contacts_protocol: personal.contacts_protocol.clone(),
    };
    let calendar = CalendarAccount {
        id,
        server_url: personal.server_url.clone(),
        username: personal.username.clone(),
        password: personal.password.clone(),
        auth_mode: personal.auth_mode.clone(),
        calendar_protocol: personal.calendar_protocol.clone(),
    };

    crate::db::mail::save_account(&mail)?;
    crate::db::contacts::save_account(&contacts)?;
    crate::db::calendar::save_account(&calendar)?;
    Ok(())
}

fn load_personal_data_from_db(state: &mut AppState) {
    if let Some(account) = state.mail_account.as_ref() {
        state.mail_state.mailboxes = crate::db::mail::load_mailboxes(&account.id).unwrap_or_default();
        state.mail_state.emails = crate::db::mail::load_emails(&account.id, None).unwrap_or_default();
        state.mail_state.filters = crate::db::mail::load_filters(&account.id).unwrap_or_default();
    }
    if let Some(account) = state.contacts_account.as_ref() {
        state.contacts_state.addressbooks = crate::db::contacts::load_addressbooks(&account.id)
            .unwrap_or_default();
        state.contacts_state.contacts = crate::db::contacts::load_contacts(&account.id, None)
            .unwrap_or_default();
    }
    if let Some(account) = state.calendar_account.as_ref() {
        state.calendar_state.calendars = crate::db::calendar::load_calendars(&account.id)
            .unwrap_or_default();
        state.calendar_state.events = crate::db::calendar::load_events(&account.id, None)
            .unwrap_or_default();
        state.calendar_state.tasks = crate::db::calendar::load_tasks(&account.id, None)
            .unwrap_or_default();
    }
    update_tray_unread(state);
}

pub fn boot() -> (AppState, Task<Message>) {
    let state = AppState::default();

    crate::tray::set_unread_count(total_unread_count(&state));
    crate::tray::init_tray();

    let task = if state.mail_account.is_some()
        || state.contacts_account.is_some()
        || state.calendar_account.is_some()
    {
        Task::done(Message::SyncPersonalData)
    } else {
        Task::none()
    };
    (state, task)
}

pub fn update(state: &mut AppState, message: Message) -> Task<Message> {
    let has_unsaved_changes = |state: &AppState| !state.draft.trim().is_empty();
    let scroll_chat_to_end = || operation::snap_to_end(crate::ui::chat::CHAT_SCROLL_ID);
    let should_scroll_selected = |state: &AppState, jid: &str| {
        state
            .selected_conversation
            .and_then(|idx| state.conversations.get(idx))
            .is_some_and(|conv| conv.contact_jid == jid)
    };

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
            if let Some(jid) = state.conversations.get(idx).map(|c| c.contact_jid.as_str()) {
                state.omemo_enabled = omemo_pref_for(state.account.as_ref(), jid);
            }
            refresh_chat_message_bodies(state);
            let selected_jid = state.conversations.get(idx).map(|c| c.contact_jid.clone());
            if let Some(conv) = state.conversations.get_mut(idx) {
                conv.mark_read();
            }
            update_tray_unread(state);
            if let Some(jid) = selected_jid
                && let Some(ref mut sender) = state.xmpp_sender
            {
                let mut sender = sender.clone();
                let fetch_task = Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::FetchAvatar { jid: jid.clone() })
                            .await;
                        let _ = sender.send(XmppCommand::FetchDeviceList { jid }).await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
                return Task::batch([fetch_task, scroll_chat_to_end()]);
            }
            scroll_chat_to_end()
        }
        Message::DraftChanged(text) => {
            state.draft = text;
            if let Some(idx) = state.selected_conversation
                && let Some(conv) = state.conversations.get(idx)
                && let Some(ref mut sender) = state.xmpp_sender
            {
                let to = conv.contact_jid.clone();
                let state_val = if state.draft.trim().is_empty() {
                    ChatState::Active
                } else {
                    ChatState::Composing
                };
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::SendChatState {
                                to,
                                state: state_val,
                            })
                            .await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::ChatMessageBodyAction { message_id, action } => {
            if !action.is_edit()
                && let Some(content) = state.chat_message_bodies.get_mut(&message_id) {
                    content.perform(action);
                }
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

            if body.starts_with("/edit ") {
                let rest = body.trim_start_matches("/edit ").to_string();
                let mut parts = rest.splitn(2, ' ');
                let Some(target_id) = parts.next() else {
                    return Task::none();
                };
                let Some(new_body) = parts.next() else {
                    return Task::none();
                };
                let target_id_owned = target_id.to_string();
                let new_body_owned = new_body.to_string();
                let correction_id = uuid::Uuid::new_v4().to_string();
                let corrected = ChatMessage {
                    id: target_id_owned.clone(),
                    from: account_jid.clone(),
                    body: new_body_owned.clone(),
                    timestamp: chrono::Utc::now(),
                    status: crate::models::message::MessageStatus::Sent,
                    direction: Direction::Outgoing,
                };
                if let Some(conv) = state.conversations.get_mut(idx)
                    && let Some(existing) = conv
                        .messages
                        .iter_mut()
                        .find(|m| m.id == target_id_owned)
                {
                    *existing = corrected.clone();
                }
                let _ = crate::db::update_message_body(&target_id_owned, &new_body_owned);
                refresh_chat_message_bodies(state);
                state.draft.clear();
                if let Some(ref mut sender) = state.xmpp_sender {
                    let mut sender = sender.clone();
                    return Task::perform(
                        async move {
                            let _ = sender
                                .send(XmppCommand::SendMessageCorrection {
                                    id: correction_id,
                                    to,
                                    replace_id: target_id_owned,
                                    body: new_body_owned,
                                })
                                .await;
                        },
                        |_| Message::JidChanged(String::new()),
                    )
                    .discard();
                }
                return Task::none();
            }

            let msg = ChatMessage::new(account_jid.clone(), body.clone(), Direction::Outgoing);
            let msg_id = msg.id.clone();

            if let Err(e) = crate::db::save_message(&msg, &account_jid, &to) {
                tracing::info!("Failed to save message: {}", e);
            }

            if let Some(conv) = state.conversations.get_mut(idx) {
                conv.add_message(msg);
            }
            refresh_chat_message_bodies(state);
            state.draft.clear();
            sort_conversations(state);

            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                let send_task = Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::SendMessage {
                                id: msg_id.clone(),
                                to,
                                body,
                                omemo,
                            })
                            .await;
                        msg_id
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
                return Task::batch([send_task, scroll_chat_to_end()]);
            }

            scroll_chat_to_end()
        }
        Message::SendFileClicked => {
            let Some(idx) = state.selected_conversation else {
                return Task::none();
            };
            let Some(conv) = state.conversations.get(idx) else {
                return Task::none();
            };
            let to = conv.contact_jid.clone();
            let Some(path) = rfd::FileDialog::new().pick_file() else {
                return Task::none();
            };
            let path_string = path.to_string_lossy().to_string();
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::SendFile {
                                to,
                                path: path_string,
                            })
                            .await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::StartCallClicked => {
            let Some(idx) = state.selected_conversation else {
                return Task::none();
            };
            let Some(conv) = state.conversations.get(idx) else {
                return Task::none();
            };
            if state.audio_engine.state() == AudioCallState::Active {
                return Task::none();
            }
            let to = conv.contact_jid.clone();
            let sid = uuid::Uuid::new_v4().to_string();
            if state.audio_engine.start_call(&to, &sid).is_err() {
                state.connection_status = "Failed to start audio engine".to_string();
                return Task::none();
            }
            state.active_call_with = Some(to.clone());
            state.active_call_sid = Some(sid.clone());
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                let candidates = state
                    .audio_engine
                    .local_candidates()
                    .into_iter()
                    .map(|c| IceCandidate {
                        foundation: c.foundation,
                        component: c.component,
                        protocol: c.protocol,
                        priority: c.priority,
                        ip: c.ip,
                        port: c.port,
                        typ: c.typ,
                    })
                    .collect::<Vec<_>>();
                return Task::perform(
                    async move {
                        let _ = sender.send(XmppCommand::InitiateCall { to: to.clone() }).await;
                        let _ = sender
                            .send(XmppCommand::SendTransportInfo {
                                with: to.clone(),
                                sid: sid.clone(),
                                candidates,
                            })
                            .await;
                        let _ = sender
                            .send(XmppCommand::SendTransportInfoEnd { with: to, sid })
                            .await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::EndCallClicked => {
            let (Some(with), Some(sid)) = (
                state.active_call_with.clone(),
                state.active_call_sid.clone(),
            ) else {
                return Task::none();
            };
            state.audio_engine.stop_call();
            state.active_call_with = None;
            state.active_call_sid = None;
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender.send(XmppCommand::EndCall { with, sid }).await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::AcceptIncomingCallClicked => {
            let Some((with, sid)) = state.pending_incoming_call.clone() else {
                return Task::none();
            };
            state.pending_incoming_call = None;
            if state.audio_engine.start_call(&with, &sid).is_err() {
                state.connection_status = "Failed to start audio engine".to_string();
                return Task::none();
            }
            state.active_call_with = Some(with.clone());
            state.active_call_sid = Some(sid.clone());
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                let candidates = state
                    .audio_engine
                    .local_candidates()
                    .into_iter()
                    .map(|c| IceCandidate {
                        foundation: c.foundation,
                        component: c.component,
                        protocol: c.protocol,
                        priority: c.priority,
                        ip: c.ip,
                        port: c.port,
                        typ: c.typ,
                    })
                    .collect::<Vec<_>>();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::AcceptCall {
                                with: with.clone(),
                                sid: sid.clone(),
                            })
                            .await;
                        let _ = sender
                            .send(XmppCommand::SendTransportInfo {
                                with: with.clone(),
                                sid: sid.clone(),
                                candidates,
                            })
                            .await;
                        let _ = sender
                            .send(XmppCommand::SendTransportInfoEnd { with, sid })
                            .await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::DeclineIncomingCallClicked => {
            let Some((with, sid)) = state.pending_incoming_call.take() else {
                return Task::none();
            };
            if let Some(ref mut sender) = state.xmpp_sender {
                let mut sender = sender.clone();
                return Task::perform(
                    async move {
                        let _ = sender
                            .send(XmppCommand::RejectCall {
                                with,
                                sid,
                                reason: CallRejectReason::Decline,
                            })
                            .await;
                    },
                    |_| Message::JidChanged(String::new()),
                )
                .discard();
            }
            Task::none()
        }
        Message::DownloadFileClicked { url, filename } => {
            let Some(path) = rfd::FileDialog::new().set_file_name(&filename).save_file() else {
                return Task::none();
            };
            state.connection_status = format!("Downloading {}...", filename);
            Task::perform(
                async move {
                    let bytes = if url.starts_with("aesgcm://") {
                        download_aesgcm_file(&url).await?
                    } else {
                        let response = reqwest::get(&url)
                            .await
                            .map_err(|e| format!("download failed: {}", e))?;
                        if !response.status().is_success() {
                            return Err(format!("download failed: HTTP {}", response.status()));
                        }
                        response
                            .bytes()
                            .await
                            .map_err(|e| format!("download failed: {}", e))?
                            .to_vec()
                    };
                    std::fs::write(&path, &bytes)
                        .map_err(|e| format!("save failed: {}", e))?;
                    Ok(path.to_string_lossy().to_string())
                },
                Message::FileDownloadFinished,
            )
        }
        Message::FileDownloadFinished(result) => {
            match result {
                Ok(path) => {
                    state.connection_status = format!("File saved: {}", path);
                }
                Err(err) => {
                    state.connection_status = format!("File download failed: {}", err);
                }
            }
            Task::none()
        }
        Message::ToggleOmemo => {
            state.omemo_enabled = !state.omemo_enabled;
            if let Some(idx) = state.selected_conversation
                && let Some(jid) = state.conversations.get(idx).map(|c| c.contact_jid.clone())
                && let Some(account) = state.account.as_mut()
            {
                account.omemo_prefs.insert(jid, state.omemo_enabled);
                let _ = save_config(account);
            }
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
            if has_unsaved_changes(state) {
                state.show_unsaved_quit_confirm = true;
                Task::none()
            } else {
                std::process::exit(0)
            }
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
                std::process::exit(0);
            }
        }
        Message::ConfirmQuitDiscard => {
            std::process::exit(0);
        }
        Message::ConfirmQuitCancel => {
            state.show_unsaved_quit_confirm = false;
            Task::none()
        }
        Message::Settings(msg) => match msg {
            SettingsMessage::SaveClicked => {
                Task::done(Message::SavePersonalDataConfig)
            }
            SettingsMessage::ClearClicked => {
                Task::done(Message::ClearPersonalDataConfig)
            }
            _ => {
                state.settings_state.update(msg);
                Task::none()
            }
        }
        Message::SavePersonalDataConfig => {
            let (mail, contacts, calendar) = state.settings_state.to_configs();

            let old_mail_id = state.mail_account.as_ref().map(|a| a.id.clone());
            let old_contacts_id = state.contacts_account.as_ref().map(|a| a.id.clone());
            let old_calendar_id = state.calendar_account.as_ref().map(|a| a.id.clone());

            state.mail_account = mail.clone();
            state.contacts_account = contacts.clone();
            state.calendar_account = calendar.clone();

            if let Some(account) = mail {
                let _ = crate::db::mail::save_account(&account);
            } else if let Some(id) = old_mail_id {
                let _ = crate::db::mail::delete_account(&id);
            }
            if let Some(account) = contacts {
                let _ = crate::db::contacts::save_account(&account);
            } else if let Some(id) = old_contacts_id {
                let _ = crate::db::contacts::delete_account(&id);
            }
            if let Some(account) = calendar {
                let _ = crate::db::calendar::save_account(&account);
            } else if let Some(id) = old_calendar_id {
                let _ = crate::db::calendar::delete_account(&id);
            }

            if state.mail_account.is_some()
                || state.contacts_account.is_some()
                || state.calendar_account.is_some()
            {
                state.personal_data_status = "Configuration saved; syncing...".to_string();
                return Task::done(Message::SyncPersonalData);
            }
            Task::none()
        }
        Message::ClearPersonalDataConfig => {
            if let Some(account) = state.mail_account.take() {
                let _ = crate::db::mail::delete_account(&account.id);
            }
            if let Some(account) = state.contacts_account.take() {
                let _ = crate::db::contacts::delete_account(&account.id);
            }
            if let Some(account) = state.calendar_account.take() {
                let _ = crate::db::calendar::delete_account(&account.id);
            }
            state.settings_state = SettingsState::default();
            state.personal_data_status = "Configuration cleared".to_string();
            Task::none()
        }
        Message::TabSelected(tab) => {
            state.current_tab = tab;
            Task::none()
        }
        Message::SyncPersonalData => {
            let mail = state.mail_account.clone();
            let contacts = state.contacts_account.clone();
            let calendar = state.calendar_account.clone();
            if mail.is_some() || contacts.is_some() || calendar.is_some() {
                state.personal_data_status = "Syncing...".to_string();
                return Task::perform(
                    async move {
                        let result = sync_all_personal_data(mail, contacts, calendar).await;
                        Message::PersonalDataEvent(result)
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        Message::PersonalDataEvent(event) => match event {
            crate::personal_data::PersonalDataEvent::Emails(emails) => {
                state.mail_state.emails = emails.into_vec();
                Task::none()
            }
            crate::personal_data::PersonalDataEvent::EmailBody(email) => {
                let email = *email;
                if let Some(selected) = &mut state.mail_state.selected_email
                    && selected.id == email.id {
                        *selected = email.clone();
                    }
                if let Some(existing) = state.mail_state.emails.iter_mut().find(|e| e.id == email.id) {
                    *existing = email;
                }
                Task::none()
            }
            crate::personal_data::PersonalDataEvent::Contacts(contacts) => {
                state.contacts_state.contacts = contacts.into_vec();
                Task::none()
            }
            crate::personal_data::PersonalDataEvent::Events(events) => {
                state.calendar_state.events = events.into_vec();
                Task::none()
            }
            crate::personal_data::PersonalDataEvent::Filters(filters) => {
                state.mail_state.filters = filters.into_vec();
                if let Some(selected) = state.mail_state.selected_filter.as_ref() {
                    if let Some(updated) = state
                        .mail_state
                        .filters
                        .iter()
                        .find(|f| f.name == selected.name)
                        .cloned()
                    {
                        state.mail_state.selected_filter = Some(updated.clone());
                        if !state.mail_state.editing_filter {
                            state.mail_state.filter_name = updated.name;
                            state.mail_state.filter_content =
                                text_editor::Content::with_text(&updated.content);
                        }
                    } else {
                        state.mail_state.selected_filter = None;
                    }
                }
                Task::none()
            }
            crate::personal_data::PersonalDataEvent::SyncFinished(result) => {
                state.personal_data_status = match &result {
                    Ok(msg) => msg.clone(),
                    Err(err) => format!("Sync error: {}", err),
                };
                if result.is_ok() {
                    load_personal_data_from_db(state);
                    update_tray_unread(state);
                }
                Task::none()
            }
        },
        Message::Mail(msg) => handle_mail_message(state, msg),
        Message::Contacts(msg) => handle_contacts_message(state, msg),
        Message::Calendar(msg) => handle_calendar_message(state, msg),
        Message::XmppEvent(event) => match event {
            XmppEvent::Ready(sender) => {
                state.xmpp_sender = Some(sender);
                if let Err(e) = crate::db::run_migrations() {
                    tracing::info!("Database migration failed: {}", e);
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
                if let Some(idx) = state.selected_conversation
                    && let Some(contact_jid) =
                        state.conversations.get(idx).map(|c| c.contact_jid.as_str())
                {
                    state.omemo_enabled = omemo_pref_for(state.account.as_ref(), contact_jid);
                } else {
                    state.omemo_enabled = true;
                }

                // Load message history from database
                state.conversations.clear();
                match crate::db::load_messages(&jid) {
                    Ok(msgs) => {
                        tracing::info!("[UI] Loaded {} messages from local DB", msgs.len());
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
                        refresh_chat_message_bodies(state);
                        update_tray_unread(state);
                    }
                    Err(e) => tracing::info!("[UI] Failed to load history: {}", e),
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
                state.audio_engine.stop_call();
                state.active_call_with = None;
                state.active_call_sid = None;
                state.pending_incoming_call = None;
                refresh_chat_message_bodies(state);
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
                tracing::info!("[UI] RosterItem: jid={} name={:?}", jid, display_name);
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
                let is_incoming = msg.direction == Direction::Incoming;
                let notify_body = msg.body.clone();
                let account_jid = state
                    .account
                    .as_ref()
                    .map(|a| a.jid.clone())
                    .unwrap_or_default();

                if let Err(e) = crate::db::save_message(&msg, &account_jid, &from_bare) {
                    tracing::info!("Failed to save message: {}", e);
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
                refresh_chat_message_bodies(state);
                update_tray_unread(state);
                if is_incoming && state.window_hidden_to_tray {
                    crate::notify::incoming_message(&from_bare, &notify_body);
                }

                if should_scroll_selected(state, &from_bare) {
                    scroll_chat_to_end()
                } else {
                    Task::none()
                }
            }
            XmppEvent::MessageSent { .. } => Task::none(),
            XmppEvent::MessageDelivered { id } => {
                for conv in &mut state.conversations {
                    if let Some(msg) = conv.messages.iter_mut().find(|m| m.id == id) {
                        msg.status = crate::models::message::MessageStatus::Delivered;
                        break;
                    }
                }
                if let Err(e) = crate::db::update_message_status(
                    &id,
                    crate::models::message::MessageStatus::Delivered,
                ) {
                    tracing::info!("Failed to update message status: {}", e);
                }
                refresh_chat_message_bodies(state);
                Task::none()
            }
            XmppEvent::MessageCorrected {
                from,
                target_id,
                body,
            } => {
                for conv in &mut state.conversations {
                    if conv.contact_jid != from {
                        continue;
                    }
                    if let Some(msg) = conv.messages.iter_mut().find(|m| m.id == target_id) {
                        msg.body = body.clone();
                        msg.timestamp = chrono::Utc::now();
                    }
                }
                if let Err(e) = crate::db::update_message_body(&target_id, &body) {
                    tracing::info!("Failed to update corrected message: {}", e);
                }
                refresh_chat_message_bodies(state);
                Task::none()
            }
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
                let is_incoming = msg.direction == Direction::Incoming;
                let notify_body = msg.body.clone();

                if let Err(e) = crate::db::save_message(&msg, &account_jid, &from) {
                    tracing::info!("Failed to save message: {}", e);
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
                refresh_chat_message_bodies(state);
                update_tray_unread(state);
                if is_incoming && state.window_hidden_to_tray {
                    crate::notify::incoming_message(&from, &notify_body);
                }
                if should_scroll_selected(state, &from) {
                    scroll_chat_to_end()
                } else {
                    Task::none()
                }
            }
            XmppEvent::BundleReceived => Task::none(),
            XmppEvent::AvatarReceived { jid, bytes } => {
                tracing::info!("[UI] AvatarReceived: jid={} bytes={}", jid, bytes.len());
                save_cached_avatar(&jid, &bytes);
                let handle = iced::widget::image::Handle::from_bytes(bytes);
                state.avatar_handles.insert(jid, handle);
                Task::none()
            }
            XmppEvent::IncomingCall { from, sid } => {
                if state.audio_engine.state() == AudioCallState::Active {
                    if let Some(ref mut sender) = state.xmpp_sender {
                        let mut sender = sender.clone();
                        return Task::perform(
                            async move {
                                let _ = sender
                                    .send(XmppCommand::RejectCall {
                                        with: from,
                                        sid,
                                        reason: CallRejectReason::Busy,
                                    })
                                    .await;
                            },
                            |_| Message::JidChanged(String::new()),
                        )
                        .discard();
                    }
                    return Task::none();
                }
                state.connection_status = format!("Incoming call from {}", from);
                state.pending_incoming_call = Some((from, sid));
                if let Some((with, sid)) = state.pending_incoming_call.clone()
                    && let Some(ref mut sender) = state.xmpp_sender
                {
                    let mut sender = sender.clone();
                    return Task::perform(
                        async move {
                            let _ = sender.send(XmppCommand::SendCallRinging { with, sid }).await;
                        },
                        |_| Message::JidChanged(String::new()),
                    )
                    .discard();
                }
                Task::none()
            }
            XmppEvent::CallAccepted { with, sid } => {
                state.connection_status = format!("Call active with {}", with);
                if state.audio_engine.start_call(&with, &sid).is_ok() {
                    state.active_call_with = Some(with);
                    state.active_call_sid = Some(sid);
                }
                Task::none()
            }
            XmppEvent::CallRinging { with, sid } => {
                state.connection_status = format!("Ringing {} ({})...", with, sid);
                Task::none()
            }
            XmppEvent::CallTransportInfo {
                with,
                sid,
                candidates,
            } => {
                if state.active_call_sid.as_deref() == Some(sid.as_str())
                    || state.pending_incoming_call.as_ref().is_some_and(|(w, s)| {
                        w == &with && s == &sid
                    })
                {
                    let mapped = candidates
                        .into_iter()
                        .map(|c| crate::audio::AudioIceCandidate {
                            foundation: c.foundation,
                            component: c.component,
                            protocol: c.protocol,
                            priority: c.priority,
                            ip: c.ip,
                            port: c.port,
                            typ: c.typ,
                        })
                        .collect();
                    state.audio_engine.apply_remote_candidates(mapped);
                }
                Task::none()
            }
            XmppEvent::CallEnded { with, sid, reason } => {
                state.connection_status =
                    format!("Call ended with {} ({}, reason={})", with, sid, reason);
                state.audio_engine.stop_call();
                state.active_call_with = None;
                state.active_call_sid = None;
                state.pending_incoming_call = None;
                Task::none()
            }
        },
    }
}

fn handle_mail_message(state: &mut AppState, msg: MailMessage) -> Task<Message> {
    match msg {
        MailMessage::MailboxSelected(id) => {
            state.mail_state.selected_mailbox = Some(id.clone());
            state.mail_state.selected_email = None;
            if let Some(account) = state.mail_account.clone() {
                let account_id = account.id.clone();
                return Task::perform(
                    async move {
                        let mut client = crate::personal_data::mail::MailClient::new(&account);
                        match client.fetch_emails(&id).await {
                            Ok(emails) => {
                                let _ = crate::db::mail::save_emails(&account_id, &emails);
                                Message::PersonalDataEvent(
                                    crate::personal_data::PersonalDataEvent::Emails(emails.into_boxed_slice()),
                                )
                            }
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        MailMessage::EmailSelected(id) => {
            if let Some(email) = state.mail_state.emails.iter().find(|e| e.id == id).cloned() {
                state.mail_state.selected_email = Some(email.clone());
                if email.body_text.is_none()
                    && let Some(account) = state.mail_account.clone() {
                        return Task::perform(
                            async move {
                                let mut client = crate::personal_data::mail::MailClient::new(&account);
                                match client.fetch_email_body(&id).await {
                                    Ok(email) => Message::PersonalDataEvent(
                                        crate::personal_data::PersonalDataEvent::EmailBody(Box::new(email)),
                                    ),
                                    Err(e) => Message::PersonalDataEvent(
                                        crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                                    ),
                                }
                            },
                            |m| m,
                        );
                    }
            }
            Task::none()
        }
        MailMessage::ComposeClicked => {
            state.mail_state.composing = true;
            state.mail_state.compose_to.clear();
            state.mail_state.compose_cc.clear();
            state.mail_state.compose_subject.clear();
            state.mail_state.compose_body.clear();
            Task::none()
        }
        MailMessage::CancelCompose => {
            state.mail_state.composing = false;
            Task::none()
        }
        MailMessage::ToChanged(v) => {
            state.mail_state.compose_to = v;
            Task::none()
        }
        MailMessage::CcChanged(v) => {
            state.mail_state.compose_cc = v;
            Task::none()
        }
        MailMessage::SubjectChanged(v) => {
            state.mail_state.compose_subject = v;
            Task::none()
        }
        MailMessage::BodyChanged(v) => {
            state.mail_state.compose_body = v;
            Task::none()
        }
        MailMessage::SendClicked => {
            let to: Vec<String> = state
                .mail_state
                .compose_to
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let cc: Vec<String> = state
                .mail_state
                .compose_cc
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let subject = state.mail_state.compose_subject.clone();
            let body = state.mail_state.compose_body.clone();

            if let Some(account) = state.mail_account.clone() {
                state.mail_state.composing = false;
                return Task::perform(
                    async move {
                        let mut client = crate::personal_data::mail::MailClient::new(&account);
                        match client.send_email(&to, &cc, &[], &subject, &body, None).await {
                            Ok(()) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                    "Email sent".to_string(),
                                )),
                            ),
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        MailMessage::MarkReadClicked(id, read) => {
            if let Some(account) = state.mail_account.clone() {
                if let Some(email) = state.mail_state.emails.iter_mut().find(|e| e.id == id) {
                    email.set_read(read);
                    let keywords = email.keywords.clone();
                    let _ = crate::db::mail::update_email_keywords(&id, &keywords);
                }
                return Task::perform(
                    async move {
                        let mut client = crate::personal_data::mail::MailClient::new(&account);
                        match client.mark_email_read(&id, read).await {
                            Ok(()) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                    "Updated".to_string(),
                                )),
                            ),
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        MailMessage::DeleteClicked(id) => {
            if let Some(account) = state.mail_account.clone() {
                state.mail_state.emails.retain(|e| e.id != id);
                state.mail_state.selected_email = None;
                let _ = crate::db::mail::delete_email(&id);
                return Task::perform(
                    async move {
                        let mut client = crate::personal_data::mail::MailClient::new(&account);
                        match client.delete_email(&id).await {
                            Ok(()) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                    "Deleted".to_string(),
                                )),
                            ),
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
    MailMessage::FiltersClicked => {
        state.mail_state.viewing_filters = true;
        if let Some(account) = state.mail_account.clone()
            && account.sieve_config.is_some() {
                return Task::perform(
                    async move { fetch_filters(account).await },
                    Message::PersonalDataEvent,
                );
            }
        Task::none()
    }
    MailMessage::MailboxViewClicked => {
        state.mail_state.viewing_filters = false;
        Task::none()
    }
    MailMessage::FilterSelected(id) => {
        if let Some(filter) = state.mail_state.filters.iter().find(|f| f.id == id).cloned() {
            state.mail_state.selected_filter = Some(filter.clone());
            state.mail_state.editing_filter = false;
            state.mail_state.filter_name = filter.name;
            state.mail_state.filter_content = text_editor::Content::with_text(&filter.content);
        }
        Task::none()
    }
    MailMessage::FilterNameChanged(v) => {
        state.mail_state.filter_name = v;
        Task::none()
    }
    MailMessage::FilterContentChanged(action) => {
        state.mail_state.filter_content.perform(action);
        Task::none()
    }
    MailMessage::NewFilter => {
        state.mail_state.selected_filter = None;
        state.mail_state.editing_filter = true;
        state.mail_state.filter_name.clear();
        state.mail_state.filter_content = text_editor::Content::new();
        Task::none()
    }
    MailMessage::CancelFilterEdit => {
        state.mail_state.editing_filter = false;
        if let Some(filter) = state.mail_state.selected_filter.clone() {
            state.mail_state.filter_name = filter.name;
            state.mail_state.filter_content = text_editor::Content::with_text(&filter.content);
        } else {
            state.mail_state.filter_name.clear();
            state.mail_state.filter_content = text_editor::Content::new();
        }
        Task::none()
    }
    MailMessage::SaveFilter => {
        if let Some(account) = state.mail_account.clone()
            && account.sieve_config.is_some() {
                let _filter_id = state
                    .mail_state
                    .selected_filter
                    .as_ref()
                    .map(|f| f.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let name = state.mail_state.filter_name.clone();
                let content = state.mail_state.filter_content.text();
                state.mail_state.editing_filter = false;
                return Task::perform(
                    async move {
                        let mut client = match crate::personal_data::sieve::ManageSieveClient::connect(&account).await {
                            Ok(c) => c,
                            Err(e) => return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                        };
                        if let Err(e) = client.put_script(&name, &content).await {
                            return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e));
                        }
                        let _ = client.logout().await;
                        fetch_filters(account).await
                    },
                    Message::PersonalDataEvent,
                );
            }
        Task::none()
    }
    MailMessage::DeleteFilter => {
        if let Some(account) = state.mail_account.clone()
            && let Some(filter) = state.mail_state.selected_filter.clone() {
                state.mail_state.selected_filter = None;
                state.mail_state.editing_filter = false;
                return Task::perform(
                    async move {
                        let mut client = match crate::personal_data::sieve::ManageSieveClient::connect(&account).await {
                            Ok(c) => c,
                            Err(e) => return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                        };
                        if let Err(e) = client.delete_script(&filter.name).await {
                            return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e));
                        }
                        let _ = client.logout().await;
                        fetch_filters(account).await
                    },
                    Message::PersonalDataEvent,
                );
            }
        Task::none()
    }
    MailMessage::ActivateFilter => {
        if let Some(account) = state.mail_account.clone()
            && let Some(filter) = state.mail_state.selected_filter.clone() {
                return Task::perform(
                    async move {
                        let mut client = match crate::personal_data::sieve::ManageSieveClient::connect(&account).await {
                            Ok(c) => c,
                            Err(e) => return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                        };
                        if let Err(e) = client.set_active(&filter.name).await {
                            return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e));
                        }
                        let _ = client.logout().await;
                        fetch_filters(account).await
                    },
                    Message::PersonalDataEvent,
                );
            }
        Task::none()
    }
    }
}

async fn fetch_filters(
    account: MailAccount,
) -> crate::personal_data::PersonalDataEvent {
    let mut client = match crate::personal_data::sieve::ManageSieveClient::connect(&account).await {
        Ok(c) => c,
        Err(e) => return crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
    };
    let account_id = account.id.clone();
    match client.list_scripts().await {
        Ok(scripts) => {
            let mut filters = Vec::new();
            for (name, is_active) in scripts {
                match client.get_script(&name).await {
                    Ok(content) => {
                        filters.push(MailFilter {
                            id: uuid::Uuid::new_v4().to_string(),
                            account_id: account_id.clone(),
                            name,
                            content,
                            is_active,
                        });
                    }
                    Err(e) => {
                        return crate::personal_data::PersonalDataEvent::SyncFinished(Err(format!(
                            "Failed to fetch script {}: {}",
                            name, e
                        )));
                    }
                }
            }
            let _ = crate::db::mail::save_filters(&account_id, &filters);
            crate::personal_data::PersonalDataEvent::Filters(filters.into_boxed_slice())
        }
        Err(e) => crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
    }
}

fn handle_contacts_message(state: &mut AppState, msg: ContactsMessage) -> Task<Message> {
    match msg {
        ContactsMessage::AddressbookSelected(id) => {
            state.contacts_state.selected_addressbook = Some(id.clone());
            state.contacts_state.selected_contact = None;
            if let Some(account) = state.contacts_account.clone() {
                return Task::perform(
                    async move {
                        let client = crate::personal_data::contacts::ContactsClient::new(&account);
                        match client.list_addressbooks().await {
                            Ok(books) => {
                                if let Some(book) = books.iter().find(|b| b.id == id).cloned() {
                                    match client.list_contacts(&book).await {
                                        Ok(contacts) => Message::PersonalDataEvent(
                                            crate::personal_data::PersonalDataEvent::Contacts(
                                                contacts.into_boxed_slice(),
                                            ),
                                        ),
                                        Err(e) => Message::PersonalDataEvent(
                                            crate::personal_data::PersonalDataEvent::SyncFinished(
                                                Err(e),
                                            ),
                                        ),
                                    }
                                } else {
                                    Message::PersonalDataEvent(
                                        crate::personal_data::PersonalDataEvent::Contacts(Vec::new().into_boxed_slice()),
                                    )
                                }
                            }
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        ContactsMessage::ContactSelected(id) => {
            if let Some(contact) = state.contacts_state.contacts.iter().find(|c| c.id == id).cloned() {
                state.contacts_state.selected_contact = Some(contact);
            }
            Task::none()
        }
        ContactsMessage::NewContact => {
            state.contacts_state.editing = true;
            state.contacts_state.edit_display_name.clear();
            state.contacts_state.edit_first_name.clear();
            state.contacts_state.edit_last_name.clear();
            state.contacts_state.edit_email.clear();
            state.contacts_state.edit_phone.clear();
            state.contacts_state.edit_org.clear();
            state.contacts_state.edit_note.clear();
            if let Some(contact) = &state.contacts_state.selected_contact {
                state.contacts_state.edit_display_name = contact.display_name.clone();
                state.contacts_state.edit_first_name = contact.first_name.clone();
                state.contacts_state.edit_last_name = contact.last_name.clone();
                state.contacts_state.edit_email = contact.emails.first().cloned().unwrap_or_default();
                state.contacts_state.edit_phone = contact.phones.first().cloned().unwrap_or_default();
                state.contacts_state.edit_org = contact.org.clone();
                state.contacts_state.edit_note = contact.note.clone();
            }
            Task::none()
        }
        ContactsMessage::CancelEdit => {
            state.contacts_state.editing = false;
            Task::none()
        }
        ContactsMessage::DisplayNameChanged(v) => {
            state.contacts_state.edit_display_name = v;
            Task::none()
        }
        ContactsMessage::FirstNameChanged(v) => {
            state.contacts_state.edit_first_name = v;
            Task::none()
        }
        ContactsMessage::LastNameChanged(v) => {
            state.contacts_state.edit_last_name = v;
            Task::none()
        }
        ContactsMessage::EmailChanged(v) => {
            state.contacts_state.edit_email = v;
            Task::none()
        }
        ContactsMessage::PhoneChanged(v) => {
            state.contacts_state.edit_phone = v;
            Task::none()
        }
        ContactsMessage::OrgChanged(v) => {
            state.contacts_state.edit_org = v;
            Task::none()
        }
        ContactsMessage::NoteChanged(v) => {
            state.contacts_state.edit_note = v;
            Task::none()
        }
        ContactsMessage::SaveContact => {
            let mut contact = state
                .contacts_state
                .selected_contact
                .clone()
                .unwrap_or_else(|| ContactCard {
                    id: uuid::Uuid::new_v4().to_string(),
                    account_id: String::new(),
                    addressbook_id: state.contacts_state.selected_addressbook.clone().unwrap_or_default(),
                    href: String::new(),
                    etag: None,
                    uid: uuid::Uuid::new_v4().to_string(),
                    display_name: String::new(),
                    first_name: String::new(),
                    last_name: String::new(),
                    emails: Vec::new(),
                    phones: Vec::new(),
                    org: String::new(),
                    note: String::new(),
                    raw_vcard: String::new(),
                });
            contact.display_name = state.contacts_state.edit_display_name.clone();
            contact.first_name = state.contacts_state.edit_first_name.clone();
            contact.last_name = state.contacts_state.edit_last_name.clone();
            contact.emails = state
                .contacts_state
                .edit_email
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            contact.phones = state
                .contacts_state
                .edit_phone
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            contact.org = state.contacts_state.edit_org.clone();
            contact.note = state.contacts_state.edit_note.clone();

            if contact.href.is_empty()
                && let Some(book) = state.contacts_state.addressbooks.iter().find(|b| {
                    state
                        .contacts_state
                        .selected_addressbook
                        .as_deref()
                        == Some(b.id.as_str())
                }) {
                    contact.href = format!("{}/{}.vcf", book.href.trim_end_matches('/'), contact.uid);
                }

            state.contacts_state.editing = false;
            if let Some(account) = state.contacts_account.clone() {
                contact.account_id = account.id.clone();
                return Task::perform(
                    async move {
                        let client = crate::personal_data::contacts::ContactsClient::new(&account);
                        match client.save_contact(&contact).await {
                            Ok(()) => Message::SyncPersonalData,
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        ContactsMessage::DeleteContact => {
            if let Some(contact) = state.contacts_state.selected_contact.clone() {
                state.contacts_state.selected_contact = None;
                if let Some(account) = state.contacts_account.clone() {
                    return Task::perform(
                        async move {
                            let client = crate::personal_data::contacts::ContactsClient::new(&account);
                            match client.delete_contact(&contact).await {
                                Ok(()) => Message::SyncPersonalData,
                                Err(e) => Message::PersonalDataEvent(
                                    crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                                ),
                            }
                        },
                        |m| m,
                    );
                }
            }
            Task::none()
        }
        ContactsMessage::ImportVcf => {
            let account = state.contacts_account.clone();
            let book = state
                .contacts_state
                .selected_addressbook
                .as_ref()
                .and_then(|id| state.contacts_state.addressbooks.iter().find(|b| b.id == *id))
                .cloned();
            Task::perform(
                async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("vCard", &["vcf", "vcard"])
                        .set_title("Import vCard")
                        .pick_file()
                        .await;
                    let Some(file) = file else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                "Import cancelled".to_string(),
                            )),
                        );
                    };
                    let data = file.read().await;
                    let text = String::from_utf8_lossy(&data);
                    let Some(account) = account else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No contacts account configured".to_string(),
                            )),
                        );
                    };
                    let Some(book) = book else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No addressbook selected".to_string(),
                            )),
                        );
                    };
                    let account_id = account.id.clone();
                    let contacts = crate::personal_data::contacts::import_vcards(
                        &text,
                        &account_id,
                        &book.id,
                    );
                    if contacts.is_empty() {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No contacts found in file".to_string(),
                            )),
                        );
                    }
                    let client = crate::personal_data::contacts::ContactsClient::new(&account);
                    let mut errors = Vec::new();
                    let mut count = 0usize;
                    for mut contact in contacts {
                        contact.account_id = account_id.clone();
                        contact.addressbook_id = book.id.clone();
                        if contact.href.is_empty() {
                            contact.href = format!(
                                "{}/{}.vcf",
                                book.href.trim_end_matches('/'),
                                contact.uid
                            );
                        }
                        if let Err(e) = client.save_contact(&contact).await {
                            errors.push(e);
                        } else {
                            count += 1;
                        }
                    }
                    Message::PersonalDataEvent(
                        crate::personal_data::PersonalDataEvent::SyncFinished(if errors.is_empty() {
                            Ok(format!("Imported {} contacts", count))
                        } else {
                            Err(format!(
                                "Imported {} contacts, {} failed: {}",
                                count,
                                errors.len(),
                                errors.join(", ")
                            ))
                        }),
                    )
                },
                |m| m,
            )
        }
        ContactsMessage::ExportVcf => {
            let contacts = state.contacts_state.contacts.clone();
            let book_name = state
                .contacts_state
                .selected_addressbook
                .clone()
                .unwrap_or_else(|| "contacts".to_string());
            Task::perform(
                async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("vCard", &["vcf"])
                        .set_title("Export vCard")
                        .set_file_name(format!("{}.vcf", book_name))
                        .save_file()
                        .await;
                    let Some(file) = file else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                "Export cancelled".to_string(),
                            )),
                        );
                    };
                    let data = crate::personal_data::contacts::export_vcards(&contacts);
                    if let Err(e) = file.write(data.as_bytes()).await {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(format!(
                                "Failed to write file: {}",
                                e
                            ))),
                        );
                    }
                    Message::PersonalDataEvent(
                        crate::personal_data::PersonalDataEvent::SyncFinished(Ok(format!(
                            "Exported {} contacts",
                            contacts.len()
                        ))),
                    )
                },
                |m| m,
            )
        }
    }
}

fn handle_calendar_message(state: &mut AppState, msg: CalendarMessage) -> Task<Message> {
    match msg {
        CalendarMessage::CalendarSelected(id) => {
            state.calendar_state.selected_calendar = Some(id.clone());
            state.calendar_state.selected_event = None;
            state.calendar_state.selected_task = None;
            if let Some(account) = state.calendar_account.clone() {
                return Task::perform(
                    async move {
                        let client = crate::personal_data::calendar::CalendarClient::new(&account);
                        let year = chrono::Utc::now().year();
                        match client.list_calendars().await {
                            Ok(calendars) => {
                                if let Some(cal) = calendars.iter().find(|c| c.id == id).cloned() {
                                    match client.list_events(&cal, year).await {
                                        Ok(events) => Message::PersonalDataEvent(
                                            crate::personal_data::PersonalDataEvent::Events(
                                                events.into_boxed_slice(),
                                            ),
                                        ),
                                        Err(e) => Message::PersonalDataEvent(
                                            crate::personal_data::PersonalDataEvent::SyncFinished(
                                                Err(e),
                                            ),
                                        ),
                                    }
                                } else {
                                    Message::PersonalDataEvent(
                                        crate::personal_data::PersonalDataEvent::Events(Vec::new().into_boxed_slice()),
                                    )
                                }
                            }
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        CalendarMessage::EventSelected(id) => {
            if let Some(event) = state.calendar_state.events.iter().find(|e| e.id == id).cloned() {
                state.calendar_state.selected_event = Some(event);
                state.calendar_state.selected_task = None;
            }
            Task::none()
        }
        CalendarMessage::TaskSelected(id) => {
            if let Some(task) = state.calendar_state.tasks.iter().find(|t| t.id == id).cloned() {
                state.calendar_state.selected_task = Some(task);
                state.calendar_state.selected_event = None;
            }
            Task::none()
        }
        CalendarMessage::NewEvent => {
            state.calendar_state.editing_event = true;
            state.calendar_state.editing_task = false;
            state.calendar_state.edit_title.clear();
            state.calendar_state.edit_start = chrono::Utc::now().to_rfc3339();
            state.calendar_state.edit_end = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
            state.calendar_state.edit_all_day = false;
            state.calendar_state.edit_description.clear();
            state.calendar_state.edit_location.clear();
            if let Some(event) = &state.calendar_state.selected_event {
                state.calendar_state.edit_title = event.title.clone();
                state.calendar_state.edit_start = event.start.to_rfc3339();
                state.calendar_state.edit_end = event.end.to_rfc3339();
                state.calendar_state.edit_all_day = event.all_day;
                state.calendar_state.edit_description = event.description.clone();
                state.calendar_state.edit_location = event.location.clone();
            }
            Task::none()
        }
        CalendarMessage::NewTask => {
            state.calendar_state.editing_task = true;
            state.calendar_state.editing_event = false;
            state.calendar_state.edit_title.clear();
            state.calendar_state.edit_due = chrono::Utc::now().to_rfc3339();
            state.calendar_state.edit_status = "NEEDS-ACTION".to_string();
            state.calendar_state.edit_description.clear();
            if let Some(task) = &state.calendar_state.selected_task {
                state.calendar_state.edit_title = task.title.clone();
                state.calendar_state.edit_due = task.due.map(|d| d.to_rfc3339()).unwrap_or_default();
                state.calendar_state.edit_status = task.status.clone();
                state.calendar_state.edit_description = task.description.clone();
            }
            Task::none()
        }
        CalendarMessage::CancelEdit => {
            state.calendar_state.editing_event = false;
            state.calendar_state.editing_task = false;
            Task::none()
        }
        CalendarMessage::TitleChanged(v) => {
            state.calendar_state.edit_title = v;
            Task::none()
        }
        CalendarMessage::StartChanged(v) => {
            state.calendar_state.edit_start = v;
            Task::none()
        }
        CalendarMessage::EndChanged(v) => {
            state.calendar_state.edit_end = v;
            Task::none()
        }
        CalendarMessage::AllDayChanged(v) => {
            state.calendar_state.edit_all_day = v;
            Task::none()
        }
        CalendarMessage::DescriptionChanged(v) => {
            state.calendar_state.edit_description = v;
            Task::none()
        }
        CalendarMessage::LocationChanged(v) => {
            state.calendar_state.edit_location = v;
            Task::none()
        }
        CalendarMessage::DueChanged(v) => {
            state.calendar_state.edit_due = v;
            Task::none()
        }
        CalendarMessage::StatusChanged(v) => {
            state.calendar_state.edit_status = v;
            Task::none()
        }
        CalendarMessage::SaveEvent => {
            let mut event = state
                .calendar_state
                .selected_event
                .clone()
                .unwrap_or_else(|| CalendarEvent {
                    id: uuid::Uuid::new_v4().to_string(),
                    account_id: String::new(),
                    calendar_id: state.calendar_state.selected_calendar.clone().unwrap_or_default(),
                    href: String::new(),
                    etag: None,
                    uid: uuid::Uuid::new_v4().to_string(),
                    title: String::new(),
                    start: chrono::Utc::now(),
                    end: chrono::Utc::now(),
                    all_day: false,
                    description: String::new(),
                    location: String::new(),
                    status: String::new(),
                    raw_ics: String::new(),
                });
            event.title = state.calendar_state.edit_title.clone();
            event.start = state.calendar_state.edit_start.parse().unwrap_or_else(|_| chrono::Utc::now());
            event.end = state.calendar_state.edit_end.parse().unwrap_or_else(|_| chrono::Utc::now());
            event.all_day = state.calendar_state.edit_all_day;
            event.description = state.calendar_state.edit_description.clone();
            event.location = state.calendar_state.edit_location.clone();

            if event.href.is_empty()
                && let Some(cal) = state.calendar_state.calendars.iter().find(|c| {
                    state.calendar_state.selected_calendar.as_deref() == Some(c.id.as_str())
                }) {
                    event.href = format!("{}/{}.ics", cal.href.trim_end_matches('/'), event.uid);
                }

            state.calendar_state.editing_event = false;
            if let Some(account) = state.calendar_account.clone() {
                event.account_id = account.id.clone();
                return Task::perform(
                    async move {
                        let client = crate::personal_data::calendar::CalendarClient::new(&account);
                        match client.save_event(&event).await {
                            Ok(()) => Message::SyncPersonalData,
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        CalendarMessage::SaveTask => {
            let mut task = state
                .calendar_state
                .selected_task
                .clone()
                .unwrap_or_else(|| CalendarTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    account_id: String::new(),
                    calendar_id: state.calendar_state.selected_calendar.clone().unwrap_or_default(),
                    href: String::new(),
                    etag: None,
                    uid: uuid::Uuid::new_v4().to_string(),
                    title: String::new(),
                    due: None,
                    all_day: false,
                    description: String::new(),
                    location: String::new(),
                    status: String::new(),
                    priority: 0,
                    percent_complete: 0,
                    completed: None,
                    raw_ics: String::new(),
                });
            task.title = state.calendar_state.edit_title.clone();
            task.due = state.calendar_state.edit_due.parse().ok();
            task.status = state.calendar_state.edit_status.clone();
            task.description = state.calendar_state.edit_description.clone();

            if task.href.is_empty()
                && let Some(cal) = state.calendar_state.calendars.iter().find(|c| {
                    state.calendar_state.selected_calendar.as_deref() == Some(c.id.as_str())
                }) {
                    task.href = format!("{}/{}.ics", cal.href.trim_end_matches('/'), task.uid);
                }

            state.calendar_state.editing_task = false;
            if let Some(account) = state.calendar_account.clone() {
                task.account_id = account.id.clone();
                return Task::perform(
                    async move {
                        let client = crate::personal_data::calendar::CalendarClient::new(&account);
                        match client.save_task(&task).await {
                            Ok(()) => Message::SyncPersonalData,
                            Err(e) => Message::PersonalDataEvent(
                                crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                            ),
                        }
                    },
                    |m| m,
                );
            }
            Task::none()
        }
        CalendarMessage::DeleteEvent => {
            if let Some(event) = state.calendar_state.selected_event.clone() {
                state.calendar_state.selected_event = None;
                if let Some(account) = state.calendar_account.clone() {
                    return Task::perform(
                        async move {
                            let client = crate::personal_data::calendar::CalendarClient::new(&account);
                            match client.delete_event(&event).await {
                                Ok(()) => Message::SyncPersonalData,
                                Err(e) => Message::PersonalDataEvent(
                                    crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                                ),
                            }
                        },
                        |m| m,
                    );
                }
            }
            Task::none()
        }
        CalendarMessage::DeleteTask => {
            if let Some(task) = state.calendar_state.selected_task.clone() {
                state.calendar_state.selected_task = None;
                if let Some(account) = state.calendar_account.clone() {
                    return Task::perform(
                        async move {
                            let client = crate::personal_data::calendar::CalendarClient::new(&account);
                            match client.delete_task(&task).await {
                                Ok(()) => Message::SyncPersonalData,
                                Err(e) => Message::PersonalDataEvent(
                                    crate::personal_data::PersonalDataEvent::SyncFinished(Err(e)),
                                ),
                            }
                        },
                        |m| m,
                    );
                }
            }
            Task::none()
        }
        CalendarMessage::ImportIcs => {
            let account = state.calendar_account.clone();
            let calendar = state
                .calendar_state
                .selected_calendar
                .as_ref()
                .and_then(|id| state.calendar_state.calendars.iter().find(|c| c.id == *id))
                .cloned();
            Task::perform(
                async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("iCalendar", &["ics", "ical", "icalendar"])
                        .set_title("Import ICS")
                        .pick_file()
                        .await;
                    let Some(file) = file else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                "Import cancelled".to_string(),
                            )),
                        );
                    };
                    let data = file.read().await;
                    let text = String::from_utf8_lossy(&data);
                    let Some(account) = account else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No calendar account configured".to_string(),
                            )),
                        );
                    };
                    let Some(calendar) = calendar else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No calendar selected".to_string(),
                            )),
                        );
                    };
                    let account_id = account.id.clone();
                    let (mut events, mut tasks) = crate::personal_data::calendar::import_ics(
                        &text,
                        &account_id,
                        &calendar.id,
                    );
                    if events.is_empty() && tasks.is_empty() {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(
                                "No events or tasks found in file".to_string(),
                            )),
                        );
                    }
                    let client = crate::personal_data::calendar::CalendarClient::new(&account);
                    let mut errors = Vec::new();
                    let mut event_count = 0usize;
                    let mut task_count = 0usize;
                    for event in &mut events {
                        event.account_id = account_id.clone();
                        event.calendar_id = calendar.id.clone();
                        if event.href.is_empty() {
                            event.href = format!(
                                "{}/{}.ics",
                                calendar.href.trim_end_matches('/'),
                                event.uid
                            );
                        }
                        if let Err(e) = client.save_event(event).await {
                            errors.push(e);
                        } else {
                            event_count += 1;
                        }
                    }
                    for task in &mut tasks {
                        task.account_id = account_id.clone();
                        task.calendar_id = calendar.id.clone();
                        if task.href.is_empty() {
                            task.href = format!(
                                "{}/{}.ics",
                                calendar.href.trim_end_matches('/'),
                                task.uid
                            );
                        }
                        if let Err(e) = client.save_task(task).await {
                            errors.push(e);
                        } else {
                            task_count += 1;
                        }
                    }
                    Message::PersonalDataEvent(
                        crate::personal_data::PersonalDataEvent::SyncFinished(if errors.is_empty() {
                            Ok(format!(
                                "Imported {} events and {} tasks",
                                event_count, task_count
                            ))
                        } else {
                            Err(format!(
                                "Imported {} events and {} tasks, {} failed: {}",
                                event_count,
                                task_count,
                                errors.len(),
                                errors.join(", ")
                            ))
                        }),
                    )
                },
                |m| m,
            )
        }
        CalendarMessage::ExportIcs => {
            let events = state.calendar_state.events.clone();
            let tasks = state.calendar_state.tasks.clone();
            let cal_name = state
                .calendar_state
                .selected_calendar
                .clone()
                .unwrap_or_else(|| "calendar".to_string());
            Task::perform(
                async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("iCalendar", &["ics"])
                        .set_title("Export ICS")
                        .set_file_name(format!("{}.ics", cal_name))
                        .save_file()
                        .await;
                    let Some(file) = file else {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Ok(
                                "Export cancelled".to_string(),
                            )),
                        );
                    };
                    let data = crate::personal_data::calendar::export_ics(&events, &tasks);
                    if let Err(e) = file.write(data.as_bytes()).await {
                        return Message::PersonalDataEvent(
                            crate::personal_data::PersonalDataEvent::SyncFinished(Err(format!(
                                "Failed to write file: {}",
                                e
                            ))),
                        );
                    }
                    Message::PersonalDataEvent(
                        crate::personal_data::PersonalDataEvent::SyncFinished(Ok(format!(
                            "Exported {} events and {} tasks",
                            events.len(),
                            tasks.len()
                        ))),
                    )
                },
                |m| m,
            )
        }
    }
}

async fn sync_all_personal_data(
    mail: Option<crate::models::account::MailAccount>,
    contacts: Option<crate::models::account::ContactsAccount>,
    calendar: Option<crate::models::account::CalendarAccount>,
) -> crate::personal_data::PersonalDataEvent {
    let mut errors = Vec::new();

    let mut mailboxes = Vec::new();
    let mut emails = Vec::new();
    if let Some(account) = mail.as_ref() {
        let mut mail_client = crate::personal_data::mail::MailClient::new(account);
        mailboxes = match mail_client.fetch_mailboxes().await {
            Ok(mbs) => {
                let _ = crate::db::mail::save_mailboxes(&account.id, &mbs);
                mbs
            }
            Err(e) => {
                errors.push(format!("mailboxes: {}", e));
                Vec::new()
            }
        };

        for mb in &mailboxes {
            match mail_client.fetch_emails(&mb.id).await {
                Ok(mut es) => emails.append(&mut es),
                Err(e) => errors.push(format!("emails: {}", e)),
            }
        }
        let _ = crate::db::mail::save_emails(&account.id, &emails);
    }

    let mut addressbooks = Vec::new();
    let mut contacts_list = Vec::new();
    if let Some(account) = contacts.as_ref() {
        let contacts_client = crate::personal_data::contacts::ContactsClient::new(account);
        addressbooks = match contacts_client.list_addressbooks().await {
            Ok(books) => {
                let _ = crate::db::contacts::save_addressbooks(&account.id, &books);
                books
            }
            Err(e) => {
                errors.push(format!("addressbooks: {}", e));
                Vec::new()
            }
        };

        for book in &addressbooks {
            match contacts_client.list_contacts(book).await {
                Ok(mut cs) => contacts_list.append(&mut cs),
                Err(e) => errors.push(format!("contacts: {}", e)),
            }
        }
        let _ = crate::db::contacts::save_contacts(&account.id, &contacts_list);
    }

    let mut calendars = Vec::new();
    let mut events = Vec::new();
    let mut tasks = Vec::new();
    if let Some(account) = calendar.as_ref() {
        let calendar_client = crate::personal_data::calendar::CalendarClient::new(account);
        calendars = match calendar_client.list_calendars().await {
            Ok(cals) => {
                let _ = crate::db::calendar::save_calendars(&account.id, &cals);
                cals
            }
            Err(e) => {
                errors.push(format!("calendars: {}", e));
                Vec::new()
            }
        };

        let year = chrono::Utc::now().year();
        for cal in &calendars {
            match calendar_client.list_events(cal, year).await {
                Ok(mut es) => events.append(&mut es),
                Err(e) => errors.push(format!("events: {}", e)),
            }
            match calendar_client.list_tasks(cal, year).await {
                Ok(mut ts) => tasks.append(&mut ts),
                Err(e) => errors.push(format!("tasks: {}", e)),
            }
        }
        let _ = crate::db::calendar::save_events(&account.id, &events);
        let _ = crate::db::calendar::save_tasks(&account.id, &tasks);
    }

    if errors.is_empty() {
        crate::personal_data::PersonalDataEvent::SyncFinished(Ok(format!(
            "Synced: {} mailboxes, {} emails, {} addressbooks, {} contacts, {} calendars, {} events, {} tasks",
            mailboxes.len(),
            emails.len(),
            addressbooks.len(),
            contacts_list.len(),
            calendars.len(),
            events.len(),
            tasks.len()
        )))
    } else {
        crate::personal_data::PersonalDataEvent::SyncFinished(Ok(format!(
            "Partial sync. Errors: {}",
            errors.join("; ")
        )))
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

            let tabs = row![
                tab_button("Chat", Tab::Chat, state),
                tab_button("Mail", Tab::Mail, state),
                tab_button("Contacts", Tab::Contacts, state),
                tab_button("Calendar", Tab::Calendar, state),
                tab_button("Settings", Tab::Settings, state),
                Space::new().width(Length::Fill),
                button("Sync")
                    .on_press(Message::SyncPersonalData)
                    .padding(4),
            ]
            .spacing(4)
            .padding(8);

            let content: Element<'_, Message> = match state.current_tab {
                Tab::Chat => {
                    let sidebar = conversation_list::view(
                        &state.conversations,
                        &state.avatar_handles,
                        state.selected_conversation,
                    );
                    let selected_conv = state
                        .selected_conversation
                        .and_then(|idx| state.conversations.get(idx));
                    let chat_view = chat::view(
                        selected_conv,
                        &state.draft,
                        &state.chat_message_bodies,
                        &state.avatar_handles,
                        state.active_call_with.as_deref(),
                    );
                    row![sidebar, chat_view].spacing(0).into()
                }
                Tab::Mail => mail::view(&state.mail_state),
                Tab::Contacts => contacts::view(&state.contacts_state),
                Tab::Calendar => calendar::view(&state.calendar_state),
                Tab::Settings => settings::view(&state.settings_state),
            };

            let status_bar = row![
                text(&state.personal_data_status).size(11),
                Space::new().width(Length::Fill),
            ]
            .padding(4);

            let mut root = Column::new().push(toolbar).push(tabs);
            if let Some((from, _sid)) = &state.pending_incoming_call {
                root = root.push(
                    row![
                        text(format!("Incoming call from {}", from)).size(12),
                        button("Accept").on_press(Message::AcceptIncomingCallClicked),
                        button("Decline").on_press(Message::DeclineIncomingCallClicked),
                    ]
                    .spacing(8)
                    .padding(8),
                );
            }

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
                .push(status_bar)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }
}

fn tab_button<'a>(label: &'a str, tab: Tab, state: &AppState) -> iced::Element<'a, Message> {
    let selected = state.current_tab == tab;
    let btn = button(text(label)).padding(6).on_press(Message::TabSelected(tab));
    if selected {
        container(btn)
            .style(|theme: &Theme| container::Style {
                background: Some(iced::Background::Color(theme.palette().primary)),
                ..Default::default()
            })
            .into()
    } else {
        btn.into()
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

    let sync_sub = iced::time::every(std::time::Duration::from_secs(300))
        .map(|_| Message::SyncPersonalData);

    Subscription::batch([xmpp_sub, close_sub, open_sub, tray_sub, sync_sub])
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
    let mut config: ConfigFile = serde_json::from_str(&contents)?;
    if config.account.password.is_empty() {
        match crate::secrets::get_password(crate::secrets::SERVICE_XMPP, &config.account.jid) {
            Ok(Some(password)) => config.account.password = password,
            Ok(None) => {}
            Err(e) => tracing::warn!("Failed to retrieve XMPP password from keyring: {}", e),
        }
    }
    Ok(config.account)
}

fn save_config(account: &Account) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path().ok_or("No config directory")?;
    std::fs::create_dir_all(path.parent().unwrap())?;

    if !account.password.is_empty() {
        crate::secrets::store_password(crate::secrets::SERVICE_XMPP, &account.jid, &account.password)?;
    }

    let mut stored = account.clone();
    stored.password.clear();
    let config = ConfigFile { account: stored };
    let contents = serde_json::to_string_pretty(&config)?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("invalid hex length".to_string());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = (bytes[i] as char)
            .to_digit(16)
            .ok_or_else(|| "invalid hex".to_string())?;
        let lo = (bytes[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| "invalid hex".to_string())?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

async fn download_aesgcm_file(url: &str) -> Result<Vec<u8>, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid aesgcm url: {}", e))?;
    let mut https_url = parsed.clone();
    https_url
        .set_scheme("https")
        .map_err(|_| "invalid aesgcm scheme".to_string())?;

    let frag = parsed
        .fragment()
        .ok_or_else(|| "aesgcm url missing key fragment".to_string())?;
    let key_material = hex_decode(frag)?;
    if key_material.len() < 48 {
        return Err("aesgcm key material too short".to_string());
    }
    let key = &key_material[..32];
    let nonce = &key_material[32..48];

    https_url.set_fragment(None);
    let response = reqwest::get(https_url)
        .await
        .map_err(|e| format!("download failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("download failed: HTTP {}", response.status()));
    }
    let encrypted = response
        .bytes()
        .await
        .map_err(|e| format!("download failed: {}", e))?;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("invalid aes key: {}", e))?;
    let decrypted = cipher
        .decrypt(Nonce::from_slice(nonce), encrypted.as_ref())
        .map_err(|_| "file decrypt failed".to_string())?;
    Ok(decrypted)
}
