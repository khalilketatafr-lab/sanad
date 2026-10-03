//! A small namespace-resolved XML tree for EPUB's package, navigation and
//! content documents.
//!
//! Built on `quick-xml`, which never fetches external entities or expands
//! DTD declarations: an EPUB cannot make Atelier read a file or grow a
//! billion laughs. Only the five XML entities and character references are
//! resolved. HTML named entities (`&nbsp;`) are not XML and are reported.

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

use super::diag::line_of;

pub const XHTML: &str = "http://www.w3.org/1999/xhtml";
pub const OPS: &str = "http://www.idpf.org/2007/ops";
pub const OPF: &str = "http://www.idpf.org/2007/opf";
pub const DC: &str = "http://purl.org/dc/elements/1.1/";
pub const CONTAINER: &str = "urn:oasis:names:tc:opendocument:xmlns:container";
pub const XMLENC: &str = "http://www.w3.org/2001/04/xmlenc#";
pub const XML: &str = "http://www.w3.org/XML/1998/namespace";
pub const SVG: &str = "http://www.w3.org/2000/svg";
pub const MATHML: &str = "http://www.w3.org/1998/Math/MathML";

/// Deeper nesting than this is refused (a hostile document could exhaust the
/// stack of any recursive walker).
pub const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub ns: Option<String>,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub ns: Option<String>,
    /// Local name.
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
    /// 1-based line of the start tag.
    pub line: u32,
}

impl Element {
    #[must_use]
    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.ns.as_deref() == Some(ns) && self.name == name
    }

    /// An attribute in no namespace.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.is_none() && a.name == name)
            .map(|a| a.value.as_str())
    }

    #[must_use]
    pub fn attr_ns(&self, ns: &str, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.as_deref() == Some(ns) && a.name == name)
            .map(|a| a.value.as_str())
    }

    /// Whitespace-separated tokens of an attribute (`class`, `epub:type`).
    pub fn tokens<'a>(&'a self, ns: Option<&str>, name: &str) -> impl Iterator<Item = &'a str> {
        let v = match ns {
            Some(ns) => self.attr_ns(ns, name),
            None => self.attr(name),
        };
        v.unwrap_or("").split_ascii_whitespace()
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// First descendant (depth-first, self excluded) matching `pred`.
    pub fn find(&self, pred: &impl Fn(&Element) -> bool) -> Option<&Element> {
        for e in self.elements() {
            if pred(e) {
                return Some(e);
            }
            if let Some(f) = e.find(pred) {
                return Some(f);
            }
        }
        None
    }

    /// All descendant text, concatenated.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for c in &self.children {
            match c {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.collect_text(out),
            }
        }
    }
}

/// A well-formedness failure: parsing stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: u32,
    pub message: String,
}

/// A recoverable problem: the document parsed, with this text dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEntity {
    pub line: u32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub root: Element,
    pub unknown_entities: Vec<UnknownEntity>,
}

fn ns_of(r: &ResolveResult<'_>) -> Option<String> {
    match r {
        ResolveResult::Bound(ns) => Some(ns.0.to_owned()),
        ResolveResult::Unbound | ResolveResult::Unknown(_) => None,
    }
}

fn predefined(name: &str) -> Option<char> {
    Some(match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "apos" => '\'',
        "quot" => '"',
        _ => return None,
    })
}

fn push_text(stack: &mut [Element], text: &str) {
    let Some(top) = stack.last_mut() else {
        return; // text outside the root element: whitespace or junk, ignored
    };
    if let Some(Node::Text(t)) = top.children.last_mut() {
        t.push_str(text);
    } else {
        top.children.push(Node::Text(text.to_owned()));
    }
}

