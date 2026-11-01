//! A small XML tree and the exclusive canonicalization (exc-c14n) the token-broker signatures are
//! computed over, plus a plain serializer for the request body.
//!
//! Each node carries the namespace URI of its name and of every prefixed attribute, so
//! canonicalization needs no separate scope map: the "visibly utilized" namespaces of an element
//! are exactly those URIs.
//!
//! Only the constructs the broker envelopes use are supported (elements, text, attributes,
//! namespaces); there are no comments or processing instructions. Verified against `lxml`'s
//! `exclusive=True` canonicalizer.

use std::collections::BTreeMap;

/// An attribute. `prefix` is empty for an unprefixed attribute (which is in no namespace).
#[derive(Debug, Clone)]
pub struct Attr {
    pub prefix: String,
    pub local: String,
    pub uri: String,
    pub value: String,
}

/// An element, a text run, or a well-formed XML fragment embedded as-is (a token the service issued,
/// echoed byte-for-byte).
///
/// A raw fragment is never inside a signed subtree: canonicalizing one is a
/// programming error.
#[derive(Debug, Clone)]
pub enum Node {
    Elem(Elem),
    Text(String),
    Raw(String),
}

/// An element. `prefix` empty means it is in the default namespace `uri` (or in no namespace when
/// `uri` is also empty).
#[derive(Debug, Clone)]
pub struct Elem {
    pub prefix: String,
    pub local: String,
    pub uri: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
}

impl Elem {
    pub fn new(prefix: &str, local: &str, uri: &str) -> Self {
        Self { prefix: prefix.into(), local: local.into(), uri: uri.into(), attrs: Vec::new(), children: Vec::new() }
    }

    pub fn attr(mut self, prefix: &str, local: &str, uri: &str, value: &str) -> Self {
        self.attrs.push(Attr { prefix: prefix.into(), local: local.into(), uri: uri.into(), value: value.into() });
        self
    }

    pub fn child(mut self, e: Elem) -> Self {
        self.children.push(Node::Elem(e));
        self
    }

    pub fn text(mut self, t: &str) -> Self {
        self.children.push(Node::Text(t.into()));
        self
    }

    /// Appends `xml` verbatim. It must be a standalone well-formed element (every prefix it uses
    /// declared inside it).
    pub fn raw(mut self, xml: &str) -> Self {
        self.children.push(Node::Raw(xml.into()));
        self
    }

    fn tag(&self) -> String {
        if self.prefix.is_empty() { self.local.clone() } else { format!("{}:{}", self.prefix, self.local) }
    }

    /// Exclusive-canonical form of this element as a standalone apex (empty inherited output
    /// context), the form a `Reference` digest and `SignedInfo` are computed over.
    pub fn exc_c14n(&self) -> String {
        let mut out = String::new();
        self.exc_c14n_into(&mut out, &BTreeMap::new());
        out
    }

    fn exc_c14n_into(&self, out: &mut String, inherited: &BTreeMap<String, String>) {
        // Namespaces visibly used by this element: its own prefix→uri and each prefixed attribute's.
        let mut used: BTreeMap<String, String> = BTreeMap::new();
        if !self.uri.is_empty() {
            used.insert(self.prefix.clone(), self.uri.clone());
        }
        for a in &self.attrs {
            if !a.prefix.is_empty() {
                used.insert(a.prefix.clone(), a.uri.clone());
            }
        }

        // Render the used namespaces not already output identically by an ancestor; carry the rest.
        let mut rendered: Vec<(String, String)> = Vec::new();
        let mut output = inherited.clone();
        for (prefix, uri) in &used {
            if inherited.get(prefix) != Some(uri) {
                rendered.push((prefix.clone(), uri.clone()));
                output.insert(prefix.clone(), uri.clone());
            }
        }

        // Default namespace (empty prefix) first, then by prefix; BTreeMap already orders "" first.
        out.push('<');
        out.push_str(&self.tag());
        for (prefix, uri) in &rendered {
            if prefix.is_empty() {
                out.push_str(&format!(" xmlns=\"{}\"", escape_attr(uri)));
            } else {
                out.push_str(&format!(" xmlns:{}=\"{}\"", prefix, escape_attr(uri)));
            }
        }

        // Attributes sorted by (namespace uri, local name); unprefixed attrs have empty uri.
        let mut attrs = self.attrs.clone();
        attrs.sort_by(|x, y| (&x.uri, &x.local).cmp(&(&y.uri, &y.local)));
        for a in &attrs {
            let name = if a.prefix.is_empty() { a.local.clone() } else { format!("{}:{}", a.prefix, a.local) };
            out.push_str(&format!(" {}=\"{}\"", name, escape_attr(&a.value)));
        }
        out.push('>');

        for c in &self.children {
            match c {
                Node::Text(t) => out.push_str(&escape_text(t)),
                Node::Elem(e) => e.exc_c14n_into(out, &output),
                Node::Raw(_) => panic!("a raw XML fragment cannot be inside a signed element"),
            }
        }

        out.push_str(&format!("</{}>", self.tag()));
    }

