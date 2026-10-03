//! Intake diagnostics: what a publisher's EPUB gets wrong, located.
//!
//! Every code is stable and documented in `docs/atelier/intake.md`. Where
//! EPUBCheck (the W3C validator) reports the same condition, the code
//! records its message id, so a publisher can match our report against
//! theirs. Errors make an EPUB non-ingestible; warnings and info do not.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The edition cannot be built from this file.
    Error,
    /// Built, but something was dropped, guessed or repaired.
    Warning,
    /// A normalization Atelier applied (counts, not problems).
    Info,
}

/// One intake rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Code {
    pub id: &'static str,
    pub severity: Severity,
    /// The EPUBCheck message id for the same condition, if there is one.
    pub epubcheck: Option<&'static str>,
    pub summary: &'static str,
}

impl Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id)
    }
}

macro_rules! codes {
    ($($name:ident = ($id:literal, $sev:ident, $ec:expr, $summary:literal);)*) => {
        $(pub const $name: Code = Code {
            id: $id,
            severity: Severity::$sev,
            epubcheck: $ec,
            summary: $summary,
        };)*
        /// Every rule, in documentation order.
        pub const ALL: &[Code] = &[$($name),*];
    };
}

codes! {
    OCF_ZIP = ("OCF-001", Error, None, "not a readable ZIP archive");
    OCF_MIMETYPE_FIRST = ("OCF-002", Error, Some("PKG-006"), "`mimetype` missing or not the first entry");
    OCF_MIMETYPE_VALUE = ("OCF-003", Error, Some("PKG-007"), "`mimetype` is not exactly `application/epub+zip`");
    OCF_MIMETYPE_STORED = ("OCF-004", Error, None, "`mimetype` is compressed");
    OCF_CONTAINER = ("OCF-005", Error, None, "`META-INF/container.xml` missing, unreadable or without a package rootfile");
    OCF_PATH = ("OCF-006", Error, None, "entry name escapes the container (absolute, `..`, backslash, NUL)");
    OCF_COMPRESSION = ("OCF-007", Error, None, "entry uses a compression method other than stored or deflate");
    OCF_LIMITS = ("OCF-008", Error, None, "archive exceeds size, entry-count or compression-ratio limits");
    OCF_DRM = ("OCF-009", Error, None, "content is encrypted (DRM); only font obfuscation is accepted");
    OCF_DUPLICATE = ("OCF-010", Error, None, "two entries with the same name");
    OPF_XML = ("OPF-001", Error, Some("RSC-005"), "package document is not well-formed");
    OPF_VERSION = ("OPF-002", Error, None, "not an EPUB 3 package (`version` must be 3.x)");
    OPF_UID = ("OPF-003", Error, Some("OPF-030"), "`unique-identifier` does not reference a `dc:identifier`");
    OPF_METADATA = ("OPF-004", Error, Some("RSC-005"), "required metadata missing (`dc:title`, `dc:language`)");
    OPF_MISSING = ("OPF-005", Error, Some("RSC-001"), "manifest item's file is not in the container");
    OPF_SPINE_REF = ("OPF-006", Error, Some("OPF-049"), "spine `idref` not found in the manifest");
    OPF_NAV = ("OPF-007", Error, Some("RSC-005"), "not exactly one manifest item with the `nav` property");
    OPF_SPINE_TYPE = ("OPF-008", Error, None, "spine item is not XHTML");
    OPF_DUPLICATE_ID = ("OPF-009", Error, None, "duplicate manifest `id`");
    OPF_EMPTY_SPINE = ("OPF-010", Error, None, "spine has no linear items");
    OPF_HREF = ("OPF-011", Error, None, "manifest `href` is not a resolvable relative path");
    XHT_XML = ("XHT-001", Error, Some("RSC-005"), "content document is not well-formed XML");
    XHT_ENTITY = ("XHT-002", Error, Some("RSC-005"), "undeclared entity (HTML named entities are not XML)");
    XHT_UNSUPPORTED = ("XHT-003", Warning, None, "element not supported in Phase 1a; content dropped or flattened");
    XHT_DIR_DETECTED = ("XHT-004", Warning, None, "right-to-left text with no `dir` in its ancestry; direction detected");
    XHT_RESOURCE = ("XHT-005", Error, None, "referenced image is not in the manifest");
    XHT_LINK = ("XHT-006", Warning, None, "internal link target is not in the spine");
    NAV_TOC = ("NAV-001", Error, None, "navigation document has no `toc` nav");
    NRM_NFC = ("NRM-001", Info, None, "text normalized to NFC");
    NRM_INVISIBLE = ("NRM-002", Info, None, "invisible characters removed (soft hyphen, ZWSP, bidi controls)");
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    /// Container path of the file concerned (empty for the archive itself).
    pub path: String,
    /// 1-based line in that file, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub fn push(&mut self, code: Code, path: &str, line: Option<u32>, message: impl Into<String>) {
        let d = Diagnostic {
            code,
            severity: code.severity,
            path: path.to_owned(),
            line,
            message: message.into(),
        };
        match d.severity {
            Severity::Error => tracing::warn!(code = code.id, path, ?line, "{}", d.message),
            Severity::Warning => tracing::info!(code = code.id, path, ?line, "{}", d.message),
            Severity::Info => tracing::debug!(code = code.id, path, "{}", d.message),
        }
        self.0.push(d);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    #[must_use]
    pub fn count(&self, severity: Severity) -> usize {
        self.0.iter().filter(|d| d.severity == severity).count()
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.count(Severity::Error) > 0
    }

    #[must_use]
    pub fn has(&self, code: Code) -> bool {
        self.0.iter().any(|d| d.code == code)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 1-based line of a byte offset.
#[must_use]
pub fn line_of(text: &str, byte: usize) -> u32 {
    let mut end = byte.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    u32::try_from(text[..end].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = ALL.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn lines_are_one_based() {
        assert_eq!(line_of("a\nb\nc", 0), 1);
        assert_eq!(line_of("a\nb\nc", 2), 2);
        assert_eq!(line_of("a\nb\nc", 99), 3);
    }
}
