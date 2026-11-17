//! The messages part's `query` and `perform` names, routed to what answers or runs them.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use serde_json::Value;
use tracing::{info, warn};

use super::arguments::{Compose, DroppedImage, Flag, ListItem, SavePart, Setting, parse, strings};
use super::refresh::LIST;
use super::reload::not_started;
use super::view_json::settings_json;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::views::messages::{Detail, parse_part_key};

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_messages(
        &self,
        engine: &Arc<Engine>,
        name: &str,
        _arg: &str,
    ) -> Option<Value> {
        let state = &self.areas.messages;
        let value = match name {
            "messages.list" => state.list_json.clone(),
            "messages.detail" => state.detail_json.clone(),
            "messages.draft" => state.draft_json.clone(),
            "messages.settings" => settings_json(engine, state),
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn perform_messages(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        let flag = arg == "true";
        self.as_mut().perform_messages_view(engine.clone(), name, arg, flag)
            || self.as_mut().perform_messages_recipients(engine.clone(), name, arg)
            || self.as_mut().perform_messages_composer(engine.clone(), name, arg)
            || self.perform_messages_settings(engine, name, arg, flag)
    }

    /// The actions on what the window shows; whether `name` is one.
    ///
    /// They change the view, the selection, the new message, the search and the transcript, and carry
    /// another part's request to write to people.
    fn perform_messages_view(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str, flag: bool) -> bool {
        match name {
            "messages.viewActive" => {
                self.as_mut().msg().view_active = flag;
                if let Err(e) = engine.messaging().messages_view_changed(flag) {
                    info!(error = %e, "the messages view change was not passed on");
                }
            }
            // The engine reads this computer's Bluetooth state when asked: refresh it when the user
            // looks again.
            "messages.windowActive" => {
                if flag {
                    self.reload_state();
                }
            }
            "messages.contactsChanged" => self.contacts_changed_here(),
            "messages.select" => match parse(name, arg, ListItem::read) {
                Some(None) => self.set_detail(None),
                Some(Some(ListItem::Conversation(thread_id))) => self.set_detail(Some(Detail::Conversation(thread_id))),
                Some(Some(ListItem::NewMessage)) => self.start_new_message(),
                Some(Some(ListItem::Unsent(addresses))) => self.open_unsent(&addresses),
                None => {}
            },
            "messages.newMessage" => self.start_new_message(),
            "messages.cancelNewMessage" => self.cancel_new_message(),
            "messages.discardNewMessage" => self.discard_new_message(),
            "messages.search" => {
                self.as_mut().msg().search = arg.to_owned();
                self.mark(LIST);
            }
            "messages.reloadTranscript" => self.reload_transcript(),
            "messages.compose" => {
                if let Some(compose) = parse(name, arg, Compose::read) {
                    self.compose(engine, compose);
                }
            }
            _ => return false,
        }
        true
    }

    /// The actions on the new message's recipients; whether `name` is one.
    ///
    /// They include the contacts the recipients are suggested from.
    fn perform_messages_recipients(self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) -> bool {
        match name {
            "messages.recipientQuery" => self.set_recipient_query(arg),
            "messages.submitRecipient" => self.submit_recipient_query(),
            "messages.addSuggestion" => match arg.parse::<usize>() {
                Ok(index) => self.add_suggestion(index),
                Err(e) => warn!(%arg, error = %e, "messages.addSuggestion takes an index"),
            },
            "messages.removeRecipient" => self.remove_recipient(arg),
            "messages.requestContactsPermission" => self.request_contacts_permission(engine),
            "messages.turnOnContacts" => self.turn_on_contacts(engine),
            _ => return false,
        }
        true
    }

    /// The actions of the composer and the transcript's messages; whether `name` is one.
    ///
    /// They change the draft, send, send again, and download, open or save attachments.
    fn perform_messages_composer(self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) -> bool {
        match name {
            "messages.draftText" => self.set_draft_text(arg),
            "messages.addFiles" => {
                if let Some(urls) = parse(name, arg, strings) {
                    self.add_files(urls);
                }
            }
            "messages.addImageData" => {
                if let Some(data) = parse(name, arg, DroppedImage::read) {
                    self.add_image_data(data);
                }
            }
            "messages.paste" => self.paste(),
            "messages.removeImage" => match arg.parse::<u64>() {
                Ok(id) => self.remove_image(id),
                Err(e) => warn!(%arg, error = %e, "messages.removeImage takes an image id"),
            },
            "messages.send" => self.send(engine),
            "messages.retry" => match arg.parse::<i64>() {
                Ok(send_id) => self.retry(engine, send_id),
                Err(e) => warn!(%arg, error = %e, "messages.retry takes a send id"),
            },
            "messages.download" | "messages.openPart" => match parse_part_key(arg) {
                Some(part) if name == "messages.download" => self.download(engine, part),
                Some(part) => self.open_part(engine, part),
                None => warn!(action = name, %arg, "the action takes a part key"),
            },
            "messages.savePart" => {
                if let Some(save) = parse(name, arg, SavePart::read) {
                    self.save_part(engine, save);
                }
            }
            _ => return false,
        }
        true
    }

    /// The actions on the messaging settings and a conversation's; whether `name` is one.
    ///
    /// They turn messages on and ask the phone to allow them, change a conversation's alerts, hiding
    /// and SIM, and the Settings window's switches and contacts.
    fn perform_messages_settings(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
        flag: bool,
    ) -> bool {
        match name {
            "messages.turnOn" => self.turn_on_messages(engine),
            "messages.requestPermission" => self.request_permission(engine),
            "messages.setMuted" | "messages.setHidden" => {
                if let Some(flag) = parse(name, arg, Flag::read) {
                    if name == "messages.setMuted" {
                        self.set_muted(engine, flag.thread_id, flag.on);
                    } else {
                        self.set_hidden(engine, flag.thread_id, flag.on);
                    }
                }
            }
            "messages.selectSim" => match arg.parse::<i64>() {
                Ok(id) => self.select_subscription(engine, id),
                Err(e) => warn!(%arg, error = %e, "messages.selectSim takes a subscription id"),
            },
            "messages.setting" => {
                if let Some(setting) = parse(name, arg, Setting::read) {
                    self.change_setting(engine, &setting);
                }
            }
            "messages.useContacts" => {
                if let Err(e) = engine.contacts().set_pc_consent(flag)
                    && !not_started(&e)
                {
                    self.as_mut().show_failure(Failure::setting(&e));
                }
                self.as_mut().reload_contacts_state();
                self.bump(Revision::Settings);
            }
            "messages.requestContactsAccess" => self.request_contacts_access(engine),
            _ => return false,
        }
        true
    }
}
