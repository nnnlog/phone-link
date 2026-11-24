//! The links other programs hand the app: `tel:` links, which open the Calls section's keypad with
//! their number.

#[cfg(test)]
mod tests;

const TEL: &str = "tel:";

/// Whether a command-line `argument` is a link the app opens.
pub fn is_link(argument: &str) -> bool {
    argument.get(..TEL.len()).is_some_and(|scheme| scheme.eq_ignore_ascii_case(TEL))
}

/// The number of a `tel:` link: the text after the scheme up to the first `;` or `?`, with its `%XX`
/// escapes decoded. `None` for another link, an empty number, or one that is not text once decoded.
pub fn tel_number(link: &str) -> Option<String> {
    if !is_link(link) {
        return None;
    }

    let number = link[TEL.len()..].split([';', '?']).next().unwrap_or_default();
    let number = String::from_utf8(percent_decoded(number.as_bytes())).ok()?;
    let number = number.trim();
    (!number.is_empty()).then(|| number.to_owned())
}

/// `bytes` with each `%XX` replaced by the byte it names; a `%` not followed by two hex digits stays.
fn percent_decoded(bytes: &[u8]) -> Vec<u8> {
    let hex = |byte: u8| char::from(byte).to_digit(16).and_then(|digit| u8::try_from(digit).ok());
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let [first, tail @ ..] = rest {
        match (first, tail) {
            (b'%', [high, low, after @ ..]) if hex(*high).is_some() && hex(*low).is_some() => {
                decoded.push(hex(*high).unwrap_or(0) << 4 | hex(*low).unwrap_or(0));
                rest = after;
            }
            _ => {
                decoded.push(*first);
                rest = tail;
            }
        }
    }
    decoded
}
