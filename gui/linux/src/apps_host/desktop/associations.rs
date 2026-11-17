//! The association between MIME types and applications: the installed desktop entries, the
//! `mimeapps.list` files, and the applications and the default that open a type.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::entry::DesktopEntry;
use super::key_file::{key_file, list, locale_keys};
use super::xdg::Xdg;

/// One desktop entry file found under an `applications` directory.
#[derive(Debug, Clone)]
struct Found {
    id: String,
    path: PathBuf,
    /// The `applications` directory it is in (its subdirectories included).
    dir: PathBuf,
}

/// Every desktop file under `dir`, with its desktop file ID: the path below `dir`, `/` turned to `-`.
fn desktop_files(dir: &Path) -> Vec<Found> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<Found>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
        paths.sort();

        for path in paths {
            if path.is_dir() {
                walk(root, &path, found);
            } else if path.extension().is_some_and(|ext| ext == "desktop")
                && let Ok(relative) = path.strip_prefix(root)
            {
                let id = relative.to_string_lossy().replace('/', "-");
                found.push(Found { id, path: path.clone(), dir: root.to_owned() });
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found
}

/// What one `mimeapps.list` says about one MIME type.
#[derive(Debug, Default)]
struct ListEntries {
    default: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
}

fn read_list(path: &Path, mime: &str) -> ListEntries {
    let Ok(text) = std::fs::read_to_string(path) else { return ListEntries::default() };
    let groups = key_file(&text);
    let ids = |group: &str| {
        groups
            .get(group)
            .and_then(|entries| entries.iter().find(|(key, _)| key.eq_ignore_ascii_case(mime)))
            .map(|(_, value)| list(value))
            .unwrap_or_default()
    };
    ListEntries {
        default: ids("Default Applications"),
        added: ids("Added Associations"),
        removed: ids("Removed Associations"),
    }
}

/// The applications installed in this session, as the association lookups see them.
pub struct Applications<'a> {
    xdg: &'a Xdg,
    locale: Vec<String>,
    /// Every desktop file, the data directories in order of precedence.
    files: Vec<Found>,
}

impl<'a> Applications<'a> {
    pub fn scan(xdg: &'a Xdg, locale: &str) -> Self {
        let files = xdg.data.iter().flat_map(|dir| desktop_files(&dir.join("applications"))).collect();
        Self { xdg, locale: locale_keys(locale), files }
    }

    /// The entry a desktop file ID names: the first data directory's file of that ID.
    pub fn entry(&self, id: &str) -> Option<DesktopEntry> {
        let found = self.files.iter().find(|found| found.id == id)?;
        let text = std::fs::read_to_string(&found.path).ok()?;
        DesktopEntry::parse(&found.id, &found.path, &text, &self.locale)
    }

    fn installed_entry(&self, id: &str) -> Option<DesktopEntry> {
        self.entry(id).filter(|entry| entry.installed(self.xdg))
    }

    /// The applications associated with `mime`, in the specification's order: each directory's
    /// added associations, then its desktop files listing the type, minus what a directory of higher
    /// precedence removed or shadows; only installed ones.
    pub fn associated(&self, mime: &str) -> Vec<DesktopEntry> {
        let mut ids: Vec<String> = Vec::new();
        let mut blocked: HashSet<String> = HashSet::new();
        for (dir, holds_entries) in self.xdg.list_dirs() {
            for file in self.xdg.list_files(&dir) {
                let entries = read_list(&file, mime);
                for id in entries.added {
                    if !blocked.contains(&id) && !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                blocked.extend(entries.removed);
            }
            if !holds_entries {
                continue;
            }
            let here: Vec<&Found> = self.files.iter().filter(|found| found.dir == dir).collect();
            for found in &here {
                if blocked.contains(&found.id) || ids.contains(&found.id) {
                    continue;
                }
                let listed = std::fs::read_to_string(&found.path)
                    .ok()
                    .and_then(|text| DesktopEntry::parse(&found.id, &found.path, &text, &self.locale))
                    .is_some_and(|entry| entry.lists(mime));
                if listed {
                    ids.push(found.id.clone());
                }
            }
            blocked.extend(here.iter().map(|found| found.id.clone()));
        }

        ids.iter().filter_map(|id| self.installed_entry(id)).collect()
    }

    /// The application that opens `mime`: the first installed one a `[Default Applications]` group
    /// names, the lists in order of precedence; else the first associated application.
    pub fn default_for(&self, mime: &str) -> Option<DesktopEntry> {
        for (dir, _) in self.xdg.list_dirs() {
            for file in self.xdg.list_files(&dir) {
                if let Some(entry) = read_list(&file, mime).default.iter().find_map(|id| self.installed_entry(id)) {
                    return Some(entry);
                }
            }
        }

        self.associated(mime).into_iter().next()
    }
}
