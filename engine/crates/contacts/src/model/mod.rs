//! What the UI shows of contacts (the stored state, the sequence numbers, the contacts
//! `/permission` request, the `displayDate` of sub-records).
//!
//! It covers plain structs the UI reads, the contacts-view prompt, and the messaging birthday rule.

use std::sync::LazyLock;
use std::time::SystemTime;

use regex::{Captures, Regex};

use phonelink_app::AppOutcome;

use super::parse::{
    ContactDateRecord, ContactRecord, ContactUrlRecord, EmailAddressRecord, PhoneNumberRecord, PostalAddressRecord,
};
use super::wire::{ContactDateType, RecordType};

/// One row of the contacts list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactSummary {
    pub id: i64,
    pub display_name: String,
    pub has_thumbnail: bool,
}

/// One phone number of a stored contact with the contact's name: what the composer's recipient
/// suggestions search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactNumber {
    pub contact_id: i64,
    pub display_name: String,
    /// The contact has a thumbnail (its bytes are read with [`super::Contacts::thumbnail`]).
    pub has_thumbnail: bool,
    pub number: String,
    /// The number as the phone displays it; empty when the phone sent none.
    pub display_number: String,
    /// `PhoneNumberType`, as stored (read it with [`super::wire::PhoneNumberType::from_i32`]).
    pub kind: i32,
    pub label: String,
}

/// One contact with its sub-records (each by record id). Enum fields are the stored `Int32`s; read
/// them with the `from_i32` of [`super::wire::PhoneNumberType`] and its siblings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactDetail {
    /// Every contact field, the thumbnail bytes included (`None` = no photo).
    pub contact: ContactRecord,
    pub phone_numbers: Vec<PhoneNumberRecord>,
    pub email_addresses: Vec<EmailAddressRecord>,
    pub postal_addresses: Vec<PostalAddressRecord>,
    pub dates: Vec<ContactDateRecord>,
    pub urls: Vec<ContactUrlRecord>,
}

impl ContactDetail {
    /// The contact's first `Birthday` date, as display text (the birthday banner's input).
    pub fn birthday(&self) -> Option<&str> {
        self.dates.iter().find(|d| d.kind == ContactDateType::Birthday as i32).map(|d| d.display_date.as_str())
    }
}

/// The up-to-date state: contacts are up to date when `current ≥ expected` for every stored type.
/// UI only — it triggers no traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    /// *Current* per type in [`RecordType`] order (`-1` = none).
    pub current: [i64; 6],
    /// *Expected* per type (`-1` = none).
    pub expected: [i64; 6],
    pub up_to_date: bool,
    /// When the last sync session completed; `None` = never.
    pub last_updated: Option<SystemTime>,
}

impl SyncState {
    pub fn current_of(&self, t: RecordType) -> i64 {
        self.current[t.index()]
    }
    pub fn expected_of(&self, t: RecordType) -> i64 {
        self.expected[t.index()]
    }
}

/// The contacts view's prompt: which action the view offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactsPrompt {
    /// Nothing to ask.
    None,
    /// PC consent is off; the action gives it ([`super::Contacts::set_pc_consent`]).
    PcConsentRequired,
    /// The contacts device permission is not granted and contacts were never set up; the action
    /// asks the phone ([`super::Contacts::request_device_permission`]).
    DeviceConsentRequired,
    /// As above, after contacts were set up once (the phone denied access later).
    DeviceConsentDenied,
}

impl ContactsPrompt {
    /// PC consent first, then the device permission.
    pub fn from_state(pc_consent: bool, device_permission: bool, setup_completed_once: bool) -> Self {
        if !pc_consent {
            ContactsPrompt::PcConsentRequired
        } else if !device_permission {
            if setup_completed_once {
                ContactsPrompt::DeviceConsentDenied
            } else {
                ContactsPrompt::DeviceConsentRequired
            }
        } else {
            ContactsPrompt::None
        }
    }
}

/// The permission and capability state of one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionState {
    /// The contacts device permission (default `true`).
    pub device_permission: bool,
    /// PC consent (default `true`).
    pub pc_consent: bool,
    /// "Contacts set up once" (default `false`).
    pub setup_completed_once: bool,
    /// The phone's feature bits include `0x800`.
    pub contacts_capable: bool,
    /// `ContactsCapabilities` (default `0`).
    pub contacts_capabilities: i32,
}

