//! The unsent text and images of a composer.

use phonelink_messaging::edge::FileFamily;

use crate::messaging_edges::file_types;

/// At most this many images per message.
pub(super) const MAX_IMAGES: usize = 2;

/// An image the user added to a draft: picked, pasted or dropped.
#[derive(Debug, Clone)]
pub(super) struct DraftImage {
    pub(super) id: u64,
    /// The file name; empty for pasted or dropped image data.
    pub(super) name: String,
    /// The MIME type when known; the engine detects it from the bytes otherwise.
    pub(super) content_type: Option<String>,
    pub(super) bytes: Vec<u8>,
    /// Where the window shows it from.
    pub(super) url: String,
}

impl DraftImage {
    /// A message carries images only; without a type the engine decides.
    pub(super) fn is_image(&self) -> bool {
        self.content_type.as_deref().is_none_or(|t| file_types().database().family_of(t) == FileFamily::Image)
    }
}

/// The unsent text and images of one composer.
#[derive(Debug, Clone, Default)]
pub(super) struct Draft {
    pub(super) text: String,
    pub(super) images: Vec<DraftImage>,
}

impl Draft {
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty()
    }

    /// Something worth sending: text other than white space, or an image.
    pub(super) fn has_content(&self) -> bool {
        !self.images.is_empty() || !self.text.trim().is_empty()
    }

    /// `self` with `then` after it: the texts on separate lines, the images as many as fit.
    pub(super) fn joined(self, then: Option<Draft>) -> Draft {
        let Some(then) = then.filter(|then| !then.is_empty()) else { return self };
        let text = [self.text, then.text].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n");
        let images = self.images.into_iter().chain(then.images).take(MAX_IMAGES).collect();
        Draft { text, images }
    }
}
