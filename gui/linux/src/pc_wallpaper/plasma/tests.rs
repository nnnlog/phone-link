use std::collections::HashMap;
use std::path::PathBuf;

use super::*;
use crate::pc_wallpaper::testing::TestDir;

#[test]
fn a_package_sends_a_landscape_picture_tall_enough() {
    assert_eq!(package_size("1920x1080.png"), Some((1920, 1080)));
    assert_eq!(package_size("5120x2880.jpg"), Some((5120, 2880)));
    assert_eq!(package_size("screenshot.png"), None);
    let sizes = [(1080, 1920), (5120, 2880), (1920, 1080), (800, 450), (1280, 800)];
    assert_eq!(package_choice(&sizes), Some((1280, 800)));
    assert_eq!(package_choice(&[(800, 450), (640, 360)]), Some((800, 450)));
    assert_eq!(package_choice(&[(1080, 1920)]), Some((1080, 1920)));
    assert_eq!(package_choice(&[]), None);
}

#[test]
fn plasma_colors_are_dark_below_a_gray_of_192() {
    let colors = |window: &str| format!("[General]\nColorScheme=X\n\n[Colors:Window]\nBackgroundNormal={window}\n");
    assert!(kdeglobals_dark(&colors("32,35,38")));
    assert!(!kdeglobals_dark(&colors("239,240,241")));
    assert!(!kdeglobals_dark("[Colors:View]\nBackgroundNormal=0,0,0\n"));
    assert!(!kdeglobals_dark(""));
}

/// Plasma's shell as far as the wallpaper goes: `wallpaper(screen)` answers the wallpaper's
/// settings.
struct PlasmaShell {
    config: Vec<(&'static str, &'static str)>,
}

#[zbus::interface(name = "org.kde.PlasmaShell")]
impl PlasmaShell {
    // Qt names the method after its slot, in lower camel case.
    #[zbus(name = "wallpaper")]
    fn wallpaper(&self, screen: u32) -> HashMap<String, zbus::zvariant::Value<'static>> {
        assert_eq!(screen, 0, "the primary screen is asked for");
        self.config.iter().map(|(key, value)| ((*key).to_owned(), zbus::zvariant::Value::from(*value))).collect()
    }
}

#[test]
#[ignore = "needs a D-Bus session bus of its own (run it under dbus-run-session)"]
#[expect(unsafe_code, reason = "the test sets the environment, which Rust marks unsafe")]
fn plasmas_picture_comes_from_its_shell_over_d_bus() {
    let shell = zbus::blocking::connection::Builder::session()
        .unwrap()
        .serve_at(
            "/PlasmaShell",
            PlasmaShell {
                config: vec![("wallpaperPlugin", "org.kde.image"), ("Image", "file:///usr/share/wallpapers/Next/")],
            },
        )
        .unwrap()
        .name("org.kde.plasmashell")
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(plasma_setting().unwrap(), Setting::Picture(PathBuf::from("/usr/share/wallpapers/Next/")));

    let set = |config: Vec<(&'static str, &'static str)>| {
        let shell_interface = shell.object_server().interface::<_, PlasmaShell>("/PlasmaShell").unwrap();
        shell_interface.get_mut().config = config;
    };
    set(vec![("wallpaperPlugin", "org.kde.color"), ("Color", "0,0,0")]);
    assert_eq!(plasma_setting().unwrap(), Setting::NoPicture);
    set(vec![("wallpaperPlugin", "org.kde.slideshow"), ("SlidePaths", "/usr/share/wallpapers/")]);
    assert!(plasma_setting().is_err(), "a slideshow names no single picture");

    // The default picture, from the look-and-feel package of the data folders.
    let dir = TestDir::new("plasma-shell");
    dir.write("data/plasma/look-and-feel/org.kde.breeze.desktop/contents/defaults", "[Wallpaper]\nImage=Custom\n");
    let package = dir.package("data/wallpapers/Custom");
    // SAFETY: this test runs alone, in a process of its own (under dbus-run-session).
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path("config"));
        std::env::set_var("XDG_CONFIG_DIRS", dir.path("config-system"));
        std::env::set_var("XDG_DATA_HOME", dir.path("data"));
        std::env::set_var("XDG_DATA_DIRS", dir.path("data-system"));
    }
    set(vec![("wallpaperPlugin", "org.kde.image"), ("Image", "")]);
    assert_eq!(plasma_setting().unwrap(), Setting::Picture(package));
    std::fs::remove_dir_all(dir.path("data/wallpapers")).unwrap();
    assert!(plasma_setting().is_err(), "no default picture is installed");
}

#[test]
fn kconfig_values_are_read_by_group() {
    let text = "[KDE]\nLookAndFeelPackage = org.kde.breezedark.desktop\n[Wallpaper]\nImage=Next\n";
    assert_eq!(kconfig_value(text, "KDE", "LookAndFeelPackage"), Some("org.kde.breezedark.desktop"));
    assert_eq!(kconfig_value(text, "Wallpaper", "Image"), Some("Next"));
    assert_eq!(kconfig_value(text, "KDE", "Image"), None);
    assert_eq!(kconfig_value("[kdeglobals][KDE]\nLookAndFeelPackage=x\n", "KDE", "LookAndFeelPackage"), None);
}

#[test]
fn plasmas_default_picture_comes_from_its_look_and_feel_package() {
    let dir = TestDir::new("plasma-default");
    let (config, data) = (vec![dir.path("config"), dir.path("xdg")], vec![dir.path("local"), dir.path("share")]);
    assert_eq!(plasma_default_picture(&config, &data), None, "nothing installed");

    let next = dir.package("share/wallpapers/Next");
    assert_eq!(plasma_default_picture(&config, &data), Some(next.clone()), "the theme's wallpaper");

    dir.write("share/plasma/look-and-feel/org.kde.breeze.desktop/contents/defaults", "[Wallpaper]\nImage=Flow\n");
    assert_eq!(plasma_default_picture(&config, &data), Some(next.clone()), "a package that is not installed");
    let flow = dir.package("share/wallpapers/Flow");
    assert_eq!(plasma_default_picture(&config, &data), Some(flow));

    dir.write("xdg/kdeglobals", "[KDE]\nLookAndFeelPackage=org.example.desktop\n");
    dir.write("local/plasma/look-and-feel/org.example.desktop/contents/defaults", "[Wallpaper]\nImage=Mine\n");
    let mine = dir.package("local/wallpapers/Mine");
    assert_eq!(plasma_default_picture(&config, &data), Some(mine));
    dir.write("config/kdeglobals", "[KDE]\nLookAndFeelPackage=org.kde.breeze.desktop\n");
    assert_eq!(plasma_default_picture(&config, &data), Some(dir.path("share/wallpapers/Flow")), "the user's choice");
}
