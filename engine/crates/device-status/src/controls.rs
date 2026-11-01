//! The do-not-disturb and ringer controls.

use super::{
    ControlOutcome, DeviceStatusEvent, DndPending, Inner, Overlay, Prompt, RingerPending, Seen, ViewInputs, build_view,
    command, model, phone_link, select,
};
use phonelink_app::PhoneRecord;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use tracing::{debug, info, warn};

impl Inner {
    /// The DND teaching tip that applies to `record` now, if any.
    pub(super) fn dnd_tip(&self, record: &PhoneRecord, rule_enabled: bool) -> Option<Prompt> {
        if !model::notifications_sync_granted(record) {
            Some(Prompt::DndNotificationsPermission)
        } else if self.config.android15_do_not_disturb && !rule_enabled {
            Some(Prompt::DndRuleDisabled)
        } else {
            None
        }
    }

    pub(super) fn toggle_do_not_disturb(&self, phone: &DcgClient) -> ControlOutcome {
        let Some(record) = self.selected_record(phone) else { return ControlOutcome::NotAvailable };
        let connected = self.env.sessions.is_connected(phone);

        let (value, id) = {
            let mut state = self.state.lock_or_panic();
            let id = state.next_id();
            let ps = state.phone(phone);
            let sel = select(&record.status_keys, ps.cdeh.as_ref());
            let overlay = ps.dnd.overlay(sel.do_not_disturb_status);
            let view = build_view(&ViewInputs {
                phone,
                record: Some(&record),
                cdeh: ps.cdeh.as_ref(),
                connected,
                dnd: overlay,
                ringer: Overlay::None,
            });

            if !view.do_not_disturb.is_some_and(|c| c.enabled) {
                return ControlOutcome::NotAvailable;
            }
            if let Some(prompt) = self.dnd_tip(&record, sel.do_not_disturb_rule_enabled) {
                return ControlOutcome::Prompt(prompt);
            }

            let shown = model::dnd_shown(sel.do_not_disturb_status, overlay);
            let value = if shown == 1 { 2 } else { 1 };
            let pending = if shown == 2 || shown == 3 { DndPending::TurningOff } else { DndPending::TurningOn };
            ps.dnd.pending = Some((id, pending));
            ps.dnd.reverted = None;
            // Later record changes are compared with what this decision saw (the ringer keeps its own).
            ps.dnd.seen = Some(Seen::of(&record));
            (value, id)
        };

        phone_link::spawn_request(
            &self.runtime,
            self.env.app.clone(),
            phone_link::command_request(command::DO_NOT_DISTURB, value, "ToggleDoNotDisturbIndicator"),
            "DND command",
        );

        let (weak, phone_owned, after) = (self.weak.clone(), phone.clone(), self.config.do_not_disturb_confirmation);
        self.runtime.spawn(async move {
            tokio::time::sleep(after).await;
            if let Some(inner) = weak.upgrade() {
                inner.on_dnd_timer(&phone_owned, id);
            }
        });

        self.changed(phone);
        ControlOutcome::Sent { command_type: command::DO_NOT_DISTURB, value }
    }

    /// The DND timer fired: a still-pending control reverts (turning on → `1`, turning off → `3`); a
    /// reverted turn-off raises the failure prompt when the DND is not `1` and the Android 15 DND rule
    /// support is on. A change the control re-reads on that is already in the record, its event not
    /// yet processed, ends the pending state as that event will (no revert).
    pub(super) fn on_dnd_timer(&self, phone: &DcgClient, id: u64) {
        let record = self.record_for(phone);
        let prompt = {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            let Some((pending_id, pending)) = ps.dnd.pending else { return };
            if pending_id != id {
                return;
            }

            if record.as_ref().is_some_and(|r| ps.dnd.unprocessed_change(r, Seen::dnd_reset)) {
                ps.dnd.reset();
                drop(state);
                debug!(%phone, "DND change applied before its confirmation timer fired; not reverted");
                self.changed(phone);
                return;
            }

            let default_keys = phonelink_app::StatusKeys::default();
            let sel = select(record.as_ref().map_or(&default_keys, |r| &r.status_keys), ps.cdeh.as_ref());
            ps.dnd.pending = None;
            let shown = match pending {
                DndPending::TurningOn => 1,
                DndPending::TurningOff => 3,
            };
            ps.dnd.reverted = Some((shown, sel.do_not_disturb_status));
            let prompt = pending == DndPending::TurningOff
                && sel.do_not_disturb_status != Some(1)
                && self.config.android15_do_not_disturb;
            if prompt {
                ps.failure_prompt = true;
            }
            prompt
        };

        warn!(%phone, "DND change not confirmed by the phone in time; the control reverted");
        self.changed(phone);
        if prompt {
            self.emit(DeviceStatusEvent::PromptRequested { phone: phone.clone(), prompt: Prompt::DndTurnOffFailed });
        }
    }

