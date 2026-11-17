use super::*;

#[test]
fn the_desktop_comes_from_the_first_known_name() {
    assert_eq!(Desktop::from_current_desktop("ubuntu:GNOME"), Some(Desktop::Gnome));
    assert_eq!(Desktop::from_current_desktop("Budgie:GNOME"), Some(Desktop::Budgie));
    assert_eq!(Desktop::from_current_desktop("X-Cinnamon"), Some(Desktop::Cinnamon));
    assert_eq!(Desktop::from_current_desktop("MATE"), Some(Desktop::Mate));
    assert_eq!(Desktop::from_current_desktop("KDE"), Some(Desktop::Plasma));
    assert_eq!(Desktop::from_current_desktop("XFCE"), Some(Desktop::Xfce));
    assert_eq!(Desktop::from_current_desktop("sway:wlroots"), None);
    assert_eq!(Desktop::from_current_desktop(""), None);
}

#[test]
fn only_local_files_are_pictures() {
    assert_eq!(
        Setting::from_uri("file:///home/u/My%20Pictures/a.jpg"),
        Setting::Picture(PathBuf::from("/home/u/My Pictures/a.jpg"))
    );
    assert_eq!(
        Setting::from_uri("/usr/share/backgrounds/b.png"),
        Setting::Picture(PathBuf::from("/usr/share/backgrounds/b.png"))
    );
    assert_eq!(Setting::from_uri(""), Setting::NoPicture);
    assert_eq!(Setting::from_uri("https://example.com/c.jpg"), Setting::NoPicture);
    assert_eq!(Setting::from_path(""), Setting::NoPicture);
}
