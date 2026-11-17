//! The picture setting of the desktops that keep it in gsettings: GNOME (with its dark picture),
//! Budgie, Cinnamon and MATE.

use std::process::Command;

use phonelink_engine::edge::EdgeError;

use super::Setting;
use crate::edges::unexpected;

#[cfg(test)]
mod tests;

/// A string `gsettings get` prints in `GVariant`'s text form: single-quoted, or double-quoted when it
/// holds a single quote, with backslash escapes. `None` for any other text.
fn gvariant_string(text: &str) -> Option<String> {
    let mut chars = text.trim().chars();
    let quote = chars.next().filter(|c| matches!(c, '\'' | '"'))?;
    let mut value = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => value.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'a' => '\u{7}',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'v' => '\u{b}',
                'u' => char::from_u32(u32::from_str_radix(&chars.by_ref().take(4).collect::<String>(), 16).ok()?)?,
                'U' => char::from_u32(u32::from_str_radix(&chars.by_ref().take(8).collect::<String>(), 16).ok()?)?,
                escaped @ ('\\' | '\'' | '"') => escaped,
                _ => return None,
            }),
            c if c == quote => return chars.next().is_none().then_some(value),
            c => value.push(c),
        }
    }

    None
}

/// The string value of a gsettings key.
fn gsettings(schema: &str, key: &str) -> Result<String, EdgeError> {
    let output = Command::new("gsettings")
        .args(["get", schema, key])
        .output()
        .map_err(|e| unexpected(format!("gsettings did not run: {e}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(unexpected(format!("gsettings get {schema} {key}: {}", error.trim())));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    gvariant_string(&text)
        .ok_or_else(|| unexpected(format!("gsettings get {schema} {key} printed {:?}, not a string", text.trim())))
}

/// GNOME's (and Budgie's and Cinnamon's) picture: none while `picture-options` is `none`; with
/// `dark_variant`, `picture-uri-dark` while the dark style is preferred, as GNOME Shell shows it.
pub(super) fn gnome_setting(schema: &str, dark_variant: bool) -> Result<Setting, EdgeError> {
    if gsettings(schema, "picture-options")? == "none" {
        return Ok(Setting::NoPicture);
    }
    let dark = dark_variant && gsettings("org.gnome.desktop.interface", "color-scheme")? == "prefer-dark";
    let uri = gsettings(schema, if dark { "picture-uri-dark" } else { "picture-uri" })?;
    Ok(Setting::from_uri(&uri))
}

/// MATE's picture: a path, empty for none.
pub(super) fn mate_setting() -> Result<Setting, EdgeError> {
    if gsettings("org.mate.background", "picture-options")? == "none" {
        return Ok(Setting::NoPicture);
    }
    Ok(Setting::from_path(&gsettings("org.mate.background", "picture-filename")?))
}
