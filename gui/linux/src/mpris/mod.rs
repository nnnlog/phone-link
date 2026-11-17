//! The phone's media session as an MPRIS player on the session bus, so the desktop's media controls
//! and media keys play, pause and skip what plays on the phone.
//!
//! A [`Player`] owns a connection of its own, named `org.mpris.MediaPlayer2.phonelink`, on a thread
//! of its own: it connects when it is first shown an item, mirrors the latest item shown after that,
//! and leaves the bus when it is dropped. The phone gives no position, length or volume, so the
//! player offers play, pause, next and previous only.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use tokio::runtime::Handle;
use tracing::{info, warn};
use zbus::interface;
use zbus::zvariant::{ObjectPath, Value};

const BUS_NAME: &str = "org.mpris.MediaPlayer2.phonelink";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
/// The desktop entry the desktop takes the player's name and icon from.
const DESKTOP_ENTRY: &str = "phonelink-linux";

/// What the player shows: the phone's selected media session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    /// The phone's id of the session.
    pub session: i64,
    pub title: String,
    pub artist: Option<String>,
    /// The media app on the phone, shown as the album so that the desktop's media controls name it.
    pub app: String,
    /// A `file://` URL of the album art.
    pub art_url: Option<String>,
    pub playing: bool,
}

/// What the desktop asked of the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Bring the app's window forward.
    Raise,
}

/// What the player does with the desktop's requests.
type OnCommand = Arc<dyn Fn(Command) + Send + Sync>;

/// The player while it shows the phone's media; dropping it removes it from the desktop.
pub struct Player {
    shown: Sender<NowPlaying>,
}

impl Player {
    /// A player that hands the desktop's requests to `on_command`, from the connection's thread. It
    /// appears on the bus with the first item [`Player::show`] gives it.
    pub fn start(runtime: Handle, on_command: impl Fn(Command) + Send + Sync + 'static) -> Self {
        let (shown, items) = mpsc::channel();
        let on_command: OnCommand = Arc::new(on_command);
        thread::spawn(move || serve(&runtime, &items, &on_command));
        Self { shown }
    }

    /// Shows `item` from now on.
    pub fn show(&self, item: NowPlaying) {
        // The thread ends early only when the bus could not be reached, which it has logged.
        let _ = self.shown.send(item);
    }
}

/// Connects with the first item, then mirrors the latest of each batch of items until the player is
/// dropped; the connection goes with the thread, and the bus name with it.
fn serve(runtime: &Handle, items: &Receiver<NowPlaying>, on_command: &OnCommand) {
    let Ok(mut item) = items.recv() else { return };
    let connection = match runtime.block_on(connect(&item, on_command.clone())) {
        Ok(connection) => connection,
        Err(e) => {
            warn!(error = %e, "the phone's media is not in the desktop's media controls");
            return;
        }
    };
    info!("the phone's media is in the desktop's media controls");

    while let Ok(next) = items.recv() {
        let next = items.try_iter().last().unwrap_or(next);
        if next != item {
            if let Err(e) = runtime.block_on(update(&connection, &item, &next)) {
                warn!(error = %e, "the desktop's media controls were not updated");
            }
            item = next;
        }
    }

    info!("the phone's media left the desktop's media controls");
}

/// A connection that serves the player showing `item`, under the player's bus name.
async fn connect(item: &NowPlaying, on_command: OnCommand) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .serve_at(OBJECT_PATH, Root { on_command: on_command.clone() })?
        .serve_at(OBJECT_PATH, PlayerInterface { item: item.clone(), on_command })?
        .name(BUS_NAME)?
        // A player that replaces one still on its way off the bus takes the name over from it.
        .allow_name_replacements(true)
        .replace_existing_names(true)
        .build()
        .await
}

/// Shows `new` instead of `old`, and tells the desktop what changed.
async fn update(connection: &zbus::Connection, old: &NowPlaying, new: &NowPlaying) -> zbus::Result<()> {
    let player = connection.object_server().interface::<_, PlayerInterface>(OBJECT_PATH).await?;
    player.get_mut().await.item = new.clone();
    let emitter = player.signal_emitter();
    let shown = player.get().await;
    if metadata(old) != metadata(new) {
        shown.metadata_changed(emitter).await?;
    }
    if old.playing != new.playing {
        shown.playback_status_changed(emitter).await?;
    }
    Ok(())
}

/// The track's id: one per session, title and artist, so a new track reads as one. MPRIS keeps the
/// paths under `/org/mpris` for itself, so the ids are the app's own.
fn track_id(item: &NowPlaying) -> String {
    let artist = item.artist.as_deref().unwrap_or_default();
    // The session id's bits in hex: an object path element allows no sign.
    format!(
        "/org/phonelink/MediaPlayer/Track/s{:x}_{:x}",
        item.session as u64,
        crate::images::fnv1a(format!("{}\n{artist}", item.title).as_bytes())
    )
}

/// The track's metadata: its id, title, artist, the media app as its album, and the album art.
fn metadata(item: &NowPlaying) -> HashMap<String, Value<'static>> {
    let mut map = HashMap::new();
    match ObjectPath::try_from(track_id(item)) {
        Ok(path) => {
            map.insert("mpris:trackid".to_owned(), Value::from(path));
        }
        Err(e) => warn!(error = %e, "the track's id is not an object path"),
    }

    map.insert("xesam:title".to_owned(), Value::from(item.title.clone()));
    map.insert("xesam:album".to_owned(), Value::from(item.app.clone()));
    if let Some(artist) = &item.artist {
        map.insert("xesam:artist".to_owned(), Value::from(vec![artist.clone()]));
    }
    if let Some(url) = &item.art_url {
        map.insert("mpris:artUrl".to_owned(), Value::from(url.clone()));
    }
    map
}

/// `org.mpris.MediaPlayer2`: who the player is.
struct Root {
    on_command: OnCommand,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        (self.on_command)(Command::Raise);
    }

    /// The player cannot quit Phone Link (`CanQuit` is false), so this does nothing.
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Phone Link".to_owned()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        DESKTOP_ENTRY.to_owned()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `org.mpris.MediaPlayer2.Player`: the phone's selected session and its buttons.
struct PlayerInterface {
    item: NowPlaying,
    on_command: OnCommand,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerInterface {
    fn next(&self) {
        (self.on_command)(Command::Next);
    }

    fn previous(&self) {
        (self.on_command)(Command::Previous);
    }

    fn pause(&self) {
        (self.on_command)(Command::Pause);
    }

    fn play_pause(&self) {
        (self.on_command)(Command::PlayPause);
    }

    fn stop(&self) {
        (self.on_command)(Command::Stop);
    }

    fn play(&self) {
        (self.on_command)(Command::Play);
    }

    /// The phone gives no position (`CanSeek` is false), so this does nothing.
    fn seek(&self, _offset: i64) {}

    /// The phone gives no position (`CanSeek` is false), so this does nothing.
    fn set_position(&self, _track_id: ObjectPath<'_>, _position: i64) {}

    fn open_uri(&self, _uri: &str) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported("the phone's media player opens no URI".to_owned()))
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        if self.item.playing { "Playing" } else { "Paused" }.to_owned()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        metadata(&self.item)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
