use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    ShowRequested,
    QuitRequested,
}

static TRAY_EVENTS: OnceLock<Mutex<std::sync::mpsc::Receiver<TrayEvent>>> = OnceLock::new();
static UNREAD_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn init_tray() {
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    gtk_tray::init_tray_impl();
}

pub fn try_recv_event() -> Option<TrayEvent> {
    let rx = TRAY_EVENTS.get()?;
    let guard = rx.lock().ok()?;
    guard.try_recv().ok()
}

/// Update the unread-message badge shown on the tray icon.
pub fn set_unread_count(count: u32) {
    UNREAD_COUNT.store(count, Ordering::Relaxed);
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
mod gtk_tray {
    use std::f64::consts::PI;
    use std::os::raw::c_void;

    use gtk::glib::prelude::*;
    use gtk::prelude::*;

    use glib_sys::{gboolean, gpointer};
    use gobject_sys::{GObject, g_signal_connect_data};
    use gtk_sys::{
        GtkMenu, GtkMenuItem, GtkMenuShell, GtkStatusIcon, gtk_init_check, gtk_main,
        gtk_menu_item_new_with_label, gtk_menu_new, gtk_menu_popup, gtk_menu_shell_append,
        gtk_status_icon_new, gtk_status_icon_set_from_pixbuf, gtk_status_icon_set_tooltip_text,
        gtk_status_icon_set_visible, gtk_widget_show_all,
    };

    use super::{TRAY_EVENTS, TrayEvent, UNREAD_COUNT};

    /// Render the themed icon with a red unread-count badge.
    fn render_badge_pixbuf(count: u32) -> Option<gtk::gdk_pixbuf::Pixbuf> {
        let theme = gtk::IconTheme::default()?;
        let pixbuf = theme
            .load_icon("mail-message-new", 24, gtk::IconLookupFlags::empty())
            .ok()
            .flatten()?;

        let width = pixbuf.width();
        let height = pixbuf.height();
        let surface =
            gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height).ok()?;
        let cr = gtk::cairo::Context::new(&surface).ok()?;

        cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
        cr.paint().ok()?;

        if count > 0 {
            let radius = (width.min(height) as f64) * 0.28;
            let cx = width as f64 - radius - 1.0;
            let cy = radius + 1.0;

            // Red badge circle.
            cr.set_source_rgb(0.9, 0.1, 0.1);
            cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
            cr.fill().ok()?;

            // White count text.
            let text = if count > 99 {
                "99+".to_string()
            } else {
                count.to_string()
            };
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.select_font_face(
                "Sans",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Bold,
            );
            cr.set_font_size(radius * 1.1);
            let ext = cr.text_extents(&text).ok()?;
            cr.move_to(
                cx - ext.width / 2.0 - ext.x_bearing,
                cy + ext.height / 2.0 - ext.y_bearing,
            );
            cr.show_text(&text).ok()?;
        }

        gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height)
    }

    extern "C" fn on_tray_activate(_tray: *mut GtkStatusIcon, user_data: gpointer) {
        let sender = unsafe { &*(user_data as *const std::sync::mpsc::Sender<TrayEvent>) };
        let _ = sender.send(TrayEvent::ShowRequested);
    }

    extern "C" fn on_popup_menu(
        _tray: *mut GtkStatusIcon,
        button: u32,
        activate_time: u32,
        user_data: gpointer,
    ) {
        let menu = user_data as *mut GtkMenu;
        unsafe {
            gtk_menu_popup(
                menu,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
                button,
                activate_time,
            );
        }
    }

    extern "C" fn on_show_activate(_item: *mut GtkMenuItem, user_data: gpointer) {
        let sender = unsafe { &*(user_data as *const std::sync::mpsc::Sender<TrayEvent>) };
        let _ = sender.send(TrayEvent::ShowRequested);
    }

    extern "C" fn on_quit_activate(_item: *mut GtkMenuItem, user_data: gpointer) {
        let sender = unsafe { &*(user_data as *const std::sync::mpsc::Sender<TrayEvent>) };
        let _ = sender.send(TrayEvent::QuitRequested);
    }

    extern "C" fn on_refresh_icon(tray: gpointer) -> gboolean {
        let count = UNREAD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(pb) = render_badge_pixbuf(count) {
            unsafe {
                gtk_status_icon_set_from_pixbuf(tray as *mut GtkStatusIcon, pb.as_ptr());
            }
        }
        1 // keep repeating
    }

    pub fn init_tray_impl() {
        if TRAY_EVENTS.get().is_some() {
            return;
        }

        let (event_tx, event_rx) = std::sync::mpsc::channel::<TrayEvent>();
        let _ = TRAY_EVENTS.set(std::sync::Mutex::new(event_rx));

        std::thread::spawn(move || {
            let ok: gboolean =
                unsafe { gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) };
            if ok == 0 {
                tracing::info!("Failed to init GTK tray");
                return;
            }

            let tooltip = std::ffi::CString::new("Dziber").expect("tooltip");
            let show_label = std::ffi::CString::new("Show Dziber").expect("show label");
            let quit_label = std::ffi::CString::new("Quit Dziber").expect("quit label");
            let activate_sig = std::ffi::CString::new("activate").expect("activate signal");
            let popup_sig = std::ffi::CString::new("popup-menu").expect("popup signal");

            let show_tx_box = Box::new(event_tx.clone());
            let quit_tx_box = Box::new(event_tx.clone());
            let tray_tx_box = Box::new(event_tx.clone());

            let tray = unsafe { gtk_status_icon_new() };
            if tray.is_null() {
                tracing::info!("Failed to create GTK status icon");
                return;
            }
            unsafe {
                gtk_status_icon_set_visible(tray, 1);
                gtk_status_icon_set_tooltip_text(tray, tooltip.as_ptr());
            }

            // Set initial icon.
            let count = UNREAD_COUNT.load(std::sync::atomic::Ordering::Relaxed);
            if let Some(pb) = render_badge_pixbuf(count) {
                unsafe {
                    gtk_status_icon_set_from_pixbuf(tray, pb.as_ptr());
                }
            }

            let menu = unsafe { gtk_menu_new() };
            if menu.is_null() {
                tracing::info!("Failed to create GTK tray menu");
                return;
            }

            let show_item = unsafe { gtk_menu_item_new_with_label(show_label.as_ptr()) };
            let quit_item = unsafe { gtk_menu_item_new_with_label(quit_label.as_ptr()) };
            unsafe {
                gtk_menu_shell_append(menu as *mut GtkMenuShell, show_item as *mut GtkMenuItem);
                gtk_menu_shell_append(menu as *mut GtkMenuShell, quit_item as *mut GtkMenuItem);
                gtk_widget_show_all(menu);
            }

            unsafe {
                g_signal_connect_data(
                    tray as *mut GObject,
                    activate_sig.as_ptr(),
                    Some(std::mem::transmute::<
                        extern "C" fn(*mut GtkStatusIcon, gpointer),
                        unsafe extern "C" fn(),
                    >(on_tray_activate)),
                    Box::into_raw(tray_tx_box) as *mut c_void,
                    None,
                    0,
                );

                g_signal_connect_data(
                    tray as *mut GObject,
                    popup_sig.as_ptr(),
                    Some(std::mem::transmute::<
                        extern "C" fn(*mut GtkStatusIcon, u32, u32, gpointer),
                        unsafe extern "C" fn(),
                    >(on_popup_menu)),
                    menu as *mut c_void,
                    None,
                    0,
                );

                g_signal_connect_data(
                    show_item as *mut GObject,
                    activate_sig.as_ptr(),
                    Some(std::mem::transmute::<
                        extern "C" fn(*mut GtkMenuItem, gpointer),
                        unsafe extern "C" fn(),
                    >(on_show_activate)),
                    Box::into_raw(show_tx_box) as *mut c_void,
                    None,
                    0,
                );

                g_signal_connect_data(
                    quit_item as *mut GObject,
                    activate_sig.as_ptr(),
                    Some(std::mem::transmute::<
                        extern "C" fn(*mut GtkMenuItem, gpointer),
                        unsafe extern "C" fn(),
                    >(on_quit_activate)),
                    Box::into_raw(quit_tx_box) as *mut c_void,
                    None,
                    0,
                );
            }

            unsafe {
                glib_sys::g_timeout_add(
                    500,
                    Some(std::mem::transmute::<
                        extern "C" fn(gpointer) -> gboolean,
                        unsafe extern "C" fn(gpointer) -> gboolean,
                    >(on_refresh_icon)),
                    tray as gpointer,
                );
            }

            unsafe { gtk_main() };
        });
    }
}
