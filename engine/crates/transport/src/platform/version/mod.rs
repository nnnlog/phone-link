//! Parsing rules of the platform layer: integers (for `_rejectionVersion`) and four-part versions
//! (for the phone `clientVersion` check of `/DeviceResourceManager` requests).

/// White space allowed around a number: U+0009–U+000D and U+0020.
fn is_number_white_space(c: char) -> bool {
    matches!(c, '\u{09}'..='\u{0D}' | ' ')
}

/// An integer: optional white space around an optionally signed (`+`/`-`) run of ASCII digits that
/// fits an `i32`. `None` otherwise.
pub fn parse_text_int(value: &str) -> Option<i32> {
    let trimmed = value.trim_matches(is_number_white_space);
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<i32>().ok()
}

/// A version: 2 to 4 non-negative components; an absent build or revision is `-1`, which orders
/// below `0`.
///
/// The derived ordering compares major, minor, build and revision in turn; equality
/// compares all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientVersion {
    pub major: i32,
    pub minor: i32,
    pub build: i32,
    pub revision: i32,
}

impl ClientVersion {
    pub const fn new(major: i32, minor: i32, build: i32, revision: i32) -> Self {
        Self { major, minor, build, revision }
    }

    /// Splits on `.`; 2–4 components; each one parsed by [`parse_text_int`] (white space and a sign
    /// allowed) and required to be `>= 0`.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }

        let mut components = [-1i32; 4];
        for (slot, part) in components.iter_mut().zip(&parts) {
            let value = parse_text_int(part)?;
            if value < 0 {
                return None;
            }
            *slot = value;
        }

        Some(Self::new(components[0], components[1], components[2], components[3]))
    }
}

impl std::fmt::Display for ClientVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.build >= 0 {
            write!(f, ".{}", self.build)?;
            if self.revision >= 0 {
                write!(f, ".{}", self.revision)?;
            }
        }
        Ok(())
    }
}

/// The two versions the check lets through besides the minimum: exactly `1.0.0.0` or `1.0.0`.
const ALSO_PASSING: [ClientVersion; 2] = [ClientVersion::new(1, 0, 0, 0), ClientVersion::new(1, 0, 0, -1)];

/// The minimum-version check of the phone's `clientVersion`.
///
/// The text before the first `-`, parsed as a version, must be `>= minimum`, or exactly `1.0.0.0`
/// or `1.0.0`; an absent, empty or unparsable version fails.
pub fn client_version_passes(client_version: Option<&str>, minimum: ClientVersion) -> bool {
    let Some(text) = client_version else {
        return false;
    };

    let head = text.split('-').next().unwrap_or_default();
    match ClientVersion::parse(head) {
        Some(version) => version >= minimum || ALSO_PASSING.contains(&version),
        None => false,
    }
}

#[cfg(test)]
mod tests;
