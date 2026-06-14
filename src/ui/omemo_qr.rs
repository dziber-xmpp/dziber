use qrcodegen::{QrCode, QrCodeEcc};

use dziber_omemo::OmemoManager;

fn hex_lower(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn build_share_uri(jid: &str) -> Option<String> {
    let mut mgr = OmemoManager::load_or_generate(1, Box::new(crate::db::omemo::DziberOmemoStore));
    mgr.set_our_jid(jid);
    let device_id = mgr.our_device_id();
    let fp = hex_lower(&mgr.account.inner.curve25519_key().to_bytes());
    Some(format!("xmpp:{jid}?omemo-sid-{device_id}={fp}"))
}

pub fn build_qr_rgba(uri: &str) -> Option<(u32, u32, Vec<u8>)> {
    let qr = QrCode::encode_text(uri, QrCodeEcc::Medium).ok()?;
    let border: i32 = 4;
    let scale: i32 = 8;
    let size = qr.size();
    let dim = (size + border * 2) * scale;
    let w = u32::try_from(dim).ok()?;
    let h = w;
    let mut rgba = vec![255u8; (w as usize) * (h as usize) * 4];

    for y in 0..dim {
        for x in 0..dim {
            let xx = x / scale - border;
            let yy = y / scale - border;
            let dark = xx >= 0 && yy >= 0 && xx < size && yy < size && qr.get_module(xx, yy);
            let v = if dark { 0u8 } else { 255u8 };
            let idx = ((y as usize) * (w as usize) + (x as usize)) * 4;
            rgba[idx] = v;
            rgba[idx + 1] = v;
            rgba[idx + 2] = v;
            rgba[idx + 3] = 255;
        }
    }

    Some((w, h, rgba))
}
