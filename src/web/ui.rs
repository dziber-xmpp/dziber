use wasm_bindgen::{JsCast, closure::Closure};

use super::{
    model::{Shared, bare_jid},
    xmpp,
};

pub fn bind(shared: &Shared, root: &web_sys::Element) {
    let click_shared = shared.clone();
    let click = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(element) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|target| target.closest("[data-action]").ok().flatten())
        else {
            return;
        };
        let action = element.get_attribute("data-action").unwrap_or_default();
        let jid = element.get_attribute("data-jid").unwrap_or_default();
        match action.as_str() {
            "connect" => {
                if click_shared.borrow().connected {
                    xmpp::disconnect(&click_shared)
                } else {
                    xmpp::connect(&click_shared)
                }
            }
            "select" => xmpp::select(
                &click_shared,
                jid,
                element.get_attribute("data-room").as_deref() == Some("true"),
            ),
            "leave" => xmpp::leave_room(&click_shared, &jid),
            "call-start" => super::call::start(&click_shared),
            "call-accept" => super::call::accept(&click_shared),
            "call-reject" => super::call::reject(&click_shared),
            "call-end" => super::call::end(&click_shared),
            "omemo" => {
                let mut chat = click_shared.borrow_mut();
                let enabled = !chat.omemo_enabled(&jid);
                chat.omemo_enabled.insert(jid, enabled);
                drop(chat);
                render(&click_shared);
            }
            _ => {}
        }
    });
    root.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
        .unwrap();
    click.forget();

    for (id, join) in [("chat-join", true), ("chat-send", false)] {
        let Some(form) = root.query_selector(&format!("#{id}")).unwrap() else {
            continue;
        };
        let form_handler = form.clone();
        let shared = shared.clone();
        let submit = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            if join {
                let jid = input_value("chat-join", "jid");
                let nick = input_value("chat-join", "nick");
                xmpp::join_room(&shared, jid, nick);
                if let Ok(form) = form_handler.clone().dyn_into::<web_sys::HtmlFormElement>() {
                    form.reset();
                }
            } else {
                let body = message("chat-message").value();
                xmpp::send_message(&shared, body);
                message("chat-message").set_value("");
            }
        });
        form.add_event_listener_with_callback("submit", submit.as_ref().unchecked_ref())
            .unwrap();
        submit.forget();
    }

    let search_shared = shared.clone();
    let search = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| render_roster(&search_shared));
    input("chat-search")
        .add_event_listener_with_callback("input", search.as_ref().unchecked_ref())
        .unwrap();
    search.forget();

    let key_shared = shared.clone();
    let keydown =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if event.key() == "Enter" && !event.shift_key() {
                event.prevent_default();
                let body = message("chat-message").value();
                xmpp::send_message(&key_shared, body);
                message("chat-message").set_value("");
            }
        });
    message("chat-message")
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .unwrap();
    keydown.forget();
}

pub fn render(shared: &Shared) {
    let (connected, connecting, error, count) = {
        let chat = shared.borrow();
        (
            chat.connected,
            chat.connecting,
            chat.error.clone(),
            chat.contacts.len(),
        )
    };
    let status = element("chat-status");
    status.set_text_content(Some(if connecting {
        "Connecting…"
    } else if connected {
        "Connected"
    } else {
        "Disconnected"
    }));
    status.set_class_name(if connecting {
        "status connecting"
    } else if connected {
        "status connected"
    } else {
        "status disconnected"
    });
    let connect = button("chat-connect");
    connect.set_text_content(Some(if connected {
        "Disconnect"
    } else if connecting {
        "Connecting…"
    } else {
        "Connect"
    }));
    connect.set_disabled(connecting);
    input("chat-ws-url").set_disabled(connected || connecting);
    let error_element = element("chat-error");
    error_element.set_text_content(Some(&error));
    set_hidden(&error_element, error.is_empty());
    element("chat-roster-count").set_text_content(Some(&count.to_string()));
    render_roster(shared);
    render_messages(shared);
    super::call::render(shared);
}

