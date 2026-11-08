//! Authentication codes in notification text, for the Copy button on system notifications (differs from
//! Phone Link on Windows, which offers no such button).
//!
//! [`find_code`] finds an English authentication code: the text holds a keyword, a sentence of one of the
//! shapes below names the code, no false-positive term or someone else's pronoun comes with it, and the code
//! has 4 to 10 digits. Copy appears on the same English texts that get a copy button on Windows, so the
//! shapes and terms stay as they are. [`offered_code`] reads the code a messages app names in its own copy
//! action, `<label> (<digits>)`; the label follows the phone's language and is not compared.

use std::sync::LazyLock;

use regex::Regex;

/// The lines read: the first three.
const MAX_LINES: usize = 3;

/// The text holds one of these, or no code is looked for.
const KEYWORDS: [&str; 5] = ["authentication", "verification", "verify", "otp", "code"];

/// The code itself: 4 to 11 digits, or a letter, a hyphen and 4 to 11 digits.
const CODE: &str = r"([a-z][-]([0-9]){4,11}|[0-9]{4,11})";
const KEYWORD: &str = r"(authentication code|authentication|verification code|verification|confirmation code|otp|otp code|security code|passcode|code)";
/// Up to two words before the keyword.
const WORDS: &str = r"([-0-9a-z]*[ ]){0,2}";
const DETERMINER: &str = r"(the |your |his |her |their |that |my |)";

/// A match whose context or prefix holds one of these is not a code.
const FALSE_POSITIVES: [&str; 49] = [
    "microcode",
    "barcode",
    "zip-code",
    "zipcode",
    "zip",
    "decode",
    "recode",
    "morse code",
    "sourcecode",
    "source code",
    "keyboard code",
    "unicode",
    "html code",
    "assembly code",
    "error code",
    "color code",
    "genecode",
    "computer code",
    "binary code",
    "radio code",
    "fire code",
    "datacode",
    "geocode",
    "e-book code",
    "permocode",
    "country code",
    "postal code",
    "area code",
    "qr code",
    "webcode",
    "numeric code",
    "access code",
    "timecode",
    "network code",
    "exit code",
    "license code",
    "stock code",
    "user code",
    "identity code",
    "lines of code",
    "document code",
    "product code",
    "trace code",
    "enumeration code",
    "dialing code",
    "wifi code",
    "wi-fi code",
    "radio frequency code",
    "reference code",
];

/// A match whose context names someone else's code is not the reader's.
const PRONOUNS: [&str; 8] = ["his", "her", "their", "that", "my", "she", "he", "they"];

/// The shortest and longest code.
pub const MIN_DIGITS: usize = 4;
pub const MAX_DIGITS: usize = 10;

/// Which side of a split holds the code; the other side is the context.
#[derive(Clone, Copy)]
enum CodeSide {
    Left,
    Right,
}

/// How a match is cut into the code and its context.
#[derive(Clone, Copy)]
enum Split {
    /// At the first occurrence of the separator; the right side starts one character after it begins.
    At(&'static str, CodeSide),
    /// The whole match is both the code and the context.
    Whole,
}

/// One sentence shape: its forms in order, of which the first that matches is checked.
struct Evaluator {
    forms: Vec<(Regex, Split)>,
}

static EVALUATORS: LazyLock<Vec<Evaluator>> = LazyLock::new(|| {
    let form =
        |pattern: String, split: Split| (Regex::new(&pattern).expect("an evaluator pattern is a valid regex"), split);
    let one = |pattern: String, split: Split| Evaluator { forms: vec![form(pattern, split)] };
    let right = |sep| Split::At(sep, CodeSide::Right);

    vec![
        one(format!("this is {DETERMINER}{WORDS}{KEYWORD}([-0-9a-z ]*):([ ]*){CODE}"), right(":")),
        one(format!("this {WORDS}{KEYWORD}([-0-9a-z ]*):([ ]*){CODE}"), right(":")),
        one(format!("{DETERMINER}{WORDS}{KEYWORD} is[ ]*:[ ]*{CODE}"), right(" is ")),
        one(format!(r"([ ]*)(\([-0-9a-z ]+\)){{0,1}}{DETERMINER}{WORDS}{KEYWORD}([ ]*:[ ]*){CODE}"), right(":")),
        one(format!("([-0-9a-z ]*) for (this |the |your ){KEYWORD} {CODE}"), right(" for ")),
        one(format!("{CODE} is {DETERMINER}{WORDS}{KEYWORD}"), Split::At(" is ", CodeSide::Left)),
        one(format!("{DETERMINER}{WORDS}{KEYWORD} is {CODE}"), right(" is ")),
        Evaluator {
            forms: vec![
                form(format!("use {CODE} as {DETERMINER}{WORDS}{KEYWORD}"), Split::At(" as ", CodeSide::Left)),
                form(format!("use {DETERMINER}{WORDS}{KEYWORD} {CODE}"), Split::Whole),
            ],
        },
        one(format!(r"(^|\n|\.\s+){KEYWORD} {CODE}."), Split::Whole),
    ]
});

static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("a valid regex"));
static DIGITS_IN_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new("([0-9]){4,11}").expect("a valid regex"));
static DIGIT_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new("[0-9]+").expect("a valid regex"));
static OFFER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^()]+ \(([0-9]+)\)$").expect("the offer pattern is a valid regex"));

