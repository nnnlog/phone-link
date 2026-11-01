//! Phone-number normalization shared by contacts and messaging.
//!
//! It covers the E.164 form of an address and the phone's country it is normalized with, using
//! libphonenumber's metadata through the `phonenumber` crate.

use phonelink_app::PhoneRecord;

/// The ISO 3166-1 alpha-2 region in a country or locale code, upper-case: `KR`, `kr`, `ko-KR`,
/// `ko_KR`, `zh-Hant-TW` → the two-letter part; `None` when there is none (`es-419`, `""`).
pub fn region_from_code(code: &str) -> Option<String> {
    let code = code.trim();
    let is_region = |s: &str| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic());
    if is_region(code) {
        return Some(code.to_ascii_uppercase());
    }
    code.split(['-', '_']).skip(1).find(|part| is_region(part)).map(str::to_ascii_uppercase)
}

/// The phone's country.
///
/// The region of the record's country code — written by the connect response's
/// `attributes.countryCode` and by the device list's `defaultLocale`, whichever came last — or the
/// PC's region while neither has been received.
///
/// A received code without a two-letter region (`""`,
/// `es-419`) gives no region — not the PC's — so national numbers stay as received.
pub fn phone_country(record: &PhoneRecord, pc_region: Option<&str>) -> Option<String> {
    match record.country_code.as_deref() {
        Some(code) => region_from_code(code),
        None => pc_region.and_then(region_from_code),
    }
}

/// The E.164 form of `address` (`+<country code><national number>`).
///
/// The address is parsed with `region` as the default region; `None` when it does not parse as a
/// phone number (callers keep the address unchanged).
pub fn to_e164(address: &str, region: Option<&str>) -> Option<String> {
    let id = region.and_then(region_from_code).and_then(|r| r.parse::<phonenumber::country::Id>().ok());
    let number = phonenumber::parse(id, address).ok()?;
    Some(number.format().mode(phonenumber::Mode::E164).to_string())
}

#[cfg(test)]
mod tests;
