//! The semantic block tree: what Atelier keeps of an XHTML chapter.
//!
//! Presentation is not carried over (no CSS is applied). What survives is
//! structure — sections, headings, paragraphs, lists, quotations, figures,
//! notes, breaks — and, per run of text, the semantics that change how it is
//! set: emphasis, language, direction, links. Source `class` and `epub:type`
//! tokens are kept so edition designers can map them to Marginalia styles.
//!
//! This is the last stage that holds Unicode text in the open. Shaping
//! (stage ②) turns it into glyph runs, and nothing past that keeps text.

use serde::Serialize;

/// Base direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Ltr,
    Rtl,
}

impl Dir {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ltr" => Some(Self::Ltr),
            "rtl" => Some(Self::Rtl),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// Where a block's direction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DirSource {
    /// `dir="ltr|rtl"` on the element itself.
    Attribute,
    /// `dir` on an ancestor (or on `<html>`).
    Inherited,
    /// `dir="auto"`, or no `dir` anywhere: the first strong character
    /// (UAX #9 P2–P3).
    Detected,
    /// Nothing to go on (no strong character): left to right.
    Default,
}

impl DirSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attribute => "attr",
            Self::Inherited => "inherited",
            Self::Detected => "detected",
            Self::Default => "default",
        }
    }
}

/// Inline semantics that change how text is set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Style(u16);

impl Style {
    pub const ITALIC: Self = Self(1);
    pub const BOLD: Self = Self(1 << 1);
    pub const SMALL_CAPS: Self = Self(1 << 2);
    pub const SUPERSCRIPT: Self = Self(1 << 3);
    pub const SUBSCRIPT: Self = Self(1 << 4);
    pub const CODE: Self = Self(1 << 5);
    pub const UNDERLINE: Self = Self(1 << 6);
    pub const STRIKE: Self = Self(1 << 7);

