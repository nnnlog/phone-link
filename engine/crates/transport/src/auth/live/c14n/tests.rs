use super::*;

#[test]
fn exc_c14n_matches_lxml() {
    // T1: apex renders only visibly-used namespaces (a, b — not w); unprefixed attr first, then
    // prefixed attrs sorted by (uri, local).
    let t1 = Elem::new("a", "el", "urn:a")
        .attr("", "id", "", "7")
        .attr("b", "x", "urn:b", "1")
        .attr("a", "m", "urn:a", "2")
        .child(Elem::new("b", "child", "urn:b").text("hi"));
    assert_eq!(
        t1.exc_c14n(),
        r#"<a:el xmlns:a="urn:a" xmlns:b="urn:b" id="7" a:m="2" b:x="1"><b:child>hi</b:child></a:el>"#
    );

    // T2: default-namespace element; empty child is <Ref ...></Ref>.
    let t2 = Elem::new("", "SignedInfo", "http://www.w3.org/2000/09/xmldsig#")
        .child(Elem::new("", "Ref", "http://www.w3.org/2000/09/xmldsig#").attr("", "URI", "", "#x"));
    assert_eq!(
        t2.exc_c14n(),
        r##"<SignedInfo xmlns="http://www.w3.org/2000/09/xmldsig#"><Ref URI="#x"></Ref></SignedInfo>"##
    );

    // T3: an unused ancestor namespace (w) is dropped; apex renders only a.
    let t3 = Elem::new("a", "el", "urn:a").text("v");
    assert_eq!(t3.exc_c14n(), r#"<a:el xmlns:a="urn:a">v</a:el>"#);

    // T4: text escaping (&, <, > escaped; " left as-is).
    let t4 = Elem::new("a", "el", "urn:a").text("a < b & c > \"d\"");
    assert_eq!(t4.exc_c14n(), r#"<a:el xmlns:a="urn:a">a &lt; b &amp; c &gt; "d"</a:el>"#);
}

#[test]
fn child_does_not_rerender_an_inherited_namespace() {
    // b is rendered at the apex (used by attr), so the b:child must not re-render it.
    let e =
        Elem::new("a", "el", "urn:a").attr("b", "x", "urn:b", "1").child(Elem::new("b", "child", "urn:b").text("hi"));
    assert_eq!(e.exc_c14n(), r#"<a:el xmlns:a="urn:a" xmlns:b="urn:b" b:x="1"><b:child>hi</b:child></a:el>"#);
}
