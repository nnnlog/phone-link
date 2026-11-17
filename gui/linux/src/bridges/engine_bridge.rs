//! The C++ side of the app's one QML singleton, `EngineBridge`: its properties, signals and invokables,
//! and the C++ helpers it calls.

use super::EngineBridgeRust;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("phonelink-linux/src/desktop.h");

        /// Attaches the sign-in headers to the WebEngineProfile `profile`; false when it is not one.
        #[namespace = "phonelink"]
        #[cxx_name = "installSignInHeaders"]
        unsafe fn install_sign_in_headers(
            profile: *mut QObject,
            url: &str,
            names: &Vec<String>,
            values: &Vec<String>,
        ) -> bool;

        /// The primary screen as mirroring tells the phone about it, in device pixels: the width and height
        /// of its available area (less a framed window's title bar), its dpi, and its width and height.
        /// Read on the GUI thread; a call on another thread gets what was read last, and nothing before
        /// the first read.
        #[namespace = "phonelink"]
        #[cxx_name = "primaryDisplayGeometry"]
        fn primary_display_geometry() -> Vec<i32>;

        /// Hands a notification click's activation token to the next window activation (on the GUI
        /// thread, right before the window asks to be activated).
        #[namespace = "phonelink"]
        #[cxx_name = "setActivationToken"]
        fn set_activation_token(token: &str);
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, phase)]
        #[qproperty(QString, message)]
        #[qproperty(QString, message_detail, cxx_name = "messageDetail")]
        #[qproperty(QString, sign_in_url, cxx_name = "signInUrl")]
        #[qproperty(QString, sign_in_user_agent, cxx_name = "signInUserAgent")]
        #[qproperty(QString, sign_in_script, cxx_name = "signInScript")]
        #[qproperty(QString, phones_json, cxx_name = "phonesJson")]
        #[qproperty(QString, phone_name, cxx_name = "phoneName")]
        #[qproperty(QString, notifications_json, cxx_name = "notificationsJson")]
        #[qproperty(QString, conversations_json, cxx_name = "conversationsJson")]
        #[qproperty(QString, messages_json, cxx_name = "messagesJson")]
        #[qproperty(QString, status_json, cxx_name = "statusJson")]
        #[qproperty(bool, notifications_muted, cxx_name = "notificationsMuted")]
        #[qproperty(bool, phone_connected, cxx_name = "phoneConnected")]
        #[qproperty(bool, features_disabled, cxx_name = "featuresDisabled")]
        #[qproperty(i32, contacts_revision, cxx_name = "contactsRevision")]
        #[qproperty(i32, photos_revision, cxx_name = "photosRevision")]
        #[qproperty(i32, calls_revision, cxx_name = "callsRevision")]
        #[qproperty(i32, apps_revision, cxx_name = "appsRevision")]
        #[qproperty(i32, media_revision, cxx_name = "mediaRevision")]
        #[qproperty(i32, transfers_revision, cxx_name = "transfersRevision")]
        #[qproperty(i32, settings_revision, cxx_name = "settingsRevision")]
        #[qproperty(i32, wallpaper_revision, cxx_name = "wallpaperRevision")]
        #[qproperty(i32, mirroring_revision, cxx_name = "mirroringRevision")]
        #[qproperty(i32, pairing_revision, cxx_name = "pairingRevision")]
        #[qproperty(i32, messaging_revision, cxx_name = "messagingRevision")]
        type EngineBridge = super::EngineBridgeRust;

        #[qsignal]
        #[cxx_name = "mirrorRaise"]
        fn mirror_raise(self: Pin<&mut EngineBridge>, session_id: QString);

        /// Show the main window, on `section` when it is not empty.
        #[qsignal]
        #[cxx_name = "showSection"]
        fn show_section(self: Pin<&mut EngineBridge>, section: QString);

        /// Show the main window on the notification `key`.
        #[qsignal]
        #[cxx_name = "showNotification"]
        fn show_notification(self: Pin<&mut EngineBridge>, key: QString);

        /// An action failed: what did not happen, and why (both localized).
        #[qsignal]
        fn failure(self: Pin<&mut EngineBridge>, title: QString, message: QString);

        /// A status control needs the user's confirmation first; accepting it performs
        /// `status.acceptPrompt` with `kind`.
        #[qsignal]
        #[cxx_name = "statusPrompt"]
        fn status_prompt(self: Pin<&mut EngineBridge>, kind: QString);

        #[qsignal]
        #[cxx_name = "mirrorDrag"]
        fn mirror_drag(self: Pin<&mut EngineBridge>, session_id: QString, state: QString);

        #[qsignal]
        #[cxx_name = "mirrorPhoneDrag"]
        fn mirror_phone_drag(self: Pin<&mut EngineBridge>, session_id: QString, urls: QString);

        /// Something happened in one part of the app (`area`: messages, notifications, calls, contacts,
        /// photos, files, media, mirror, shell) that its QML listens for: `name`, with a JSON `payload`.
        #[qsignal]
        #[cxx_name = "areaEvent"]
        fn area_event(self: Pin<&mut EngineBridge>, area: QString, name: QString, payload: QString);

        #[qinvokable]
        fn query(self: &EngineBridge, name: &QString, arg: &QString) -> QString;

        #[qinvokable]
        fn perform(self: Pin<&mut EngineBridge>, name: &QString, arg: &QString);

        #[qinvokable]
        #[cxx_name = "setNotificationPinned"]
        fn set_notification_pinned(self: Pin<&mut EngineBridge>, key: &QString, pinned: bool);

        #[qinvokable]
        #[cxx_name = "muteNotifications"]
        fn mute_notifications(self: Pin<&mut EngineBridge>, muted: bool);

        #[qinvokable]
        #[cxx_name = "toggleDoNotDisturb"]
        fn toggle_do_not_disturb(self: Pin<&mut EngineBridge>);

        #[qinvokable]
        #[cxx_name = "cycleRinger"]
        fn cycle_ringer(self: Pin<&mut EngineBridge>);

        #[qinvokable]
        #[cxx_name = "findPhone"]
        fn find_phone(self: Pin<&mut EngineBridge>);

        #[qinvokable]
        #[cxx_name = "dismissNotification"]
        fn dismiss_notification(self: &EngineBridge, key: &QString);

        #[qinvokable]
        #[cxx_name = "pressNotificationAction"]
        fn press_notification_action(self: Pin<&mut EngineBridge>, key: &QString, index: i32);

        #[qinvokable]
        fn start(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "selectPhone"]
        fn select_phone(self: Pin<&mut Self>, id: &QString);

        #[qinvokable]
        #[cxx_name = "signInConsoleMessage"]
        fn sign_in_console_message(self: Pin<&mut Self>, message: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "signInPageLoaded"]
        fn sign_in_page_loaded(self: Pin<&mut Self>, url: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "signInScriptResult"]
        fn sign_in_script_result(self: Pin<&mut Self>, result: &QString);

        #[qinvokable]
        #[cxx_name = "cancelSignIn"]
        fn cancel_sign_in(self: Pin<&mut Self>, declined: bool);
    }

    unsafe extern "RustQt" {
        /// Has the sign-in view's WebEngineProfile `profile` send the engine's headers with the
        /// sign-in page; QML calls it before it sets the view's URL.
        #[qinvokable]
        #[cxx_name = "prepareSignIn"]
        unsafe fn prepare_sign_in(self: Pin<&mut EngineBridge>, profile: *mut QObject);
    }

    impl cxx_qt::Threading for EngineBridge {}
}
