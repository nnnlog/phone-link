//! The sign-in page the engine asks for: its headers, what the page reports back, and returning the
//! window to where it was once the page is done.

use std::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use tracing::warn;

use super::{EngineBridge, QObject, qobject};
use crate::sign_in::Step;

impl EngineBridge {
    /// Carries out what the sign-in page asked for: a script for the view to run, or the engine told
    /// that the sign-in completed.
    fn apply(mut self: Pin<&mut Self>, step: Step) -> QString {
        match step {
            Step::Nothing => QString::default(),
            Step::RunScript(script) => QString::from(script.as_str()),
            Step::Complete(json) => {
                if let (Some(engine), Some(page)) = (self.engine.clone(), self.as_mut().rust_mut().sign_in.take()) {
                    engine.complete_web_sign_in(page.request_id, json);
                }
                let _ = self.qt_thread().queue(|bridge| bridge.sign_in_done());
                QString::default()
            }
        }
    }

    /// Installs the engine's sign-in headers on the sign-in view's profile.
    ///
    /// # Safety
    /// `profile` is the `QObject` QML passes for the view's `profile`, alive for the call.
    #[expect(unsafe_code, reason = "QML hands the sign-in view's profile as a raw QObject pointer")]
    pub unsafe fn prepare_sign_in(self: Pin<&mut Self>, profile: *mut QObject) {
        let Some(page) = self.sign_in.as_ref() else {
            warn!("prepareSignIn was called with no sign-in under way");
            return;
        };

        let (names, values): (Vec<String>, Vec<String>) = page.headers.iter().cloned().unzip();
        // SAFETY: QML passes the sign-in view's WebEngineProfile, which outlives this call; the C++
        // side checks its type before it uses it.
        let installed = unsafe { qobject::install_sign_in_headers(profile, &page.url, &names, &values) };
        if !installed {
            // The page would load without the headers the engine requires; end the sign-in instead.
            warn!("the sign-in view's profile is not a WebEngineProfile; the sign-in is cancelled");
            self.cancel_sign_in(false);
        }
    }

    pub fn sign_in_console_message(mut self: Pin<&mut Self>, message: &QString) -> QString {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.console_message(&message.to_string()),
            None => Step::Nothing,
        };
        self.apply(step)
    }

    pub fn sign_in_page_loaded(mut self: Pin<&mut Self>, url: &QString) -> QString {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.page_loaded(&url.to_string()),
            None => Step::Nothing,
        };
        self.apply(step)
    }

    pub fn sign_in_script_result(mut self: Pin<&mut Self>, result: &QString) {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.script_result(&result.to_string()),
            None => Step::Nothing,
        };
        self.apply(step);
    }

    pub fn cancel_sign_in(mut self: Pin<&mut Self>, declined: bool) {
        let Some(page) = self.as_mut().rust_mut().sign_in.take() else {
            warn!("a sign-in was cancelled with none under way");
            return;
        };

        if let Some(engine) = self.engine.clone() {
            engine.cancel_web_sign_in(page.request_id, declined);
        }
        self.sign_in_done();
    }

    /// The sign-in page is done (completed or cancelled).
    ///
    /// During a start the window waits for its result; a page the engine asked for during the session
    /// returns the window to where it was.
    fn sign_in_done(mut self: Pin<&mut Self>) {
        if self.sign_in.is_some() || self.phase != QString::from("signIn") {
            return;
        }

        let phase = self.as_mut().rust_mut().phase_before_sign_in.take();
        match phase {
            Some(phase) if !self.starting => self.set_phase(phase),
            _ => self.set_phase(QString::from("starting")),
        }
    }
}
