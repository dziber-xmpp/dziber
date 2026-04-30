use std::sync::OnceLock;

static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_inited() -> Result<(), String> {
    INIT_RESULT
        .get_or_init(|| libnotify::init("Dziber"))
        .clone()
}

pub fn incoming_message(from: &str, body: &str) {
    let mut preview = body.trim().replace('\n', " ");
    if preview.len() > 200 {
        preview.truncate(200);
        preview.push_str("...");
    }
    if preview.is_empty() {
        preview = "(empty message)".to_string();
    }

    if let Err(err) = ensure_inited() {
        tracing::info!("Notification init failed: {err}");
        return;
    }

    let n = libnotify::Notification::new(
        &format!("Message from {from}"),
        Some(preview.as_str()),
        Some("mail-message-new"),
    );
    if let Err(err) = n.show() {
        tracing::info!("Notification show failed: {err}");
    }
}