/// What the contacts view's permission action did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptResult {
    /// The `/permission` outcome.
    pub outcome: AppOutcome,
    /// The prompt timed out: the view shows "waited too long"; the permission is unchanged.
    pub timed_out: bool,
    /// The follow-up `/contacts/v2` outcome, sent because the permission was `true` afterwards.
    pub sync: Option<AppOutcome>,
}

/// The messaging birthday banner (from the sub-records' `displayDate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BirthdayBanner {
    None,
    Yesterday,
    Today,
    Tomorrow,
    /// 2 to 15 days ahead.
    Soon,
}

/// A calendar date (the UI's local "today").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl CivilDate {
    /// A valid calendar date, else `None`.
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        ((1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)).then_some(Self {
            year,
            month,
            day,
        })
    }

    /// Days since 1970-01-01 (proleptic Gregorian).
    fn days(self) -> i64 {
        // Howard Hinnant's days_from_civil.
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1..=12 => phonelink_core::time::days_in_month(i64::from(year), i64::from(month)) as u32,
        _ => 0,
    }
}

/// A parsed `displayDate`: the date and the time of day, in 100-ns ticks after midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayDate {
    pub date: CivilDate,
    pub ticks: i64,
}

const TICKS_PER_SECOND: i64 = 10_000_000;
const TICKS_PER_DAY: i64 = 86_400 * TICKS_PER_SECOND;

/// The English month names; each abbreviation is the name's first three letters.
const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a display-date pattern is a valid regex")
}

/// `M<s>d`, `M<s>d<s>y` and `yyyy<s>M<s>d`, with one separator `/`, `-` or `.` throughout.
static NUMERIC: LazyLock<Regex> = LazyLock::new(|| regex(r"^([0-9]{1,4})([-/.])([0-9]{1,2})(?:([-/.])([0-9]{1,4}))?$"));
/// `yyyy-M-d` and a time of day after `T` or a space: `HH:mm`, `HH:mm:ss`, `HH:mm:ss.fffffff`.
static ISO_TIME: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})[T ]([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]{1,7}))?)?$")
});
/// `<month> d` and `<month> d y`.
static MONTH_DAY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^([A-Za-z]+)(\.?)[ ,]*([0-9]{1,2})(?:[ ,]+([0-9]{2}|[0-9]{4}))?$"));
/// `d <month> y`.
static DAY_MONTH_YEAR: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^([0-9]{1,2})[ ,]+([A-Za-z]+)(\.?)[ ,]+([0-9]{2}|[0-9]{4})$"));
/// `yyyy <month> d` (no separator needed between `<month>` and `d`).
static YEAR_MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| regex(r"^([0-9]{4})[ ,]+([A-Za-z]+)(\.?)[ ,]*([0-9]{1,2})$"));

fn group<'h>(c: &Captures<'h>, i: usize) -> Option<&'h str> {
    c.get(i).map(|m| m.as_str())
}

fn int(s: &str) -> Option<u32> {
    s.parse().ok()
}

/// A year: four digits as written (1–9999); two digits up to 2049 — `00`–`49` are 2000–2049,
/// `50`–`99` 1950–1999.
fn year(s: &str) -> Option<i32> {
    let n: i32 = s.parse().ok()?;
    match s.len() {
        2 => Some(if n <= 49 { 2000 + n } else { 1900 + n }),
        4 => (1..=9999).contains(&n).then_some(n),
        _ => None,
    }
}

/// An English month name (full, without a following `.`) or its three-letter abbreviation (with or
/// without one), in any ASCII case.
fn month(word: &str, dot: &str) -> Option<u32> {
    let word = word.to_ascii_lowercase();
    let index = MONTHS.iter().position(|m| (dot.is_empty() && *m == word) || (word.len() == 3 && m[..3] == word))?;
    Some(index as u32 + 1)
}

fn date_only(date: Option<CivilDate>) -> Option<DisplayDate> {
    date.map(|date| DisplayDate { date, ticks: 0 })
}

