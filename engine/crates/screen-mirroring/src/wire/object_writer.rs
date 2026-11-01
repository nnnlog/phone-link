//! A JSON object writer that keeps key order.

/// How a writer escapes strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escaping {
    /// Only `"`, `\` and control characters.
    Minimal,
    /// The escaping Phone Link on Windows writes these bodies with: also every non-ASCII character and the
    /// HTML-sensitive `"&'+<>` and backtick, as `\uXXXX` (upper-case hex, UTF-16 units).
    Strict,
}

/// A compact JSON object written key by key, in the order given.
pub struct Obj {
    out: String,
    escaping: Escaping,
    empty: bool,
}

impl Obj {
    pub fn new(escaping: Escaping) -> Self {
        Self { out: "{".into(), escaping, empty: true }
    }

    fn key(&mut self, k: &str) {
        if !self.empty {
            self.out.push(',');
        }
        self.empty = false;
        write_string(&mut self.out, k, self.escaping);
        self.out.push(':');
    }

    pub fn str(mut self, k: &str, v: &str) -> Self {
        self.key(k);
        write_string(&mut self.out, v, self.escaping);
        self
    }

    #[cfg(feature = "test-support")]
    pub fn opt_str(mut self, k: &str, v: Option<&str>) -> Self {
        match v {
            Some(v) => self.str(k, v),
            None => {
                self.key(k);
                self.out.push_str("null");
                self
            }
        }
    }

    pub fn num(mut self, k: &str, v: i64) -> Self {
        self.key(k);
        self.out.push_str(&v.to_string());
        self
    }

    pub fn bool(mut self, k: &str, v: bool) -> Self {
        self.key(k);
        self.out.push_str(if v { "true" } else { "false" });
        self
    }

    /// `v` written as it is (already JSON).
    pub fn raw(mut self, k: &str, v: &str) -> Self {
        self.key(k);
        self.out.push_str(v);
        self
    }

    pub fn finish(mut self) -> String {
        self.out.push('}');
        self.out
    }

    pub fn bytes(self) -> Vec<u8> {
        self.finish().into_bytes()
    }
}

pub(super) fn write_string(out: &mut String, s: &str, escaping: Escaping) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '"' if escaping == Escaping::Minimal => out.push_str("\\\""),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            '"' | '&' | '\'' | '+' | '<' | '>' | '`' if escaping == Escaping::Strict => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c if escaping == Escaping::Strict && (c as u32) > 0x7E => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04X}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
