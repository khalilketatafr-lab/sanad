//! Text diagram of a normalized book: one line per block with its kind,
//! direction (and where it came from), language, `epub:type` and classes,
//! then the runs that differ from plain text.
//!
//! Server-side diagnostics only: it prints the book's text.

use std::fmt::Write as _;

use super::Book;
use super::blocks::{Block, Chapter, Content, Inline, Link, Run};
use super::nav::TocEntry;

/// Characters of text shown per block or run.
const PREVIEW: usize = 34;

fn preview(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { '⏎' } else { c })
        .collect();
    let mut out: String = flat.chars().take(PREVIEW).collect();
    if flat.chars().count() > PREVIEW {
        out.push('…');
    }
    out
}

fn attrs(b: &Block, parent_lang: Option<&str>) -> String {
    let mut s = format!("{}·{}", b.dir.as_str(), b.dir_source.as_str());
    if let Some(l) = &b.lang
        && Some(l.as_str()) != parent_lang
    {
        let _ = write!(s, "  lang={l}");
    }
    if !b.epub_type.is_empty() {
        let _ = write!(s, "  [{}]", b.epub_type.join(" "));
    }
    if !b.classes.is_empty() {
        let _ = write!(s, "  .{}", b.classes.join("."));
    }
    if let Some(id) = &b.id {
        let _ = write!(s, "  #{id}");
    }
    s
}

fn run_label(r: &Run) -> Option<String> {
    let mut tags = Vec::new();
    if let Some(l) = &r.lang {
        tags.push(format!("lang={l}"));
    }
    if let Some(d) = r.dir {
        tags.push(format!("dir={}", d.as_str()));
    }
    if !r.style.is_plain() {
        tags.push(r.style.label());
    }
    match &r.link {
        Some(Link::NoteRef { fragment, .. }) => {
            tags.push(format!("noteref→#{}", fragment.as_deref().unwrap_or("")));
        }
        Some(Link::Internal { path, fragment }) => tags.push(format!(
            "link→{}{}",
            path.rsplit('/').next().unwrap_or(path),
            fragment
                .as_ref()
                .map(|f| format!("#{f}"))
                .unwrap_or_default()
        )),
        Some(Link::External { url }) => tags.push(format!("link→{url}")),
        None => {}
    }
    if !r.classes.is_empty() {
        tags.push(format!(".{}", r.classes.join(".")));
    }
    (!tags.is_empty()).then(|| tags.join(" "))
}

fn block(out: &mut String, b: &Block, prefix: &str, last: bool, parent_lang: Option<&str>) {
    let (branch, cont) = if last {
        ("└─ ", "   ")
    } else {
        ("├─ ", "│  ")
    };
    let _ = write!(
        out,
        "{prefix}{branch}{} {}  {}",
        b.tag,
        b.kind.label(),
        attrs(b, parent_lang)
    );
    let child_prefix = format!("{prefix}{cont}");
    match &b.content {
        Content::Empty => out.push('\n'),
        Content::Inlines(items) => {
            let _ = writeln!(out, "  «{}»", preview(&b.text()));
            let special: Vec<(String, String)> = items
                .iter()
                .filter_map(|i| match i {
                    Inline::Text(r) => run_label(r).map(|l| (l, preview(&r.text))),
                    Inline::Break => None,
                    Inline::Image { src, .. } => Some(("image".into(), src.clone())),
                })
                .collect();
            let runs = items
                .iter()
                .filter(|i| matches!(i, Inline::Text(_)))
                .count();
            if !special.is_empty() {
                let _ = writeln!(out, "{child_prefix}    runs: {runs}");
                for (label, text) in special {
                    let _ = writeln!(out, "{child_prefix}    · {label}  «{text}»");
                }
            }
        }
        Content::Blocks(children) => {
            out.push('\n');
            for (k, c) in children.iter().enumerate() {
                block(
                    out,
                    c,
                    &child_prefix,
                    k + 1 == children.len(),
                    b.lang.as_deref(),
                );
            }
        }
    }
}

/// One chapter as a tree.
#[must_use]
pub fn chapter(ch: &Chapter) -> String {
    let mut out = format!(
        "{} ({}){}  {}·{}  lang={}  paragraphs={}\n",
        ch.path,
        ch.idref,
        if ch.linear { "" } else { " non-linear" },
        ch.dir.as_str(),
        ch.dir_source.as_str(),
        ch.lang.as_deref().unwrap_or("?"),
        ch.paragraph_count(),
    );
    for (k, b) in ch.blocks.iter().enumerate() {
        block(
            &mut out,
            b,
            "",
            k + 1 == ch.blocks.len(),
            ch.lang.as_deref(),
        );
    }
    out
}

fn toc(out: &mut String, entries: &[TocEntry], depth: usize) {
    for e in entries {
        let _ = writeln!(
            out,
            "{}• {}  → {}",
            "  ".repeat(depth + 1),
            e.label,
            e.target.as_deref().unwrap_or("-")
        );
        toc(out, &e.children, depth + 1);
    }
}

/// The whole book: package summary, TOC, every chapter.
#[must_use]
pub fn book(b: &Book) -> String {
    let p = &b.package;
    let mut out = format!(
        "EPUB {}  {}\n  title: {}\n  language: {}  page-progression: {}\n  spine: {} items, {} paragraphs\n",
        p.version,
        p.identifier.as_deref().unwrap_or("(no identifier)"),
        p.titles.join(" / "),
        p.languages.join(", "),
        p.page_progression.map_or("default", |d| d.as_str()),
        b.chapters.len(),
        b.paragraph_count(),
    );
    out.push_str("  toc:\n");
    toc(&mut out, &b.toc, 1);
    for ch in &b.chapters {
        out.push('\n');
        out.push_str(&chapter(ch));
    }
    out
}