/// The `displayDate` forms the app reads for the banner (Phone Link on Windows reads more; anything
/// else is unparsable here).
///
/// After
/// trimming white space, with `M` and `d` of one or two digits, `y` of two or four digits (two-digit
/// years as [`year`] says), `<month>` an English month name or three-letter abbreviation (an
/// abbreviation may end in `.`), in any case, and a missing year taken from `today`:
/// - `M<s>d`, `M<s>d<s>y`, `yyyy<s>M<s>d`, with one separator `<s>` — `/`, `-` or `.` — throughout;
/// - `yyyy-M-d` followed by `T` or a space and `HH:mm`, `HH:mm:ss` or `HH:mm:ss.f` (1–7 digits), no
///   zone;
/// - `<month> d`, `<month> d y`, `d <month> y`, `yyyy <month> d`, the parts separated by spaces and
///   commas (none needed between `<month>` and `d`). Not read: hyphens in these forms, and
///   `d <month>` alone.
///
/// A day the month does not have is unparsable here (Phone Link on Windows may read such text with
/// its fields swapped: `February 31 26` as 2031-02-26).
pub fn parse_display_date(text: &str, today: CivilDate) -> Option<DisplayDate> {
    let text = text.trim();

    if let Some(c) = NUMERIC.captures(text) {
        return numeric_date(&c, today);
    }
    if let Some(c) = ISO_TIME.captures(text) {
        return iso_date_time(&c);
    }
    if let Some(c) = MONTH_DAY.captures(text) {
        return month_day_date(&c, today);
    }
    if let Some(c) = DAY_MONTH_YEAR.captures(text) {
        let m = month(group(&c, 2)?, group(&c, 3)?)?;
        return date_only(CivilDate::new(year(group(&c, 4)?)?, m, int(group(&c, 1)?)?));
    }
    if let Some(c) = YEAR_MONTH_DAY.captures(text) {
        let m = month(group(&c, 2)?, group(&c, 3)?)?;
        return date_only(CivilDate::new(year(group(&c, 1)?)?, m, int(group(&c, 4)?)?));
    }
    None
}

/// The `M<s>d`, `M<s>d<s>y` and `yyyy<s>M<s>d` forms, from a [`NUMERIC`] match.
fn numeric_date(c: &Captures<'_>, today: CivilDate) -> Option<DisplayDate> {
    let first = group(c, 1)?;
    let (sep, second) = (group(c, 2)?, group(c, 3)?);
    if group(c, 4).is_some_and(|s| s != sep) {
        return None;
    }
    match (first.len(), group(c, 5)) {
        (4, Some(day)) if day.len() <= 2 => date_only(CivilDate::new(year(first)?, int(second)?, int(day)?)),
        (1 | 2, Some(y)) => date_only(CivilDate::new(year(y)?, int(first)?, int(second)?)),
        (1 | 2, None) => date_only(CivilDate::new(today.year, int(first)?, int(second)?)),
        _ => None,
    }
}

/// The `yyyy-M-d` form followed by a time of day, from an [`ISO_TIME`] match.
fn iso_date_time(c: &Captures<'_>) -> Option<DisplayDate> {
    let date = CivilDate::new(year(group(c, 1)?)?, int(group(c, 2)?)?, int(group(c, 3)?)?)?;
    let (hour, minute) = (int(group(c, 4)?)?, int(group(c, 5)?)?);
    let second = group(c, 6).map_or(Some(0), int)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let fraction = group(c, 7).map_or(Some(0), |f| format!("{f:0<7}").parse::<i64>().ok())?;
    let ticks = (i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)) * TICKS_PER_SECOND + fraction;
    Some(DisplayDate { date, ticks })
}

/// The `<month> d` and `<month> d y` forms, from a [`MONTH_DAY`] match.
fn month_day_date(c: &Captures<'_>, today: CivilDate) -> Option<DisplayDate> {
    let m = month(group(c, 1)?, group(c, 2)?)?;
    let y = match group(c, 4) {
        Some(y) => year(y)?,
        None => today.year,
    };
    date_only(CivilDate::new(y, m, int(group(c, 3)?)?))
}

/// The banner for a contact's first `Birthday` display date.
///
/// The parsed date and time minus today's midnight in whole days, truncated toward zero, shown only
/// from 1 day before to 15 days after today; an unparsable date shows none.
pub fn birthday_banner(display_date: &str, today: CivilDate) -> BirthdayBanner {
    let Some(parsed) = parse_display_date(display_date, today) else {
        return BirthdayBanner::None;
    };

    let ticks = (parsed.date.days() - today.days()) * TICKS_PER_DAY + parsed.ticks;
    match ticks / TICKS_PER_DAY {
        -1 => BirthdayBanner::Yesterday,
        0 => BirthdayBanner::Today,
        1 => BirthdayBanner::Tomorrow,
        2..=15 => BirthdayBanner::Soon,
        _ => BirthdayBanner::None,
    }
}

#[cfg(test)]
mod tests;
