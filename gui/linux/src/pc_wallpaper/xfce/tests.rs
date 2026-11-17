use super::*;

fn properties(lines: &[&str]) -> HashMap<String, String> {
    xfconf_properties(&lines.join("\n"))
}

#[test]
fn xfce_reads_the_primary_monitors_picture() {
    let desktop = properties(&[
        "/backdrop/screen0/monitoreDP-1/workspace0/last-image   /usr/share/backgrounds/xfce/blue.png",
        "/backdrop/screen0/monitoreDP-1/workspace0/image-style  5",
        "/backdrop/screen0/monitorHDMI-1/workspace0/last-image  /home/u/Pictures/my picture.jpg",
        "/backdrop/screen0/monitorHDMI-1/workspace1/last-image  /home/u/Pictures/other.jpg",
        "/backdrop/single-workspace-mode                         true",
    ]);
    assert_eq!(
        desktop.get("/backdrop/screen0/monitorHDMI-1/workspace0/last-image").map(String::as_str),
        Some("/home/u/Pictures/my picture.jpg")
    );
    assert_eq!(xfce_workspace(&desktop).ok(), Some(0));
    assert_eq!(xfce_monitors(&desktop, 0), vec!["HDMI-1".to_owned(), "eDP-1".to_owned()]);
    assert_eq!(xfce_monitors(&desktop, 1), vec!["HDMI-1".to_owned()]);

    let displays = properties(&[
        "/ActiveProfile       Default",
        "/Default/eDP-1       Built-in display",
        "/Default/eDP-1/Primary  false",
        "/Default/HDMI-1/Primary true",
        "/Other/eDP-1/Primary    true",
    ]);
    assert_eq!(xfce_primary(&displays).as_deref(), Some("HDMI-1"));
    assert_eq!(xfce_primary(&properties(&["/Default/eDP-1/Primary false"])), None);
}
