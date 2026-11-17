//! File sizes as the transfers show them.
//!
//! A size reads "999 bytes", "12 KB", "3.4 MB" or "1.25 GB", and how much of a transfer is done reads
//! "12 of 20 MB".

use crate::l10n::{fill, translate};

/// The unit a file size shows in.
///
/// Decimal units (a kilobyte is 1000 bytes), whole kilobytes, one digit after the point for megabytes
/// and two above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SizeUnit {
    Bytes,
    Kilo,
    Mega,
    Giga,
    Tera,
}

impl SizeUnit {
    const ALL: [Self; 5] = [Self::Bytes, Self::Kilo, Self::Mega, Self::Giga, Self::Tera];

    fn scale(self) -> f64 {
        match self {
            Self::Bytes => 1.0,
            Self::Kilo => 1e3,
            Self::Mega => 1e6,
            Self::Giga => 1e9,
            Self::Tera => 1e12,
        }
    }

    fn decimals(self) -> usize {
        match self {
            Self::Bytes | Self::Kilo => 0,
            Self::Mega => 1,
            Self::Giga | Self::Tera => 2,
        }
    }

    /// The smallest unit in which `bytes`, rounded to the unit's digits, shows as less than 1000.
    pub(super) fn of(bytes: u64) -> Self {
        Self::ALL
            .into_iter()
            .find(|unit| {
                let shown: f64 = format!("{:.*}", unit.decimals(), bytes as f64 / unit.scale()).parse().unwrap_or(0.0);
                shown < 1000.0
            })
            .unwrap_or(Self::Tera)
    }

    /// The localized size text, with `number` in its place.
    fn label(self, number: &str, bytes: u64) -> String {
        let template = match self {
            Self::Bytes if bytes == 1 => "%@ byte",
            Self::Bytes => "%@ bytes",
            Self::Kilo => "%@ KB",
            Self::Mega => "%@ MB",
            Self::Giga => "%@ GB",
            Self::Tera => "%@ TB",
        };
        fill(translate(template), &[number.to_owned()])
    }
}

/// `bytes` in `unit`, without trailing zeros after the point.
pub(super) fn size_number(bytes: u64, unit: SizeUnit) -> String {
    let text = format!("{:.*}", unit.decimals(), bytes as f64 / unit.scale());
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
}

/// A file size: "999 bytes", "12 KB", "3.4 MB", "1.25 GB".
pub(super) fn file_size(bytes: u64) -> String {
    let unit = SizeUnit::of(bytes);
    unit.label(&size_number(bytes, unit), bytes)
}

/// "12 of 20 MB": how much of a transfer is done, both in the total's unit.
pub(super) fn amount(done: u64, total: u64) -> String {
    let unit = SizeUnit::of(total);
    fill(translate("%@ of %@"), &[size_number(done, unit), unit.label(&size_number(total, unit), total)])
}
