//! The phone apps' host edge on Linux: whether an installed application opens a recent app's
//! document, and the order of app names in the user's language; and the applications that open a
//! photo (Open With) for the Photos section.
//!
//! - Whether an application opens a URI: the URI's scheme as the MIME type `x-scheme-handler/<scheme>`
//!   has a default or an associated application in the freedesktop.org MIME associations
//!   (`mimeapps.list` and the desktop entries, `desktop`). A string without a scheme opens
//!   nothing, and neither does a `file:` URI, which names a file on the phone.
//! - The order of app names: the Unicode Collation Algorithm with the session locale's tailoring at
//!   the primary strength, ignoring case, diacritics, kana type and width (`collation`); equal names
//!   keep their given order.
//! - The applications that open a photo, for Open With (`open_with`).

mod collation;
pub mod desktop;
mod open_with;

use phonelink_phone_apps::PhoneAppsHost;
use tracing::info;

pub use collation::NameCollator;
use desktop::{Applications, Xdg};
pub use open_with::{OpenWithApp, open_with, open_with_apps};

/// The phone apps' host edge of this session.
pub struct LinuxAppsHost {
    collator: NameCollator,
}

impl LinuxAppsHost {
    pub fn for_session() -> Self {
        Self { collator: NameCollator::for_session() }
    }
}

impl PhoneAppsHost for LinuxAppsHost {
    fn can_open_uri(&self, uri: String) -> bool {
        let Some(mime) = desktop::uri_mime_type(&uri) else { return false };
        let xdg = Xdg::from_env();
        let opens =
            Applications::scan(&xdg, &crate::l10n::messages_locale().unwrap_or_default()).default_for(&mime).is_some();
        if !opens {
            info!(%mime, "no application opens the link's MIME type; the recent app's document link is dropped");
        }
        opens
    }

    fn name_order(&self, names: Vec<String>) -> Vec<u32> {
        self.collator.order(&names)
    }
}

/// The MIME type a file named `name` has in this session's MIME database.
pub fn mime_type_of_name(name: &str) -> Option<String> {
    desktop::mime_type_of_name(&Xdg::from_env(), name)
}
