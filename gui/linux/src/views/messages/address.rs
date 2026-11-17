//! How the new message reads the addresses the user typed, and when two of them name the same
//! recipient.

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// An address the user may enter as it is: a phone number (dial characters only, at least three
/// digits) or an email address.
pub fn is_plausible(text: &str) -> bool {
    let typed = text.trim();
    if typed.contains('@') {
        let sides: Vec<&str> = typed.split('@').collect();
        return sides.len() == 2 && !sides[0].is_empty() && sides[1].contains('.') && !typed.contains(' ');
    }
    typed.chars().all(|c| "+0123456789 -().#*".contains(c)) && digits(typed).len() >= 3
}

/// Whether two addresses name the same recipient.
///
/// The phone stores numbers in its own form (often E.164) while the user types the national one, so
/// numbers match when, with leading zeros (a trunk prefix) dropped, the shorter of at least seven
/// digits ends the longer — the longer may carry the country code. Other addresses match
/// case-insensitively.
pub fn same(a: &str, b: &str) -> bool {
    let (da, db) = (digits(a), digits(b));
    if a.contains('@') || b.contains('@') || da.is_empty() || db.is_empty() {
        return a.trim().to_lowercase() == b.trim().to_lowercase();
    }
    if da == db {
        return true;
    }

    let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
    let (short, long) = if ta.len() <= tb.len() { (ta, tb) } else { (tb, ta) };
    short.len() >= 7 && long.ends_with(short)
}

/// What to search the contacts' numbers for to find the contact of `address`: its last eight digits.
///
/// Every form of the number ends with them (the search matches a number containing the digits);
/// `None` for an email address or no digits.
pub fn contact_search(address: &str) -> Option<String> {
    let digits = digits(address);
    if address.contains('@') || digits.is_empty() {
        return None;
    }
    Some(digits[digits.len().saturating_sub(8)..].to_owned())
}

/// Whether two lists of addresses name the same people, in any order.
pub fn same_recipients(a: &[String], b: &[String]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut unmatched: Vec<&String> = b.iter().collect();
    for address in a {
        let Some(index) = unmatched.iter().position(|other| same(address, other)) else { return false };
        unmatched.remove(index);
    }
    true
}
