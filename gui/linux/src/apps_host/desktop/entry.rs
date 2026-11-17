//! A desktop entry: the `[Desktop Entry]` group of a desktop file, as far as opening a file with it
//! goes.

use std::path::{Path, PathBuf};

use super::exec::exec_program;
use super::key_file::{key_file, list, unescape};
use super::xdg::Xdg;

/// The `[Desktop Entry]` group of a desktop file: what deciding whether it opens a type, showing its
/// name and starting it need.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopEntry {
    /// Its desktop file ID (`org.gnome.Loupe.desktop`).
    pub id: String,
    pub path: PathBuf,
    /// The name in the session's language when the entry has one.
    pub name: String,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub mime_types: Vec<String>,
    application: bool,
    hidden: bool,
    /// It runs in a terminal, which this app does not open.
    pub(super) terminal: bool,
    dbus_activatable: bool,
    try_exec: Option<String>,
}

impl DesktopEntry {
    /// Reads the entry from `text`, the desktop file at `path`; `None` for a file without a
    /// `[Desktop Entry]` group.
    pub fn parse(id: &str, path: &Path, text: &str, locale: &[String]) -> Option<Self> {
        let groups = key_file(text);
        let group = groups.get("Desktop Entry")?;
        let string = |key: &str| group.get(key).map(|value| unescape(value));
        let flag = |key: &str| group.get(key).is_some_and(|value| value == "true");
        let name = locale
            .iter()
            .find_map(|code| group.get(&format!("Name[{code}]")))
            .or_else(|| group.get("Name"))
            .map(|value| unescape(value))
            .unwrap_or_else(|| id.trim_end_matches(".desktop").to_owned());
        Some(Self {
            id: id.to_owned(),
            path: path.to_owned(),
            name,
            exec: string("Exec").filter(|exec| !exec.trim().is_empty()),
            icon: string("Icon").filter(|icon| !icon.is_empty()),
            working_dir: string("Path").filter(|dir| !dir.is_empty()).map(PathBuf::from),
            mime_types: group.get("MimeType").map(|value| list(value)).unwrap_or_default(),
            application: group.get("Type").is_some_and(|value| value == "Application"),
            hidden: flag("Hidden"),
            terminal: flag("Terminal"),
            dbus_activatable: flag("DBusActivatable"),
            try_exec: string("TryExec").filter(|program| !program.is_empty()),
        })
    }

    /// The entry names `mime` in its `MimeType=` key (MIME types compare ignoring case).
    pub(super) fn lists(&self, mime: &str) -> bool {
        self.mime_types.iter().any(|listed| listed.eq_ignore_ascii_case(mime))
    }

    /// The entry starts an installed application: an `Application` that is not hidden, whose
    /// `TryExec` program (when it names one) and `Exec` program are installed. An entry that only
    /// D-Bus activation starts counts when its `Exec` is absent.
    pub(super) fn installed(&self, xdg: &Xdg) -> bool {
        if !self.application || self.hidden {
            return false;
        }
        if self.try_exec.as_deref().is_some_and(|program| xdg.find_program(program).is_none()) {
            return false;
        }

        match &self.exec {
            Some(exec) => exec_program(exec).is_some_and(|program| xdg.find_program(&program).is_some()),
            None => self.dbus_activatable,
        }
    }
}
