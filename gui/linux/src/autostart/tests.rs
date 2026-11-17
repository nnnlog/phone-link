use super::*;

#[test]
fn a_hidden_entry_does_not_start() {
    assert!(starts("[Desktop Entry]\nExec=x\n"));
    assert!(!starts("[Desktop Entry]\nExec=x\nHidden=true\n"));
    assert!(!starts("[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n"));
}

#[test]
fn the_user_entry_overrides_the_system_one() {
    let dir = std::env::temp_dir().join(format!("phonelink-autostart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (user, system) = (dir.join("user.desktop"), dir.join("system.desktop"));
    std::fs::write(&system, entry_text("/usr/bin/phonelink-linux", true)).unwrap();
    assert!(effective_entry(Some(user.as_path()), std::slice::from_ref(&system)).is_some_and(|e| starts(&e)));

    std::fs::write(&user, entry_text("/usr/bin/phonelink-linux", false)).unwrap();
    assert!(!effective_entry(Some(user.as_path()), std::slice::from_ref(&system)).is_some_and(|e| starts(&e)));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_executable_is_quoted_for_the_exec_key() {
    assert_eq!(exec_argument("/usr/bin/phonelink-linux"), r#""/usr/bin/phonelink-linux""#);
    assert_eq!(exec_argument("/opt/Phone Link/phonelink-linux"), r#""/opt/Phone Link/phonelink-linux""#);
    assert_eq!(exec_argument("/opt/$x/100%/a\\b"), r#""/opt/\\$x/100%%/a\\\\b""#);
    assert!(
        entry_text("/opt/Phone Link/phonelink-linux", true)
            .contains("\nExec=\"/opt/Phone Link/phonelink-linux\" --background\n")
    );
}
