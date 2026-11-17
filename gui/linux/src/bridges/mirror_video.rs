//! The C++ side of the `MirrorVideo` QML item, which draws a mirroring session's video.

use super::MirrorVideoRust;

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
