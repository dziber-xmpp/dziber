mod call;
mod model;
mod store;
mod ui;
mod xmpp;

use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return Ok(());
    };
    let Some(root) = document.get_element_by_id("chat") else {
        return Ok(());
    };
    let chat = Rc::new(RefCell::new(model::Chat::new(&root)));
    ui::bind(&chat, &root);
    ui::render(&chat);
    Ok(())
}
