//! Typed reads of the connect reply that log a mistyped value.

use phonelink_core::codec::valueset::{FromValue, ValueSet};
use tracing::warn;

/// The value of `key` if it has type `T`; a key of another type is logged and ignored.
pub(super) fn typed<'a, T: FromValue<'a>>(vs: &'a ValueSet, key: &str) -> Option<T> {
    vs.read_checked(key).unwrap_or_else(|e| {
        warn!(wire_key = key, value_type = e.found, want = e.want, "connect response key of another type ignored");
        None
    })
}

pub(super) fn set_string(slot: &mut Option<String>, vs: &ValueSet, key: &str) {
    if let Some(v) = typed::<&str>(vs, key) {
        *slot = Some(v.to_string());
    }
}

pub(super) fn set_i32(slot: &mut Option<i32>, vs: &ValueSet, key: &str) {
    if let Some(v) = typed::<i32>(vs, key) {
        *slot = Some(v);
    }
}

pub(super) fn set_bool(slot: &mut Option<bool>, vs: &ValueSet, key: &str) {
    if let Some(v) = typed::<bool>(vs, key) {
        *slot = Some(v);
    }
}
