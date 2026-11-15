use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QImage, QImageFormat, QPainterRenderHint, QRect, QString};

use crate::mirror::presenter;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qpainter.h");
        type QPainter = cxx_qt_lib::QPainter;
    }

    unsafe extern "C++Qt" {
        include!(<QtQuick/QQuickPaintedItem>);
        #[qobject]
        type QQuickPaintedItem;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QQuickPaintedItem]
        #[qproperty(QString, session_id, cxx_name = "sessionId")]
        type MirrorVideo = super::MirrorVideoRust;

        #[qinvokable]
        fn attach(self: Pin<&mut MirrorVideo>);

        #[cxx_override]
        unsafe fn paint(self: Pin<&mut MirrorVideo>, painter: *mut QPainter);
    }

    unsafe extern "RustQt" {
        #[inherit]
        fn update(self: Pin<&mut MirrorVideo>);

        #[inherit]
        fn width(self: &MirrorVideo) -> f64;

        #[inherit]
        fn height(self: &MirrorVideo) -> f64;
    }

    impl cxx_qt::Threading for MirrorVideo {}
    impl cxx_qt::Constructor<()> for MirrorVideo {}
}

use qobject::{MirrorVideo, QPainter};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

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

    unsafe fn paint(self: Pin<&mut Self>, painter: *mut QPainter) {
        let Some(painter) = (unsafe { painter.as_mut() }) else { return };
        let Some((session, _)) = &self.attached else { return };
        let Some(frame) = presenter().frame(session) else { return };
        let (Ok(width), Ok(height)) = (i32::try_from(frame.width), i32::try_from(frame.height)) else { return };

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
        let target = QRect::new(0, 0, self.width().round() as i32, self.height().round() as i32);
        painter.as_mut().draw_image(&target, &image);
        drop(image);
    }
}
