//! XHTML content document → semantic block tree.
//!
//! Rules, in the order they apply:
//!
//! 1. **Block or inline.** An element whose children include a block-level
//!    element becomes a container. Loose text beside those children becomes an
//!    anonymous paragraph (CSS anonymous block boxes). Any other element
//!    becomes a leaf block with inline content.
//! 2. **Direction.** `dir="ltr|rtl"` on the element (attr), then on an
//!    ancestor (inherited). With `dir="auto"`, or no `dir` anywhere, the first
//!    strong character decides (detected, UAX #9 P2–P3). Browsers would show
//!    an undirected Arabic paragraph left to right; that is a source bug, so it
//!    is detected and reported (XHT-004) rather than reproduced.
//! 3. **Language.** `xml:lang`/`lang`, inherited, falling back to the first
//!    `dc:language`. Runs record a language only where it differs from their
//!    block's.
//! 4. **Whitespace.** XML whitespace collapses to one space and is trimmed at
//!    block and line edges (`white-space: normal`); `<pre>` keeps it.
//! 5. **Unicode.** Soft hyphens, ZWSP, BOM and explicit bidi controls are
//!    removed (the Compositor owns hyphenation, markup owns direction). Text is
//!    NFC-normalized. Counts are reported as NRM-001/002.
//! 6. **Unsupported content** (tables, ruby annotations, SVG, MathML, media)
//!    is flattened or dropped with a warning (XHT-003), never silently.

use std::collections::{BTreeMap, BTreeSet};

use unicode_bidi::{Direction, get_base_direction_full};
use unicode_normalization::{UnicodeNormalization, is_nfc};

use super::blocks::{Block, BlockKind, Chapter, Content, Dir, DirSource, Inline, Link, Run, Style};
use super::diag::{self, Diagnostics};
use super::ocf::Container;
use super::opf::{Item, Package, SpineRef};
use super::path::{dir_of, is_external, resolve};
use super::xml::{self, Element, MATHML, Node, OPS, SVG, XHTML, XML};

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];
const SKIP_TAGS: &[&str] = &["head", "script", "style", "template", "title", "noscript"];
const DROP_TAGS: &[&str] = &[
    "audio", "video", "iframe", "object", "embed", "canvas", "form", "input", "button", "select",
    "textarea", "map", "track", "source",
];
const NOTE_TYPES: &[&str] = &["footnote", "endnote", "rearnote", "note"];
const NOTE_ROLES: &[&str] = &["doc-footnote", "doc-endnote"];

/// Removed from text: soft hyphens, zero-width spaces and bidi controls.
const fn is_stripped(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{200B}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// U+FEFF inside text is ZERO WIDTH NO-BREAK SPACE, the deprecated spelling
/// of WORD JOINER: Standard Ebooks puts it before em dashes so a line never
/// starts with one. It is a line-breaking constraint, not noise: kept, as
/// U+2060.
const WORD_JOINER: char = '\u{2060}';

const fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn detect(text: &str) -> Option<Dir> {
    // `_full`: element text starts with newlines (paragraph separators).
    match get_base_direction_full(text) {
        Direction::Ltr => Some(Dir::Ltr),
        Direction::Rtl => Some(Dir::Rtl),
        Direction::Mixed => None,
    }
}

fn is_block_element(e: &Element) -> bool {
    e.ns.as_deref() == Some(XHTML) && BLOCK_TAGS.contains(&e.name.as_str())
}

/// Inherited block context.
#[derive(Debug, Clone)]
struct BlockCtx {
    lang: Option<String>,
    /// Direction set by markup on an ancestor (or by `dir=auto` above).
    dir: Option<Dir>,
}

/// Inherited inline context within one block.
#[derive(Debug, Clone, Default)]
struct InlineCtx {
    style: Style,
    lang: Option<String>,
    dir: Option<Dir>,
    link: Option<Link>,
    classes: Vec<String>,
    epub_type: Vec<String>,
    quote_depth: u8,
}

impl InlineCtx {
    /// A run of `text` with this context's semantics; the language only
    /// where it differs from the block's.
    fn run(&self, text: String, block: &BlockCtx) -> Run {
        Run {
            text,
            style: self.style,
            lang: self.lang.clone().filter(|l| Some(l) != block.lang.as_ref()),
            dir: self.dir,
            link: self.link.clone(),
            classes: self.classes.clone(),
            epub_type: self.epub_type.clone(),
        }
    }
}

fn lang_of(e: &Element) -> Option<&str> {
    e.attr_ns(XML, "lang")
        .or_else(|| e.attr("lang"))
        .filter(|l| !l.is_empty())
}

fn primary_lang(lang: Option<&str>) -> &str {
    lang.and_then(|l| l.split(['-', '_']).next()).unwrap_or("")
}

struct Builder<'a> {
    path: &'a str,
    package: &'a Package,
    spine_paths: &'a BTreeSet<String>,
    d: &'a mut Diagnostics,
    stripped: usize,
    nfc: usize,
    detected_rtl: Option<u32>,
    unsupported: BTreeMap<String, (u32, usize)>,
}

