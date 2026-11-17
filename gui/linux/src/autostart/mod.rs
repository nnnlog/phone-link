//! Opening Phone Link at login, through XDG autostart: an entry named `phonelink-linux.desktop` in
//! `~/.config/autostart` overrides the one the packages install in `/etc/xdg/autostart`, so turning it
//! off writes a user entry with `Hidden=true` and turning it on writes one without.

use std::io;
use std::path::{Path, PathBuf};

const ENTRY: &str = "phonelink-linux.desktop";

/// The user's entry, in `$XDG_CONFIG_HOME/autostart`, else `~/.config/autostart`. A relative
/// `XDG_CONFIG_HOME` is ignored, as the XDG base directory rules say; with no absolute folder there is
/// none.
fn user_entry() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")).filter(|dir| dir.is_absolute())
        })
        .map(|dir| dir.join("autostart").join(ENTRY))
}

fn system_entries() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_CONFIG_DIRS").ok().filter(|dirs| !dirs.is_empty());
    dirs.as_deref()
        .unwrap_or("/etc/xdg")
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join("autostart").join(ENTRY))
        .collect()
}

/// Whether a desktop entry's text starts the app: not hidden, and not turned off for GNOME.
fn starts(entry: &str) -> bool {
    !entry.lines().map(str::trim).any(|line| line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false")
}

/// The entry that decides: the user's when there is one, else the first system one.
fn effective_entry(user: Option<&Path>, system: &[PathBuf]) -> Option<String> {
    user.into_iter().chain(system.iter().map(PathBuf::as_path)).find_map(|path| std::fs::read_to_string(path).ok())
}

/// Phone Link opens at login.
pub fn enabled() -> bool {
    effective_entry(user_entry().as_deref(), &system_entries()).is_some_and(|entry| starts(&entry))
}

/// `argument` as one argument of a desktop entry's `Exec` key, by the Desktop Entry Specification's
/// rules: in double quotes, with `"`, `` ` ``, `$` and `\` escaped by a backslash and `%` doubled; then
/// the key's value is a string, whose own escapes apply on top (a backslash doubled again, and the
/// control characters written as `\n`, `\t`, `\r`).
fn exec_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    for c in argument.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }

    quoted.push('"');
    quoted.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t").replace('\r', "\\r")
}

/// The user's entry that opens Phone Link at login in the background (`on`), or hides the system's.
fn entry_text(executable: &str, on: bool) -> String {
    let mut text = format!(
        "[Desktop Entry]\nType=Application\nName=Phone Link\nComment=Use your Android phone from this computer\n\
         Exec={} --background\nIcon=phonelink\nTerminal=false\nNoDisplay=true\n",
        exec_argument(executable)
    );
    if !on {
        text.push_str("Hidden=true\n");
    }
    text
}

/// Opens Phone Link at login (`on`) or stops doing so, for this user.
pub fn set(on: bool) -> io::Result<()> {
    let path = user_entry().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "neither XDG_CONFIG_HOME nor HOME names an absolute folder")
    })?;
    let executable = std::env::current_exe()?;
    let executable = executable
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the executable's path is not UTF-8"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry_text(executable, on))
}

#[cfg(test)]
mod tests;
