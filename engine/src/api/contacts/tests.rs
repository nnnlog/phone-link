use super::*;

fn contact(id: i64, name: &str, numbers: &[(&str, i32)]) -> Vec<ContactNumber> {
    numbers
        .iter()
        .map(|(n, kind)| ContactNumber {
            contact_id: id,
            display_name: name.into(),
            has_thumbnail: false,
            number: n.to_string(),
            display_number: String::new(),
            kind: *kind,
            label: String::new(),
        })
        .collect()
}

fn run(book: &[Vec<ContactNumber>], query: &str, limit: usize) -> Vec<(String, String)> {
    suggest(book.concat(), query, limit).into_iter().map(|s| (s.name, s.number)).collect()
}

#[test]
fn names_match_case_insensitively_and_numbers_by_their_digits() {
    let book = [
        contact(1, "Alex Kim", &[("+82 10-1111-2222", 2), ("02-333-4444", 3)]),
        contact(2, "김민지", &[("010-5555-6666", 2)]),
        contact(3, "Sarah", &[("+1 (415) 555-0100", 2)]),
    ];

    assert_eq!(
        run(&book, "alex", 10),
        vec![("Alex Kim".into(), "+82 10-1111-2222".into()), ("Alex Kim".into(), "02-333-4444".into())]
    );
    assert_eq!(run(&book, "민지", 10), vec![("김민지".into(), "010-5555-6666".into())]);
    assert_eq!(run(&book, "5550", 10), vec![("Sarah".into(), "+1 (415) 555-0100".into())], "digits only");
    assert_eq!(run(&book, "zzz", 10), Vec::<(String, String)>::new());
    assert_eq!(run(&book, "", 2).len(), 2, "an empty query lists every number up to the limit");
}

#[test]
fn number_kinds_map_from_the_stored_values() {
    assert_eq!(number_kind(2), PhoneNumberKind::Mobile);
    assert_eq!(number_kind(8), PhoneNumberKind::Custom);
    assert_eq!(number_kind(99), PhoneNumberKind::Unknown);
}
