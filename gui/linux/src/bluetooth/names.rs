//! Device names compared with the phone's names: without surrounding white space, case-folded and
//! canonically composed.

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};

/// Appends the Unicode full case folding of `c` to `folded`. The lowercase of the uppercase of the
/// lowercase puts every character in the class `CaseFolding.txt` gives it (`ß`, `ẞ` and `SS` together,
/// final and medial sigma together), except the dotless `ı`, which folds to itself.
fn push_case_folded(c: char, folded: &mut String) {
    if c == 'ı' {
        folded.push(c);
    } else {
        folded.extend(c.to_lowercase().flat_map(char::to_uppercase).flat_map(char::to_lowercase));
    }
}

/// `name` in the form names are compared in: without surrounding white space, case-folded and
/// canonically composed, so case and composed or decomposed letters do not tell names apart.
fn comparable_name(name: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfd().normalize(name.trim());
    let mut folded = String::with_capacity(decomposed.len());
    for c in decomposed.chars() {
        push_case_folded(c, &mut folded);
    }
    ComposingNormalizerBorrowed::new_nfc().normalize(&folded).into_owned()
}

/// A device named `device_name` is the phone named `phone_name`; an empty name matches nothing.
pub(super) fn names_match(device_name: &str, phone_name: &str) -> bool {
    let device = comparable_name(device_name);
    !device.is_empty() && device == comparable_name(phone_name)
}
