//! The package document (EPUB 3.3 §5): metadata, manifest, spine.

use std::collections::BTreeSet;

use serde::Serialize;

use super::blocks::Dir;
use super::diag::{self, Diagnostics};
use super::ocf::Container;
use super::path::{dir_of, is_external, resolve};
use super::xml::{self, DC, Element, OPF, XML};

pub const XHTML_MEDIA_TYPE: &str = "application/xhtml+xml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: String,
    pub href: String,
    /// Container path; `None` for remote resources and unresolvable hrefs.
    pub path: Option<String>,
    pub media_type: String,
    pub properties: Vec<String>,
}

impl Item {
    #[must_use]
    pub fn has_property(&self, p: &str) -> bool {
        self.properties.iter().any(|x| x == p)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpineRef {
    pub idref: String,
    pub linear: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Package {
    /// Container path of the OPF.
    pub path: String,
    pub version: String,
    /// The `dc:identifier` that `unique-identifier` points at.
    pub identifier: Option<String>,
    pub titles: Vec<String>,
    pub languages: Vec<String>,
    pub creators: Vec<String>,
    /// `dcterms:modified`.
    pub modified: Option<String>,
    pub lang: Option<String>,
    pub dir: Option<Dir>,
    pub manifest: Vec<Item>,
    pub spine: Vec<SpineRef>,
    /// `page-progression-direction` (page order, not text direction).
    pub page_progression: Option<Dir>,
}

impl Package {
    #[must_use]
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.manifest.iter().find(|i| i.id == id)
    }

    #[must_use]
    pub fn item_at(&self, path: &str) -> Option<&Item> {
        self.manifest
            .iter()
            .find(|i| i.path.as_deref() == Some(path))
    }

    #[must_use]
    pub fn nav(&self) -> Option<&Item> {
        self.manifest.iter().find(|i| i.has_property("nav"))
    }

    /// Spine items in reading order, with their manifest entries.
    pub fn reading_order(&self) -> impl Iterator<Item = (&SpineRef, &Item)> {
        self.spine
            .iter()
            .filter_map(|s| self.item(&s.idref).map(|i| (s, i)))
    }
}

fn texts(meta: &Element, ns: &str, name: &str) -> Vec<String> {
    meta.elements()
        .filter(|e| e.is(ns, name))
        .map(|e| e.text().split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .collect()
}

struct Metadata {
    identifier: Option<String>,
    titles: Vec<String>,
    languages: Vec<String>,
    creators: Vec<String>,
    modified: Option<String>,
}

fn metadata(root: &Element, meta: Option<&Element>, path: &str, d: &mut Diagnostics) -> Metadata {
    let empty = Element {
        ns: None,
        name: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
        line: root.line,
    };
    let meta = meta.unwrap_or(&empty);
    let identifier = root.attr("unique-identifier").and_then(|uid| {
        meta.elements()
            .find(|e| e.is(DC, "identifier") && e.attr("id") == Some(uid))
            .map(|e| e.text().trim().to_owned())
            .filter(|t| !t.is_empty())
    });
    if identifier.is_none() {
        d.push(
            diag::OPF_UID,
            path,
            Some(root.line),
            format!("unique-identifier={:?}", root.attr("unique-identifier")),
        );
    }
    let titles = texts(meta, DC, "title");
    let languages = texts(meta, DC, "language");
    for (what, v) in [("dc:title", &titles), ("dc:language", &languages)] {
        if v.is_empty() {
            d.push(
                diag::OPF_METADATA,
                path,
                Some(meta.line),
                format!("no {what}"),
            );
        }
    }
    Metadata {
        identifier,
        titles,
        languages,
        creators: texts(meta, DC, "creator"),
        modified: meta
            .elements()
            .find(|e| e.is(OPF, "meta") && e.attr("property") == Some("dcterms:modified"))
            .map(|e| e.text().trim().to_owned()),
    }
}

/// Parses and checks the package document. `None` when it is unreadable.
pub fn parse(c: &Container, d: &mut Diagnostics) -> Option<Package> {
    let path = c.rootfile.clone();
    let Some(text) = c.get(&path).and_then(|b| core::str::from_utf8(b).ok()) else {
        d.push(diag::OPF_XML, &path, None, "not UTF-8");
        return None;
    };
    let doc = match xml::parse(text) {
        Ok(doc) => doc,
        Err(e) => {
            d.push(diag::OPF_XML, &path, Some(e.line), e.message);
            return None;
        }
    };
    let root = &doc.root;
    if !root.is(OPF, "package") {
        d.push(
            diag::OPF_XML,
            &path,
            Some(root.line),
            "root element is not opf:package",
        );
        return None;
    }
    let version = root.attr("version").unwrap_or("").to_owned();
    if !version.starts_with("3.") {
        d.push(
            diag::OPF_VERSION,
            &path,
            Some(root.line),
            format!("version {version:?}"),
        );
    }
    let child = |name: &str| root.elements().find(|e| e.is(OPF, name));
    let meta = metadata(root, child("metadata"), &path, d);
    let manifest = manifest(c, &path, child("manifest"), d);
    let nav_count = manifest.iter().filter(|i| i.has_property("nav")).count();
    if nav_count != 1 {
        d.push(
            diag::OPF_NAV,
            &path,
            None,
            format!("{nav_count} items declare `nav`"),
        );
    }
    let spine_el = child("spine");
    let page_progression = spine_el
        .and_then(|s| s.attr("page-progression-direction"))
        .and_then(Dir::parse);
    let spine = spine(&path, spine_el, &manifest, d);

    let package = Package {
        path,
        version,
        identifier: meta.identifier,
        titles: meta.titles,
        languages: meta.languages,
        creators: meta.creators,
        modified: meta.modified,
        lang: root.attr_ns(XML, "lang").map(str::to_owned),
        dir: root.attr("dir").and_then(Dir::parse),
        manifest,
        spine,
        page_progression,
    };
    tracing::debug!(
        items = package.manifest.len(),
        spine = package.spine.len(),
        "package parsed"
    );
    Some(package)
}

fn manifest(c: &Container, opf: &str, el: Option<&Element>, d: &mut Diagnostics) -> Vec<Item> {
    let mut ids = BTreeSet::new();
    let mut out = Vec::new();
    for e in el
        .into_iter()
        .flat_map(|m| m.elements().filter(|e| e.is(OPF, "item")))
    {
        let id = e.attr("id").unwrap_or("").to_owned();
        let href = e.attr("href").unwrap_or("").to_owned();
        if !ids.insert(id.clone()) {
            d.push(
                diag::OPF_DUPLICATE_ID,
                opf,
                Some(e.line),
                format!("id {id:?}"),
            );
            continue;
        }
        let path = if is_external(&href) {
            None
        } else {
            let p = resolve(dir_of(opf), opf, &href).map(|t| t.path);
            match &p {
                None => d.push(diag::OPF_HREF, opf, Some(e.line), format!("href {href:?}")),
                Some(p) if !c.contains(p) => {
                    d.push(
                        diag::OPF_MISSING,
                        opf,
                        Some(e.line),
                        format!("{p} (item {id:?})"),
                    );
                }
                Some(_) => {}
            }
            p
        };
        out.push(Item {
            id,
            href,
            path,
            media_type: e.attr("media-type").unwrap_or("").to_owned(),
            properties: e.tokens(None, "properties").map(str::to_owned).collect(),
        });
    }
    out
}

fn spine(opf: &str, el: Option<&Element>, manifest: &[Item], d: &mut Diagnostics) -> Vec<SpineRef> {
    let mut out = Vec::new();
    for e in el
        .into_iter()
        .flat_map(|s| s.elements().filter(|e| e.is(OPF, "itemref")))
    {
        let idref = e.attr("idref").unwrap_or("").to_owned();
        match manifest.iter().find(|i| i.id == idref) {
            None => {
                d.push(
                    diag::OPF_SPINE_REF,
                    opf,
                    Some(e.line),
                    format!("idref {idref:?}"),
                );
                continue;
            }
            Some(item) if item.media_type != XHTML_MEDIA_TYPE => {
                d.push(
                    diag::OPF_SPINE_TYPE,
                    opf,
                    Some(e.line),
                    format!("{idref}: {}", item.media_type),
                );
                continue;
            }
            Some(_) => {}
        }
        out.push(SpineRef {
            idref,
            linear: e.attr("linear") != Some("no"),
        });
    }
    if !out.iter().any(|s| s.linear) {
        d.push(
            diag::OPF_EMPTY_SPINE,
            opf,
            el.map(|e| e.line),
            "nothing to read",
        );
    }
    out
}
