//! Where a phone item sits in the Mac's tree.
//!
//! It covers the phone's absolute path mapped to a location relative to the tree's root
//! (`storage\DCIM\Camera\a.jpg`), its cleaned-up name, and the remote path the PC sends back for an
//! upload or a rename (`\storage\Download\report.pdf`).
//!
//! Locations use `\` as their separator, as Phone Link on Windows keeps them; the Mac's file manager
//! reads `\` as a folder boundary.

/// A location in the tree: `\`-separated components, no leading or trailing `\`; `""` is the root.
pub type Location = String;

/// The characters taken out of a name.
fn forbidden(c: char) -> bool {
    (c as u32) < 0x20 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

fn reserved(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if RESERVED.contains(&upper.as_str()) {
        return true;
    }

    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

/// A name as the tree shows it: forbidden characters removed, a reserved device name prefixed with
/// `_`, white space trimmed.
pub fn clean_name(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !forbidden(*c)).collect();
    let kept = if reserved(&kept) { format!("_{kept}") } else { kept };
    kept.trim().to_owned()
}

/// Why an item is not placed in the tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejected {
    #[error("a path component of dots only")]
    DotComponent,
    #[error("the storage root itself")]
    StorageRoot,
}

/// The phone's absolute `path` mapped to a location.
///
/// `/` read as `\`, a component of dots only rejected, `\` trimmed from both ends, and the
/// `storage\emulated\0` prefix (compared without regard to case, and with no check that a component
/// ends there) replaced with `storage`.
///
/// The storage root
/// itself (`storage\emulated\0`, `storage\emulated`) is rejected.
pub fn map_path(path: &str) -> Result<Location, Rejected> {
    let slashed = path.replace('/', "\\");
    if slashed.split('\\').any(|c| !c.is_empty() && c.chars().all(|ch| ch == '.')) {
        return Err(Rejected::DotComponent);
    }

    let trimmed = slashed.trim_matches('\\');
    const PREFIX: &str = "storage\\emulated\\0";
    if trimmed.eq_ignore_ascii_case(PREFIX) || trimmed.eq_ignore_ascii_case("storage\\emulated") {
        return Err(Rejected::StorageRoot);
    }

    if trimmed.is_char_boundary(PREFIX.len()) && trimmed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return Ok(format!("storage{}", &trimmed[PREFIX.len()..]));
    }
    Ok(trimmed.to_owned())
}

/// The folder part of `location` (`""` for an item at the root).
pub fn parent(location: &str) -> &str {
    location.rfind('\\').map(|i| &location[..i]).unwrap_or("")
}

/// The last component of `location`.
pub fn leaf(location: &str) -> &str {
    location.rfind('\\').map(|i| &location[i + 1..]).unwrap_or(location)
}

/// `name` inside `folder`.
pub fn join(folder: &str, name: &str) -> Location {
    if folder.is_empty() { name.to_owned() } else { format!("{folder}\\{name}") }
}

/// The location as a lookup key: locations compare without regard to case.
pub fn key(location: &str) -> String {
    location.to_lowercase()
}

/// `location` is `ancestor` or inside it.
pub fn is_within(location: &str, ancestor: &str) -> bool {
    if ancestor.is_empty() {
        return true;
    }
    let (l, a) = (key(location), key(ancestor));
    l == a || l.starts_with(&format!("{a}\\"))
}

/// The remote path of `location`: `\` + the location.
pub fn remote(location: &str) -> String {
    format!("\\{location}")
}

/// The folder of the phone's recycle bin, from its configured name: under `storage`.
pub fn recycle_bin(configured: &str) -> Option<Location> {
    let trimmed = configured.replace('/', "\\");
    let trimmed = trimmed.trim_matches('\\');
    if trimmed.is_empty() { None } else { Some(join("storage", trimmed)) }
}

/// A new local item with this name stays on the Mac and is not sent to the phone: dot files and the
/// Windows shortcut, temporary, download-in-progress and settings files.
pub fn ignored(name: &str) -> bool {
    const SUFFIXES: [&str; 7] = [".lnk", ".tmp", ".~tmp", ".temp", ".crcdownload", ".ini", ".search-ms"];
    let lower = name.to_lowercase();
    name.starts_with('.') || SUFFIXES.iter().any(|s| lower.ends_with(s))
}

#[cfg(test)]
mod tests;
