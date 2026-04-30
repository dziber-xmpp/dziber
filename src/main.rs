mod db;
mod models;
mod notify;
mod omemo;
mod tray;
mod ui;
mod xmpp;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use ui::app;

fn init_tracing() {
    let log_path = dirs::home_dir()
        .map(|p| p.join(".log"))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("dziber.log");

    if let Some(parent) = log_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing_subscriber::fmt().with_ansi(false).init();
        tracing::error!("Failed to create log directory {:?}: {}", parent, err);
        return;
    }

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            let (non_blocking, _guard) = tracing_appender::non_blocking(file);
            let _ = Box::leak(Box::new(_guard));
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(non_blocking),
                )
                .init();
            tracing::info!("Tracing initialized at {}", log_path.display());
        }
        Err(err) => {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer().with_ansi(false))
                .init();
            tracing::error!(
                "Failed to open {}: {}. Falling back to stderr.",
                log_path.display(),
                err
            );
        }
    }
}

fn main() -> iced::Result {
    let debug_logging = std::env::args().any(|arg| arg == "--debug");
    if debug_logging {
        init_tracing();
    }
    crate::tray::init_tray();

    iced::application(app::boot, app::update, app::view)
        .subscription(app::subscription)
        .theme(app::theme)
        .title("Dziber - XMPP Client")
        .window(iced::window::Settings {
            visible: false,
            exit_on_close_request: false,
            ..Default::default()
        })
        .run()
}
