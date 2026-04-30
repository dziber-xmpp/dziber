mod db;
mod models;
mod notify;
mod omemo;
mod tray;
mod ui;
mod xmpp;

use ui::app;

fn main() -> iced::Result {
    crate::tray::init_tray();

    iced::application(app::boot, app::update, app::view)
        .subscription(app::subscription)
        .theme(app::theme)
        .title("Dziber - XMPP Client")
        .exit_on_close_request(false)
        .run()
}
