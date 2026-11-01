use super::*;
use serde_json::json;

#[test]
fn put_nests_and_reads_back() {
    let mut root = Map::new();
    put(&mut root, "ice.stun.url", leaf(""));
    put(&mut root, "ice.turn.url", leaf("t"));
    put(&mut root, "security.key", leaf("00"));

    let v = Value::Object(root);
    assert_eq!(v, json!({"ice":{"stun":{"url":""},"turn":{"url":"t"}},"security":{"key":"00"}}));
    assert_eq!(required_text(&v, "ice.turn.url").unwrap(), "t");
    assert_eq!(text(&v, "ice.nothing").unwrap(), None);
}

#[test]
fn scalars_read_as_text() {
    let v = json!({"a":"5000","b":5000,"c":true,"d":"1","e":"true","f":{"x":"1"}});

    assert_eq!(required_uint(&v, "a").unwrap(), 5000);
    assert_eq!(required_uint(&v, "b").unwrap(), 5000);
    assert_eq!(boolean(&v, "c").unwrap(), Some(true));
    assert_eq!(boolean(&v, "d").unwrap(), Some(true));
    assert!(boolean(&v, "e").is_err(), "\"true\" is not a contract boolean");
    assert!(text(&v, "f").is_err());
    assert!(matches!(required_uint(&v, "z"), Err(PtreeError::Missing(_))));
}
