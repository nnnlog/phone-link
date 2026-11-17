//! The order of names in the user's language: the Unicode Collation Algorithm with the locale's
//! tailoring (ICU4X's compiled CLDR data), compared at the primary strength, which ignores case,
//! diacritics, the kana type (hiragana or katakana) and width.

use std::cmp::Ordering;

use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;
use tracing::info;

/// `value`, a locale as the environment names it (`ko_KR.UTF-8`), in BCP 47 form (`ko-KR`); `None` for
/// the C and POSIX locales and without a value.
pub fn collation_locale_name(value: Option<&str>) -> Option<String> {
    let value = value?;
    let name = value.split(['.', '@']).next().unwrap_or_default();
    if name.is_empty() || name == "C" || name == "POSIX" {
        return None;
    }
    Some(name.replace('_', "-"))
}

/// The locale the session sorts by, in BCP 47 form: the first of `LC_ALL`, `LC_COLLATE` and `LANG` that
/// is set and not empty; `None` when none is, or for the C and POSIX locales.
fn session_collation_locale() -> Option<String> {
    let value = ["LC_ALL", "LC_COLLATE", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty());
    collation_locale_name(value.as_deref())
}

/// Compares names as the session's locale sorts them, ignoring case, diacritics, kana type and width.
pub struct NameCollator {
    collator: CollatorBorrowed<'static>,
}

impl NameCollator {
    /// The collator of `locale` (BCP 47); the root collation, CLDR's order for every language without
    /// a tailoring, for no locale or one ICU4X does not read.
    pub fn new(locale: Option<&str>) -> Self {
        let parsed = match locale {
            Some(name) => Locale::try_from_str(name)
                .inspect_err(
                    |e| info!(locale = %name, error = %e, "the locale does not parse; names sort in the root order"),
                )
                .ok(),
            None => None,
        };
        let preferences: CollatorPreferences = parsed.as_ref().map(Into::into).unwrap_or_default();
        let mut options = CollatorOptions::default();
        options.strength = Some(Strength::Primary);
        // The compiled data holds the root collation, which every locale falls back to.
        let collator = Collator::try_new(preferences, options).expect("ICU4X's compiled data has a collator");
        Self { collator }
    }

    /// The collator of this session's locale.
    pub fn for_session() -> Self {
        Self::new(session_collation_locale().as_deref())
    }

    pub fn compare(&self, left: &str, right: &str) -> Ordering {
        self.collator.compare(left, right)
    }

    /// The positions of `names` in this order; equal names keep their order in `names`.
    pub fn order(&self, names: &[String]) -> Vec<u32> {
        let mut order: Vec<usize> = (0..names.len()).collect();
        order.sort_by(|a, b| self.compare(&names[*a], &names[*b]));
        order.into_iter().map(|index| index as u32).collect()
    }
}

#[cfg(test)]
mod tests;
