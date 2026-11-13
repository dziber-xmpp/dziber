pub mod models;

#[cfg(feature = "omemo")]
pub mod omemo;

#[cfg(any(feature = "desktop", feature = "mail", feature = "calendar"))]
pub mod personal_data;

#[cfg(feature = "desktop")]
pub mod audio;
#[cfg(feature = "desktop")]
pub mod call;
#[cfg(feature = "desktop")]
pub mod db;
#[cfg(feature = "desktop")]
pub mod notify;
#[cfg(feature = "desktop")]
pub mod secrets;
#[cfg(feature = "desktop")]
pub mod tray;
#[cfg(feature = "desktop")]
pub mod ui;
#[cfg(feature = "desktop")]
pub mod xmpp;

#[cfg(feature = "web")]
pub mod web;
