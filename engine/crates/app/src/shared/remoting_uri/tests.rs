use super::*;

fn uri(device_id: Option<&str>, package_name: Option<&str>, launch_source: Option<&str>) -> Option<RemotingUri> {
    Some(RemotingUri {
        device_id: device_id.map(str::to_owned),
        package_name: package_name.map(str::to_owned),
        launch_source: launch_source.map(str::to_owned),
    })
}

#[test]
fn remoting_uris_and_their_values() {
    let cases: [(&str, Option<RemotingUri>); 14] = [
        // What the phone's Mirror to PC sends.
        (
            "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen&cid=6f1c0d5e-2b1a-3c4d-8e9f-0a1b2c3d4e5f",
            uri(None, Some("yourphone.phonescreen"), Some("protocol")),
        ),
        ("MS-PHONE:remoting?packagename=com.example.app&deviceid=abc", uri(Some("abc"), Some("com.example.app"), None)),
        ("ms-phone:remoting", uri(None, None, None)),
        ("ms-phone:remoting#frag?packagename=x", uri(None, None, None)),
        ("ms-phone:remoting?packagename=a%2Eb+c#packagename=z", uri(None, Some("a.b c"), None)),
        // A part without `=` or with an empty name is skipped.
        ("ms-phone:remoting?packagename&=x&launchsource=", uri(None, None, Some(""))),
        // Names differing only in case are one name, joined; the exact name is then looked up.
        ("ms-phone:remoting?packagename=a&PackageName=b", uri(None, Some("a,b"), None)),
        ("ms-phone:remoting?PackageName=a&packagename=b", uri(None, None, None)),
        // Not remoting URIs.
        ("ms-phone://remoting?packagename=x", None),
        ("ms-phone:///remoting", None),
        ("ms-phone:Remoting?packagename=x", None),
        ("ms-phone:remoting/?packagename=x", None),
        ("ms-phone:", None),
        ("ms-phone:notifications", None),
    ];

    for (input, expected) in cases {
        assert_eq!(RemotingUri::parse(input), expected, "{input}");
    }
}
