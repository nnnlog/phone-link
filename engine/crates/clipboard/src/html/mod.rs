//! The plain text written beside HTML received from the phone (`TEXT_HTML` content).
//!
//! It covers the text of every text node in document order, kept verbatim (whitespace unchanged),
//! without the text inside `<script>` and `<style>` elements and comments, nothing inserted for
//! `<br>` or block boundaries; then the character references decoded.
//!
//! A small tokenizer is enough for that rule (no DOM is built). Its edge rules, which the rule
//! above leaves open:
//! - `<` starts markup only when followed by an ASCII letter (a start tag), `/` (an end tag), `!` or
//!   `?`; any other `<` is text.
//! - `<!--` opens a comment that ends at the first `-->` after `<!` (so `<!-->` is an empty comment);
//!   any other `<!…>` or `<?…>` (doctype, CDATA, processing instruction) is skipped to the next `>`.
//! - Attribute values in quotes may contain `>`.
//! - A `<script>` or `<style>` element's content is raw text up to its end tag (ASCII
//!   case-insensitive); a closed one is excluded, an unclosed one keeps its text to the end of the
//!   document. A self-closed `<script/>` has no content.
//! - Markup cut off by the end of the document is dropped.
//!
//! Character references decoded: only `&…;` with no other `&` before the `;`; `&#N;` (decimal,
//! surrounding white space and a sign allowed) and `&#xH;` (hex) for U+0000–U+10FFFF without
//! surrogates; and the 253 HTML 4.01 names plus `&apos;`, case-sensitive. Anything else stays as
//! written.

/// The derived plain text of `html`.
pub fn plain_text(html: &str) -> String {
    html_decode(&inner_text(html))
}

/// The text nodes of `html` concatenated (before decoding).
pub fn inner_text(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    // `text_start..i` is text not yet copied; markup boundaries are ASCII, so slices stay on char
    // boundaries.
    let mut text_start = 0;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        let next = bytes.get(i + 1).copied();
        let markup_end = match next {
            Some(b'!') if bytes[i + 2..].starts_with(b"--") => find(bytes, i + 2, b"-->").map(|p| p + 3),
            Some(b'!' | b'?') => find(bytes, i + 2, b">").map(|p| p + 1),
            Some(b'/') if bytes.get(i + 2).is_some_and(u8::is_ascii_alphabetic) => tag_end(bytes, i + 2),
            // `</>` and `</` + anything else: skipped to the next `>`.
            Some(b'/') => find(bytes, i + 2, b">").map(|p| p + 1),
            Some(c) if c.is_ascii_alphabetic() => {
                out.push_str(&html[text_start..i]);
                let Some(end) = tag_end(bytes, i + 1) else {
                    return out;
                };
                let name = tag_name(bytes, i + 1);
                let self_closed = bytes[..end - 1].ends_with(b"/");
                let raw = name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style");

                if raw && !self_closed {
                    match find_end_tag(bytes, end, name) {
                        // Closed: its text is excluded.
                        Some(close_end) => {
                            i = close_end;
                            text_start = close_end;
                            continue;
                        }
                        // Unclosed: its text stays in, to the end of the document.
                        None => {
                            out.push_str(&html[end..]);
                            return out;
                        }
                    }
                }
                i = end;
                text_start = end;
                continue;
            }
            // A `<` that starts no markup is text.
            _ => {
                i += 1;
                continue;
            }
        };

        out.push_str(&html[text_start..i]);
        match markup_end {
            Some(end) => {
                i = end;
                text_start = end;
            }
            None => return out,
        }
    }

    out.push_str(&html[text_start..]);
    out
}

/// The first `needle` at or after `from`.
fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| from + p)
}

/// The tag name starting at `from`: up to white space, `/` or `>`.
fn tag_name(bytes: &[u8], from: usize) -> &[u8] {
    let len = bytes[from..].iter().position(|&c| c.is_ascii_whitespace() || c == b'/' || c == b'>');
    &bytes[from..from + len.unwrap_or(bytes.len() - from)]
}

/// The index after the `>` that closes the tag whose name starts at `from`; quoted attribute values
/// may contain `>`. `None` when the document ends first.
fn tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    // The last byte outside a quoted value that is not white space.
    let mut last: Option<u8> = None;
    for (i, &c) in bytes.iter().enumerate().skip(from) {
        match quote {
            Some(q) if c == q => {
                quote = None;
                last = Some(c);
            }
            Some(_) => {}
            None if c == b'>' => return Some(i + 1),
            // A quote opens a value only right after `=` (possibly with white space between).
            None if (c == b'"' || c == b'\'') && last == Some(b'=') => quote = Some(c),
            None if c.is_ascii_whitespace() => {}
            None => last = Some(c),
        }
    }
    None
}