    pub(super) fn cycle_ringer_mode(&self, phone: &DcgClient) -> ControlOutcome {
        let Some(record) = self.selected_record(phone) else { return ControlOutcome::NotAvailable };
        let connected = self.env.sessions.is_connected(phone);

        let (target, id) = {
            let mut state = self.state.lock_or_panic();
            let id = state.next_id();
            let ps = state.phone(phone);
            let sel = select(&record.status_keys, ps.cdeh.as_ref());
            let overlay = ps.ringer.overlay(sel.ringer_mode);
            let view = build_view(&ViewInputs {
                phone,
                record: Some(&record),
                cdeh: ps.cdeh.as_ref(),
                connected,
                dnd: Overlay::None,
                ringer: overlay,
            });

            if !view.ringer.is_some_and(|c| c.enabled) {
                return ControlOutcome::NotAvailable;
            }
            if !model::notifications_sync_granted(&record) {
                return ControlOutcome::Prompt(Prompt::RingerNotificationsPermission);
            }

            let (target, pending) = match model::ringer_shown(sel.ringer_mode, overlay) {
                2 => (1, RingerPending::Vibrate),
                1 => (0, RingerPending::Off),
                _ => (2, RingerPending::On),
            };
            ps.ringer.pending = Some((id, pending));
            ps.ringer.reverted = None;
            ps.ringer.seen = Some(Seen::of(&record));
            (target, id)
        };

        phone_link::spawn_request(
            &self.runtime,
            self.env.app.clone(),
            phone_link::command_request(command::RINGER, target, "ToggleRingerIndicator"),
            "ringer command",
        );

        let (weak, phone_owned, after) = (self.weak.clone(), phone.clone(), self.config.ringer_confirmation);
        self.runtime.spawn(async move {
            tokio::time::sleep(after).await;
            if let Some(inner) = weak.upgrade() {
                inner.on_ringer_timer(&phone_owned, id);
            }
        });

        self.changed(phone);
        ControlOutcome::Sent { command_type: command::RINGER, value: target }
    }

    /// The ringer delay ended: a still-pending control reverts to the mode it came from (turning
    /// vibrate → `2`, turning off → `1`, turning on → `0`). A change the control re-reads on that is
    /// already in the record, its event not yet processed, ends the pending state as that event will
    /// (no revert).
    pub(super) fn on_ringer_timer(&self, phone: &DcgClient, id: u64) {
        let record = self.record_for(phone);
        {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            let Some((pending_id, pending)) = ps.ringer.pending else { return };
            if pending_id != id {
                return;
            }

            if record.as_ref().is_some_and(|r| ps.ringer.unprocessed_change(r, Seen::ringer_reset)) {
                ps.ringer.reset();
                drop(state);
                debug!(%phone, "ringer change applied before its confirmation delay ended; not reverted");
                self.changed(phone);
                return;
            }

            let default_keys = phonelink_app::StatusKeys::default();
            let sel = select(record.as_ref().map_or(&default_keys, |r| &r.status_keys), ps.cdeh.as_ref());
            ps.ringer.pending = None;
            let shown = match pending {
                RingerPending::Vibrate => 2,
                RingerPending::Off => 1,
                RingerPending::On => 0,
            };
            ps.ringer.reverted = Some((shown, sel.ringer_mode));
        }

        warn!(%phone, "ringer change not confirmed by the phone in time; the control reverted");
        self.changed(phone);
    }

    pub(super) fn accept_prompt(&self, phone: &DcgClient, prompt: Prompt) {
        let Some(record) = self.selected_record(phone) else {
            debug!(%phone, prompt_kind = ?prompt, "prompt accepted for a phone that is not selected; nothing sent");
            return;
        };

        match prompt {
            Prompt::DndNotificationsPermission | Prompt::DndRuleDisabled => self.accept_dnd_prompt(phone, &record),
            Prompt::RingerNotificationsPermission => self.request_notifications_permission(phone),
            Prompt::StopRinging => {
                let outcome = self.stop_ringing(phone);
                info!(%phone, ?outcome, "stop-ringing confirmation accepted");
            }
            Prompt::DndTurnOffFailed => {
                let outstanding = std::mem::take(&mut self.state.lock_or_panic().phone(phone).failure_prompt);
                if outstanding {
                    self.launch_dnd_settings();
                } else {
                    debug!(%phone, "DND failure prompt accepted after it was withdrawn; nothing sent");
                }
            }
        }
    }

    /// An accepted DND tip: the action of the tip that applies now, nothing when none does.
    fn accept_dnd_prompt(&self, phone: &DcgClient, record: &PhoneRecord) {
        let rule_enabled = {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            select(&record.status_keys, ps.cdeh.as_ref()).do_not_disturb_rule_enabled
        };
        match self.dnd_tip(record, rule_enabled) {
            Some(Prompt::DndNotificationsPermission) => self.request_notifications_permission(phone),
            Some(_) => self.launch_dnd_settings(),
            None => debug!(%phone, "DND prompt accepted after it stopped applying; nothing sent"),
        }
    }

    /// Spawns the notifications-sync permission request to the phone.
    fn request_notifications_permission(&self, phone: &DcgClient) {
        phone_link::spawn_notifications_sync_permission_request(&self.runtime, self.env.app.clone(), phone.clone());
    }

    /// Spawns the command that opens the phone's DND settings.
    fn launch_dnd_settings(&self) {
        phone_link::spawn_request(
            &self.runtime,
            self.env.app.clone(),
            phone_link::command_request(command::LAUNCH_DO_NOT_DISTURB_SETTINGS, 0, "LaunchDoNotDisturbSettings"),
            "launch DND settings command",
        );
    }
}
