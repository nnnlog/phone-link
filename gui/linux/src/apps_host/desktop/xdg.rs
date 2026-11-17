//! The XDG base directories, desktop names and program folders of the session, which every lookup
//! reads.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The XDG directories and desktop names the lookups read, each list in order of precedence.
#[derive(Debug, Clone, Default)]
pub struct Xdg {
    /// `$XDG_CONFIG_HOME`, then each of `$XDG_CONFIG_DIRS`.
    pub config: Vec<PathBuf>,
    /// `$XDG_DATA_HOME`, then each of `$XDG_DATA_DIRS`.
    pub data: Vec<PathBuf>,
    /// The names in `$XDG_CURRENT_DESKTOP`, lower-cased: each has its own `<name>-mimeapps.list`.
    pub desktops: Vec<String>,
    /// The directories of `$PATH`, where a desktop entry's program is looked for.
    pub path: Vec<PathBuf>,
}

/// The absolute paths in a colon-separated list; the base directory specification has relative ones
/// ignored.
fn absolute_paths(list: &str) -> Vec<PathBuf> {
    list.split(':').filter(|p| p.starts_with('/')).map(PathBuf::from).collect()
}

impl Xdg {
    /// The directories of this session, with the base directory specification's defaults for unset
    /// variables.
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let home_dir = |name: &str, relative: &str| {
            var(name)
                .filter(|dir| dir.starts_with('/'))
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(relative)))
        };
        let mut config: Vec<PathBuf> = home_dir("XDG_CONFIG_HOME", ".config").into_iter().collect();
        config.extend(absolute_paths(&var("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".to_owned())));
        let mut data: Vec<PathBuf> = home_dir("XDG_DATA_HOME", ".local/share").into_iter().collect();
        data.extend(absolute_paths(&var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned())));
        let desktops = var("XDG_CURRENT_DESKTOP")
            .map(|names| names.split(':').filter(|n| !n.is_empty()).map(str::to_lowercase).collect())
            .unwrap_or_default();
        let path = absolute_paths(&var("PATH").unwrap_or_default());
        Self { config, data, desktops, path }
    }

    /// Each directory that may hold a `mimeapps.list`, in order of precedence, and whether desktop
    /// entries live in it (the data directories' `applications`).
    pub(super) fn list_dirs(&self) -> Vec<(PathBuf, bool)> {
        let config = self.config.iter().map(|dir| (dir.clone(), false));
        let data = self.data.iter().map(|dir| (dir.join("applications"), true));
        config.chain(data).collect()
    }

    /// The `mimeapps.list` files of `dir`, the desktop-specific ones first.
    pub(super) fn list_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> =
            self.desktops.iter().map(|desktop| dir.join(format!("{desktop}-mimeapps.list"))).collect();
        files.push(dir.join("mimeapps.list"));
        files
    }

    /// `program` as a command line names it: a path as it is, a bare name in `$PATH`; `None` when it
    /// is no executable file.
    pub(super) fn find_program(&self, program: &str) -> Option<PathBuf> {
        let executable = |path: &Path| {
            std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        };
        if program.contains('/') {
            let path = PathBuf::from(program);
            return executable(&path).then_some(path);
        }
        self.path.iter().map(|dir| dir.join(program)).find(|path| executable(path))
    }
}