/// The index after the end tag `</name …>` of a raw-text element whose content starts at `from`.
fn find_end_tag(bytes: &[u8], from: usize, name: &[u8]) -> Option<usize> {
    let mut i = from;
    while let Some(p) = find(bytes, i, b"</") {
        let after = p + 2;
        let candidate = bytes.get(after..after + name.len());
        let boundary = bytes.get(after + name.len()).copied();
        let matches = candidate.is_some_and(|c| c.eq_ignore_ascii_case(name))
            && matches!(boundary, None | Some(b'>' | b'/') | Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'));
        if matches {
            return Some(find(bytes, after, b">").map_or(bytes.len(), |e| e + 1));
        }
        i = after;
    }
    None
}

/// Decodes character references (see the module header).
pub fn html_decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }

    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            i += 1;
            continue;
        }

        // The next `;` or `&`: an `&` first means this one is literal.
        let Some(rel) = bytes[i + 1..].iter().position(|&c| c == b';' || c == b'&') else {
            break;
        };
        let semicolon = i + 1 + rel;
        if bytes[semicolon] != b';' {
            i += 1;
            continue;
        }

        let entity = &text[i + 1..semicolon];
        let decoded = if entity.len() > 1 && entity.starts_with('#') { numeric(&entity[1..]) } else { named(entity) };
        match decoded {
            Some(c) => {
                out.push_str(&text[copied..i]);
                out.push(c);
                i = semicolon + 1;
                copied = i;
            }
            // An unknown name is kept with its `;`; an unparsable number leaves the `&` literal and
            // the rest is scanned again.
            None if entity.starts_with('#') && entity.len() > 1 => i += 1,
            None => i = semicolon + 1,
        }
    }

    out.push_str(&text[copied..]);
    out
}

/// `&#…;` after the `#`: `x`/`X` + hex digits, or a decimal integer with optional surrounding white
/// space and sign; U+0000–U+10FFFF without surrogates.
fn numeric(digits: &str) -> Option<char> {
    let value = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    } else {
        // White space around the number: U+0009–U+000D and U+0020.
        let trimmed = digits.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' '));
        let (negative, unsigned) = match trimmed.as_bytes().first() {
            Some(b'+') => (false, &trimmed[1..]),
            Some(b'-') => (true, &trimmed[1..]),
            _ => (false, trimmed),
        };
        if unsigned.is_empty() || !unsigned.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value: u32 = unsigned.parse().ok()?;
        if negative && value != 0 {
            return None;
        }
        value
    };
    char::from_u32(value)
}

