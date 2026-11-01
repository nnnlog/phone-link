//! Reading and validating inbound messaging content: metadata and item messages, identity,
//! timestamps and enumerations, and error handling.
//!
//! Parsing is pure: a message becomes a typed [`Metadata`] or [`Item`], or a [`Malformed`] naming the
//! rule it broke, before anything is changed. The rules that depend on state — RCS receive before
//! and after the message's own `settings`, the forced-resync condition, `Participants`, the PC
//! setting "receive MMS" — are passed in by the caller.

mod items;
mod metadata;
mod shared;

#[cfg(feature = "test-support")]
pub use items::clamp_message_date;
#[cfg(feature = "test-support")]
pub use items::processed_ids;
pub use items::{
    ConversationRow, Item, ItemRules, MmsPartRow, MmsRow, RcsChatRow, RcsConversationRow, RcsFileTransferRow, RcsRow,
    Rows, SmsRow, SubscriptionRow, item,
};
#[cfg(feature = "test-support")]
pub use metadata::Metadata;
pub use metadata::{MetadataRules, TypeLists, metadata};
pub use shared::{AnchorUpdate, acks, correlation_vector, sequence_numbers, settings};

use phonelink_app::Malformed;
use phonelink_core::codec::valueset::{FromValue, Value, ValueSet};

pub type Parsed<T> = Result<T, Malformed>;

const MISSING: &str = "messaging key missing";
const MISTYPED: &str = "messaging key of another type";

// ------------------------------------------------------------------------------------------------
// Typed reads
// ------------------------------------------------------------------------------------------------

/// The key with exactly type `T`; absent ⇒ `None`; another type ⇒ malformed.
pub fn opt<'a, T: FromValue<'a>>(vs: &'a ValueSet, k: &'static str) -> Parsed<Option<T>> {
    vs.read_checked(k).map_err(|_| Malformed::new(MISTYPED, k))
}

/// The key with exactly type `T`; absent or another type ⇒ malformed.
pub fn req<'a, T: FromValue<'a>>(vs: &'a ValueSet, k: &'static str) -> Parsed<T> {
    opt(vs, k)?.ok_or(Malformed::new(MISSING, k))
}

/// A sub-object: an `Object` value; absent or of another type ⇒ `None` (a part whose value is not an
/// Object is ignored).
pub fn object<'a>(vs: &'a ValueSet, k: &str) -> Option<&'a ValueSet> {
    match vs.get(k) {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
