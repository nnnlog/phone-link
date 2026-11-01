//! The media sessions of the selected phone, made from its stored media-style notifications, and
//! which one the controls act on.
//!
//! One session per stored notification row whose item has `notificationClass` `2`, in arrival
//! order; a row that already has a session keeps updating it after its item stops being media-style.
//! [`Sessions::apply`] brings the sessions in line with the stored rows: a row is *updated* when its
//! item JSON differs from the one its session was last made from.

use base64::Engine as _;
use tracing::warn;

use phonelink_notifications::item::NotificationItem;

/// `PlaybackState.STATE_PLAYING`: the only state the controls treat as playing.
pub const STATE_PLAYING: i32 = 3;
/// `PlaybackState.STATE_NONE`: an absent `playbackState`.
pub const STATE_NONE: i32 = 0;

/// One media session as the controls show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSession {
    /// The notification's local row: identifies the session while its row is stored.
    pub row_id: i64,
    /// The notification key: the target of commands.
    pub key: String,
    /// `appName`.
    pub app_name: Option<String>,
    /// `title`.
    pub title: Option<String>,
    /// `text`.
    pub artist: Option<String>,
    /// The album art's image bytes (JPEG from the phone); `None` → a placeholder.
    pub album_art: Option<Vec<u8>>,
    /// The app icon's image bytes (JPEG from the phone).
    pub app_icon: Option<Vec<u8>>,
    /// Android's `PlaybackState.STATE_*` (`0` when absent).
    pub playback_state: i32,
}

impl MediaSession {
    pub fn is_playing(&self) -> bool {
        self.playback_state == STATE_PLAYING
    }
}

/// A stored notification row as the sessions read it.
#[derive(Debug, Clone)]
pub struct MediaRow {
    pub row_id: i64,
    /// The item JSON text as stored.
    pub json: String,
    pub item: NotificationItem,
}

/// A session and what it was last made from.
#[derive(Debug, Clone)]
struct Entry {
    session: MediaSession,
    json: String,
    /// The `largeIcon` / `smallIcon` strings the images were decoded from (`""` when absent).
    art_text: String,
    icon_text: String,
}

/// The sessions and the remembered selection.
#[derive(Debug, Default)]
pub struct Sessions {
    entries: Vec<Entry>,
    /// The row the user or a session event selected; the first session when it is gone.
    remembered: Option<i64>,
}

/// Decoded image bytes of a base64 string; `None` for an empty or undecodable one.
fn image_of(text: &str, what: &'static str) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }

    match base64::prelude::BASE64_STANDARD.decode(text) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            warn!(what, error = %e, "media session image not base64; the placeholder is shown");
            None
        }
    }
}

impl Sessions {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The sessions, in arrival order.
    pub fn list(&self) -> Vec<MediaSession> {
        self.entries.iter().map(|e| e.session.clone()).collect()
    }

    /// The session the controls act on: the remembered one while it exists, else the first.
    pub fn selected(&self) -> Option<&MediaSession> {
        self.remembered.and_then(|id| self.find(id)).or_else(|| self.entries.first().map(|e| &e.session))
    }

    pub fn find(&self, row_id: i64) -> Option<&MediaSession> {
        self.entries.iter().find(|e| e.session.row_id == row_id).map(|e| &e.session)
    }

    /// Drops every session and the remembered selection.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.remembered = None;
    }

    /// The user picked `row_id` (a stored session). Returns the session to send *Play* to: the new
    /// selection, when the previous one was playing and the new one is not.
    pub fn pick(&mut self, row_id: i64) -> Option<MediaSession> {
        let was_playing = self.selected().is_some_and(MediaSession::is_playing);
        let picked = self.find(row_id)?.clone();
        self.remembered = Some(row_id);
        (was_playing && !picked.is_playing()).then_some(picked)
    }

    /// Brings the sessions in line with `rows` (every stored row, any order): a session whose row is
    /// gone is removed (a removed selection is forgotten); a row with a session whose JSON changed
    /// updates it, and becomes the selection when it is now playing; a media-style row without one
    /// adds a session at the end, which becomes the selection when it is playing or the selection is
    /// not. Returns whether any session was added, updated or removed.
    pub fn apply(&mut self, mut rows: Vec<MediaRow>) -> bool {
        rows.sort_by_key(|r| r.row_id);
        let mut changed = false;

        // Removals.
        let before = self.entries.len();
        self.entries.retain(|e| rows.iter().any(|r| r.row_id == e.session.row_id));
        if self.entries.len() != before {
            changed = true;
            if self.remembered.is_some_and(|id| self.find(id).is_none()) {
                self.remembered = None;
            }
        }

        for row in rows {
            match self.entries.iter().position(|e| e.session.row_id == row.row_id) {
                Some(i) => {
                    if self.entries[i].json == row.json {
                        continue;
                    }

                    update(&mut self.entries[i], row);
                    changed = true;

                    if self.entries[i].session.is_playing() {
                        self.remembered = Some(self.entries[i].session.row_id);
                    }
                }
                None if row.item.is_media_style() => {
                    let entry = new_entry(row);
                    let take = entry.session.is_playing() || !self.selected().is_some_and(MediaSession::is_playing);
                    let id = entry.session.row_id;
                    self.entries.push(entry);
                    if take {
                        self.remembered = Some(id);
                    }
                    changed = true;
                }
                None => {}
            }
        }

        changed
    }
}

fn new_entry(row: MediaRow) -> Entry {
    let item = &row.item;
    let art_text = item.large_icon().unwrap_or_default().to_string();
    let icon_text = item.small_icon().unwrap_or_default().to_string();

    Entry {
        session: MediaSession {
            row_id: row.row_id,
            key: item.key.clone(),
            app_name: item.app_name().map(str::to_string),
            title: item.title().map(str::to_string),
            artist: item.text().map(str::to_string),
            album_art: image_of(&art_text, "album art"),
            app_icon: image_of(&icon_text, "app icon"),
            playback_state: item.playback_state().unwrap_or(STATE_NONE),
        },
        art_text,
        icon_text,
        json: row.json,
    }
}

/// An update: the text fields and the state as sent; each image replaced when its key is present,
/// or — when app, title or artist changed — cleared when it is absent; otherwise kept.
fn update(entry: &mut Entry, row: MediaRow) {
    let item = &row.item;
    let s = &mut entry.session;
    let (app, title, artist) =
        (item.app_name().map(str::to_string), item.title().map(str::to_string), item.text().map(str::to_string));
    let metadata_changed = s.app_name != app || s.title != title || s.artist != artist;

    for (sent, text, image, what) in [
        (item.large_icon(), &mut entry.art_text, &mut s.album_art, "album art"),
        (item.small_icon(), &mut entry.icon_text, &mut s.app_icon, "app icon"),
    ] {
        if sent.is_none() && !metadata_changed {
            continue;
        }
        let sent = sent.unwrap_or_default();
        if sent != text.as_str() {
            *text = sent.to_string();
            *image = image_of(text, what);
        }
    }

    s.key = item.key.clone();
    s.app_name = app;
    s.title = title;
    s.artist = artist;
    s.playback_state = item.playback_state().unwrap_or(STATE_NONE);
    entry.json = row.json;
}

#[cfg(test)]
mod tests;