    const NAMES: [(Self, &'static str); 8] = [
        (Self::ITALIC, "i"),
        (Self::BOLD, "b"),
        (Self::SMALL_CAPS, "sc"),
        (Self::SUPERSCRIPT, "sup"),
        (Self::SUBSCRIPT, "sub"),
        (Self::CODE, "code"),
        (Self::UNDERLINE, "u"),
        (Self::STRIKE, "s"),
    ];

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_plain(self) -> bool {
        self.0 == 0
    }

    /// Short names, `+`-joined (`i+sc`).
    #[must_use]
    pub fn label(self) -> String {
        Self::NAMES
            .iter()
            .filter(|(s, _)| self.contains(*s))
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join("+")
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
fn plain(s: &Style) -> bool {
    s.is_plain()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Link {
    /// A target inside the book (container path and fragment).
    Internal {
        path: String,
        fragment: Option<String>,
    },
    /// A note reference (`epub:type="noteref"`, `role="doc-noteref"`).
    NoteRef {
        path: String,
        fragment: Option<String>,
    },
    External {
        url: String,
    },
}

/// A maximal run of text with uniform semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub text: String,
    #[serde(skip_serializing_if = "plain")]
    pub style: Style,
    /// Set only where it differs from the block's language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// An explicit isolate or override (`dir` on an inline, `<bdi>`, `<bdo>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<Dir>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub epub_type: Vec<String>,
}

impl Run {
    /// Same attributes: adjacent runs that can merge.
    #[must_use]
    pub fn same_attrs(&self, o: &Self) -> bool {
        self.style == o.style
            && self.lang == o.lang
            && self.dir == o.dir
            && self.link == o.link
            && self.classes == o.classes
            && self.epub_type == o.epub_type
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Inline {
    Text(Run),
    /// `<br/>`: a forced line break (verse, addresses).
    Break,
    /// An image inside running text (not supported for setting in Phase 1a).
    Image {
        src: String,
        alt: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BlockKind {
    /// A grouping: `section`, `article`, `div`, `header`, `body`…
    Section,
    Heading {
        level: u8,
    },
    Paragraph,
    List {
        ordered: bool,
        start: u32,
    },
    ListItem {
        ordinal: Option<u32>,
    },
    Blockquote,
    Figure,
    Caption,
    Image {
        src: String,
        alt: String,
    },
    /// A footnote or endnote body.
    Note,
    /// Any other `aside`.
    Aside,
    /// `<hr/>`: a thematic break.
    SectionBreak,
    /// `<pre>`: whitespace preserved, never justified.
    Preformatted,
}

impl BlockKind {
    /// Short label for diagrams.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Section => "Section".into(),
            Self::Heading { level } => format!("Heading({level})"),
            Self::Paragraph => "Paragraph".into(),
            Self::List { ordered, start } => {
                if *ordered {
                    format!("List(ordered, start {start})")
                } else {
                    "List(unordered)".into()
                }
            }
            Self::ListItem { ordinal } => match ordinal {
                Some(n) => format!("ListItem({n})"),
                None => "ListItem".into(),
            },
            Self::Blockquote => "Blockquote".into(),
            Self::Figure => "Figure".into(),
            Self::Caption => "Caption".into(),
            Self::Image { src, .. } => format!("Image({src})"),
            Self::Note => "Note".into(),
            Self::Aside => "Aside".into(),
            Self::SectionBreak => "SectionBreak".into(),
            Self::Preformatted => "Preformatted".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "items", rename_all = "kebab-case")]
pub enum Content {
    Empty,
    Inlines(Vec<Inline>),
    Blocks(Vec<Block>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Block {
    pub kind: BlockKind,
    /// Source element (local name).
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub epub_type: Vec<String>,
    /// Effective language (BCP 47).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub dir: Dir,
    pub dir_source: DirSource,
    pub content: Content,
    /// 1-based source line.
    pub line: u32,
}

impl Block {
    /// The block's text (descendants included), breaks as `\n`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        match &self.content {
            Content::Empty => {}
            Content::Inlines(items) => {
                for i in items {
                    match i {
                        Inline::Text(r) => out.push_str(&r.text),
                        Inline::Break => out.push('\n'),
                        Inline::Image { .. } => {}
                    }
                }
            }
            Content::Blocks(blocks) => {
                for b in blocks {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    b.collect_text(out);
                }
            }
        }
    }

    /// Depth-first walk, self first.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Block, usize)) {
        self.walk_at(0, f);
    }

    fn walk_at<'a>(&'a self, depth: usize, f: &mut impl FnMut(&'a Block, usize)) {
        f(self, depth);
        if let Content::Blocks(children) = &self.content {
            for c in children {
                c.walk_at(depth + 1, f);
            }
        }
    }
}

/// One spine item, normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chapter {
    pub idref: String,
    pub path: String,
    pub linear: bool,
    /// First heading's text, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub dir: Dir,
    pub dir_source: DirSource,
    pub blocks: Vec<Block>,
}

impl Chapter {
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Block, usize)) {
        for b in &self.blocks {
            b.walk(f);
        }
    }

    /// Paragraphs of running text: `Paragraph` blocks outside notes.
    #[must_use]
    pub fn paragraph_count(&self) -> usize {
        fn count(b: &Block, in_note: bool) -> usize {
            let in_note = in_note || b.kind == BlockKind::Note;
            let own = usize::from(b.kind == BlockKind::Paragraph && !in_note);
            own + match &b.content {
                Content::Blocks(c) => c.iter().map(|x| count(x, in_note)).sum(),
                _ => 0,
            }
        }
        self.blocks.iter().map(|b| count(b, false)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_labels() {
        assert_eq!(Style::ITALIC.union(Style::SMALL_CAPS).label(), "i+sc");
        assert!(Style::default().is_plain());
        assert_eq!(Dir::parse(" RTL "), Some(Dir::Rtl));
        assert_eq!(Dir::parse("auto"), None);
    }
}
