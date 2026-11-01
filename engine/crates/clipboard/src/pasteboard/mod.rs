//! The pasteboard OS edge and the app's pasteboard rules.
//!
//! The rules differ from Phone Link on Windows (change detection, reading, writing, concealed and
//! transient pasteboards); the module also holds the fixed kind priority of announcing and the
//! plain-text read limit.
//!
//! The edge is thin — change count, types, data, one write — so every rule (which type is which kind,
//! the priority, the markers, the limit, the UTF-8 decoding) is engine code.

#[cfg(feature = "test-support")]
use std::sync::Arc;

use phonelink_core::edge::EdgeError;

/// One pasteboard representation: a type (UTI) and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardPasteboardData {
    pub uti: String,
    pub data: Vec<u8>,
}

/// What a pasteboard write did, once it changed the pasteboard.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardPasteboardWrite {
    /// The change count the pasteboard has once the write ended — the value `clearContents()`
    /// returned, since `setData` does not change it — also when a representation was refused after
    /// it. Echo suppression records it either way.
    pub change_count: i64,
    /// Why the write stopped before its last representation (the pasteboard refused one); the
    /// representations before it are on the pasteboard. `None`: every representation was written.
    pub failure: Option<String>,
}

/// The general pasteboard (macOS: `NSPasteboard.general`). Called off the async runtime.
#[uniffi::export(with_foreign)]
pub trait ClipboardPasteboard: Send + Sync {
    /// `NSPasteboard.general.changeCount`; reading it reads no content.
    fn change_count(&self) -> Result<i64, EdgeError>;
    /// The types on the pasteboard (`NSPasteboard.general.types`; empty when `nil`).
    fn types(&self) -> Result<Vec<String>, EdgeError>;
    /// `NSPasteboard.general.data(forType: uti)`; `None` when the type is absent or its data cannot
    /// be read.
    fn data(&self, uti: String) -> Result<Option<Vec<u8>>, EdgeError>;
    /// `clearContents()`, then `setData(_:forType:)` for each representation in order, as one item,
    /// stopping at the first one the pasteboard refuses. Once `clearContents()` ran the write is
    /// reported in [`ClipboardPasteboardWrite`], a refused representation included; `Err` means the
    /// write changed nothing.
    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError>;
}

/// The pasteboard types the rules name.
pub mod uti {
    /// Raster image.
    pub const PNG: &str = "public.png";
    /// Raster image.
    pub const TIFF: &str = "public.tiff";
    pub const HTML: &str = "public.html";
    /// `NSPasteboard.PasteboardType.string`.
    pub const PLAIN_TEXT: &str = "public.utf8-plain-text";
    /// Copied files: the pasteboard then has no recognized kind.
    pub const FILE_URL: &str = "public.file-url";
    /// nspasteboard.org marker of concealed content (password managers): no recognized kind.
    pub const CONCEALED: &str = "org.nspasteboard.ConcealedType";
    /// nspasteboard.org marker of transient content: no recognized kind.
    pub const TRANSIENT: &str = "org.nspasteboard.TransientType";
}

/// The kinds the clipboard recognizes, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipKind {
    Image,
    Html,
    Text,
}

/// The kind of a pasteboard with `types`.
///
/// **image** (`public.png` or `public.tiff`) > **HTML** (`public.html`) > **text**
/// (`public.utf8-plain-text`), whatever order the copying application declared its types in; `None`
/// when none is present, or when `public.file-url`, the concealed or the transient marker is
/// present.
///
/// A vector rendering (`com.adobe.pdf`), RTF and files are never
/// recognized.
pub fn classify(types: &[String]) -> Option<ClipKind> {
    let has = |uti: &str| types.iter().any(|t| t == uti);
    if has(uti::FILE_URL) || has(uti::CONCEALED) || has(uti::TRANSIENT) {
        return None;
    }

    if has(uti::PNG) || has(uti::TIFF) {
        Some(ClipKind::Image)
    } else if has(uti::HTML) {
        Some(ClipKind::Html)
    } else if has(uti::PLAIN_TEXT) {
        Some(ClipKind::Text)
    } else {
        None
    }
}

