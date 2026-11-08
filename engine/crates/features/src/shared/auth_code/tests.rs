use super::*;

fn code(text: &str) -> Option<String> {
    find_code(&[text])
}

#[test]
fn each_sentence_shape_yields_the_code() {
    let cases = [
        ("This is your verification code: 123456", "123456"),
        ("This code: 123456", "123456"),
        ("Your code is: 123456", "123456"),
        ("Your GitHub authentication code: 123456", "123456"),
        ("Enter this for your code 123456 now", "123456"),
        ("G-123456 is your Google verification code.", "123456"),
        ("Your verification code is 123456", "123456"),
        ("Use 123456 as your Microsoft account security code", "123456"),
        ("Use your code 123456 to sign in", "123456"),
        ("Code 123456", "123456"),
        ("Hello. OTP 123456 expires soon", "123456"),
    ];

    for (text, expected) in cases {
        assert_eq!(code(text).as_deref(), Some(expected), "{text}");
    }
}

#[test]
fn text_around_an_english_sentence_is_ignored() {
    assert_eq!(code("[Web발신]\n123456 is your verification code").as_deref(), Some("123456"));
    assert_eq!(code("[Web발신] [서비스] code: 123456").as_deref(), Some("123456"));
    assert_eq!(code("[Web발신] [서비스] OTP: 123456 타인에게 알리지 마세요").as_deref(), Some("123456"));
}

#[test]
fn no_keyword_means_no_code() {
    for text in [
        "PIN: 123456",
        "Your one-time password is 123456",
        "[네이버] 인증번호 [123456]를 입력해 주세요.",
        "인증번호는 [123456]입니다",
        "[Web발신] 인증코드 123456",
    ] {
        assert_eq!(code(text), None, "{text}");
    }
}

#[test]
fn false_positives_pronouns_and_shapes_that_do_not_fit_yield_nothing() {
    for text in [
        "Your access code is 123456",
        "Error code: 123456",
        "His code is 123456",
        "Your code is ABC123",
        "Your code is 123",
        "Your code is 12345678901",
        "Your verification code is [123456]",
        "code: [123456]",
        "[123456] is your verification code",
        "[Web발신] OTP 123456 입니다",
        "[Web발신] 인증번호(code): 123456",
    ] {
        assert_eq!(code(text), None, "{text}");
    }
}

#[test]
fn a_false_positive_before_the_match_drops_it() {
    assert_eq!(code("Your zip is 12345. Your code is 123456"), None);
}

#[test]
fn an_evaluator_that_matches_without_a_code_lets_the_next_one_run() {
    // The colon shape matches "my code: 1111", which names someone's own code; the "is" shape finds the
    // reader's code after it.
    assert_eq!(code("my code: 1111. your code is 123456").as_deref(), Some("123456"));

    // The later shape's match has "error code" before it, so it yields nothing either.
    assert_eq!(code("Your error code is: 1234. Your verification code is 123456"), None);
}

#[test]
fn only_the_first_three_lines_are_read() {
    assert_eq!(find_code(&["sender", "", "Your code is 123456"]).as_deref(), Some("123456"));
    assert_eq!(find_code(&["a", "b", "c", "Your code is 123456"]), None);
}

#[test]
fn line_breaks_inside_a_line_become_spaces() {
    assert_eq!(find_code(&["Your verification\ncode is 123456"]).as_deref(), Some("123456"));
}

#[test]
fn the_offered_code_is_the_number_in_parentheses() {
    assert_eq!(offered_code("복사 (123456)"), Some("123456"));
    assert_eq!(offered_code("Copy (1234)"), Some("1234"));

    for name in ["통화", "읽음으로 표시", "복사", "(123456)", "복사 (12a456)", "복사(123456)", "a (b) (123456)"]
    {
        assert_eq!(offered_code(name), None, "{name}");
    }
}

#[test]
fn a_digit_run_matches_only_whole() {
    assert!(has_digit_run("[서비스] 인증번호 [123456]를 입력", "123456"));
    assert!(!has_digit_run("1234561", "123456"));
    assert!(!has_digit_run("no digits", "123456"));
}