impl Builder<'_> {
    fn unsupported(&mut self, what: &str, line: u32) {
        self.unsupported
            .entry(what.to_owned())
            .or_insert((line, 0))
            .1 += 1;
    }

    /// Resolves the element's direction: (dir, source, dir passed to children).
    fn direction(&mut self, e: &Element, ctx: &BlockCtx) -> (Dir, DirSource, Option<Dir>) {
        match e.attr("dir").map(str::trim) {
            Some(v) if Dir::parse(v).is_some() => {
                let dir = Dir::parse(v).unwrap_or(Dir::Ltr);
                (dir, DirSource::Attribute, Some(dir))
            }
            Some(v) if v.eq_ignore_ascii_case("auto") => match detect(&e.text()) {
                Some(dir) => (dir, DirSource::Detected, Some(dir)),
                None => (Dir::Ltr, DirSource::Default, Some(Dir::Ltr)),
            },
            _ => match ctx.dir {
                Some(dir) => (dir, DirSource::Inherited, Some(dir)),
                None => match detect(&e.text()) {
                    Some(dir) => {
                        if dir == Dir::Rtl && self.detected_rtl.is_none() {
                            self.detected_rtl = Some(e.line);
                        }
                        (dir, DirSource::Detected, None)
                    }
                    None => (Dir::Ltr, DirSource::Default, None),
                },
            },
        }
    }
}

fn kind_of(e: &Element) -> BlockKind {
    let types: Vec<&str> = e.tokens(Some(OPS), "type").collect();
    let roles: Vec<&str> = e.tokens(None, "role").collect();
    match e.name.as_str() {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => BlockKind::Heading {
            level: e.name.as_bytes()[1] - b'0',
        },
        "p" | "dt" | "dd" | "td" | "th" | "summary" | "address" => BlockKind::Paragraph,
        "li" => BlockKind::ListItem { ordinal: None },
        "ol" => BlockKind::List {
            ordered: true,
            start: e
                .attr("start")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(1),
        },
        "ul" => BlockKind::List {
            ordered: false,
            start: 1,
        },
        "blockquote" => BlockKind::Blockquote,
        "figure" => BlockKind::Figure,
        "figcaption" | "caption" => BlockKind::Caption,
        "pre" => BlockKind::Preformatted,
        "hr" => BlockKind::SectionBreak,
        "aside"
            if types.iter().any(|t| NOTE_TYPES.contains(t))
                || roles.iter().any(|r| NOTE_ROLES.contains(r)) =>
        {
            BlockKind::Note
        }
        "aside" => BlockKind::Aside,
        _ => BlockKind::Section,
    }
}

