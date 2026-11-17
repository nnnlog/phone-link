//! Conversations and messages as the window reads them, and the Messages section's rules: how typed
//! addresses are read and matched, how messages sent to new recipients are grouped, how a transcript
//! is laid out, and the texts for what the engine reports.

pub mod address;
mod attachments;
mod composer;
mod keys;
mod list;
mod search;
mod texts;
mod transcript;
mod unsent;

pub use attachments::{attachment_kind, bare_content_type, part_file_name};
pub use composer::{ComposerTarget, Detail, Recipient, clamp_utf16};
pub use keys::{parse_part_key, part_key};
pub use list::conversations_json;
pub use search::{conversation_matches, unsent_matches};
pub use texts::{
    bluetooth_notice, cap_notice, cap_sentence, contact_name, conversation_preview, conversation_subtitle,
    conversation_title, new_message_preview, number_line, sim_name, unsent_preview, unsent_title,
};
pub use transcript::{TranscriptRow, transcript_row_json, transcript_rows};
pub use unsent::{UnsentGroup, unsent_groups};

#[cfg(test)]
mod tests;
