//! The revisions views re-read on: each is a counter property the QML binds its queries to.

use std::pin::Pin;

use crate::engine_bridge::EngineBridge;

/// What a view re-reads when it changes.
#[derive(Debug, Clone, Copy)]
pub(in crate::engine_bridge) enum Revision {
    Contacts,
    Photos,
    Calls,
    Apps,
    Media,
    Transfers,
    Settings,
    Wallpaper,
    Mirroring,
    Pairing,
    Messaging,
}

impl Revision {
    pub(super) const ALL: [Self; 11] = [
        Self::Contacts,
        Self::Photos,
        Self::Calls,
        Self::Apps,
        Self::Media,
        Self::Transfers,
        Self::Settings,
        Self::Wallpaper,
        Self::Mirroring,
        Self::Pairing,
        Self::Messaging,
    ];
}

impl EngineBridge {
    /// Has the views of `revision` read again.
    pub(in crate::engine_bridge) fn bump(mut self: Pin<&mut Self>, revision: Revision) {
        match revision {
            Revision::Contacts => {
                let next = self.contacts_revision + 1;
                self.as_mut().set_contacts_revision(next);
            }
            Revision::Photos => {
                let next = self.photos_revision + 1;
                self.as_mut().set_photos_revision(next);
            }
            Revision::Calls => {
                let next = self.calls_revision + 1;
                self.as_mut().set_calls_revision(next);
            }
            Revision::Apps => {
                let next = self.apps_revision + 1;
                self.as_mut().set_apps_revision(next);
            }
            Revision::Media => {
                let next = self.media_revision + 1;
                self.as_mut().set_media_revision(next);
            }
            Revision::Transfers => {
                let next = self.transfers_revision + 1;
                self.as_mut().set_transfers_revision(next);
            }
            Revision::Settings => {
                let next = self.settings_revision + 1;
                self.as_mut().set_settings_revision(next);
            }
            Revision::Wallpaper => {
                let next = self.wallpaper_revision + 1;
                self.as_mut().set_wallpaper_revision(next);
            }
            Revision::Mirroring => {
                let next = self.mirroring_revision + 1;
                self.as_mut().set_mirroring_revision(next);
            }
            Revision::Pairing => {
                let next = self.pairing_revision + 1;
                self.as_mut().set_pairing_revision(next);
            }
            Revision::Messaging => {
                let next = self.messaging_revision + 1;
                self.as_mut().set_messaging_revision(next);
            }
        }
    }
}
