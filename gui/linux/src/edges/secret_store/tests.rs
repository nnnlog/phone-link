use super::*;

#[test]
fn one_item_holds_the_store() {
    assert_eq!(only_item(Vec::<u8>::new()).unwrap(), None);
    assert_eq!(only_item([7]).unwrap(), Some(7));
    assert!(only_item([7, 8]).is_err());
}

#[test]
fn key_map_round_trips() {
    let map = KeyMap::from([("auth/key".to_owned(), vec![0, 1, 255])]);
    assert_eq!(decode_map(&encode_map(&map)).unwrap(), map);
}
