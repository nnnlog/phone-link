use std::sync::OnceLock;

use cxx_qt_lib::{QString, QStringList};

mod table {
    include!(concat!(env!("OUT_DIR"), "/translations.rs"));
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type L10n = super::L10nRust;

        #[qinvokable]
        fn t(self: &L10n, text: &QString) -> QString;

        #[qinvokable]
        fn f(self: &L10n, text: &QString, args: &QStringList) -> QString;
    }
}

#[derive(Default)]
pub struct L10nRust;

fn korean_enabled() -> bool {
    static KOREAN: OnceLock<bool> = OnceLock::new();
    *KOREAN.get_or_init(|| {
        ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .is_some_and(|value| value.starts_with("ko"))
    })
}

pub fn translate(text: &str) -> &str {
    if !korean_enabled() {
        return text;
    }
    table::KOREAN.binary_search_by(|(key, _)| (*key).cmp(text)).map_or(text, |index| table::KOREAN[index].1)
}

const SPECIFIERS: [&str; 4] = ["@", "lld", "ld", "d"];

/// Fills `%@`, `%lld`, `%ld` and `%d`, in order, and their positional forms such as `%2$@`, as the
/// macOS catalog writes them.
pub fn fill(template: &str, args: &[String]) -> String {
    let mut result = String::new();
    let mut rest = template;
    let mut next = 0;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        let (index, spec_start) = if digits > 0 && after[digits..].starts_with('$') {
            (after[..digits].parse::<usize>().ok().map(|n| n.saturating_sub(1)), digits + 1)
        } else {
            (None, 0)
        };
        match SPECIFIERS.iter().find(|spec| after[spec_start..].starts_with(*spec)) {
            Some(spec) => {
                let position = index.unwrap_or_else(|| {
                    next += 1;
                    next - 1
                });
                result.push_str(args.get(position).map_or("", String::as_str));
                rest = &after[spec_start + spec.len()..];
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

impl qobject::L10n {
    pub fn t(&self, text: &QString) -> QString {
        QString::from(translate(&text.to_string()))
    }

    pub fn f(&self, text: &QString, args: &QStringList) -> QString {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        QString::from(fill(translate(&text.to_string()), &args).as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn fills_sequential_and_positional_specifiers() {
        assert_eq!(fill("%@ Camera", &args(&["Fold"])), "Fold Camera");
        assert_eq!(fill("%2$@GB of %1$@GB", &args(&["10", "64"])), "64GB of 10GB");
        assert_eq!(fill("%lld new", &args(&["3"])), "3 new");
        assert_eq!(fill("100% sure", &[]), "100% sure");
    }

    #[test]
    fn table_is_sorted_for_lookup() {
        assert!(table::KOREAN.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }
}
