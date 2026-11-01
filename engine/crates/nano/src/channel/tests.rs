use super::*;

#[test]
fn announcements() {
    let a = Announcement::parse(
            "Microsoft::Basix::Dct::Channel::Class::Video,7,1,ScenarioId=appRemoting_X&ConsumerId=c-1&VideoSourceChannelFps=30",
        )
        .unwrap();
    assert_eq!(a.channel_type, Some(ChannelType::VideoTarget));
    assert_eq!(a.scenario_id(), Some("appRemoting_X"));
    assert_eq!(a.consumer_id(), Some("c-1"));
    assert_eq!(a.nano_channel_id, "7");

    let m = Announcement::parse("Microsoft::Basix::Dct::Channel::Class::Messaging,1,4,ScenarioId=DeviceConnection")
        .unwrap();
    assert_eq!(m.channel_type, Some(ChannelType::Message));
    assert!(m.consumer_id().is_none());

    assert!(Announcement::parse("only,two").is_err());
    assert_eq!(Announcement::parse("c,1,9").unwrap().channel_type, None);
}

#[test]
fn framing() {
    let f = frame(2, b"abc");
    assert_eq!(f, [2, 0, 0, 0, 3, 0, 0, 0, b'a', b'b', b'c']);
    assert_eq!(unframe(&f).unwrap(), (2, &b"abc"[..]));
    assert!(unframe(&f[..9]).is_err());
}
