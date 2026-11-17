use super::*;

#[test]
fn desktop_comes_from_the_first_known_token() {
    assert_eq!(Desktop::from_current_desktop("KDE"), Desktop::Kde);
    assert_eq!(Desktop::from_current_desktop("ubuntu:GNOME"), Desktop::Gnome);
    assert_eq!(Desktop::from_current_desktop("X-Cinnamon"), Desktop::Cinnamon);
    assert_eq!(Desktop::from_current_desktop("sway:wlroots"), Desktop::Other);
    assert_eq!(Desktop::from_current_desktop(""), Desktop::Other);
}
