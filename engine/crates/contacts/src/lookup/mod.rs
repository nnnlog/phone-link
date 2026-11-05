//! Lookup by address, done locally: the app's single rule for every consumer.
//!
//! Only an address that passes [`is_lookup_address`] is looked up; any other address resolves to no
//! contact, and a stored value that fails the same test (a phone number in its normalized form, an
//! e-mail address as stored) is not indexed.
//!
//! Every stored `phoneNumbers` value (not the display number) is normalized to E.164 with the
//! phone's country and compared with the address normalized with the caller's region (the message's
//! subscription country), else the phone's; an address that looks like an e-mail address is matched
//! verbatim against the stored e-mail addresses. A value that does not parse as a phone number is
//! compared unchanged. Only records whose owning contact is stored take part. No traffic is
//! involved.
//!
//! Several contacts can share an address. They are taken by contact id while the birthday flag
//! (`ContactsConfig::birthday_text`) is on, by display name while it is off, and the match takes its
//! name from the first of them with a non-empty display name (none when no contact has one: the view
//! then shows the address) and its photo from the first with a thumbnail. An e-mail address is
//! recognized by a fixed pattern ([`is_email`]).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::Regex;

use phonelink_core::types::DcgClient;
use phonelink_features::ContactMatch;
use phonelink_features::shared::db::StoreError;
use phonelink_features::shared::phone_number::{region_from_code, to_e164};

use super::store::ContactsStore;
use phonelink_core::sync::MutexExt;

/// `number` in E.164 with `region` as the default region; unchanged when it does not parse.
pub fn normalize(number: &str, region: Option<&str>) -> String {
    to_e164(number, region).unwrap_or_else(|| number.to_string())
}

/// The pattern `(?i)\b((mailto:)?[\w.+,-]+@[a-z0-9.-]+\.[a-z0-9.-]+[a-z0-9]*)`, matched at index 0
/// over the whole address in UTF-16 code units, with its classes spelled out:
/// - `\w` is a Basic Multilingual Plane character of category L, Mn, Nd or Pc (a character outside
///   the BMP is two surrogates, neither of them a word character);
/// - `\b` at index 0 holds when the first character is a word character (ZWJ and ZWNJ, the two other
///   characters `\b` counts, cannot start a match);
/// - `(?i)` turns `mailto:` into its ASCII case variants and `[a-z0-9.-]` into `[-.0-9A-Za-z]` plus
///   U+212A KELVIN SIGN, and adds nothing to `[\w.+,-]`;
/// - `[a-z0-9.-]+\.[a-z0-9.-]+[a-z0-9]*` is a run of the domain class with a `.` that has a class
///   character on each side (the tail class is part of the domain class).
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    const WORD: &str = r"\p{L}\p{Mn}\p{Nd}\p{Pc}";
    const BMP: &str = r"\x{0}-\x{FFFF}";
    const DOMAIN: &str = r"[\-.0-9A-Za-z\x{212A}]";

    let pattern = format!(
        r"^(?:[mM][aA][iI][lL][tT][oO]:[{WORD}.+,\-&&{BMP}]+|[{WORD}&&{BMP}][{WORD}.+,\-&&{BMP}]*)@{DOMAIN}+\.{DOMAIN}+$"
    );
    Regex::new(&pattern).expect("the e-mail pattern is a valid regex")
});

/// Whether `address` looks like an e-mail address, by the pattern [`EMAIL`].
pub fn is_email(address: &str) -> bool {
    EMAIL.is_match(address)
}

/// One printable UTF-16 code unit outside the phone-number characters.
///
/// A code unit is printable when it is a whole character (not half of a surrogate pair) of Unicode
/// general category letter, mark, number, punctuation, symbol or space separator (Unicode 16.0, the
/// regex library's tables); control, format, private-use and unassigned characters, line and paragraph
/// separators, and both halves of a character outside the Basic Multilingual Plane are not printable.
/// The phone-number characters are `0`–`9`, `+`, `-`, `(`, `)`, space, `*`, `#`, `?`, `.`, `/`,
/// U+00A0 NO-BREAK SPACE and U+2011 NON-BREAKING HYPHEN.
static NOT_PHONE_NUMBER_CHAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[[\p{L}\p{M}\p{N}\p{P}\p{S}\p{Zs}]&&[\x{0}-\x{FFFF}]&&[^0-9+\-()\x{20}*#?./\x{A0}\x{2011}]]")
        .expect("the phone-number character class is a valid regex")
});

/// Whether every printable UTF-16 code unit of `address` is a phone-number character
/// ([`NOT_PHONE_NUMBER_CHAR`]); code units that are not printable are ignored, so an empty address
/// passes.
pub fn has_only_phone_number_chars(address: &str) -> bool {
    !NOT_PHONE_NUMBER_CHAR.is_match(address)
}

