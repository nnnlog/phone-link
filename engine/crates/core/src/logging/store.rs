//! The log files: `PhoneLink.log` and the generations rotated out of it, `PhoneLink.1.log` (newer)
//! and `PhoneLink.2.log` (older). A write that would take the current file past [`ROTATE_AT`]
//! rotates first, so the log stays under about three times that.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use super::LogError;
use crate::sync::MutexExt;

/// The size at which the current file is rotated.
pub(crate) const ROTATE_AT: u64 = 4 * 1024 * 1024;

const CURRENT: &str = "PhoneLink.log";
const ROTATED: [&str; 2] = ["PhoneLink.1.log", "PhoneLink.2.log"];

/// The log's files in one directory. Cheap to clone; every clone writes to the same files.
#[derive(Clone)]
pub(crate) struct LogStore {
    inner: Arc<Mutex<Files>>,
}

struct Files {
    dir: PathBuf,
    current: File,
    written: u64,
}

impl LogStore {
    /// Opens the current file in `dir` (which must exist) for appending.
    pub(crate) fn open(dir: &Path) -> Result<Self, LogError> {
        let (current, written) =
            open_current(dir).map_err(|e| LogError::Open { path: dir.join(CURRENT), source: e })?;
        Ok(Self { inner: Arc::new(Mutex::new(Files { dir: dir.to_owned(), current, written })) })
    }

    /// The bytes held in every generation.
    pub(crate) fn size(&self) -> u64 {
        let files = self.inner.lock_or_recover();
        files.written
            + ROTATED.iter().filter_map(|name| fs::metadata(files.dir.join(name)).ok()).map(|m| m.len()).sum::<u64>()
    }

    /// Writes every generation, oldest first, to `destination` (replacing it).
    pub(crate) fn export(&self, destination: &Path) -> Result<(), LogError> {
        let export = |e| LogError::Export { path: destination.to_owned(), source: e };
        let mut files = self.inner.lock_or_recover();
        files.current.flush().map_err(export)?;
        let mut out = File::create(destination).map_err(export)?;

        for name in ROTATED.iter().rev().chain([&CURRENT]) {
            match File::open(files.dir.join(name)) {
                Ok(mut f) => io::copy(&mut f, &mut out).map(drop).map_err(export)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(export(e)),
            }
        }

        out.sync_all().map_err(export)
    }

    /// Empties the log: removes the rotated files and truncates the current one.
    pub(crate) fn clear(&self) -> Result<(), LogError> {
        let mut files = self.inner.lock_or_recover();
        for name in ROTATED {
            remove_if_present(&files.dir.join(name))?;
        }
        files.current.set_len(0).map_err(|e| LogError::Clear { path: files.dir.join(CURRENT), source: e })?;
        files.written = 0;
        Ok(())
    }
}

impl Files {
    fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if self.written > 0 && self.written + line.len() as u64 > ROTATE_AT {
            self.rotate()?;
        }
        self.current.write_all(line)?;
        self.written += line.len() as u64;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let path = |name: &str| self.dir.join(name);
        // Rename the oldest first, so no generation is overwritten before it moves.
        for i in (0..ROTATED.len()).rev() {
            let from = if i == 0 { path(CURRENT) } else { path(ROTATED[i - 1]) };
            match fs::rename(&from, path(ROTATED[i])) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }

        let (current, written) = open_current(&self.dir)?;
        self.current = current;
        self.written = written;
        Ok(())
    }
}

fn open_current(dir: &Path) -> io::Result<(File, u64)> {
    let file = OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join(CURRENT))?;
    let written = file.metadata()?.len();
    Ok((file, written))
}

fn remove_if_present(path: &Path) -> Result<(), LogError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LogError::Clear { path: path.to_owned(), source: e }),
    }
}

/// Writes `bytes` to a new file at `path` readable only by its owner, replacing what is there.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

/// One formatted event: `fmt` formats a whole line into a buffer and writes it in one call.
pub(crate) struct LineWriter<'a> {
    store: &'a LogStore,
}

impl Write for LineWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.store.inner.lock_or_recover().write_line(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.store.inner.lock_or_recover().current.flush()
    }
}

impl<'a> MakeWriter<'a> for LogStore {
    type Writer = LineWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter { store: self }
    }
}
