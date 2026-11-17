//! The C++ side of the `L10n` QML singleton, which translates the QML's texts.

use super::L10nRust;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type L10n = super::L10nRust;

        #[qinvokable]
        fn t(self: &L10n, text: &QString) -> QString;

        #[qinvokable]
        fn f(self: &L10n, text: &QString, args: &QStringList) -> QString;

        #[qinvokable]
        fn p(self: &L10n, text: &QString, count: i64, args: &QStringList) -> QString;
    }
}
