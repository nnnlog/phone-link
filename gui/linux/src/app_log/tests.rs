use super::*;

#[test]
fn the_system_is_named_by_its_pretty_name() {
    let text = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 26.10\"\nID=ubuntu\n";
    assert_eq!(pretty_name(text).as_deref(), Some("Ubuntu 26.10"));
    assert_eq!(pretty_name("NAME=Arch\nPRETTY_NAME=\n"), None);
    assert_eq!(pretty_name("ID=x\n"), None);
}

#[test]
fn the_launch_line_names_the_desktop_when_known() {
    assert_eq!(launch_line("0.4.1", "Ubuntu 26.10", Some("KDE")), "app launched: version 0.4.1 on Ubuntu 26.10 (KDE)");
    assert_eq!(launch_line("0.4.1", "Fedora Linux 43", None), "app launched: version 0.4.1 on Fedora Linux 43");
}