impl Builder<'_> {
    fn block(&mut self, e: &Element, ctx: &BlockCtx, ordinal: Option<u32>) -> Option<Block> {
        if e.name == "table" {
            self.unsupported("table (flattened to paragraphs)", e.line);
        }
        let lang = lang_of(e).map(str::to_owned).or_else(|| ctx.lang.clone());
        let (dir, dir_source, child_dir) = self.direction(e, ctx);
        let child_ctx = BlockCtx {
            lang: lang.clone(),
            dir: child_dir,
        };
        let mut kind = kind_of(e);
        if let BlockKind::ListItem { .. } = kind {
            kind = BlockKind::ListItem { ordinal };
        }
        let content = if kind == BlockKind::SectionBreak {
            Content::Empty
        } else if e.elements().any(is_block_element) {
            Content::Blocks(self.children(e, &child_ctx, &kind))
        } else {
            let pre = kind == BlockKind::Preformatted;
            Content::Inlines(self.inline_content(e.children.iter(), &child_ctx, pre))
        };
        let content = match content {
            Content::Inlines(items) if items.is_empty() => return None,
            Content::Blocks(b) if b.is_empty() => return None,
            // A leaf holding only an image is the image.
            Content::Inlines(items)
                if matches!(kind, BlockKind::Paragraph | BlockKind::Section)
                    && items.len() == 1
                    && matches!(items[0], Inline::Image { .. }) =>
            {
                if let Inline::Image { src, alt } = &items[0] {
                    kind = BlockKind::Image {
                        src: src.clone(),
                        alt: alt.clone(),
                    };
                }
                Content::Empty
            }
            c => c,
        };
        Some(Block {
            kind,
            tag: e.name.clone(),
            id: e.attr("id").map(str::to_owned),
            classes: e.tokens(None, "class").map(str::to_owned).collect(),
            epub_type: e.tokens(Some(OPS), "type").map(str::to_owned).collect(),
            lang,
            dir,
            dir_source,
            content,
            line: e.line,
        })
    }

    /// A container's children: block elements as blocks, runs of loose
    /// inline content between them as anonymous paragraphs.
    fn children(&mut self, e: &Element, ctx: &BlockCtx, kind: &BlockKind) -> Vec<Block> {
        let mut out = Vec::new();
        let mut loose: Vec<&Node> = Vec::new();
        let (ordered, mut counter) = match kind {
            BlockKind::List { ordered, start } => (*ordered, *start),
            _ => (false, 1),
        };
        let flush = |b: &mut Self, loose: &mut Vec<&Node>, out: &mut Vec<Block>| {
            if loose.is_empty() {
                return;
            }
            let items = b.inline_content(loose.drain(..), ctx, false);
            if !items.is_empty() {
                let text: String = items
                    .iter()
                    .filter_map(|i| match i {
                        Inline::Text(r) => Some(r.text.as_str()),
                        _ => None,
                    })
                    .collect();
                let (dir, dir_source) = match ctx.dir {
                    Some(d) => (d, DirSource::Inherited),
                    None => detect(&text)
                        .map_or((Dir::Ltr, DirSource::Default), |d| (d, DirSource::Detected)),
                };
                out.push(Block {
                    kind: BlockKind::Paragraph,
                    tag: "#anonymous".into(),
                    id: None,
                    classes: Vec::new(),
                    epub_type: Vec::new(),
                    lang: ctx.lang.clone(),
                    dir,
                    dir_source,
                    content: Content::Inlines(items),
                    line: e.line,
                });
            }
        };
        for node in &e.children {
            match node {
                Node::Element(c) if is_block_element(c) => {
                    flush(self, &mut loose, &mut out);
                    let ordinal = if c.name == "li" && ordered {
                        if let Some(v) = c.attr("value").and_then(|v| v.trim().parse().ok()) {
                            counter = v;
                        }
                        let n = counter;
                        counter += 1;
                        Some(n)
                    } else {
                        None
                    };
                    if let Some(b) = self.block(c, ctx, ordinal) {
                        out.push(b);
                    }
                }
                Node::Element(c) if c.ns.as_deref() == Some(XHTML) && c.name == "img" => {
                    // An image between blocks is a block.
                    flush(self, &mut loose, &mut out);
                    if let Some(Inline::Image { src, alt }) = self.image(c) {
                        out.push(Block {
                            kind: BlockKind::Image { src, alt },
                            tag: "img".into(),
                            id: c.attr("id").map(str::to_owned),
                            classes: c.tokens(None, "class").map(str::to_owned).collect(),
                            epub_type: Vec::new(),
                            lang: ctx.lang.clone(),
                            dir: ctx.dir.unwrap_or(Dir::Ltr),
                            dir_source: if ctx.dir.is_some() {
                                DirSource::Inherited
                            } else {
                                DirSource::Default
                            },
                            content: Content::Empty,
                            line: c.line,
                        });
                    }
                }
                Node::Element(c) if self.skip(c) => {}
                other => loose.push(other),
            }
        }
        flush(self, &mut loose, &mut out);
        out
    }

    /// Elements that contribute nothing (with a warning where content is lost).
    fn skip(&mut self, e: &Element) -> bool {
        match e.ns.as_deref() {
            Some(SVG) => {
                self.unsupported("svg", e.line);
                true
            }
            Some(MATHML) => {
                self.unsupported("math", e.line);
                true
            }
            Some(XHTML) if SKIP_TAGS.contains(&e.name.as_str()) => true,
            Some(XHTML) if DROP_TAGS.contains(&e.name.as_str()) => {
                self.unsupported(&e.name, e.line);
                true
            }
            Some(XHTML) => false,
            _ => {
                self.unsupported(&format!("foreign element <{}>", e.name), e.line);
                true
            }
        }
    }

    fn image(&mut self, e: &Element) -> Option<Inline> {
        let src = e.attr("src")?;
        let alt = e.attr("alt").unwrap_or("").to_owned();
        let Some(target) = resolve(dir_of(self.path), self.path, src) else {
            self.d.push(
                diag::XHT_RESOURCE,
                self.path,
                Some(e.line),
                format!("src {src:?}"),
            );
            return None;
        };
        if self.package.item_at(&target.path).is_none() {
            self.d.push(
                diag::XHT_RESOURCE,
                self.path,
                Some(e.line),
                format!("{} is not in the manifest", target.path),
            );
            return None;
        }
        Some(Inline::Image {
            src: target.path,
            alt,
        })
    }

    fn link(&mut self, e: &Element, href: &str) -> Option<Link> {
        if is_external(href) {
            return Some(Link::External { url: href.into() });
        }
        let target = resolve(dir_of(self.path), self.path, href)?;
        if !self.spine_paths.contains(&target.path) {
            self.d.push(
                diag::XHT_LINK,
                self.path,
                Some(e.line),
                format!("{href} → {}", target.path),
            );
        }
        let noteref = e.tokens(Some(OPS), "type").any(|t| t == "noteref")
            || e.tokens(None, "role").any(|r| r == "doc-noteref");
        Some(if noteref {
            Link::NoteRef {
                path: target.path,
                fragment: target.fragment,
            }
        } else {
            Link::Internal {
                path: target.path,
                fragment: target.fragment,
            }
        })
    }

    /// Normalized inline content of `nodes` (rules 4–5).
    fn inline_content<'n>(
        &mut self,
        nodes: impl IntoIterator<Item = &'n Node>,
        ctx: &BlockCtx,
        pre: bool,
    ) -> Vec<Inline> {
        let mut raw = Vec::new();
        let ictx = InlineCtx::default();
        for n in nodes {
            self.collect(n, ctx, &ictx, &mut raw);
        }
        let items = if pre {
            split_pre_lines(raw)
        } else {
            collapse_whitespace(raw)
        };
        let items = items
            .into_iter()
            .map(|i| match i {
                Inline::Text(mut r) => {
                    if !is_nfc(&r.text) {
                        self.nfc += 1;
                        r.text = r.text.nfc().collect();
                    }
                    Inline::Text(r)
                }
                other => other,
            })
            .collect();
        merge_runs(items)
    }

    fn collect(&mut self, node: &Node, ctx: &BlockCtx, ictx: &InlineCtx, out: &mut Vec<Inline>) {
        let e = match node {
            Node::Text(t) => {
                let before = t.chars().count();
                let text: String = t
                    .chars()
                    .filter(|&c| !is_stripped(c))
                    .map(|c| if c == '\u{FEFF}' { WORD_JOINER } else { c })
                    .collect();
                self.stripped += before - text.chars().count();
                if !text.is_empty() {
                    out.push(Inline::Text(ictx.run(text, ctx)));
                }
                return;
            }
            Node::Element(e) => e,
        };
        if self.skip(e) {
            return;
        }
        match e.name.as_str() {
            "br" => return out.push(Inline::Break),
            "img" => {
                if let Some(img) = self.image(e) {
                    out.push(img);
                }
                return;
            }
            "rt" | "rp" => {
                self.unsupported("ruby annotation (base text kept)", e.line);
                return;
            }
            _ => {}
        }
        let mut c = ictx.clone();
        c.style = c.style.union(match e.name.as_str() {
            "em" | "i" | "cite" | "dfn" | "var" => Style::ITALIC,
            "strong" | "b" => Style::BOLD,
            "sup" => Style::SUPERSCRIPT,
            "sub" => Style::SUBSCRIPT,
            "code" | "kbd" | "samp" | "tt" => Style::CODE,
            "u" | "ins" => Style::UNDERLINE,
            "s" | "del" | "strike" => Style::STRIKE,
            _ => Style::default(),
        });
        let classes: Vec<&str> = e.tokens(None, "class").collect();
        if classes
            .iter()
            .any(|c| matches!(*c, "small-caps" | "smallcaps" | "smcap"))
        {
            c.style = c.style.union(Style::SMALL_CAPS);
        }
        c.classes.extend(classes.iter().map(|s| (*s).to_owned()));
        c.epub_type
            .extend(e.tokens(Some(OPS), "type").map(str::to_owned));
        if let Some(l) = lang_of(e) {
            c.lang = Some(l.to_owned());
        }
        match (e.name.as_str(), e.attr("dir").and_then(Dir::parse)) {
            (_, Some(dir)) => c.dir = Some(dir),
            ("bdi", None) => c.dir = detect(&e.text()),
            _ => {}
        }
        if e.name == "a"
            && let Some(href) = e.attr("href")
        {
            c.link = self.link(e, href);
        }
        let quotes = (e.name == "q").then(|| {
            let lang = c.lang.as_deref().or(ctx.lang.as_deref());
            quote_marks(primary_lang(lang), ictx.quote_depth)
        });
        if quotes.is_some() {
            c.quote_depth = ictx.quote_depth.saturating_add(1);
        }
        if let Some((open, _)) = quotes {
            out.push(Inline::Text(c.run(open.to_owned(), ctx)));
        }
        for child in &e.children {
            self.collect(child, ctx, &c, out);
        }
        if let Some((_, close)) = quotes {
            out.push(Inline::Text(c.run(close.to_owned(), ctx)));
        }
    }
}