/// The white-space characters: U+0009–U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000–U+200A, U+2028,
/// U+2029, U+202F, U+205F and U+3000.
fn is_white_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{D}'
            | ' '
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Whether `address` takes part in the lookup at all.
///
/// It must not be empty or white space alone, and must be an e-mail address ([`is_email`]) or made
/// only of phone-number characters ([`has_only_phone_number_chars`]). A sender name such as `AMAZON`
/// or a number written with letters fails, so it never resolves to a contact.
pub fn is_lookup_address(address: &str) -> bool {
    !address.chars().all(is_white_space) && (is_email(address) || has_only_phone_number_chars(address))
}

/// One phone's lookup index, built from its store for one phone country.
struct Index {
    country: Option<String>,
    by_number: HashMap<String, Vec<i64>>,
    by_email: HashMap<String, Vec<i64>>,
}

fn push(map: &mut HashMap<String, Vec<i64>>, key: String, contact_id: i64) {
    let ids = map.entry(key).or_default();
    if !ids.contains(&contact_id) {
        ids.push(contact_id);
    }
}

impl Index {
    fn build(store: &ContactsStore, country: Option<String>) -> Result<Self, StoreError> {
        let mut by_number = HashMap::new();
        for row in store.phone_number_rows()? {
            let key = normalize(&row.address, country.as_deref());
            if is_lookup_address(&key) {
                push(&mut by_number, key, row.contact_id);
            }
        }

        let mut by_email = HashMap::new();
        for row in store.email_rows()? {
            if is_lookup_address(&row.address) {
                push(&mut by_email, row.address, row.contact_id);
            }
        }
        Ok(Self { country, by_number, by_email })
    }
}

/// The per-phone indexes, rebuilt after every store change ([`LookupCache::invalidate`]) and when the
/// phone's country changes.
#[derive(Default)]
pub struct LookupCache {
    indexes: Mutex<HashMap<DcgClient, Index>>,
}

impl LookupCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops `phone`'s index; called after each committed change of its store.
    pub fn invalidate(&self, phone: &DcgClient) {
        self.indexes.lock_or_panic().remove(phone);
    }

    /// The contacts `address` resolves to on `phone`; `country` is the phone's country, `region` the
    /// caller's (used for the address when it names a region).
    pub fn candidates(
        &self,
        store: &ContactsStore,
        phone: &DcgClient,
        country: Option<String>,
        address: &str,
        region: Option<&str>,
    ) -> Result<Vec<i64>, StoreError> {
        if !is_lookup_address(address) {
            return Ok(Vec::new());
        }

        // Held while an index is built, so an invalidation after a write waits for the build and
        // then drops it.
        let mut indexes = self.indexes.lock_or_panic();
        let stale = indexes.get(phone).is_none_or(|i| i.country != country);
        if stale {
            let index = Index::build(store, country.clone())?;
            indexes.insert(phone.clone(), index);
        }

        let Some(index) = indexes.get(phone) else {
            return Ok(Vec::new());
        };

        let found = if is_email(address) {
            index.by_email.get(address)
        } else {
            let region = region.and_then(region_from_code).or(country);
            index.by_number.get(&normalize(address, region.as_deref()))
        };
        Ok(found.cloned().unwrap_or_default())
    }
}

/// The match for the `candidates` (contact ids), taken by id, or by display name (then id) when
/// `by_name`.
///
/// The first with a non-empty display name names it (when none has one, the first is the match and
/// it has no name: an empty name is no name), the first with a thumbnail gives its photo.
///
/// Reads only names and whether a thumbnail is stored.
pub fn build_match(
    store: &ContactsStore,
    candidates: &[i64],
    by_name: bool,
) -> Result<Option<ContactMatch>, StoreError> {
    let mut contacts = Vec::new();
    for id in candidates {
        if let Some(c) = store.contact_summary(*id)? {
            contacts.push(c);
        }
    }

    if by_name {
        contacts.sort_by(|a, b| a.display_name.cmp(&b.display_name).then(a.id.cmp(&b.id)));
    } else {
        contacts.sort_by_key(|c| c.id);
    }

    let Some(first) = contacts.first() else {
        return Ok(None);
    };

    let named = contacts.iter().find(|c| !c.display_name.is_empty());
    Ok(Some(ContactMatch {
        contact_id: named.unwrap_or(first).id,
        display_name: named.map(|c| c.display_name.clone()),
        photo_contact_id: contacts.iter().find(|c| c.has_thumbnail).map(|c| c.id),
    }))
}

#[cfg(test)]
mod tests;
