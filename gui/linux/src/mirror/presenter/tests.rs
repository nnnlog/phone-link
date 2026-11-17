use super::*;

fn sessions(presenter: &LinuxMirrorPresenter) -> Vec<String> {
    let mut ids: Vec<String> = presenter.streams.lock_or_panic().keys().cloned().collect();
    ids.sort();
    ids
}

/// Only a window attaching and a frame arriving make a session's stream; a closed session's
/// stream is not made again by its window going or being painted.
#[test]
fn a_closed_sessions_stream_is_not_made_again() {
    let presenter = LinuxMirrorPresenter::default();
    presenter.attach("A", 1, Arc::new(|| {}));
    assert_eq!(sessions(&presenter), ["A"]);

    presenter.forget("A");
    presenter.detach("A", 1);
    assert!(presenter.frame("A").is_none());
    presenter.reset_video("A".to_owned());
    assert!(sessions(&presenter).is_empty());
}

#[test]
fn a_window_detaches_only_its_own_repaint() {
    let presenter = LinuxMirrorPresenter::default();
    presenter.attach("A", 2, Arc::new(|| {}));
    presenter.detach("A", 1);
    assert!(presenter.existing("A").unwrap().repaint.lock_or_panic().is_some(), "an older window's token");
    presenter.detach("A", 2);
    assert!(presenter.existing("A").unwrap().repaint.lock_or_panic().is_none());
}

#[test]
fn the_display_takes_five_values() {
    assert_eq!(
        display_geometry(&[2560, 1380, 192, 2560, 1440]),
        Some(DisplayGeometry { usable_width: 2560, usable_height: 1380, dpi: 192, width: 2560, height: 1440 })
    );
    assert_eq!(display_geometry(&[]), None, "no screen read yet");
    assert_eq!(display_geometry(&[1920, 1020, 96, 1920]), None);
    assert_eq!(display_geometry(&[1920, -1, 96, 1920, 1080]), None);
}

#[test]
fn the_phone_is_told_the_screen_read_last() {
    let presenter = LinuxMirrorPresenter::default();
    assert_eq!(presenter.display_geometry(), UNREAD_DISPLAY, "nothing read yet");
    presenter.read_screen_with(|| vec![3024, 1890, 192, 3024, 1964]);
    assert_eq!(presenter.display_geometry().usable_height, 1890);
}
