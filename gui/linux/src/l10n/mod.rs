//! The app's texts in the user's language: the English texts of the QML and of the Rust code, with
//! their Korean from the catalog the build generates when the messages locale is Korean, and the
//! catalog's format specifiers filled in. The `L10n` QML singleton gives the QML the same lookups.

use std::sync::OnceLock;

use cxx_qt_lib::{QString, QStringList};

mod table {
    include!(concat!(env!("OUT_DIR"), "/translations.rs"));
}

// cxx-qt registers every bridge of the QML module from one folder (see `engine_bridge`).
#[expect(unsafe_code, reason = "a cxx-qt bridge declares functions that cross the C++ boundary")]
#[path = "../bridges/l10n.rs"]
mod bridge;

use bridge::qobject;

/// The Rust side of the `L10n` QML singleton.
#[derive(Default)]
pub struct L10nRust;

/// The language the user's messages are in: the first of `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` and `LANG`
/// that is set and not empty, as it is set (such as `ko_KR.UTF-8`, or a `LANGUAGE` list).
pub fn messages_locale() -> Option<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
}

fn korean_enabled() -> bool {
    static KOREAN: OnceLock<bool> = OnceLock::new();
    *KOREAN.get_or_init(|| messages_locale().is_some_and(|value| value.starts_with("ko")))
}

/// `text`'s entry in `table`, or `text` itself when it has none.
fn lookup<'a>(table: &[(&'static str, &'static str)], text: &'a str) -> &'a str {
    table.binary_search_by(|(key, _)| (*key).cmp(text)).map_or(text, |index| table[index].1)
}

/// `text` translated, as a string of its own.
pub fn translated(text: &str) -> String {
    translate(text).to_owned()
}

/// `template` translated, then filled with `args` (`fill`).
pub fn filled(template: &str, args: &[String]) -> String {
    fill(translate(template), args)
}

/// `text` in the user's language: its Korean when the messages locale is Korean and the catalog has
/// it, else `text` itself.
pub fn translate(text: &str) -> &str {
    if !korean_enabled() {
        return text;
    }
    lookup(table::KOREAN, text)
}

/// `text`, a catalog key with plural variants, for a count that is one (`one`) or not: in English the
/// catalog's singular form for one, and `text` itself for any other count; in Korean, which has no
/// plural forms, its one translation for every count.
fn translate_plural(text: &str, one: bool, korean: bool) -> &str {
    if korean {
        lookup(table::KOREAN, text)
    } else if one {
        lookup(table::ENGLISH_ONE, text)
    } else {
        text
    }
}

/// `fill(translate(text), args)` for `text`, a catalog key with plural variants, that counts `count`
/// things: in English, one thing takes the key's singular form ("1 file", not "1 files").
pub fn fill_plural(text: &str, count: usize, args: &[String]) -> String {
    fill(translate_plural(text, count == 1, korean_enabled()), args)
}

const SPECIFIERS: [&str; 5] = ["@", "lld", "ld", "d", "u"];

/// Fills `%@`, `%lld`, `%ld`, `%d` and `%u`, in order, and their positional forms such as `%2$@`, as the
/// macOS catalog writes them, and writes `%%` as `%`.
pub fn fill(template: &str, args: &[String]) -> String {
    let mut result = String::new();
    let mut rest = template;
    let mut next = 0;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(after_percent) = after.strip_prefix('%') {
            result.push('%');
            rest = after_percent;
            continue;
        }
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        let (index, spec_start) = if digits > 0 && after[digits..].starts_with('$') {
            (after[..digits].parse::<usize>().ok().map(|n| n.saturating_sub(1)), digits + 1)
        } else {
            (None, 0)
        };
        match SPECIFIERS.iter().find(|specifier| after[spec_start..].starts_with(*specifier)) {
            Some(specifier) => {
                let position = index.unwrap_or_else(|| {
                    next += 1;
                    next - 1
                });
                result.push_str(args.get(position).map_or("", String::as_str));
                rest = &after[spec_start + specifier.len()..];
            }
            None => {
                result.push('%');
                rest = after;
            }
        }
    }

    result.push_str(rest);
    result
}

// The QML calls these by their one-letter names (`L10n.t`, `L10n.f`, `L10n.p`).
impl qobject::L10n {
    /// `text` translated.
    pub fn t(&self, text: &QString) -> QString {
        QString::from(translate(&text.to_string()))
    }

    /// `text` translated, then filled with `args`.
    pub fn f(&self, text: &QString, args: &QStringList) -> QString {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        QString::from(fill(translate(&text.to_string()), &args).as_str())
    }

    /// `f` for a catalog key with plural variants that counts `count` things.
    pub fn p(&self, text: &QString, count: i64, args: &QStringList) -> QString {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        QString::from(fill(translate_plural(&text.to_string(), count == 1, korean_enabled()), &args).as_str())
    }
}

#[cfg(test)]
mod tests;
