use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Write};
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use phonelink_clipboard::image::{ClipboardImageCodec, ClipboardImageSize};
use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, uti};
use phonelink_engine::edge::EdgeError;
pub use x11::X11Pasteboard;

mod x11;

static NEXT_SOURCE_ID: AtomicI64 = AtomicI64::new(0);

fn own_source_mime() -> String {
    format!(
        "application/x-phonelink-source;pid={};id={}",
        std::process::id(),
        NEXT_SOURCE_ID.fetch_add(1, Ordering::SeqCst)
    )
}
const TEXT_MIMES: [&str; 4] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING"];
/// KDE's convention (also set by KeePassXC) for clipboard content that history must not keep.
const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";
const PASSWORD_HINT_VALUE: &[u8] = b"secret";
const READ_TIMEOUT: Duration = Duration::from_secs(2);

fn mimes_for(uti_name: &str) -> &'static [&'static str] {
    match uti_name {
        uti::PLAIN_TEXT => &TEXT_MIMES,
        uti::HTML => &["text/html"],
        uti::PNG => &["image/png"],
        uti::TIFF => &["image/tiff"],
        uti::FILE_URL => &["text/uri-list"],
        uti::CONCEALED | uti::TRANSIENT => &[PASSWORD_HINT_MIME],
        _ => &[],
    }
}

const READABLE_UTIS: [&str; 6] = [uti::PNG, uti::TIFF, uti::HTML, uti::PLAIN_TEXT, uti::FILE_URL, uti::CONCEALED];

/// Wayland's data-control protocols reach the clipboard without focus; GNOME's Mutter has neither,
/// so there and in X11 sessions the X11 selection (through XWayland on GNOME) is used.
pub fn system_pasteboard() -> Result<Arc<dyn ClipboardPasteboard>, String> {
    let mut failures = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match ext::WaylandPasteboard::connect() {
            Ok(wayland) => return Ok(Arc::new(wayland)),
            Err(e) => failures.push(format!("Wayland: {e}")),
        }
        match wlr::WaylandPasteboard::connect() {
            Ok(wayland) => return Ok(Arc::new(wayland)),
            Err(e) => failures.push(format!("Wayland: {e}")),
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        match X11Pasteboard::connect() {
            Ok(x11) => return Ok(Arc::new(x11)),
            Err(e) => failures.push(e),
        }
    }
    Err(if failures.is_empty() { "no display".to_owned() } else { failures.join("; ") })
}

fn mime_data(representations: Vec<ClipboardPasteboardData>) -> HashMap<String, Arc<Vec<u8>>> {
    let mut data: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
    for representation in representations {
        let bytes = if mimes_for(&representation.uti) == [PASSWORD_HINT_MIME] {
            Arc::new(PASSWORD_HINT_VALUE.to_vec())
        } else {
            Arc::new(representation.data)
        };
        for mime in mimes_for(&representation.uti) {
            data.entry((*mime).to_owned()).or_insert_with(|| bytes.clone());
        }
    }
    data
}

fn first_offered<'a>(uti_name: &str, offered: &'a [String]) -> Option<&'a str> {
    mimes_for(uti_name).iter().find_map(|m| offered.iter().find(|o| o == m).map(String::as_str))
}

pub fn utis_of(mimes: &[String]) -> Vec<String> {
    READABLE_UTIS
        .into_iter()
        .filter(|u| mimes_for(u).iter().any(|m| mimes.iter().any(|offered| offered == m)))
        .map(str::to_owned)
        .collect()
}

fn edge_err(e: impl std::fmt::Display) -> EdgeError {
    EdgeError::Unexpected { message: format!("clipboard: {e}") }
}

mod ext {
    use wayland_protocols::ext::data_control::v1::client::{
        ext_data_control_device_v1 as device_v1, ext_data_control_device_v1::ExtDataControlDeviceV1 as Device,
        ext_data_control_manager_v1::ExtDataControlManagerV1 as Manager, ext_data_control_offer_v1 as offer_v1,
        ext_data_control_offer_v1::ExtDataControlOfferV1 as Offer, ext_data_control_source_v1 as source_v1,
        ext_data_control_source_v1::ExtDataControlSourceV1 as Source,
    };

