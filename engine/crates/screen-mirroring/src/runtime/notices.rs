//! What the connection reports: health, ended workflows, its close.

use super::{Inner, PhoneArgs};
use crate::link::LinkNotice;
use crate::machine::PhoneCommand;
use crate::wire::workflow;
use phonelink_core::sync::MutexExt;
use tokio::sync::mpsc;
use tracing::debug;

impl Inner {
    pub(super) fn spawn_notices(&self, mut notices: mpsc::UnboundedReceiver<LinkNotice>) {
        let me = self.weak.clone();
        self.spawn(async move {
            while let Some(n) = notices.recv().await {
                let Some(me) = me.upgrade() else { return };
                me.notice(n);
            }
        });
    }

    pub(super) fn notice(&self, n: LinkNotice) {
        match n {
            // Missed and recovered packets reach the phone state through the orchestrator only.
            LinkNotice::Unstable | LinkNotice::Recovered if self.state.lock_or_panic().orchestrator.is_none() => {
                debug!(notice_kind = ?n, "connection health before the remote workflow; ignored")
            }
            LinkNotice::Unstable => self.phone_command(PhoneCommand::OnConnectionUnstable, PhoneArgs::default()),
            LinkNotice::Recovered => self.phone_command(PhoneCommand::OnConnectionRecovered, PhoneArgs::default()),
            // The connection logged why the workflow ended (the phone stopped it, or it closed).
            LinkNotice::WorkflowEnded { name, code } => {
                match name.as_str() {
                    // Any workflow of the set-up or the early launch ending releases everything.
                    workflow::REMOTE | workflow::PERMISSION | workflow::NEARBY | workflow::EARLY_LAUNCH => {
                        self.released(code)
                    }
                    workflow::AUDIO => self.audio_ended(),
                    _ => {}
                }
            }
            LinkNotice::Closed { code } => {
                let any = !self.state.lock_or_panic().sessions.is_empty();
                if any {
                    self.released(code);
                } else {
                    let mut st = self.state.lock_or_panic();
                    self.drop_channels(&mut st);
                }
            }
        }
    }
}
