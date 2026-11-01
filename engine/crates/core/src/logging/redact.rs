//! Redaction tokens: `{{REDACTED:<kind>|<tag>}}`, where the tag is the first 4 bytes of
//! HMAC-SHA256 over the value under a key kept beside the log. Equal values get equal tags, so a
//! reader can follow one phone or one request across lines; without the key a tag does not confirm a
//! guessed value, which matters for short values such as phone numbers.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use hmac::{Hmac, KeyInit, Mac};
use rand::RngCore;
use regex::{Captures, Regex};
use sha2::Sha256;

use super::LogError;
use crate::sync::RwLockExt;

type HmacSha256 = Hmac<Sha256>;

const KEY_FILE: &str = "log.key";
const KEY_LEN: usize = 32;

/// The tagging key, read from `log.key` in the log directory or made there.
pub(crate) struct Redactor {
    path: PathBuf,
    key: RwLock<[u8; KEY_LEN]>,
}

impl Redactor {
    /// Reads the key in `dir`, or makes one when there is none or it is not a key (a new key only
    /// breaks the link between old and new tags).
    pub(crate) fn open(dir: &Path) -> Result<Self, LogError> {
        let path = dir.join(KEY_FILE);
        let key = match fs::read(&path) {
            Ok(bytes) => <[u8; KEY_LEN]>::try_from(bytes.as_slice()).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(LogError::Key { path, source: e }),
        };
        let key = match key {
            Some(key) => key,
            None => write_new_key(&path)?,
        };
        Ok(Self { path, key: RwLock::new(key) })
    }

    /// Replaces the key, so tags written from now on do not match earlier ones.
    pub(crate) fn rekey(&self) -> Result<(), LogError> {
        let key = write_new_key(&self.path)?;
        *self.key.write_or_recover() = key;
        Ok(())
    }

    /// `{{REDACTED:<kind>|<tag of value>}}`.
    pub(crate) fn token(&self, kind: &str, value: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&*self.key.read_or_recover()).expect("HMAC takes a key of any length");
        mac.update(value.as_bytes());
        let tag = mac.finalize().into_bytes();
        format!("{{{{REDACTED:{kind}|{}}}}}", hex::encode(&tag[..4]))
    }

    /// `text` with every URL, email address, home or temporary path, GUID, long hex or base64 run, and
    /// long digit run replaced by a token. A heuristic for texts the code does not compose itself
    /// (library errors, panics): it narrows what such a text can carry, it does not prove it.
    pub(crate) fn scrub(&self, text: &str) -> String {
        SHAPES
            .replace_all(text, |c: &Captures<'_>| {
                let (kind, m) =
                    [("url", "url"), ("email", "email"), ("path", "path"), ("id", "id"), ("number", "number")]
                        .into_iter()
                        .find_map(|(group, kind)| c.name(group).map(|m| (kind, m)))
                        .expect("every alternative of SHAPES is a named group");
                if kind == "id" && !is_identifier_like(m.as_str()) {
                    return m.as_str().to_owned();
                }
                self.token(kind, m.as_str())
            })
            .into_owned()
    }
}

fn write_new_key(path: &Path) -> Result<[u8; KEY_LEN], LogError> {
    let mut key = [0u8; KEY_LEN];
    rand::rng().fill_bytes(&mut key);
    super::store::write_private(path, &key).map_err(|e| LogError::Key { path: path.to_owned(), source: e })?;
    Ok(key)
}

/// Identifier shapes, leftmost first and, at one position, in the order listed.
static SHAPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?P<url>\b[A-Za-z][A-Za-z0-9+.\-]*://[^\s"'<>()\[\]]+)"#,
        r"|(?P<email>[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,})",
        r#"|(?P<path>(?:~|/Users|/private|/var/folders|/Volumes)(?:/[^\s"':,()\[\]]+)+)"#,
        r"|(?P<id>\{?\b[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\b\}?",
        r"|\b[0-9A-Fa-f]{16,}\b",
        r"|[A-Za-z0-9+/_\-]{24,}={0,2})",
        r"|(?P<number>\+?\b\d(?:[ \-]?\d){7,})",
    ))
    .expect("SHAPES is a valid pattern")
});

/// Whether a match of the `id` alternatives is an identifier rather than a long word or name
/// (`DeviceResourceManagerResponseMessage`): it has a digit.
fn is_identifier_like(s: &str) -> bool {
    s.bytes().any(|b| b.is_ascii_digit())
}
