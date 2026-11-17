use super::failures::failure_title;

/// The window's own bookkeeping and the input fail quietly; what the user asked for says so.
#[test]
fn user_actions_show_their_failure() {
    assert_eq!(failure_title("mirror.retry"), Some("Couldn't Connect"));
    assert_eq!(failure_title("mirror.resume"), Some("Couldn't Resume the App"));
    assert_eq!(failure_title("mirror.moveToComputer"), Some("Couldn't Respond to Your Phone"));
    assert_eq!(failure_title("mirror.launchNotification"), Some("Couldn't Open on Your Phone"));
    for quiet in ["mirror.close", "mirror.ready", "mirror.focus", "mirror.inputFocus", "mirror.nearbyRefreshed"] {
        assert_eq!(failure_title(quiet), None, "{quiet}");
    }
    for input in ["mirror.mouse", "mirror.wheel", "mirror.key", "mirror.touch", "mirror.pen", "mirror.nav"] {
        assert_eq!(failure_title(input), None, "{input}");
    }
}