fn render_roster(shared: &Shared) {
    let query = input("chat-search").value().to_lowercase();
    let roster = element("chat-roster");
    roster.set_text_content(None);
    let mut contacts = shared.borrow().contacts.clone();
    contacts.retain(|contact| {
        query.is_empty()
            || contact.jid.to_lowercase().contains(&query)
            || contact
                .name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query)
    });
    let own = bare_jid(&shared.borrow().email);
    contacts.sort_by(|left, right| {
        if left.jid == own {
            std::cmp::Ordering::Less
        } else if right.jid == own {
            std::cmp::Ordering::Greater
        } else {
            left.name
                .as_deref()
                .unwrap_or(&left.jid)
                .cmp(right.name.as_deref().unwrap_or(&right.jid))
        }
    });
    for contact in contacts {
        let selected = shared
            .borrow()
            .selected
            .as_ref()
            .is_some_and(|(jid, room)| jid == &contact.jid && !room);
        let button = create(
            "button",
            if selected {
                "roster-item selected"
            } else {
                "roster-item"
            },
        );
        button.set_attribute("type", "button").unwrap();
        button.set_attribute("data-action", "select").unwrap();
        button.set_attribute("data-jid", &contact.jid).unwrap();
        button
            .append_child(&create_text(
                "strong",
                contact.name.as_deref().unwrap_or(&contact.jid),
            ))
            .unwrap();
        button
            .append_child(&create_text(
                "span",
                &format!("{} · {}", contact.presence, contact.jid),
            ))
            .unwrap();
        if !contact.status.is_empty() {
            button
                .append_child(&create_text("small", &contact.status))
                .unwrap();
        }
        roster.append_child(&button).unwrap();
    }

    let rooms = element("chat-rooms");
    rooms.set_text_content(None);
    for room in shared.borrow().rooms.clone() {
        if !query.is_empty()
            && !room.jid.to_lowercase().contains(&query)
            && !room
                .name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query)
        {
            continue;
        }
        let selected = shared
            .borrow()
            .selected
            .as_ref()
            .is_some_and(|(jid, selected_room)| jid == &room.jid && *selected_room);
        let row = create(
            "div",
            if selected {
                "roster-item selected"
            } else {
                "roster-item"
            },
        );
        let select = create("button", "room-select");
        select.set_attribute("type", "button").unwrap();
        select.set_attribute("data-action", "select").unwrap();
        select.set_attribute("data-room", "true").unwrap();
        select.set_attribute("data-jid", &room.jid).unwrap();
        select.set_text_content(Some(&format!(
            "{} ({}){}",
            room.name.as_deref().unwrap_or(&room.jid),
            room.jid,
            if room.joined { " · joined" } else { "" }
        )));
        let leave = create("button", "button clear error");
        leave.set_attribute("type", "button").unwrap();
        leave.set_attribute("data-action", "leave").unwrap();
        leave.set_attribute("data-jid", &room.jid).unwrap();
        leave.set_text_content(Some("Leave"));
        row.append_child(&select).unwrap();
        row.append_child(&leave).unwrap();
        rooms.append_child(&row).unwrap();
    }
}

