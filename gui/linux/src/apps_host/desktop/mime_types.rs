//! The MIME type of a URI and of a file name (shared-mime-info's `globs2`), and which types are media.

use std::collections::HashSet;

use super::xdg::Xdg;

/// The MIME type of a URI as the association lookups name it: `x-scheme-handler/<scheme>`; `None` for
/// a string without a scheme and for `file:` URIs, which name files on the phone.
pub fn uri_mime_type(uri: &str) -> Option<String> {
    let (scheme, _) = uri.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let scheme = scheme.to_ascii_lowercase();
    (valid && scheme != "file").then(|| format!("x-scheme-handler/{scheme}"))
}

/// One pattern of shared-mime-info's `globs2`.
#[derive(Debug, Clone)]
struct Glob {
    weight: u32,
    mime: String,
    pattern: String,
    case_sensitive: bool,
}

/// The patterns of the `globs2` files, the data directories in order of precedence; a type that a
/// directory marks `__NOGLOBS__` keeps no pattern of the directories after it.
fn globs(xdg: &Xdg) -> Vec<Glob> {
    let mut globs = Vec::new();
    let mut cleared: HashSet<String> = HashSet::new();
    for dir in &xdg.data {
        let Ok(text) = std::fs::read_to_string(dir.join("mime").join("globs2")) else { continue };
        let mut here = Vec::new();
        let mut clearing = Vec::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let mut fields = line.split(':');
            let (Some(weight), Some(mime), Some(pattern)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            let Ok(weight) = weight.parse::<u32>() else { continue };
            if pattern == "__NOGLOBS__" {
                clearing.push(mime.to_owned());
                continue;
            }
            if cleared.contains(mime) {
                continue;
            }
            let case_sensitive = fields.next().is_some_and(|flags| flags.split(',').any(|flag| flag == "cs"));
            here.push(Glob { weight, mime: mime.to_owned(), pattern: pattern.to_owned(), case_sensitive });
        }
        globs.extend(here);
        cleared.extend(clearing);
    }

    globs
}

/// The MIME type of a file named `name` by its name alone: a literal pattern equal to it, else the
/// heaviest `*.suffix` pattern it ends with (the longest of equal weight); patterns with other wildcards
/// are not read. `None` when no pattern matches.
pub fn mime_type_of_name(xdg: &Xdg, name: &str) -> Option<String> {
    let globs = globs(xdg);
    let matches = |pattern: &str, case_sensitive: bool| {
        if case_sensitive { name == pattern } else { name.to_lowercase() == pattern.to_lowercase() }
    };
    let wildcard = |pattern: &str| pattern.contains(['*', '?', '['].as_slice());
    if let Some(glob) =
        globs.iter().find(|glob| !wildcard(&glob.pattern) && matches(&glob.pattern, glob.case_sensitive))
    {
        return Some(glob.mime.clone());
    }
    let lower = name.to_lowercase();
    globs
        .iter()
        .filter_map(|glob| {
            let suffix = glob.pattern.strip_prefix('*')?;
            if wildcard(suffix) || suffix.is_empty() {
                return None;
            }
            let ends =
                if glob.case_sensitive { name.ends_with(suffix) } else { lower.ends_with(&suffix.to_lowercase()) };
            ends.then_some(glob)
        })
        .fold(None::<&Glob>, |best, glob| match best {
            Some(best) if (best.weight, best.pattern.len()) >= (glob.weight, glob.pattern.len()) => Some(best),
            _ => Some(glob),
        })
        .map(|glob| glob.mime.clone())
}

/// A photo, video or audio type: `image/*` but SVG, which can carry scripts and opens in browsers,
/// `video/*` and `audio/*`. Only files of these types are handed to another application, since their
/// names come from the phone.
pub fn is_media(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    (mime.starts_with("image/") && !mime.starts_with("image/svg+xml"))
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
}
