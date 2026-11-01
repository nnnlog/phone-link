use super::*;

fn uri(package_name: Option<&str>, launch_source: Option<&str>) -> RemotingUri {
    RemotingUri {
        device_id: None,
        package_name: package_name.map(str::to_owned),
        launch_source: launch_source.map(str::to_owned),
    }
}

#[test]
fn remoting_targets_follow_the_gates() {
    use AppsFeatureType::{Disabled, MultipleApps, PhoneScreenOnly};
    use RemotingTarget::{App, AppsView, MainWindow, PhoneScreen};
    let screen = Some(PHONE_SCREEN_PACKAGE);
    let cases = [
        // (package, launchsource, remoting supported, phone-screen apps, apps feature type) → target
        (screen, Some("protocol"), true, true, PhoneScreenOnly, PhoneScreen),
        (screen, Some("protocol"), true, true, MultipleApps, PhoneScreen),
        (screen, None, true, true, PhoneScreenOnly, PhoneScreen),
        (Some("com.example.app"), Some("protocol"), true, true, MultipleApps, App("com.example.app".into())),
        (Some("com.example.app"), None, true, true, PhoneScreenOnly, App("com.example.app".into())),
        (screen, Some("Taskbar"), true, true, MultipleApps, AppsView),
        (screen, Some("StartMenu"), true, true, MultipleApps, AppsView),
        (screen, Some("Taskbar"), true, true, PhoneScreenOnly, PhoneScreen),
        (screen, Some("taskbar"), true, true, MultipleApps, PhoneScreen),
        (screen, Some("protocol"), true, false, Disabled, AppsView),
        (None, Some("protocol"), true, true, PhoneScreenOnly, AppsView),
        (Some("  "), Some("protocol"), true, true, PhoneScreenOnly, AppsView),
        (screen, Some("protocol"), false, true, PhoneScreenOnly, MainWindow),
        (None, None, false, false, Disabled, MainWindow),
    ];

    for (package, source, remoting, screen_apps, apps, expected) in cases {
        assert_eq!(
            remoting_target(&uri(package, source), remoting, screen_apps, apps),
            expected,
            "{package:?} {source:?} remoting={remoting} screen_apps={screen_apps} {apps:?}"
        );
    }
}
