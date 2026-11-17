//! A conversation's settings: its alerts, whether it is hidden, and the SIM it sends from.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;

impl EngineBridge {
    /// Writes a conversation's alerts and hiding; whether the engine took them.
    pub(super) fn set_flags(
        mut self: Pin<&mut Self>,
        engine: &Arc<Engine>,
        thread_id: i64,
        muted: bool,
        hidden: bool,
    ) -> bool {
        match engine.clone().messaging().set_conversation_flags(thread_id, muted, hidden) {
            Ok(()) => {
                self.reload_snapshot();
                true
            }
            Err(e) => {
                self.as_mut().show_failure(Failure::feature("Conversation Not Changed", &e));
                false
            }
        }
    }

    /// "Hide Alerts" or "Show Alerts".
    pub(super) fn set_muted(self: Pin<&mut Self>, engine: Arc<Engine>, thread_id: i64, muted: bool) {
        let Some(hidden) = self.areas.messages.conversation(thread_id).map(|c| c.hidden) else { return };
        self.set_flags(&engine, thread_id, muted, hidden);
    }

    /// Hides a conversation or shows it again.
    pub(super) fn set_hidden(mut self: Pin<&mut Self>, engine: Arc<Engine>, thread_id: i64, hidden: bool) {
        let Some(muted) = self.areas.messages.conversation(thread_id).map(|c| c.muted) else { return };
        if self.as_mut().set_flags(&engine, thread_id, muted, hidden)
            && hidden
            && self.areas.messages.selection() == Some(thread_id)
        {
            self.set_detail(None);
        }
    }

    /// Chooses the SIM the shown conversation sends from.
    pub(super) fn select_subscription(mut self: Pin<&mut Self>, engine: Arc<Engine>, subscription_id: i64) {
        let Some(thread_id) = self.areas.messages.selection() else { return };
        match engine.messaging().select_subscription(thread_id, Some(subscription_id)) {
            Ok(()) => self.reload_snapshot(),
            Err(e) => self.as_mut().show_failure(Failure::feature("SIM Not Changed", &e)),
        }
    }
}
