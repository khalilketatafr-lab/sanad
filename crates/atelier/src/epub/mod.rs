//! Ingest stage ①: EPUB 3 intake (OCF → OPF → XHTML) into a semantic block
//! tree, with EPUBCheck-style diagnostics. See `docs/atelier/intake.md`.
//!
//! ```text
//! bytes ─ ocf::read ─▶ Container ─ opf::parse ─▶ Package ─┬─ nav::toc ─▶ TOC
//!         (limits,      (inflated    (metadata,           └─ xhtml::chapter ×spine
//!          DRM, paths)   files)       manifest, spine)        ─▶ Chapter { blocks }
//! ```
//!
//! [`intake`] returns a [`Book`] whenever the archive and package are
//! readable, even if some rules fail, so the report can show everything at
//! once. [`Book::is_ingestible`] is the gate for typesetting.

pub mod blocks;
pub mod diag;
pub mod nav;
pub mod ocf;
pub mod opf;
pub mod path;
pub mod tree;
pub mod xhtml;
pub mod xml;

use std::collections::BTreeSet;

use serde::Serialize;
use thiserror::Error;

pub use blocks::{Block, BlockKind, Chapter, Content, Dir, DirSource, Inline, Link, Run, Style};
pub use diag::{Diagnostic, Diagnostics, Severity};
pub use ocf::{Container, Limits};
pub use opf::Package;

use nav::TocEntry;

/// A normalized EPUB.
#[derive(Debug, Clone, Serialize)]
pub struct Book {
    pub package: Package,
    pub toc: Vec<TocEntry>,
    pub chapters: Vec<Chapter>,
    pub diagnostics: Diagnostics,
    /// The inflated archive (images and other resources for later stages).
    #[serde(skip)]
    pub container: Container,
}

impl Book {
    /// No error-level diagnostic: the book may be typeset.
    #[must_use]
    pub fn is_ingestible(&self) -> bool {
        !self.diagnostics.has_errors()
    }

    #[must_use]
    pub fn paragraph_count(&self) -> usize {
        self.chapters.iter().map(Chapter::paragraph_count).sum()
    }
}

/// The archive or its package document could not be read at all.
#[derive(Debug, Error)]
#[error("EPUB rejected: {}", .diagnostics.iter().map(|d| format!("{} {}", d.code.id, d.message)).collect::<Vec<_>>().join("; "))]
pub struct Rejected {
    pub diagnostics: Diagnostics,
}

/// Reads, validates and normalizes an EPUB 3.
#[tracing::instrument(level = "info", skip_all, fields(bytes = bytes.len()))]
pub fn intake(bytes: &[u8], limits: &Limits) -> Result<Book, Rejected> {
    let mut d = Diagnostics::default();
    let Some(container) = ocf::read(bytes, limits, &mut d) else {
        return Err(Rejected { diagnostics: d });
    };
    let Some(package) = opf::parse(&container, &mut d) else {
        return Err(Rejected { diagnostics: d });
    };
    let toc = nav::toc(&container, &package, &mut d);
    let spine_paths: BTreeSet<String> = package
        .reading_order()
        .filter_map(|(_, i)| i.path.clone())
        .collect();
    let chapters: Vec<Chapter> = package
        .reading_order()
        .filter_map(|(s, item)| xhtml::chapter(&container, &package, &spine_paths, s, item, &mut d))
        .collect();
    let book = Book {
        package,
        toc,
        chapters,
        diagnostics: d,
        container,
    };
    tracing::info!(
        chapters = book.chapters.len(),
        paragraphs = book.paragraph_count(),
        errors = book.diagnostics.count(Severity::Error),
        warnings = book.diagnostics.count(Severity::Warning),
        "intake done"
    );
    Ok(book)
}