fn render_messages(shared: &Shared) {
    let selected = shared.borrow().selected.clone();
    let title = element("chat-title");
    let subtitle = element("chat-subtitle");
    let (title_text, subtitle_text) = match &selected {
        Some((jid, room)) => {
            let chat = shared.borrow();
            let name = if *room {
                chat.rooms
                    .iter()
                    .find(|item| &item.jid == jid)
                    .and_then(|item| item.name.clone())
            } else {
                chat.contacts
                    .iter()
                    .find(|item| &item.jid == jid)
                    .and_then(|item| item.name.clone())
            }
            .unwrap_or_else(|| jid.clone());
            (name, format!("{jid}{}", if *room { " · room" } else { "" }))
        }
        None => ("Select a contact or room".to_owned(), String::new()),
    };
    title.set_text_content(Some(&title_text));
    subtitle.set_text_content(Some(&subtitle_text));
    let omemo = button("chat-omemo");
    set_hidden(&omemo, selected.as_ref().is_none_or(|(_, room)| *room));
    if let Some((jid, _)) = &selected {
        omemo.set_text_content(Some(if shared.borrow().omemo_enabled(jid) {
            "OMEMO on"
        } else {
            "OMEMO off"
        }));
        omemo.set_attribute("data-action", "omemo").unwrap();
        omemo.set_attribute("data-jid", jid).unwrap();
    }
    let enabled = shared.borrow().connected && selected.is_some();
    message("chat-message").set_disabled(!enabled);
    element("chat-send")
        .query_selector("button")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::HtmlButtonElement>()
        .unwrap()
        .set_disabled(!enabled);

    let container = element("chat-messages");
    container.set_text_content(None);
    let Some((jid, _)) = selected else {
        container
            .append_child(&create_text(
                "p",
                "Select a contact or room to start chatting",
            ))
            .unwrap();
        return;
    };
    if shared.borrow().history_loading.contains(&jid) {
        container
            .append_child(&create_text("p", "Loading history…"))
            .unwrap();
    }
    let messages = shared
        .borrow()
        .messages
        .get(&jid)
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() && !shared.borrow().history_loading.contains(&jid) {
        container
            .append_child(&create_text("p", "No messages yet"))
            .unwrap();
    }
    for message in messages {
        let row = create(
            "div",
            if message.incoming {
                "chat-message-row incoming"
            } else {
                "chat-message-row outgoing"
            },
        );
        let bubble = create("div", "chat-bubble");
        if message.room {
            bubble
                .append_child(&create_text("strong", &message.from))
                .unwrap();
        }
        let body = create_text("p", &message.body);
        if message.failed {
            body.set_class_name("failed");
        }
        bubble.append_child(&body).unwrap();
        let date = js_sys::Date::new(&message.timestamp.into());
        let meta = format!(
            "{}:{:02}{}",
            date.get_hours(),
            date.get_minutes(),
            if message.encrypted {
                " · encrypted"
            } else {
                ""
            }
        );
        bubble.append_child(&create_text("small", &meta)).unwrap();
        row.append_child(&bubble).unwrap();
        container.append_child(&row).unwrap();
    }
    container.set_scroll_top(container.scroll_height());
}

fn input_value(form: &str, name: &str) -> String {
    element(form)
        .query_selector(&format!("[name={name}]"))
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()
        .unwrap()
        .value()
}

fn input(id: &str) -> web_sys::HtmlInputElement {
    element(id).dyn_into::<web_sys::HtmlInputElement>().unwrap()
}

fn message(id: &str) -> web_sys::HtmlTextAreaElement {
    element(id)
        .dyn_into::<web_sys::HtmlTextAreaElement>()
        .unwrap()
}

fn button(id: &str) -> web_sys::HtmlButtonElement {
    element(id)
        .dyn_into::<web_sys::HtmlButtonElement>()
        .unwrap()
}

fn element(id: &str) -> web_sys::Element {
    document().get_element_by_id(id).unwrap()
}

fn create(tag: &str, class: &str) -> web_sys::Element {
    let element = document().create_element(tag).unwrap();
    element.set_class_name(class);
    element
}

fn create_text(tag: &str, text: &str) -> web_sys::Element {
    let element = create(tag, "");
    element.set_text_content(Some(text));
    element
}

fn set_hidden(element: &web_sys::Element, hidden: bool) {
    if hidden {
        element.set_attribute("hidden", "").unwrap();
    } else {
        element.remove_attribute("hidden").unwrap();
    }
}

fn document() -> web_sys::Document {
    web_sys::window().unwrap().document().unwrap()
}
