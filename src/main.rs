mod db;
mod models;
mod omemo;
mod ui;
mod xmpp;

use ui::app;

fn main() -> iced::Result {
    iced::application(app::boot, app::update, app::view)
        .subscription(app::subscription)
        .theme(app::theme)
        .title("Dziber - XMPP Client")
        .run()
}
