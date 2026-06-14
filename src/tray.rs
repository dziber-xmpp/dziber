use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    ShowRequested,
    QuitRequested,
}

static UNREAD_COUNT: AtomicU32 = AtomicU32::new(0);
static EVENT_RX: OnceLock<Mutex<mpsc::Receiver<TrayEvent>>> = OnceLock::new();

/// Initialize the platform tray icon.
///
/// On Linux/BSD this registers a D-Bus StatusNotifierItem via `ksni`.
/// On other platforms it is a no-op.
pub fn init_tray() {
    sni_tray::init_tray_impl();
}

/// Poll for a tray event that should be handled by the UI event loop.
pub fn try_recv_event() -> Option<TrayEvent> {
    let rx = EVENT_RX.get()?;
    let guard = rx.lock().ok()?;
    guard.try_recv().ok()
}

/// Update the unread-message badge shown on the tray icon.
pub fn set_unread_count(count: u32) {
    UNREAD_COUNT.store(count, Ordering::Relaxed);
    sni_tray::refresh_icon();
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
pub(crate) mod sni_tray {
    use super::{TrayEvent, EVENT_RX, UNREAD_COUNT};
    use image::{Rgba, RgbaImage};
    use ksni::TrayMethods;
    use std::sync::atomic::Ordering;
    use std::sync::{Mutex, OnceLock, mpsc};

    static HANDLE: OnceLock<ksni::Handle<DziberTray>> = OnceLock::new();

    pub(super) fn init_tray_impl() {
        if EVENT_RX.get().is_some() {
            return;
        }

        let (tx, rx) = mpsc::channel();
        let _ = EVENT_RX.set(Mutex::new(rx));

        let tray_impl = DziberTray { events: tx };

        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => match runtime.block_on(tray_impl.spawn()) {
                Ok(handle) => {
                    let _ = HANDLE.set(handle);
                }
                Err(error) => {
                    tracing::info!("Failed to create SNI tray icon: {error}");
                }
            },
            Err(error) => {
                tracing::info!("No Tokio runtime available for SNI tray: {error}");
            }
        }
    }

    pub(super) fn refresh_icon() {
        if let Some(handle) = HANDLE.get() {
            let handle = handle.clone();
            tokio::spawn(async move {
                let _ = handle.update(|_| {}).await;
            });
        }
    }

    pub(crate) struct DziberTray {
        pub(crate) events: mpsc::Sender<TrayEvent>,
    }

    impl ksni::Tray for DziberTray {
        fn id(&self) -> String {
            "dziber".into()
        }

        fn title(&self) -> String {
            format!("Dziber - {} unread", UNREAD_COUNT.load(Ordering::Relaxed))
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![ksni_icon(UNREAD_COUNT.load(Ordering::Relaxed))]
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{MenuItem, StandardItem};

            vec![
                StandardItem {
                    label: "Show Dziber".into(),
                    activate: Box::new(|this: &mut DziberTray| {
                        let _ = this.events.send(TrayEvent::ShowRequested);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit Dziber".into(),
                    activate: Box::new(|this: &mut DziberTray| {
                        let _ = this.events.send(TrayEvent::QuitRequested);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.events.send(TrayEvent::ShowRequested);
        }
    }

    pub(crate) fn ksni_icon(count: u32) -> ksni::Icon {
        let (rgba, width, height) = render_rgba(count);
        let mut data = rgba;

        // ksni expects ARGB data; `image` gives RGBA.
        for pixel in data.chunks_exact_mut(4) {
            pixel.rotate_right(1);
        }

        ksni::Icon {
            width: width as i32,
            height: height as i32,
            data,
        }
    }

    pub(crate) fn render_rgba(count: u32) -> (Vec<u8>, u32, u32) {
        const SIZE: u32 = 64;
        let mut img = RgbaImage::from_pixel(SIZE, SIZE, Rgba([46, 134, 222, 255]));

        draw_envelope(&mut img);

        if count > 0 {
            draw_badge(&mut img, count.min(99));
        }

        (img.into_raw(), SIZE, SIZE)
    }

    pub(crate) fn draw_envelope(img: &mut RgbaImage) {
        let white = Rgba([255, 255, 255, 255]);
        let left = 12i32;
        let right = 52i32;
        let top = 20i32;
        let bottom = 44i32;

        // Envelope body.
        for y in top..=bottom {
            for x in left..=right {
                img.put_pixel(x as u32, y as u32, white);
            }
        }

        // Top flap outline.
        draw_line(img, left, top, 32, 34, Rgba([46, 134, 222, 255]));
        draw_line(img, 32, 34, right, top, Rgba([46, 134, 222, 255]));
    }

    pub(crate) fn draw_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba<u8>) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut x = x0;
        let mut y = y0;

        loop {
            if x >= 0 && y >= 0 && x < img.width() as i32 && y < img.height() as i32 {
                img.put_pixel(x as u32, y as u32, color);
            }
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub(crate) fn draw_badge(img: &mut RgbaImage, count: u32) {
        let red = Rgba([220, 53, 69, 255]);
        let white = Rgba([255, 255, 255, 255]);
        let radius = 14u32;
        let cx = img.width() - radius - 2;
        let cy = radius + 2;

        for y in cy.saturating_sub(radius)..(cy + radius).min(img.height()) {
            for x in cx.saturating_sub(radius)..(cx + radius).min(img.width()) {
                let dx = x as i32 - cx as i32;
                let dy = y as i32 - cy as i32;
                if dx * dx + dy * dy <= (radius * radius) as i32 {
                    img.put_pixel(x, y, red);
                }
            }
        }

        let text = format!("{count}");
        let char_width = 4i32;
        let char_height = 7i32;
        let total_width = text.len() as i32 * char_width + (text.len() as i32 - 1).max(0);
        let start_x = cx as i32 - total_width / 2;
        let start_y = cy as i32 - char_height / 2;

        for (i, ch) in text.chars().enumerate() {
            let digit = ch.to_digit(10).unwrap_or(0) as usize;
            let offset_x = start_x + i as i32 * (char_width + 1);
            draw_digit(img, digit, offset_x, start_y, white);
        }
    }

    pub(crate) fn draw_digit(img: &mut RgbaImage, digit: usize, x: i32, y: i32, color: Rgba<u8>) {
        #[rustfmt::skip]
        const FONT: [[u8; 7]; 10] = [
            [15, 9, 9, 9, 9, 9, 15],
            [1, 3, 1, 1, 1, 1, 7],
            [15, 1, 15, 8, 8, 8, 15],
            [15, 1, 15, 1, 1, 1, 15],
            [9, 9, 15, 1, 1, 1, 1],
            [15, 8, 15, 1, 1, 1, 15],
            [15, 8, 15, 9, 9, 9, 15],
            [15, 1, 1, 2, 2, 4, 4],
            [15, 9, 15, 9, 9, 9, 15],
            [15, 9, 15, 1, 1, 1, 15],
        ];

        let pattern = FONT.get(digit).copied().unwrap_or(FONT[0]);
        for (row, bits) in pattern.iter().enumerate() {
            for col in 0..4 {
                if (bits >> (3 - col)) & 1 == 1 {
                    let px = x + col;
                    let py = y + row as i32;
                    if px >= 0 && py >= 0 && px < img.width() as i32 && py < img.height() as i32 {
                        img.put_pixel(px as u32, py as u32, color);
                    }
                }
            }
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
mod sni_tray {
    use super::{EVENT_RX, Mutex, mpsc};

    pub(super) fn init_tray_impl() {
        let _ = EVENT_RX.set(Mutex::new(mpsc::channel().1));
    }

    pub(super) fn refresh_icon() {}
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn try_recv_event_before_init_returns_none() {
        // EVENT_RX has not been set yet, so polling should return None.
        assert!(try_recv_event().is_none());
    }

    #[test]
    fn set_unread_count_updates_static() {
        set_unread_count(12);
        assert_eq!(UNREAD_COUNT.load(Ordering::Relaxed), 12);
        set_unread_count(0);
        assert_eq!(UNREAD_COUNT.load(Ordering::Relaxed), 0);
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    mod unix {
        use super::sni_tray::*;
        use super::{UNREAD_COUNT};
        use image::{Rgba, RgbaImage};
        use ksni::Tray;
        use ksni::menu::MenuItem;
        use std::sync::atomic::Ordering;

        fn test_tray() -> DziberTray {
            let (tx, _rx) = std::sync::mpsc::channel();
            DziberTray { events: tx }
        }

        #[test]
        fn tray_title_reflects_count() {
            let tray = test_tray();
            UNREAD_COUNT.store(5, Ordering::Relaxed);
            assert_eq!(tray.title(), "Dziber - 5 unread");
            UNREAD_COUNT.store(0, Ordering::Relaxed);
        }

        #[test]
        fn tray_menu_has_show_and_quit() {
            let tray = test_tray();
            let items = tray.menu();
            assert_eq!(items.len(), 3);
            match &items[0] {
                MenuItem::Standard(item) => assert_eq!(item.label, "Show Dziber"),
                _ => panic!("expected Show standard item"),
            }
            assert!(matches!(items[1], MenuItem::Separator));
            match &items[2] {
                MenuItem::Standard(item) => assert_eq!(item.label, "Quit Dziber"),
                _ => panic!("expected Quit standard item"),
            }
        }

        #[test]
        fn ksni_icon_has_expected_size() {
            let icon = ksni_icon(7);
            assert_eq!(icon.width, 64);
            assert_eq!(icon.height, 64);
            assert_eq!(icon.data.len(), 64 * 64 * 4);
        }

        #[test]
        fn render_rgba_produces_64x64_image() {
            let (data, width, height) = render_rgba(3);
            assert_eq!(width, 64);
            assert_eq!(height, 64);
            assert_eq!(data.len(), 64 * 64 * 4);
        }

        #[test]
        fn draw_line_changes_pixels() {
            let mut img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 255]));
            draw_line(&mut img, 0, 0, 63, 63, Rgba([255, 255, 255, 255]));
            assert_eq!(*img.get_pixel(32, 32), Rgba([255, 255, 255, 255]));
        }

        #[test]
        fn draw_digit_changes_pixels() {
            let mut img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 255]));
            draw_digit(&mut img, 8, 10, 10, Rgba([255, 255, 255, 255]));
            let pixel = img.get_pixel(10, 10);
            assert!(pixel.0.iter().take(3).any(|c| *c > 0));
        }

        #[test]
        fn draw_badge_draws_red_circle() {
            let mut img = RgbaImage::from_pixel(64, 64, Rgba([46, 134, 222, 255]));
            draw_badge(&mut img, 5);
            // Center of the badge (cx = 64 - 14 - 2 = 48, cy = 14 + 2 = 16).
            assert_eq!(*img.get_pixel(48, 16), Rgba([220, 53, 69, 255]));
        }

        #[test]
        fn draw_envelope_draws_white_body() {
            let mut img = RgbaImage::from_pixel(64, 64, Rgba([46, 134, 222, 255]));
            draw_envelope(&mut img);
            assert_eq!(*img.get_pixel(20, 30), Rgba([255, 255, 255, 255]));
        }
    }
}
