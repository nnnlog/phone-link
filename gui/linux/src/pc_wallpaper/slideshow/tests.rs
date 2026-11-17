use super::*;

const TIMED: &str = r#"<background>
        <starttime><year>2011</year><month>11</month><day>24</day><hour>7</hour><minute>00</minute><second>00</second></starttime>
        <static><duration>3600.0</duration><file>morning.png</file></static>
        <transition type="overlay"><duration>600.0</duration><from>morning.png</from><to>day.png</to></transition>
        <static><duration>7200.0</duration>
            <file><size width="1024" height="768">day-small.png</size><size width="1920" height="1080">day.png</size></file>
        </static>
        <transition><duration>600.0</duration><from>/srv/day.png</from><to>night.png</to></transition>
    </background>"#;

#[test]
fn a_slideshow_shows_the_picture_of_the_moment() {
    let folder = Path::new("/usr/share/backgrounds/timed");
    let slideshow = Slideshow::parse(TIMED, folder).unwrap();
    let start = local_time(2011, 11, 24, 7, 0, 0).unwrap();
    assert_eq!(slideshow.start, start);
    let shown = |seconds: i64| slideshow.picture_at(start + seconds).unwrap().to_owned();
    assert_eq!(shown(0), folder.join("morning.png"));
    assert_eq!(shown(3600 + 200), folder.join("morning.png"), "early in a cross-fade");
    assert_eq!(shown(3600 + 400), folder.join("day.png"), "late in a cross-fade");
    assert_eq!(shown(3600 + 600 + 7000), folder.join("day-small.png"), "the smallest size tall enough");
    assert_eq!(shown(3600 + 600 + 7200 + 500), folder.join("night.png"));
    assert_eq!(shown(12_000 + 10), folder.join("morning.png"), "the slides repeat");
    assert_eq!(shown(-10), folder.join("night.png"), "before the start, the round before it");

    let still = Slideshow::parse(
        "<background><static><duration>1e9</duration><file>/a.png</file></static></background>",
        folder,
    )
    .unwrap();
    assert_eq!((still.start, still.picture_at(0).unwrap()), (0, Path::new("/a.png")));
    assert!(Slideshow::parse("<background><static><file>/a.png</file></static></background>", folder).is_err());
    let empty = Slideshow::parse("<background/>", folder).unwrap();
    assert!(empty.picture_at(0).is_err());
}
