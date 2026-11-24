//! The engine's start and the window's phase it sets: starting, selecting a phone, running, failed.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::engine::EngineStatus;
use phonelink_engine::engine::EngineError;
use tracing::warn;

use super::EngineBridge;
use super::engine::{build_engine, data_dir};
use super::events::QtEventSink;
use crate::user_text;

impl EngineBridge {
    pub fn start(mut self: Pin<&mut Self>) {
        if self.starting {
            return;
        }

        self.as_mut().watch_quit_signals();
        let thread = self.qt_thread();
        crate::single_instance::on_show(move || {
            let _ = thread.queue(|bridge| bridge.show_section(QString::default()));
        });
        let thread = self.qt_thread();
        crate::single_instance::on_open(move |link| {
            let _ = thread.queue(move |bridge| bridge.open_link(&link));
        });

        let engine = match self.engine.clone() {
            Some(engine) => engine,
            None => {
                let data_dir = match data_dir() {
                    Ok(dir) => dir,
                    Err(reason) => {
                        warn!(reason, "Phone Link has no folder for its data");
                        self.failed_with(crate::l10n::translate("Phone Link couldn't start."), reason);
                        return;
                    }
                };
                let events = Arc::new(QtEventSink::new(self.qt_thread()));
                let (engine, notifier) = build_engine(data_dir, events, self.qt_thread());
                self.as_mut().rust_mut().engine = Some(engine.clone());
                self.as_mut().rust_mut().notifier = notifier;
                engine
            }
        };

        self.as_mut().rust_mut().starting = true;
        self.as_mut().rust_mut().phase_before_sign_in = None;
        self.as_mut().set_phase(QString::from("starting"));
        self.as_mut().set_message(QString::default());
        self.as_mut().set_message_detail(QString::default());

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.start().await;
            let _ = thread.queue(move |bridge| bridge.started(result));
        });
    }

    fn started(mut self: Pin<&mut Self>, result: Result<EngineStatus, EngineError>) {
        self.as_mut().rust_mut().starting = false;
        self.as_mut().rust_mut().sign_in = None;
        match result {
            Ok(status) if status.needs_phone_selection => {
                self.as_mut().reload_phones();
                self.set_phase(QString::from("selectingPhone"));
            }
            Ok(_) => self.running(),
            Err(_) if self.phase == QString::from("accountRemoved") => {}
            Err(e) => self.failed(&e),
        }
    }

    /// The phone session runs: every part reads what it shows, and the window shows it.
    pub(super) fn running(mut self: Pin<&mut Self>) {
        self.as_mut().reload_phone();
        self.as_mut().reload_phones();
        self.as_mut().reload_features();
        self.as_mut().sync_phone_mount();
        self.as_mut().watch_camera_readers();
        self.as_mut().reapply_messaging_view();
        self.set_phase(QString::from("running"));
    }

    /// The failure screen: a short sentence, and the engine's own text below it.
    fn failed(self: Pin<&mut Self>, error: &EngineError) {
        warn!(%error, "Phone Link did not start");
        self.failed_with(&user_text::engine_error(error), &error.to_string());
    }

    /// The failure screen with `message`, and `detail` below it.
    pub(super) fn failed_with(mut self: Pin<&mut Self>, message: &str, detail: &str) {
        self.as_mut().set_message(QString::from(message));
        self.as_mut().set_message_detail(QString::from(detail));
        self.set_phase(QString::from("failed"));
    }
}
