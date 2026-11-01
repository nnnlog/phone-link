//! File-backed stand-ins for the OS-edge traits: one `0600` file per `SecureStore` key under a state
//! directory (default `.local/port/` at the repository root, which `.gitignore` excludes).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use phonelink_engine::edge::{EdgeError, SecureStore};

pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, EdgeError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| store_err(&root, e))?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|e| store_err(&root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Maps a `/`-separated key to a path under the root; keys that would escape it are refused.
    fn path(&self, key: &str) -> Result<PathBuf, EdgeError> {
        let rel = Path::new(key);
        if key.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(EdgeError::Store { message: format!("invalid store key {key:?}") });
        }
        Ok(self.root.join(rel))
    }

    /// Every key currently stored, sorted.
    pub fn keys(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else if let Ok(rel) = p.strip_prefix(base) {
                    if !rel.to_string_lossy().ends_with(".tmp") {
                        out.push(rel.to_string_lossy().into_owned());
                    }
                }
            }
        }

        let mut out = Vec::new();
        walk(&self.root, &self.root, &mut out);
        out.sort();
        out
    }
}

fn store_err(path: &Path, e: std::io::Error) -> EdgeError {
    EdgeError::Store { message: format!("{}: {e}", path.display()) }
}

impl SecureStore for FileStore {
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, EdgeError> {
        let path = self.path(&key)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(store_err(&path, e)),
        }
    }

    fn store(&self, key: String, value: Vec<u8>) -> Result<(), EdgeError> {
        let path = self.path(&key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| store_err(parent, e))?;
        }

        let tmp = path.with_extension("tmp");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| store_err(&tmp, e))?;

        f.write_all(&value).map_err(|e| store_err(&tmp, e))?;
        f.sync_all().map_err(|e| store_err(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| store_err(&path, e))
    }

    fn delete(&self, key: String) -> Result<(), EdgeError> {
        let path = self.path(&key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(store_err(&path, e)),
        }
    }

    fn keys_with_prefix(&self, prefix: String) -> Result<Vec<String>, EdgeError> {
        Ok(self.keys().into_iter().filter(|k| k.starts_with(&prefix)).collect())
    }
}
