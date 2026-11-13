use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use futures::channel::mpsc::UnboundedSender;

use super::store::BrowserStore;
use crate::omemo::OmemoManager;

#[derive(Clone)]
pub struct Contact {
    pub jid: String,
    pub name: Option<String>,
    pub presence: String,
    pub status: String,
}

#[derive(Clone)]
pub struct Room {
    pub jid: String,
    pub name: Option<String>,
    pub nick: String,
    pub joined: bool,
}

#[derive(Clone)]
pub struct ChatMessage {
    pub id: String,
    pub from: String,
    pub body: String,
    pub timestamp: f64,
    pub incoming: bool,
    pub room: bool,
    pub encrypted: bool,
    pub failed: bool,
}

pub struct PendingOutgoing {
    pub to: String,
    pub body: String,
    pub id: String,
    pub needed: HashSet<u32>,
}

pub enum Fetch {
    Devices { jid: String, publish: bool },
    Bundle { jid: String, device: u32 },
    Roster,
    Bookmarks,
    History { jid: String },
}

#[derive(Clone, Copy, PartialEq)]
pub enum StreamState {
    Authenticate,
    Bind,
    Online,
}

pub struct Chat {
    pub email: String,
    pub token: String,
    pub save_url: String,
    pub ws_url: String,
    pub snapshot: String,
    pub outgoing: Option<UnboundedSender<String>>,
    pub stream_state: StreamState,
    pub connected: bool,
    pub connecting: bool,
    pub manual_disconnect: bool,
    pub error: String,
    pub bound_jid: String,
    pub contacts: Vec<Contact>,
    pub rooms: Vec<Room>,
    pub messages: HashMap<String, Vec<ChatMessage>>,
    pub selected: Option<(String, bool)>,
    pub history: HashSet<String>,
    pub history_loading: HashSet<String>,
    pub omemo: Option<OmemoManager>,
    pub omemo_store: Option<BrowserStore>,
    pub omemo_enabled: HashMap<String, bool>,
    pub pending_outgoing: HashMap<String, PendingOutgoing>,
    pub fetches: HashMap<String, Fetch>,
    pub counter: u64,
    pub call: super::call::Call,
}

impl Chat {
    pub fn new(root: &web_sys::Element) -> Self {
        let data = |name: &str| {
            root.get_attribute(&format!("data-{name}"))
                .unwrap_or_default()
        };
        Self {
            email: data("email"),
            token: data("token"),
            save_url: data("save-url"),
            ws_url: data("ws-url"),
            snapshot: data("omemo"),
            outgoing: None,
            stream_state: StreamState::Authenticate,
            connected: false,
            connecting: false,
            manual_disconnect: false,
            error: String::new(),
            bound_jid: String::new(),
            contacts: Vec::new(),
            rooms: Vec::new(),
            messages: HashMap::new(),
            selected: None,
            history: HashSet::new(),
            history_loading: HashSet::new(),
            omemo: None,
            omemo_store: None,
            omemo_enabled: HashMap::new(),
            pending_outgoing: HashMap::new(),
            fetches: HashMap::new(),
            counter: 0,
            call: super::call::Call::default(),
        }
    }

    pub fn id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{}-{}", js_sys::Date::now() as u64, self.counter)
    }

    pub fn omemo_enabled(&self, jid: &str) -> bool {
        self.omemo_enabled.get(jid).copied().unwrap_or(true)
    }
}

pub type Shared = Rc<RefCell<Chat>>;

pub fn bare_jid(jid: &str) -> String {
    jid.split('/').next().unwrap_or(jid).to_owned()
}