    const MANAGER_INTERFACE: &str = "ext_data_control_manager_v1";

    include!("clipboard/wayland.rs");
}

mod wlr {
    use wayland_protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1 as device_v1, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1 as Device,
        zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 as Manager, zwlr_data_control_offer_v1 as offer_v1,
        zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 as Offer, zwlr_data_control_source_v1 as source_v1,
        zwlr_data_control_source_v1::ZwlrDataControlSourceV1 as Source,
    };

    const MANAGER_INTERFACE: &str = "zwlr_data_control_manager_v1";

    include!("clipboard/wayland.rs");
}

fn send_bytes(fd: OwnedFd, bytes: &[u8]) {
    if let Err(e) = File::from(fd).write_all(bytes) {
        eprintln!("clipboard data was not sent: {e}");
    }
}

pub struct ImageCrateCodec;

fn decode(bytes: &[u8]) -> Result<DynamicImage, EdgeError> {
    image::load_from_memory(bytes).map_err(|e| EdgeError::Unexpected { message: format!("image decode: {e}") })
}

fn encode(image: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>, EdgeError> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.to_rgba8())
        .write_to(&mut bytes, format)
        .map_err(|e| EdgeError::Unexpected { message: format!("image encode: {e}") })?;
    Ok(bytes.into_inner())
}

impl ClipboardImageCodec for ImageCrateCodec {
    fn size(&self, image: Vec<u8>) -> Result<ClipboardImageSize, EdgeError> {
        let decoded = decode(&image)?;
        Ok(ClipboardImageSize { width: decoded.width(), height: decoded.height() })
    }

    fn encode_png(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
        encode(&decode(&image)?, ImageFormat::Png)
    }

    fn resize_png(&self, image: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, EdgeError> {
        encode(&decode(&image)?.resize_exact(width, height, FilterType::CatmullRom), ImageFormat::Png)
    }

    fn encode_tiff(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
        encode(&decode(&image)?, ImageFormat::Tiff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mimes(list: &[&str]) -> Vec<String> {
        list.iter().map(|m| (*m).to_owned()).collect()
    }

    #[test]
    fn offered_mimes_map_to_utis() {
        assert_eq!(utis_of(&mimes(&["UTF8_STRING", "text/html"])), [uti::HTML, uti::PLAIN_TEXT]);
        assert_eq!(utis_of(&mimes(&["image/png", "x-kde-passwordManagerHint"])), [uti::PNG, uti::CONCEALED]);
        assert!(utis_of(&mimes(&["application/x-unknown"])).is_empty());
    }

    #[test]
    #[ignore = "needs a Wayland session and replaces its clipboard"]
    fn round_trips_through_the_live_wayland_clipboard() {
        let ours = ext::WaylandPasteboard::connect().unwrap();
        let other = ext::WaylandPasteboard::connect().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let original = other.data(uti::PLAIN_TEXT.to_owned()).unwrap();
        let count = other.change_count().unwrap();

        let written = ours
            .write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: b"phonelink".to_vec() }])
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(ours.change_count().unwrap(), written.change_count);
        assert!(other.change_count().unwrap() > count);
        assert!(other.types().unwrap().contains(&uti::PLAIN_TEXT.to_owned()));
        assert_eq!(other.data(uti::PLAIN_TEXT.to_owned()).unwrap().as_deref(), Some(&b"phonelink"[..]));

        if let Some(original) = original {
            ours.write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: original }]).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    #[test]
    fn png_round_trips_through_resize() {
        let png = encode(&DynamicImage::new_rgba8(8, 4), ImageFormat::Png).unwrap();
        let resized = ImageCrateCodec.resize_png(png, 4, 2).unwrap();
        assert_eq!(ImageCrateCodec.size(resized).unwrap(), ClipboardImageSize { width: 4, height: 2 });
    }
}
