use super::*;

#[test]
fn rejection_text_form() {
    assert_eq!(rejection_message("duplicate notification item"), "duplicate notification item 80004005");
    let r = Rejection::new("missing key");
    assert_eq!(r.answer, ValueSet::new().with("result", 1i32));
    assert_eq!(r.message_text(), "missing key 80004005");
}
