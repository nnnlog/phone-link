//! Messages: the conversation list and the shown detail, with its transcript and composer.
//!
//! The detail is a conversation, the new message, or what was sent to new recipients before the phone
//! created their conversation. The part keeps a draft per composer; sends, retries and downloads
//! attachments; suggests the phone's contacts as recipients; chooses the SIM a conversation sends
//! from; turns a conversation's alerts and hiding on and off; and changes the messaging settings. It
//! tells the engine what the window shows of the messages view (which conversation is shown, whether
//! the view and window are in front), from which the engine marks conversations read.
//!
//! The section's state lives here; the views read it as JSON (`messages.list`, `messages.detail`,
//! `messages.draft`, and the transcript in `messagesJson`) and re-read it on the `messages` area
//! events `list`, `detail` and `draft`. A failed action the user started is shown in the window.
//!
//! `query` names: `messages.*`. `perform` names: `messages.*`.

mod arguments;
mod attachments;
mod composer;
mod composing;
mod conversation_settings;
mod draft;
mod draft_images;
mod messaging_settings;
mod navigation;
mod recipients;
mod refresh;
mod reload;
mod routing;
mod sending;
mod state;
#[cfg(test)]
mod tests;
mod transcript;
mod view_json;

pub(super) use state::State;
