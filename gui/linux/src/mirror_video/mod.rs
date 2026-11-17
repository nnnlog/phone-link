//! The `MirrorVideo` QML item: a mirroring session's latest picture, drawn with its aspect kept.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QImage, QImageFormat, QPainterRenderHint, QRect, QString};

use crate::mirror::presenter;

// cxx-qt registers every bridge of the QML module from one folder (see `engine_bridge`).
#[expect(unsafe_code, reason = "a cxx-qt bridge declares functions that cross the C++ boundary")]
#[expect(
    unused_qualifications,
    reason = "the lint fires on cxx-qt's expansion of the bridge's `Constructor` impl, not on code written there"
)]
#[path = "../bridges/mirror_video.rs"]
mod bridge;

use bridge::qobject::{MirrorVideo, QPainter};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Where a picture `picture` pixels large is drawn in an item `item` large.
///
/// It is as large as fits with the picture's aspect, centered, so all of it shows undistorted; whole
/// pixels as x, y, width and height. `None` when either has no area.
fn aspect_fit(picture: (usize, usize), item: (f64, f64)) -> Option<[i32; 4]> {
    let has_area = |width: f64, height: f64| width > 0.0 && height > 0.0;
    let (width, height) = (picture.0 as f64, picture.1 as f64);
    if !has_area(width, height) || !has_area(item.0, item.1) {
        return None;
    }
    let scale = (item.0 / width).min(item.1 / height);
    let (shown_width, shown_height) = (width * scale, height * scale);
    let (x, y) = ((item.0 - shown_width) / 2.0, (item.1 - shown_height) / 2.0);
    Some([x.round() as i32, y.round() as i32, shown_width.round() as i32, shown_height.round() as i32])
}

/// The Rust side of a `MirrorVideo` item: the session it shows, and its attachment to that session's
/// stream.
#[derive(Default)]
pub struct MirrorVideoRust {
    session_id: QString,
    attached: Option<(String, u64)>,
}

impl Drop for MirrorVideoRust {
    fn drop(&mut self) {
        if let Some((session, token)) = self.attached.take() {
            presenter().detach(&session, token);
        }
    }
}

impl cxx_qt::Initialize for MirrorVideo {
    fn initialize(self: Pin<&mut Self>) {}
}

impl MirrorVideo {
    /// Paints the session `sessionId` names from now on, in place of the one it painted.
    pub fn attach(mut self: Pin<&mut Self>) {
        let session = self.session_id.to_string();
        if self.attached.as_ref().is_some_and(|(attached, _)| *attached == session) {
            return;
        }
        if let Some((old, token)) = self.as_mut().rust_mut().attached.take() {
            presenter().detach(&old, token);
        }
        if session.is_empty() {
            return;
        }

        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let thread = self.qt_thread();
        presenter().attach(
            &session,
            token,
            Arc::new(move || {
                let _ = thread.queue(|item| item.update());
            }),
        );
        self.as_mut().rust_mut().attached = Some((session, token));
        self.update();
    }

    #[expect(
        unsafe_code,
        reason = "Qt hands the painter as a raw pointer, and the frame's pixels reach QImage without a copy"
    )]
    unsafe fn paint(self: Pin<&mut Self>, painter: *mut QPainter) {
        let Some(painter) = (unsafe { painter.as_mut() }) else { return };
        let Some((session, _)) = &self.attached else { return };
        let Some(frame) = presenter().frame(session) else { return };
        let (Ok(width), Ok(height)) = (i32::try_from(frame.width), i32::try_from(frame.height)) else { return };
        let Some([x, y, shown_width, shown_height]) =
            aspect_fit((frame.width, frame.height), (self.width(), self.height()))
        else {
            return;
        };

        extern "C" fn keep(_: *mut cxx_qt_lib::c_void) {}
        let image = unsafe {
            QImage::from_raw_parts(
                frame.rgba.as_ptr(),
                width,
                height,
                QImageFormat::Format_RGBX8888,
                keep,
                std::ptr::null_mut(),
            )
        };
        let mut painter = unsafe { Pin::new_unchecked(painter) };
        painter.as_mut().set_render_hint(QPainterRenderHint::SmoothPixmapTransform, true);
        let target = QRect::new(x, y, shown_width, shown_height);
        painter.as_mut().draw_image(&target, &image);
        drop(image);
    }
}

#[cfg(test)]
mod tests;