/// Quotation marks for `<q>` by language and nesting depth.
fn quote_marks(lang: &str, depth: u8) -> (&'static str, &'static str) {
    match (lang, depth % 2) {
        ("fr", 0) => ("«\u{202F}", "\u{202F}»"),
        ("ar", 0) => ("«", "»"),
        ("fr" | "ar", _) | (_, 0) => ("“", "”"),
        _ => ("‘", "’"),
    }
}

/// `white-space: normal`: runs of XML whitespace become one space, none at
/// the start or end of a line.
fn collapse_whitespace(raw: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(raw.len());
    let mut line_start = true;
    let mut last_space = false;
    for item in raw {
        match item {
            Inline::Text(mut r) => {
                let mut text = String::with_capacity(r.text.len());
                for ch in r.text.chars() {
                    if is_xml_space(ch) {
                        if !line_start && !last_space {
                            text.push(' ');
                            last_space = true;
                        }
                    } else {
                        text.push(ch);
                        line_start = false;
                        last_space = false;
                    }
                }
                r.text = text;
                out.push(Inline::Text(r));
            }
            Inline::Break => {
                trim_trailing_space(&mut out);
                out.push(Inline::Break);
                line_start = true;
                last_space = false;
            }
            image @ Inline::Image { .. } => {
                out.push(image);
                line_start = false;
                last_space = false;
            }
        }
    }
    trim_trailing_space(&mut out);
    out.retain(|i| !matches!(i, Inline::Text(r) if r.text.is_empty()));
    while matches!(out.last(), Some(Inline::Break)) {
        out.pop();
    }
    while matches!(out.first(), Some(Inline::Break)) {
        out.remove(0);
    }
    out
}

