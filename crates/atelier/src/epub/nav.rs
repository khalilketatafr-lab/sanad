//! The navigation document's `toc` nav (EPUB 3.3 §7): the edition's table of
//! contents, which Lumen shows and the Kernel uses for sync anchors.

use serde::Serialize;

use super::diag::{self, Diagnostics};
use super::ocf::Container;
use super::opf::Package;
use super::path::{dir_of, resolve};
use super::xml::{self, Element, OPS, XHTML};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TocEntry {
    pub label: String,
    /// Container path, with `#fragment` when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TocEntry>,
}

fn label(e: &Element) -> String {
    e.text().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entries(ol: &Element, nav_path: &str) -> Vec<TocEntry> {
    ol.elements()
        .filter(|li| li.is(XHTML, "li"))
        .map(|li| {
            let head = li
                .elements()
                .find(|e| e.is(XHTML, "a") || e.is(XHTML, "span"));
            let target = head.and_then(|a| a.attr("href")).and_then(|href| {
                resolve(dir_of(nav_path), nav_path, href).map(|t| match t.fragment {
                    Some(f) => format!("{}#{f}", t.path),
                    None => t.path,
                })
            });
            TocEntry {
                label: head.map(label).unwrap_or_default(),
                target,
                children: li
                    .elements()
                    .find(|e| e.is(XHTML, "ol"))
                    .map(|ol| entries(ol, nav_path))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The `toc` nav's entries; empty (with NAV-001) when there is none.
pub fn toc(c: &Container, package: &Package, d: &mut Diagnostics) -> Vec<TocEntry> {
    let Some(path) = package.nav().and_then(|i| i.path.clone()) else {
        return Vec::new(); // OPF-007 already reported
    };
    let Some(text) = c.get(&path).and_then(|b| core::str::from_utf8(b).ok()) else {
        return Vec::new(); // OPF-005 already reported
    };
    let doc = match xml::parse(text) {
        Ok(doc) => doc,
        Err(e) => {
            d.push(diag::XHT_XML, &path, Some(e.line), e.message);
            return Vec::new();
        }
    };
    let nav = doc
        .root
        .find(&|e| e.is(XHTML, "nav") && e.tokens(Some(OPS), "type").any(|t| t == "toc"));
    if let Some(ol) = nav.and_then(|n| n.find(&|e| e.is(XHTML, "ol"))) {
        entries(ol, &path)
    } else {
        d.push(
            diag::NAV_TOC,
            &path,
            None,
            "no <nav epub:type=\"toc\"> with a list",
        );
        Vec::new()
    }
}
