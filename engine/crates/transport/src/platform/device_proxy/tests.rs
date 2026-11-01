use super::*;

#[test]
fn reply_body_shape() {
    let body = reply_body(&[1, 2, 3]);

    let outer: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(outer["type"], "SendCommandResponse");

    let inner: serde_json::Value = serde_json::from_str(outer["value"].as_str().unwrap()).unwrap();
    assert_eq!(inner["responseType"], 1);
    assert_eq!(inner["payload"], "AQID");

    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"{"type":"SendCommandResponse","value":"{\"responseType\":1,\"payload\":\"AQID\"}"}"#
    );
}

#[test]
fn enum_values() {
    let table = REQUEST_TYPE_NAMES;

    assert_eq!(enum_value(&Some(EnumValue::Number(2)), &table), Some(2));
    assert_eq!(enum_value(&Some(EnumValue::Name("get".into())), &table), Some(1));
    assert_eq!(enum_value(&Some(EnumValue::Name("Sync".into())), &table), Some(4));
    assert_eq!(enum_value(&Some(EnumValue::Name("Fetch".into())), &table), None);
    assert_eq!(enum_value(&None, &table), Some(0));
    assert_eq!(enum_value(&Some(EnumValue::Number(i64::MAX)), &table), None);
}
