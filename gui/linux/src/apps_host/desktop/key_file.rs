//! The key file format of desktop entries and `mimeapps.list`: groups of keys, string and list
//! values with their escapes, and the locale names a localized key is looked up by.

use std::collections::HashMap;

/// The value of a desktop entry or `mimeapps.list` key with the string escapes undone.
pub(super) fn unescape(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => text.push(' '),
            Some('n') => text.push('\n'),
            Some('t') => text.push('\t'),
            Some('r') => text.push('\r'),
            Some('\\') => text.push('\\'),
            Some(other) => {
                text.push('\\');
                text.push(other);
            }
            None => text.push('\\'),
        }
    }

    text
}

/// A `;`-separated list value, `\;` standing for a semicolon in an item; empty items dropped.
pub(super) fn list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut item = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&';') => {
                item.push(';');
                chars.next();
            }
            ';' => items.push(std::mem::take(&mut item)),
            _ => item.push(c),
        }
    }

    items.push(item);
    items.into_iter().map(|item| unescape(item.trim())).filter(|item| !item.is_empty()).collect()
}

/// The groups of a key file: group name → key (with its locale, as written) → raw value. A key
/// repeated in a group keeps its first value.
pub(super) fn key_file(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = Some(name.to_owned());
            groups.entry(name.to_owned()).or_default();
            continue;
        }
        let (Some(group), Some((key, value))) = (current.as_ref(), line.split_once('=')) else { continue };
        let entries = groups.entry(group.clone()).or_default();
        entries.entry(key.trim().to_owned()).or_insert_with(|| value.trim().to_owned());
    }

    groups
}

/// The locale's names a localized key is looked up by, best first: `lang_COUNTRY@MODIFIER`,
/// `lang_COUNTRY`, `lang@MODIFIER`, `lang`.
pub fn locale_keys(locale: &str) -> Vec<String> {
    let locale = locale.split(':').next().unwrap_or_default();
    let (rest, modifier) = match locale.split_once('@') {
        Some((rest, modifier)) => (rest, Some(modifier)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or_default();
    if rest.is_empty() || rest == "C" || rest == "POSIX" {
        return Vec::new();
    }
    let (lang, country) = match rest.split_once('_') {
        Some((lang, country)) => (lang, Some(country)),
        None => (rest, None),
    };
    let mut keys = Vec::new();
    if let (Some(country), Some(modifier)) = (country, modifier) {
        keys.push(format!("{lang}_{country}@{modifier}"));
    }
    if let Some(country) = country {
        keys.push(format!("{lang}_{country}"));
    }
    if let Some(modifier) = modifier {
        keys.push(format!("{lang}@{modifier}"));
    }
    keys.push(lang.to_owned());
    keys
}