#[allow(clippy::too_many_lines)]
pub fn parse(text: &str) -> Result<Document, SyntaxError> {
    let mut reader = NsReader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut unknown_entities = Vec::new();
    let err = |position: u64, message: String| SyntaxError {
        line: line_of(text, usize::try_from(position).unwrap_or(usize::MAX)),
        message,
    };
    loop {
        let pos = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let (resolved, event) = match reader.read_resolved_event() {
            Ok((r, e)) => (ns_of(&r), e),
            Err(e) => return Err(err(reader.error_position(), e.to_string())),
        };
        let line = line_of(text, pos);
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if root.is_some() {
                    return Err(SyntaxError {
                        line,
                        message: "content after the root element".into(),
                    });
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(SyntaxError {
                        line,
                        message: format!("nesting deeper than {MAX_DEPTH} elements"),
                    });
                }
                let mut attrs = Vec::new();
                for a in e.attributes() {
                    let a = a.map_err(|x| SyntaxError {
                        line,
                        message: x.to_string(),
                    })?;
                    if a.key.as_ref() == "xmlns" || a.key.as_ref().starts_with("xmlns:") {
                        continue;
                    }
                    let (r, local) = reader.resolver().resolve_attribute(a.key);
                    let value =
                        a.normalized_value(XmlVersion::Implicit1_0)
                            .map_err(|x| SyntaxError {
                                line,
                                message: format!("attribute {}: {x}", a.key.as_ref()),
                            })?;
                    attrs.push(Attr {
                        ns: ns_of(&r),
                        name: local.into_inner().to_owned(),
                        value: value.into_owned(),
                    });
                }
                let el = Element {
                    ns: resolved,
                    name: e.local_name().into_inner().to_owned(),
                    attrs,
                    children: Vec::new(),
                    line,
                };
                if matches!(event, Event::Empty(_)) {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Node::Element(el)),
                        None => root = Some(el),
                    }
                } else {
                    stack.push(el);
                }
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else {
                    return Err(SyntaxError {
                        line,
                        message: "unmatched end tag".into(),
                    });
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(done)),
                    None => root = Some(done),
                }
            }
            Event::Text(t) => push_text(&mut stack, &t.xml10_content()),
            Event::CData(c) => push_text(&mut stack, &c.xml10_content()),
            Event::GeneralRef(r) => {
                let resolved = r
                    .resolve_char_ref()
                    .map_err(|x| SyntaxError {
                        line,
                        message: x.to_string(),
                    })?
                    .or_else(|| predefined(&r));
                match resolved {
                    Some(c) => push_text(&mut stack, c.encode_utf8(&mut [0; 4])),
                    None => unknown_entities.push(UnknownEntity {
                        line,
                        name: r.to_string(),
                    }),
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
        }
    }
    if !stack.is_empty() {
        return Err(SyntaxError {
            line: line_of(text, text.len()),
            message: format!("unclosed element <{}>", stack[stack.len() - 1].name),
        });
    }
    let root = root.ok_or_else(|| SyntaxError {
        line: 1,
        message: "no root element".into(),
    })?;
    Ok(Document {
        root,
        unknown_entities,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn resolves_namespaces_and_entities() {
        let doc = parse(
            r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="ar">
<body><p epub:type="epigraph" class="a b">x &amp; &#x627;&lt;</p></body></html>"#,
        )
        .unwrap();
        assert!(doc.root.is(XHTML, "html"));
        assert_eq!(doc.root.attr_ns(XML, "lang"), Some("ar"));
        let p = doc.root.find(&|e| e.name == "p").unwrap();
        assert_eq!(p.line, 3);
        assert_eq!(p.attr_ns(OPS, "type"), Some("epigraph"));
        assert_eq!(p.tokens(None, "class").collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(p.text(), "x & ا<");
    }

    #[test]
    fn html_entities_are_reported_not_expanded() {
        let doc = parse("<p>a&nbsp;b</p>").unwrap();
        assert_eq!(doc.root.text(), "ab");
        assert_eq!(doc.unknown_entities[0].name, "nbsp");
    }

    #[test]
    fn malformed_documents_fail_with_a_line() {
        let e = parse("<a>\n<b></a>").unwrap_err();
        assert_eq!(e.line, 2, "{e:?}");
        assert!(parse("<a>").is_err());
        assert!(parse("").is_err());
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).unwrap_err().message.contains("nesting"));
    }

    #[test]
    fn no_external_entities() {
        let xxe = r#"<!DOCTYPE p [<!ENTITY x SYSTEM "file:///etc/passwd">]><p>&x;</p>"#;
        let doc = parse(xxe).unwrap();
        assert_eq!(doc.root.text(), "");
        assert_eq!(doc.unknown_entities[0].name, "x");
    }
}