/// The HTML 4.01 character entity names and `apos` (case-sensitive).
const ENTITIES: &[(&str, u32)] = &[
    ("quot", 34),
    ("amp", 38),
    ("apos", 39),
    ("lt", 60),
    ("gt", 62),
    ("nbsp", 160),
    ("iexcl", 161),
    ("cent", 162),
    ("pound", 163),
    ("curren", 164),
    ("yen", 165),
    ("brvbar", 166),
    ("sect", 167),
    ("uml", 168),
    ("copy", 169),
    ("ordf", 170),
    ("laquo", 171),
    ("not", 172),
    ("shy", 173),
    ("reg", 174),
    ("macr", 175),
    ("deg", 176),
    ("plusmn", 177),
    ("sup2", 178),
    ("sup3", 179),
    ("acute", 180),
    ("micro", 181),
    ("para", 182),
    ("middot", 183),
    ("cedil", 184),
    ("sup1", 185),
    ("ordm", 186),
    ("raquo", 187),
    ("frac14", 188),
    ("frac12", 189),
    ("frac34", 190),
    ("iquest", 191),
    ("Agrave", 192),
    ("Aacute", 193),
    ("Acirc", 194),
    ("Atilde", 195),
    ("Auml", 196),
    ("Aring", 197),
    ("AElig", 198),
    ("Ccedil", 199),
    ("Egrave", 200),
    ("Eacute", 201),
    ("Ecirc", 202),
    ("Euml", 203),
    ("Igrave", 204),
    ("Iacute", 205),
    ("Icirc", 206),
    ("Iuml", 207),
    ("ETH", 208),
    ("Ntilde", 209),
    ("Ograve", 210),
    ("Oacute", 211),
    ("Ocirc", 212),
    ("Otilde", 213),
    ("Ouml", 214),
    ("times", 215),
    ("Oslash", 216),
    ("Ugrave", 217),
    ("Uacute", 218),
    ("Ucirc", 219),
    ("Uuml", 220),
    ("Yacute", 221),
    ("THORN", 222),
    ("szlig", 223),
    ("agrave", 224),
    ("aacute", 225),
    ("acirc", 226),
    ("atilde", 227),
    ("auml", 228),
    ("aring", 229),
    ("aelig", 230),
    ("ccedil", 231),
    ("egrave", 232),
    ("eacute", 233),
    ("ecirc", 234),
    ("euml", 235),
    ("igrave", 236),
    ("iacute", 237),
    ("icirc", 238),
    ("iuml", 239),
    ("eth", 240),
    ("ntilde", 241),
    ("ograve", 242),
    ("oacute", 243),
    ("ocirc", 244),
    ("otilde", 245),
    ("ouml", 246),
    ("divide", 247),
    ("oslash", 248),
    ("ugrave", 249),
    ("uacute", 250),
    ("ucirc", 251),
    ("uuml", 252),
    ("yacute", 253),
    ("thorn", 254),
    ("yuml", 255),
    ("OElig", 338),
    ("oelig", 339),
    ("Scaron", 352),
    ("scaron", 353),
    ("Yuml", 376),
    ("fnof", 402),
    ("circ", 710),
    ("tilde", 732),
    ("Alpha", 913),
    ("Beta", 914),
    ("Gamma", 915),
    ("Delta", 916),
    ("Epsilon", 917),
    ("Zeta", 918),
    ("Eta", 919),
    ("Theta", 920),
    ("Iota", 921),
    ("Kappa", 922),
    ("Lambda", 923),
    ("Mu", 924),
    ("Nu", 925),
    ("Xi", 926),
    ("Omicron", 927),
    ("Pi", 928),
    ("Rho", 929),
    ("Sigma", 931),
    ("Tau", 932),
    ("Upsilon", 933),
    ("Phi", 934),
    ("Chi", 935),
    ("Psi", 936),
    ("Omega", 937),
    ("alpha", 945),
    ("beta", 946),
    ("gamma", 947),
    ("delta", 948),
    ("epsilon", 949),
    ("zeta", 950),
    ("eta", 951),
    ("theta", 952),
    ("iota", 953),
    ("kappa", 954),
    ("lambda", 955),
    ("mu", 956),
    ("nu", 957),
    ("xi", 958),
    ("omicron", 959),
    ("pi", 960),
    ("rho", 961),
    ("sigmaf", 962),
    ("sigma", 963),
    ("tau", 964),
    ("upsilon", 965),
    ("phi", 966),
    ("chi", 967),
    ("psi", 968),
    ("omega", 969),
    ("thetasym", 977),
    ("upsih", 978),
    ("piv", 982),
    ("ensp", 8194),
    ("emsp", 8195),
    ("thinsp", 8201),
    ("zwnj", 8204),
    ("zwj", 8205),
    ("lrm", 8206),
    ("rlm", 8207),
    ("ndash", 8211),
    ("mdash", 8212),
    ("lsquo", 8216),
    ("rsquo", 8217),
    ("sbquo", 8218),
    ("ldquo", 8220),
    ("rdquo", 8221),
    ("bdquo", 8222),
    ("dagger", 8224),
    ("Dagger", 8225),
    ("bull", 8226),
    ("hellip", 8230),
    ("permil", 8240),
    ("prime", 8242),
    ("Prime", 8243),
    ("lsaquo", 8249),
    ("rsaquo", 8250),
    ("oline", 8254),
    ("frasl", 8260),
    ("euro", 8364),
    ("image", 8465),
    ("weierp", 8472),
    ("real", 8476),
    ("trade", 8482),
    ("alefsym", 8501),
    ("larr", 8592),
    ("uarr", 8593),
    ("rarr", 8594),
    ("darr", 8595),
    ("harr", 8596),
    ("crarr", 8629),
    ("lArr", 8656),
    ("uArr", 8657),
    ("rArr", 8658),
    ("dArr", 8659),
    ("hArr", 8660),
    ("forall", 8704),
    ("part", 8706),
    ("exist", 8707),
    ("empty", 8709),
    ("nabla", 8711),
    ("isin", 8712),
    ("notin", 8713),
    ("ni", 8715),
    ("prod", 8719),
    ("sum", 8721),
    ("minus", 8722),
    ("lowast", 8727),
    ("radic", 8730),
    ("prop", 8733),
    ("infin", 8734),
    ("ang", 8736),
    ("and", 8743),
    ("or", 8744),
    ("cap", 8745),
    ("cup", 8746),
    ("int", 8747),
    ("there4", 8756),
    ("sim", 8764),
    ("cong", 8773),
    ("asymp", 8776),
    ("ne", 8800),
    ("equiv", 8801),
    ("le", 8804),
    ("ge", 8805),
    ("sub", 8834),
    ("sup", 8835),
    ("nsub", 8836),
    ("sube", 8838),
    ("supe", 8839),
    ("oplus", 8853),
    ("otimes", 8855),
    ("perp", 8869),
    ("sdot", 8901),
    ("lceil", 8968),
    ("rceil", 8969),
    ("lfloor", 8970),
    ("rfloor", 8971),
    ("lang", 9001),
    ("rang", 9002),
    ("loz", 9674),
    ("spades", 9824),
    ("clubs", 9827),
    ("hearts", 9829),
    ("diams", 9830),
];

fn named(name: &str) -> Option<char> {
    ENTITIES.iter().find(|(n, _)| *n == name).and_then(|&(_, code)| char::from_u32(code))
}

#[cfg(test)]
mod tests;