fn trim_trailing_space(out: &mut [Inline]) {
    for item in out.iter_mut().rev() {
        match item {
            Inline::Text(r) if r.text.is_empty() => {}
            Inline::Text(r) => {
                if r.text.ends_with(' ') {
                    r.text.pop();
                }
                return;
            }
            _ => return,
        }
    }
}

/// `<pre>`: newlines become breaks; one leading newline is dropped (HTML).
fn split_pre_lines(raw: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut first = true;
    for item in raw {
        match item {
            Inline::Text(r) => {
                let text = r.text.replace("\r\n", "\n");
                let text = if first {
                    text.strip_prefix('\n').map(str::to_owned).unwrap_or(text)
                } else {
                    text
                };
                for (k, line) in text.split('\n').enumerate() {
                    if k > 0 {
                        out.push(Inline::Break);
                    }
                    if !line.is_empty() {
                        out.push(Inline::Text(Run {
                            text: line.to_owned(),
                            ..r.clone()
                        }));
                    }
                }
            }
            other => out.push(other),
        }
        first = false;
    }
    while matches!(out.last(), Some(Inline::Break)) {
        out.pop();
    }
    out
}

fn merge_runs(items: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        if let (Some(Inline::Text(prev)), Inline::Text(r)) = (out.last_mut(), &item)
            && prev.same_attrs(r)
        {
            prev.text.push_str(&r.text);
            continue;
        }
        out.push(item);
    }
    out
}

