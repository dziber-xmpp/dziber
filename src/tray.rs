use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    ShowRequested,
    QuitRequested,
}

static TRAY_EVENTS: OnceLock<Mutex<std::sync::mpsc::Receiver<TrayEvent>>> = OnceLock::new();

pub fn init_tray() {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    gtk_tray::init_tray_impl();
}

pub fn try_recv_event() -> Option<TrayEvent> {
    let rx = TRAY_EVENTS.get()?;
    let guard = rx.lock().ok()?;
    guard.try_recv().ok()
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
mod gtk_tray {
    use std::ffi::CString;
    use std::os::raw::c_void;

    use glib_sys::{gboolean, gpointer};
    use gobject_sys::{GObject, g_signal_connect_data};
    use gtk_sys::{
        GtkMenu, GtkMenuItem, GtkMenuShell, GtkStatusIcon, GtkWidget, gtk_init_check, gtk_main,
        gtk_menu_item_new_with_label, gtk_menu_new, gtk_menu_popup, gtk_menu_shell_append,
        gtk_status_icon_new_from_icon_name, gtk_status_icon_set_tooltip_text,
        gtk_status_icon_set_visible, gtk_widget_show_all,
    };

    use super::{TRAY_EVENTS, TrayEvent};

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

    pub fn init_tray_impl() {
        if TRAY_EVENTS.get().is_some() {
            return;
        }

        let (event_tx, event_rx) = std::sync::mpsc::channel::<TrayEvent>();
        let _ = TRAY_EVENTS.set(std::sync::Mutex::new(event_rx));

        std::thread::spawn(move || {
            let ok: gboolean = unsafe { gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) };
            if ok == 0 {
                tracing::info!("Failed to init GTK tray");
                return;
            }

            let icon_name = CString::new("mail-message-new").expect("icon name");
            let tooltip = CString::new("Dziber").expect("tooltip");
            let show_label = CString::new("Show Dziber").expect("show label");
            let quit_label = CString::new("Quit Dziber").expect("quit label");
            let activate_sig = CString::new("activate").expect("activate signal");
            let popup_sig = CString::new("popup-menu").expect("popup signal");

            let show_tx_box = Box::new(event_tx.clone());
            let quit_tx_box = Box::new(event_tx.clone());
            let tray_tx_box = Box::new(event_tx.clone());

            let tray = unsafe { gtk_status_icon_new_from_icon_name(icon_name.as_ptr()) };
            if tray.is_null() {
                tracing::info!("Failed to create GTK status icon");
                return;
            }
            unsafe {
                gtk_status_icon_set_visible(tray, 1);
                gtk_status_icon_set_tooltip_text(tray, tooltip.as_ptr());
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
                gtk_widget_show_all(menu as *mut GtkWidget);
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

            unsafe { gtk_main() };
        });
    }
}