/// The authentication code in a notification's text, `lines` in display order (only the first three are
/// read); `None` when the text holds none.
pub fn find_code(lines: &[&str]) -> Option<String> {
    let text = prepared(lines);
    if !KEYWORDS.iter().any(|k| text.contains(k)) {
        return None;
    }

    EVALUATORS.iter().find_map(|e| evaluate(e, &text))
}

/// The digits a copy action names, `<label> (<digits>)` with a non-empty label and no parenthesis in it;
/// `None` for any other action name.
pub fn offered_code(action_name: &str) -> Option<&str> {
    OFFER.captures(action_name).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// The runs of digits in `text`, in order.
pub fn digit_runs(text: &str) -> impl Iterator<Item = &str> {
    DIGIT_RUN.find_iter(text).map(|m| m.as_str())
}

/// `text` has `code` as one whole run of digits.
pub fn has_digit_run(text: &str, code: &str) -> bool {
    digit_runs(text).any(|run| run == code)
}

/// The lines cleaned, joined and lower-cased: line breaks become spaces, whitespace runs one space, each
/// non-empty line is followed by `\n`, and only `A`–`Z` change case.
fn prepared(lines: &[&str]) -> String {
    let mut text = String::new();
    for line in lines.iter().take(MAX_LINES) {
        let line = line.replace(['\r', '\n'], " ");
        let line = WHITESPACE.replace_all(&line, " ");
        if !line.is_empty() {
            text.push_str(&line);
            text.push('\n');
        }
    }

    text.make_ascii_lowercase();
    text
}

/// The code of the first form of `evaluator` that matches, when the match passes the checks.
fn evaluate(evaluator: &Evaluator, text: &str) -> Option<String> {
    let (m, split) = evaluator.forms.iter().find_map(|(re, split)| re.find(text).map(|m| (m, *split)))?;
    let whole = m.as_str();
    let prefix = &text[..m.start()];

    let (code, context) = match split {
        Split::Whole => (whole, whole),
        Split::At(sep, side) => {
            let (left, right) = match whole.find(sep) {
                Some(i) => (&whole[..i], &whole[i + 1..]),
                None => (whole, whole),
            };
            match side {
                CodeSide::Left => (left, right),
                CodeSide::Right => (right, left),
            }
        }
    };

    checked(code, context, prefix)
}

/// The code `code` holds, unless the context or the prefix marks the match as no authentication code.
fn checked(code: &str, context: &str, prefix: &str) -> Option<String> {
    if has_false_positive(context) || has_pronoun(context) {
        return None;
    }
    if !prefix.is_empty() && has_false_positive(prefix) {
        return None;
    }

    let digits = DIGITS_IN_CODE.find(code)?.as_str();
    (MIN_DIGITS..=MAX_DIGITS).contains(&digits.len()).then(|| digits.to_string())
}

fn has_false_positive(text: &str) -> bool {
    FALSE_POSITIVES.iter().any(|term| text.contains(term))
}

/// One of the pronouns as a word: at the start followed by a space, or between spaces.
fn has_pronoun(text: &str) -> bool {
    PRONOUNS.iter().any(|w| text.starts_with(&format!("{w} ")) || text.contains(&format!(" {w} ")))
}

#[cfg(test)]
mod tests;
