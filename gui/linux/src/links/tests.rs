use super::{is_link, tel_number};

#[test]
fn a_tel_link_gives_its_number() {
    let cases = [
        ("tel:+15550100", Some("+15550100")),
        ("TEL:555-0100", Some("555-0100")),
        ("tel:+1%20555%200100", Some("+1 555 0100")),
        ("tel:%2B15550100", Some("+15550100")),
        ("tel:5550100;ext=12", Some("5550100")),
        ("tel:5550100?subject=x", Some("5550100")),
        ("tel:100%", Some("100%")),
        ("tel:%zz1", Some("%zz1")),
        ("tel:", None),
        ("tel:;ext=1", None),
        ("tel:%FF", None),
        ("mailto:ada@example.com", None),
        ("te", None),
    ];
    for (link, number) in cases {
        assert_eq!(tel_number(link).as_deref(), number, "{link}");
    }
}

#[test]
fn only_tel_links_are_links() {
    assert!(is_link("tel:1") && is_link("Tel:1"));
    assert!(!is_link("--help") && !is_link("/home/ada/file") && !is_link("té"));
}
