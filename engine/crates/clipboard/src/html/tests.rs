use super::*;

#[test]
fn text_nodes_verbatim_in_document_order() {
    assert_eq!(plain_text("<p>Hello <b>world</b></p><p>again</p>"), "Hello worldagain");
    assert_eq!(plain_text("a<br>b<br/>c<div>d</div>"), "abcd", "nothing inserted for <br> or blocks");
    assert_eq!(plain_text("<p>  two  spaces\n\tand a tab </p>"), "  two  spaces\n\tand a tab ");
    assert_eq!(plain_text("<html><head><title>T</title></head><body>B</body></html>"), "TB");
    assert_eq!(plain_text("plain"), "plain");
    assert_eq!(plain_text(""), "");
}

#[test]
fn script_style_and_comments_are_excluded() {
    assert_eq!(plain_text("a<script>var x = '<b>';</script>b"), "ab");
    assert_eq!(plain_text("a<STYLE type=\"text/css\">p { color: red }</Style >b"), "ab");
    assert_eq!(plain_text("a<!-- hidden <b>x</b> -->b"), "ab");
    assert_eq!(plain_text("a<!---->b<!-->c"), "abc", "<!--> closes at once");
    assert_eq!(plain_text("<!DOCTYPE html>a<?xml version=\"1.0\"?>b"), "ab");
    assert_eq!(plain_text("a<script src=x.js />b"), "ab", "a self-closed script has no content");
    // `</scripts>` does not close a script.
    assert_eq!(plain_text("a<script>x</scripts>y</script>b"), "ab");
}

#[test]
fn an_unclosed_script_keeps_its_text() {
    assert_eq!(plain_text("a<script>var x = 1; <b>y</b>"), "avar x = 1; <b>y</b>");
    assert_eq!(plain_text("a<style>p{}"), "ap{}");
}

#[test]
fn markup_edges() {
    assert_eq!(plain_text("1 < 2 and 3 <4"), "1 < 2 and 3 <4", "< without a letter is text");
    assert_eq!(plain_text("<a title=\"x > y\" href='z'>link</a>"), "link", "quoted values may hold >");
    assert_eq!(plain_text("a</>b</ x>c"), "abc");
    assert_eq!(plain_text("a<b"), "a", "markup cut off by the end is dropped");
    assert_eq!(plain_text("a<!-- open"), "a");
    assert_eq!(plain_text("<p class=a\"b>x</p>"), "x", "a quote inside an unquoted value is a character");
}

#[test]
fn hostile_input_stays_linear() {
    // A tag full of quotes, unterminated references and stray `<`: each is scanned once.
    let quotes = format!("<a {}>x", "\"'".repeat(200_000));
    assert_eq!(plain_text(&quotes), "x");

    let refs = "&#1".repeat(200_000);
    assert_eq!(html_decode(&refs), refs);

    let lts = "< ".repeat(200_000);
    assert_eq!(plain_text(&lts), lts);

    let ends = format!("<script>{}", "</s".repeat(200_000));
    assert_eq!(plain_text(&ends), "</s".repeat(200_000), "an unclosed script keeps its text");
}

#[test]
fn references_are_decoded_after_extraction() {
    assert_eq!(plain_text("<p>a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;</p>"), "a & b <c> \"d\" 'e'");
    assert_eq!(plain_text("&lt;b&gt;not a tag&lt;/b&gt;"), "<b>not a tag</b>");
    assert_eq!(plain_text("x&nbsp;y &eacute;&Eacute; &euro;&hearts;"), "x\u{a0}y éÉ €♥");
}

#[test]
fn the_entity_table_is_html_4_plus_apos() {
    let mut names: Vec<&str> = ENTITIES.iter().map(|(n, _)| *n).collect();
    assert_eq!(names.len(), 253);
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 253, "unique names");
    assert!(ENTITIES.iter().all(|&(_, code)| char::from_u32(code).is_some()));
}

#[test]
fn decoding_follows_web_utility() {
    assert_eq!(html_decode("&#65;&#x42;&#X43;&#x1F600;"), "ABC\u{1F600}");
    assert_eq!(html_decode("&# 65 ;&#+66;&#-0;"), "AB\0", "sign and white space in a numeric reference");
    assert_eq!(html_decode("&#-1;&#xD800;&#x110000;&#abc;&#;&#x;"), "&#-1;&#xD800;&#x110000;&#abc;&#;&#x;");
    assert_eq!(html_decode("&#4294967296;"), "&#4294967296;", "overflow");
    assert_eq!(html_decode("&unknown; &AMP; &amp"), "&unknown; &AMP; &amp", "case-sensitive, ; required");
    assert_eq!(html_decode("&a&amp;"), "&a&", "an & before the ; makes the first literal");
    assert_eq!(html_decode("&hellip;&thetasym;&notin;"), "…ϑ∉");
    assert_eq!(html_decode("no references"), "no references");
    assert_eq!(html_decode("&&&"), "&&&");
}
