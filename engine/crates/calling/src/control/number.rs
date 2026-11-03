//! The number the Mac dials: what the user typed, normalized before dialing.

/// The first code point of each run of ten Unicode decimal digits (`Nd`) in the Basic Multilingual
/// Plane, plus the full-width digits.
const DIGIT_ZEROS: &[u32] = &[
    0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66, 0x0DE6,
    0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40,
    0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
];

/// The value of a decimal digit of any of [`DIGIT_ZEROS`]' runs.
fn digit_value(c: char) -> Option<u32> {
    let code = u32::from(c);
    DIGIT_ZEROS.iter().find(|&&zero| (zero..zero + 10).contains(&code)).map(|zero| code - zero)
}

/// The keypad digit of a letter: `abc` → 2 … `wxyz` → 9.
fn keypad_digit(c: char) -> Option<char> {
    let digit = match c.to_ascii_lowercase() {
        'a'..='c' => '2',
        'd'..='f' => '3',
        'g'..='i' => '4',
        'j'..='l' => '5',
        'm'..='o' => '6',
        'p'..='s' => '7',
        't'..='v' => '8',
        'w'..='z' => '9',
        _ => return None,
    };
    Some(digit)
}

/// Keeps decimal digits (as their value) and `#`, `*`, `+`; turns letters into their keypad digits;
/// drops everything else.
pub fn normalize_dialable(typed: &str) -> String {
    typed
        .chars()
        .filter_map(|c| {
            if let Some(v) = digit_value(c) {
                char::from_digit(v, 10)
            } else if matches!(c, '#' | '*' | '+') {
                Some(c)
            } else if c.is_ascii_alphabetic() {
                keypad_digit(c)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_dialable;

    #[test]
    fn keeps_digits_and_dial_symbols() {
        assert_eq!(normalize_dialable("+1 (555) 010-0*#"), "+15550100*#");
    }

    #[test]
    fn maps_letters_and_other_scripts() {
        assert_eq!(normalize_dialable("1-800-FLOWERS"), "18003569377");
        assert_eq!(normalize_dialable("０１２"), "012");
        assert_eq!(normalize_dialable("٠١٢٣"), "0123");
        assert_eq!(normalize_dialable("전화"), "");
    }
}