/// Why a representation could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("{0} is absent or unreadable")]
    Absent(&'static str),
    #[error("{uti} is {units} UTF-16 code units, more than the read limit")]
    TooLong { uti: &'static str, units: usize },
    #[error("pasteboard: {0}")]
    Edge(#[from] EdgeError),
}

/// The length the read limit counts: UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Reads `uti` as UTF-8 text no longer than `limit` UTF-16 code units. Invalid UTF-8 sequences
/// become U+FFFD.
fn read_text(pasteboard: &dyn ClipboardPasteboard, uti: &'static str, limit: usize) -> Result<String, ReadError> {
    let bytes = pasteboard.data(uti.to_owned())?.ok_or(ReadError::Absent(uti))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let units = utf16_len(&text);
    if units > limit {
        return Err(ReadError::TooLong { uti, units });
    }
    Ok(text)
}

/// The plain-text representation (`public.utf8-plain-text`), at most `limit` UTF-16 code units.
pub fn read_plain_text(pasteboard: &dyn ClipboardPasteboard, limit: usize) -> Result<String, ReadError> {
    read_text(pasteboard, uti::PLAIN_TEXT, limit)
}

/// The HTML representation (`public.html`) read as UTF-8, at most `limit` UTF-16 code units.
pub fn read_html(pasteboard: &dyn ClipboardPasteboard, limit: usize) -> Result<String, ReadError> {
    read_text(pasteboard, uti::HTML, limit)
}

/// The raster image: the `public.png` data, else the `public.tiff` data.
pub fn read_image(pasteboard: &dyn ClipboardPasteboard) -> Result<Vec<u8>, ReadError> {
    for uti in [uti::PNG, uti::TIFF] {
        if let Some(bytes) = pasteboard.data(uti.to_owned())? {
            return Ok(bytes);
        }
    }
    Err(ReadError::Absent(uti::PNG))
}

/// A representation to write.
pub fn representation(uti: &str, data: Vec<u8>) -> ClipboardPasteboardData {
    ClipboardPasteboardData { uti: uti.to_owned(), data }
}

#[cfg(feature = "test-support")]
/// Shorthand for the edge as the feature holds it.
pub type SharedPasteboard = Arc<dyn ClipboardPasteboard>;

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite};
    #[cfg(feature = "test-support")]
    use super::{representation, uti};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// A pasteboard held in memory: [`MemoryPasteboard::copy`] is another application copying,
    /// [`ClipboardPasteboard::write`] the app writing; both bump the change count as
    /// `clearContents()` does.
    #[derive(Default)]
    pub struct MemoryPasteboard {
        inner: Mutex<Inner>,
    }

    impl MemoryPasteboard {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        #[cfg(feature = "test-support")]
        /// Another application replaces the contents with `items` (`(uti, bytes)` in declared order).
        pub fn copy(&self, items: &[(&str, &[u8])]) {
            let mut inner = self.inner.lock_or_panic();
            inner.change_count += 1;
            inner.contents = items.iter().map(|(uti, data)| representation(uti, data.to_vec())).collect();
        }

        #[cfg(feature = "test-support")]
        /// Another application copies plain text.
        pub fn copy_text(&self, text: &str) {
            self.copy(&[(uti::PLAIN_TEXT, text.as_bytes())]);
        }

        #[cfg(feature = "test-support")]
        /// The ownership changes without content (e.g. `clearContents()` by another application).
        pub fn clear(&self) {
            self.copy(&[]);
        }

        #[cfg(feature = "test-support")]
        pub fn contents(&self) -> Vec<ClipboardPasteboardData> {
            self.inner.lock_or_panic().contents.clone()
        }

        #[cfg(feature = "test-support")]
        /// What the app wrote, in order.
        pub fn writes(&self) -> Vec<Vec<ClipboardPasteboardData>> {
            self.inner.lock_or_panic().writes.clone()
        }

        #[cfg(feature = "test-support")]
        pub fn current_change_count(&self) -> i64 {
            self.inner.lock_or_panic().change_count
        }

        #[cfg(feature = "test-support")]
        /// Every call fails while set.
        pub fn set_failing(&self, failing: bool) {
            self.inner.lock_or_panic().failing = failing;
        }

        #[cfg(feature = "test-support")]
        /// The app's writes are refused at `uti` (as `setData` returning `false`): a write clears
        /// the pasteboard, writes the representations before it and stops; `None` takes every type.
        pub fn refuse_type(&self, uti: Option<&str>) {
            self.inner.lock_or_panic().refused = uti.map(str::to_owned);
        }

        #[cfg(feature = "test-support")]
        /// How many `data` reads happened (the change-count poll reads no content).
        pub fn data_reads(&self) -> usize {
            self.inner.lock_or_panic().reads
        }

        fn check(inner: &Inner) -> Result<(), EdgeError> {
            if inner.failing { Err(EdgeError::Unexpected { message: "pasteboard unavailable".into() }) } else { Ok(()) }
        }
    }

    impl ClipboardPasteboard for MemoryPasteboard {
        fn change_count(&self) -> Result<i64, EdgeError> {
            let inner = self.inner.lock_or_panic();
            Self::check(&inner)?;
            Ok(inner.change_count)
        }

        fn types(&self) -> Result<Vec<String>, EdgeError> {
            let inner = self.inner.lock_or_panic();
            Self::check(&inner)?;
            Ok(inner.contents.iter().map(|r| r.uti.clone()).collect())
        }

        fn data(&self, uti: String) -> Result<Option<Vec<u8>>, EdgeError> {
            let mut inner = self.inner.lock_or_panic();
            Self::check(&inner)?;
            inner.reads += 1;
            Ok(inner.contents.iter().find(|r| r.uti == uti).map(|r| r.data.clone()))
        }

        fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
            let mut inner = self.inner.lock_or_panic();
            Self::check(&inner)?;

            inner.change_count += 1;
            let refused_at = inner.refused.as_ref().and_then(|uti| representations.iter().position(|r| &r.uti == uti));
            let written = representations[..refused_at.unwrap_or(representations.len())].to_vec();
            inner.contents = written.clone();
            inner.writes.push(written);

            let failure = refused_at.map(|i| format!("the pasteboard did not take {}", representations[i].uti));
            Ok(ClipboardPasteboardWrite { change_count: inner.change_count, failure })
        }
    }

    #[derive(Default)]
    struct Inner {
        change_count: i64,
        contents: Vec<ClipboardPasteboardData>,
        writes: Vec<Vec<ClipboardPasteboardData>>,
        failing: bool,
        /// A type the app's writes are refused at (the write stops there, after clearing).
        refused: Option<String>,
        reads: usize,
    }
}

#[cfg(test)]
mod tests;