impl Builder<'_> {
    /// Chapter-level diagnostics, once per chapter.
    fn report(&mut self) {
        let path = self.path;
        if let Some(line) = self.detected_rtl {
            self.d.push(
                diag::XHT_DIR_DETECTED,
                path,
                Some(line),
                "set dir=\"rtl\" on <html> or the block",
            );
        }
        for (what, (line, n)) in std::mem::take(&mut self.unsupported) {
            self.d.push(
                diag::XHT_UNSUPPORTED,
                path,
                Some(line),
                format!("{what} ×{n}"),
            );
        }
        if self.nfc > 0 {
            self.d.push(
                diag::NRM_NFC,
                path,
                None,
                format!("{} run(s) recomposed", self.nfc),
            );
        }
        if self.stripped > 0 {
            self.d.push(
                diag::NRM_INVISIBLE,
                path,
                None,
                format!("{} character(s)", self.stripped),
            );
        }
    }
}

/// Normalizes one spine item. `None` when it cannot be read at all.
pub fn chapter(
    c: &Container,
    package: &Package,
    spine_paths: &BTreeSet<String>,
    sref: &SpineRef,
    item: &Item,
    d: &mut Diagnostics,
) -> Option<Chapter> {
    let path = item.path.as_deref()?;
    let Some(text) = c.get(path).and_then(|b| core::str::from_utf8(b).ok()) else {
        d.push(diag::XHT_XML, path, None, "missing or not UTF-8");
        return None;
    };
    let doc = match xml::parse(text) {
        Ok(doc) => doc,
        Err(e) => {
            d.push(diag::XHT_XML, path, Some(e.line), e.message);
            return None;
        }
    };
    for u in &doc.unknown_entities {
        d.push(
            diag::XHT_ENTITY,
            path,
            Some(u.line),
            format!("&{};", u.name),
        );
    }
    let html = &doc.root;
    if !html.is(XHTML, "html") {
        d.push(
            diag::XHT_XML,
            path,
            Some(html.line),
            "root is not XHTML <html>",
        );
        return None;
    }
    let Some(body) = html.elements().find(|e| e.is(XHTML, "body")) else {
        d.push(diag::XHT_XML, path, Some(html.line), "no <body>");
        return None;
    };
    let mut b = Builder {
        path,
        package,
        spine_paths,
        d,
        stripped: 0,
        nfc: 0,
        detected_rtl: None,
        unsupported: BTreeMap::new(),
    };
    let root_ctx = BlockCtx {
        lang: lang_of(html)
            .map(str::to_owned)
            .or_else(|| package.languages.first().cloned()),
        dir: html.attr("dir").and_then(Dir::parse),
    };
    let body_block = b.block(body, &root_ctx, None);
    let (dir, dir_source, lang, blocks) = match body_block {
        Some(Block {
            dir,
            dir_source,
            lang,
            content: Content::Blocks(blocks),
            ..
        }) => (dir, dir_source, lang, blocks),
        Some(leaf) => (leaf.dir, leaf.dir_source, leaf.lang.clone(), vec![leaf]),
        None => (
            Dir::Ltr,
            DirSource::Default,
            root_ctx.lang.clone(),
            Vec::new(),
        ),
    };
    b.report();
    let mut title = None;
    for blk in &blocks {
        blk.walk(&mut |x, _| {
            if title.is_none() && matches!(x.kind, BlockKind::Heading { .. }) {
                title = Some(x.text());
            }
        });
    }
    Some(Chapter {
        idref: sref.idref.clone(),
        path: path.to_owned(),
        linear: sref.linear,
        title,
        lang,
        dir,
        dir_source,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(Run {
            text: s.into(),
            style: Style::default(),
            lang: None,
            dir: None,
            link: None,
            classes: vec![],
            epub_type: vec![],
        })
    }

    fn texts(items: &[Inline]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Inline::Text(r) => r.text.clone(),
                Inline::Break => "⏎".into(),
                Inline::Image { .. } => "▣".into(),
            })
            .collect()
    }

    #[test]
    fn whitespace_collapses_across_runs_and_trims_lines() {
        let out = collapse_whitespace(vec![
            text("\n   It is "),
            text("  a\ttruth "),
            Inline::Break,
            text("  universally\n"),
        ]);
        assert_eq!(texts(&out), ["It is ", "a truth", "⏎", "universally"]);
    }

    #[test]
    fn pre_keeps_lines() {
        let out = split_pre_lines(vec![text("\nline one\n  line two\n")]);
        assert_eq!(texts(&out), ["line one", "⏎", "  line two"]);
    }

    #[test]
    fn quotes_follow_language_and_depth() {
        assert_eq!(quote_marks("en", 0), ("“", "”"));
        assert_eq!(quote_marks("en", 1), ("‘", "’"));
        assert_eq!(quote_marks("fr", 0), ("«\u{202F}", "\u{202F}»"));
        assert_eq!(quote_marks("ar", 0), ("«", "»"));
    }

    #[test]
    fn detection_uses_the_first_strong_character() {
        assert_eq!(detect("1813، صدرت Pride"), Some(Dir::Rtl));
        assert_eq!(detect("« Pride » صدرت"), Some(Dir::Ltr));
        assert_eq!(detect("1813 — ."), None);
        assert_eq!(
            detect("\n  \n  Kalīla"),
            Some(Dir::Ltr),
            "past leading separators"
        );
        assert_eq!(
            detect("1813 ؟"),
            Some(Dir::Rtl),
            "U+061F is AL, a strong character"
        );
    }
}