    /// A well-formed serialization for the request body that declares each namespace at its point of first
    /// use (the prefixes an element and its attributes visibly use, that an ancestor has not already
    /// declared) and never declares an unused one, because the account service rejects a request whose signed
    /// elements carry the same `exc_c14n` digests but whose envelope declares every namespace on the root
    /// instead. Attributes stay in insertion order (only the signed subtrees go through the sorted
    /// [`Elem::exc_c14n`]).
    pub fn serialize_document(&self) -> String {
        let mut out = String::new();
        self.serialize_scoped(&mut out, &BTreeMap::new());
        out
    }

    /// Serializes `self` under the namespaces already in scope (`inherited`, prefix → uri, the empty
    /// prefix being the default namespace), declaring the ones it newly needs.
    fn serialize_scoped(&self, out: &mut String, inherited: &BTreeMap<String, String>) {
        // Prefixes this element visibly uses: its own and each prefixed attribute's (plus the default
        // namespace when it is unprefixed). A `BTreeMap` orders the empty prefix (default) first.
        let mut used: BTreeMap<String, String> = BTreeMap::new();
        if !self.uri.is_empty() {
            used.insert(self.prefix.clone(), self.uri.clone());
        }
        for a in &self.attrs {
            if !a.prefix.is_empty() {
                used.insert(a.prefix.clone(), a.uri.clone());
            }
        }

        let mut output = inherited.clone();
        let mut rendered: Vec<(String, String)> = Vec::new();
        for (prefix, uri) in &used {
            if inherited.get(prefix) != Some(uri) {
                rendered.push((prefix.clone(), uri.clone()));
                output.insert(prefix.clone(), uri.clone());
            }
        }

        out.push('<');
        out.push_str(&self.tag());
        for (prefix, uri) in &rendered {
            if prefix.is_empty() {
                out.push_str(&format!(" xmlns=\"{}\"", escape_attr(uri)));
            } else {
                out.push_str(&format!(" xmlns:{}=\"{}\"", prefix, escape_attr(uri)));
            }
        }

        for a in &self.attrs {
            let name = if a.prefix.is_empty() { a.local.clone() } else { format!("{}:{}", a.prefix, a.local) };
            out.push_str(&format!(" {}=\"{}\"", name, escape_attr(&a.value)));
        }
        out.push('>');

        for c in &self.children {
            match c {
                Node::Text(t) => out.push_str(&escape_text(t)),
                Node::Elem(e) => e.serialize_scoped(out, &output),
                Node::Raw(xml) => out.push_str(xml),
            }
        }

        out.push_str(&format!("</{}>", self.tag()));
    }
}

fn escape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '\r' => o.push_str("&#xD;"),
            _ => o.push(c),
        }
    }
    o
}

fn escape_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '"' => o.push_str("&quot;"),
            '\t' => o.push_str("&#x9;"),
            '\n' => o.push_str("&#xA;"),
            '\r' => o.push_str("&#xD;"),
            _ => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests;
