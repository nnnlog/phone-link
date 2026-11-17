use super::*;

fn item(title: &str, artist: Option<&str>) -> NowPlaying {
    NowPlaying {
        session: -3,
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        app: "Music".to_owned(),
        art_url: Some("file:///tmp/art".to_owned()),
        playing: true,
    }
}

#[test]
fn a_track_id_is_an_object_path_that_changes_with_the_track() {
    let first = track_id(&item("Blue Hour", Some("The Paper Lanterns")));
    assert!(ObjectPath::try_from(first.clone()).is_ok(), "{first}");
    assert_eq!(first, track_id(&item("Blue Hour", Some("The Paper Lanterns"))));
    assert_ne!(first, track_id(&item("Red Dawn", Some("The Paper Lanterns"))));
}

#[test]
fn metadata_names_the_track_the_artist_and_the_app() {
    let map = metadata(&item("Blue Hour", Some("The Paper Lanterns")));
    assert_eq!(map.get("xesam:title"), Some(&Value::from("Blue Hour".to_owned())));
    assert_eq!(map.get("xesam:album"), Some(&Value::from("Music".to_owned())));
    assert_eq!(map.get("xesam:artist"), Some(&Value::from(vec!["The Paper Lanterns".to_owned()])));
    assert_eq!(map.get("mpris:artUrl"), Some(&Value::from("file:///tmp/art".to_owned())));
    assert!(map.contains_key("mpris:trackid"));
    assert!(!metadata(&item("Podcast", None)).contains_key("xesam:artist"));
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("timed out waiting until {what}");
}

/// The player on the session bus as a desktop sees it: it appears with its first item, answers
/// the desktop's requests, announces the next item, and leaves the bus when dropped.
#[test]
#[ignore = "needs a D-Bus session bus of its own (run it under dbus-run-session)"]
fn the_player_serves_the_desktop_on_the_session_bus() {
    use zbus::blocking::fdo::DBusProxy;
    use zbus::blocking::{Connection, Proxy};
    use zbus::names::BusName;
    use zbus::zvariant::OwnedValue;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (commands, received) = mpsc::channel();
    let player = Player::start(runtime.handle().clone(), move |command| {
        commands.send(command).unwrap();
    });
    player.show(item("Blue Hour", Some("The Paper Lanterns")));

    let bus = Connection::session().unwrap();
    let dbus = DBusProxy::new(&bus).unwrap();
    let name = BusName::try_from(BUS_NAME).unwrap();
    wait_until("the player is on the bus", || dbus.name_has_owner(name.clone()).unwrap_or(false));

    let root = Proxy::new(&bus, BUS_NAME, OBJECT_PATH, "org.mpris.MediaPlayer2").unwrap();
    assert_eq!(root.get_property::<String>("Identity").unwrap(), "Phone Link");
    assert_eq!(root.get_property::<String>("DesktopEntry").unwrap(), DESKTOP_ENTRY);
    let player_interface = Proxy::new(&bus, BUS_NAME, OBJECT_PATH, "org.mpris.MediaPlayer2.Player").unwrap();
    assert_eq!(player_interface.get_property::<String>("PlaybackStatus").unwrap(), "Playing");
    assert!(!player_interface.get_property::<bool>("CanSeek").unwrap());
    let shown: HashMap<String, OwnedValue> = player_interface.get_property("Metadata").unwrap();
    assert_eq!(String::try_from(&*shown["xesam:title"]).unwrap(), "Blue Hour");
    assert_eq!(String::try_from(&*shown["xesam:album"]).unwrap(), "Music");

    for (method, command) in [("PlayPause", Command::PlayPause), ("Next", Command::Next), ("Pause", Command::Pause)] {
        player_interface.call::<_, _, ()>(method, &()).unwrap();
        assert_eq!(received.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), command);
    }
    root.call::<_, _, ()>("Raise", &()).unwrap();
    assert_eq!(received.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), Command::Raise);

    let mut paused = item("Red Dawn", None);
    paused.playing = false;
    player.show(paused);
    wait_until("the desktop hears the pause", || {
        player_interface.get_property::<String>("PlaybackStatus").is_ok_and(|status| status == "Paused")
    });
    wait_until("the desktop hears the new track", || {
        player_interface.get_property::<HashMap<String, OwnedValue>>("Metadata").is_ok_and(|shown| {
            shown.get("xesam:title").and_then(|title| String::try_from(&**title).ok()).as_deref() == Some("Red Dawn")
                && !shown.contains_key("xesam:artist")
        })
    });

    drop(player);
    wait_until("the player leaves the bus", || !dbus.name_has_owner(name.clone()).unwrap_or(true));
}
