//! EPUB intake against the sample book (`fixtures/epub/sanad-sample`) and
//! against broken variants of it, one per validation rule.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::Path;

use sanad_atelier::epub::blocks::Block;
use sanad_atelier::epub::diag::{self, Code};
use sanad_atelier::epub::{
    self, BlockKind, Book, Content, Dir, DirSource, Inline, Limits, Link, Style, ocf, tree,
};

const SAMPLE: &[u8] = include_bytes!("../../../fixtures/epub/sanad-sample.epub");
const SOURCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/epub/sanad-sample"
);
const TREE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/epub/sanad-sample.tree.txt"
);

fn source_files() -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(Path::new(SOURCE), Path::new(SOURCE), &mut out);
    out
}

fn sample() -> Book {
    epub::intake(SAMPLE, &Limits::default()).unwrap()
}

/// The sample with one file edited.
fn variant(path: &str, edit: impl FnOnce(String) -> String) -> Result<Book, epub::Rejected> {
    let mut files = source_files();
    let text = String::from_utf8(files.remove(path).unwrap()).unwrap();
    files.insert(path.to_owned(), edit(text).into_bytes());
    epub::intake(&ocf::pack(&files).unwrap(), &Limits::default())
}

fn codes(book: &Book) -> Vec<&'static str> {
    book.diagnostics.iter().map(|d| d.code.id).collect()
}

fn has(book: &Book, code: Code) -> bool {
    book.diagnostics.has(code)
}

fn find(book: &Book, pred: impl Fn(&Block) -> bool) -> Vec<&Block> {
    let mut out = Vec::new();
    for ch in &book.chapters {
        ch.walk(&mut |b, _| {
            if pred(b) {
                out.push(b);
            }
        });
    }
    out
}

#[test]
fn committed_epub_is_the_packed_source() {
    let book = sample();
    let mut packed: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for p in book.container.paths() {
        packed.insert(p.to_owned(), book.container.get(p).unwrap().to_vec());
    }
    packed.remove("mimetype");
    assert_eq!(
        packed,
        source_files(),
        "repack: cargo run -p sanad-atelier --bin atelier -- pack --input fixtures/epub/sanad-sample --output fixtures/epub/sanad-sample.epub"
    );
}

#[test]
fn sample_is_ingestible_with_only_the_expected_notes() {
    let book = sample();
    assert!(book.is_ingestible());
    assert_eq!(codes(&book), ["XHT-004", "NRM-001", "NRM-002"]);
    assert_eq!(book.chapters.len(), 2);
    assert_eq!(book.paragraph_count(), 10);
    assert_eq!(book.package.page_progression, Some(Dir::Rtl));
    assert_eq!(book.toc.len(), 2);
    assert_eq!(
        book.toc[1].children[0].target.as_deref(),
        Some("OEBPS/text/c2.xhtml#fig-ornament")
    );
}

#[test]
fn direction_and_language_resolution() {
    let book = sample();
    let c1 = &book.chapters[0];
    assert_eq!((c1.dir, c1.dir_source), (Dir::Rtl, DirSource::Inherited));
    let epigraph = find(&book, |b| b.epub_type.iter().any(|t| t == "epigraph"));
    assert_eq!(epigraph.len(), 1);
    assert_eq!(
        (epigraph[0].dir, epigraph[0].dir_source),
        (Dir::Ltr, DirSource::Attribute)
    );
    assert_eq!(epigraph[0].lang.as_deref(), Some("en"));
    let paras = find(&book, |b| b.kind == BlockKind::Paragraph);
    // The <p lang="ar"> without dir in an undirected English document.
    let detected: Vec<_> = paras
        .iter()
        .filter(|b| b.dir == Dir::Rtl && b.dir_source == DirSource::Detected)
        .collect();
    assert_eq!(detected.len(), 1);
    assert!(detected[0].text().starts_with("نقل عبد الله"));
    let english = paras
        .iter()
        .find(|b| b.text().starts_with("The Arabic title"))
        .unwrap();
    assert_eq!(
        (english.dir, english.dir_source),
        (Dir::Ltr, DirSource::Detected)
    );
    let Content::Inlines(items) = &english.content else {
        panic!("leaf");
    };
    let arabic_run = items
        .iter()
        .find_map(|i| match i {
            Inline::Text(r) if r.lang.as_deref() == Some("ar") => Some(r),
            _ => None,
        })
        .unwrap();
    assert_eq!(arabic_run.text, "كليلة ودمنة");
    assert_eq!(arabic_run.dir, Some(Dir::Rtl));
}

#[test]
fn inline_semantics_survive() {
    let book = sample();
    let noterefs: Vec<_> = find(&book, |_| true)
        .into_iter()
        .filter_map(|b| match &b.content {
            Content::Inlines(items) => Some(items),
            _ => None,
        })
        .flatten()
        .filter_map(|i| match i {
            Inline::Text(r) => match &r.link {
                Some(Link::NoteRef { path, fragment }) => Some((r, path, fragment)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(noterefs.len(), 1);
    let (run, path, fragment) = noterefs[0];
    assert_eq!(run.text, "١");
    assert!(run.style.contains(Style::SUPERSCRIPT));
    assert_eq!(
        (path.as_str(), fragment.as_deref()),
        ("OEBPS/text/c1.xhtml", Some("n1"))
    );
    assert_eq!(find(&book, |b| b.kind == BlockKind::Note).len(), 1);
    let items: Vec<_> = find(&book, |b| matches!(b.kind, BlockKind::ListItem { .. }))
        .iter()
        .map(|b| b.kind.clone())
        .collect();
    assert_eq!(
        items,
        (1..=4)
            .map(|n| BlockKind::ListItem { ordinal: Some(n) })
            .collect::<Vec<_>>()
    );
    assert_eq!(find(&book, |b| b.kind == BlockKind::SectionBreak).len(), 1);
    let images = find(&book, |b| matches!(b.kind, BlockKind::Image { .. }));
    assert_eq!(
        images[0].kind,
        BlockKind::Image {
            src: "OEBPS/images/ornament.png".into(),
            alt: "A fleuron".into()
        }
    );
    let verse = find(&book, |b| {
        b.kind == BlockKind::Paragraph && b.text().starts_with("قالَ الفيلسوفُ")
    });
    let Content::Inlines(items) = &verse[0].content else {
        panic!("leaf");
    };
    assert!(items.contains(&Inline::Break));
}

#[test]
fn text_is_normalized() {
    let book = sample();
    let p = find(&book, |b| b.text().starts_with("The Arabic title"))[0].text();
    assert!(p.contains("Kalīla"), "i + U+0304 recomposed to U+012B");
    assert!(p.contains("quarrel"), "soft hyphen removed");
    assert!(
        !p.contains("  ") && !p.contains('\n'),
        "whitespace collapsed"
    );
}

#[test]
fn tree_diagram_snapshot() {
    let rendered = tree::book(&sample());
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::write(TREE, &rendered).unwrap();
    }
    let expected = std::fs::read_to_string(TREE).expect("UPDATE_SNAPSHOT=1 to create");
    assert_eq!(rendered, expected, "UPDATE_SNAPSHOT=1 to accept");
}

// ── Validation rules ─────────────────────────────────────────────────────

const OPF: &str = "OEBPS/content.opf";
const C1: &str = "OEBPS/text/c1.xhtml";
const C2: &str = "OEBPS/text/c2.xhtml";

fn errs(book: &Book) -> bool {
    !book.is_ingestible()
}

#[test]
fn opf_rules() {
    let b = variant(OPF, |t| t.replace("idref=\"c2\"", "idref=\"c9\"")).unwrap();
    assert!(errs(&b) && has(&b, diag::OPF_SPINE_REF));
    assert_eq!(b.chapters.len(), 1);
    let b = variant(OPF, |t| t.replace("text/c2.xhtml", "text/missing.xhtml")).unwrap();
    assert!(has(&b, diag::OPF_MISSING));
    let b = variant(OPF, |t| t.replace(" properties=\"nav\"", "")).unwrap();
    assert!(has(&b, diag::OPF_NAV));
    let b = variant(OPF, |t| t.replace("version=\"3.0\"", "version=\"2.0\"")).unwrap();
    assert!(has(&b, diag::OPF_VERSION));
    let b = variant(OPF, |t| {
        t.replace("unique-identifier=\"uid\"", "unique-identifier=\"nope\"")
    })
    .unwrap();
    assert!(has(&b, diag::OPF_UID));
    let b = variant(OPF, |t| {
        t.lines()
            .filter(|l| !l.contains("dc:title"))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap();
    assert!(has(&b, diag::OPF_METADATA));
    let b = variant(OPF, |t| {
        t.replace("href=\"css/sanad.css\"", "href=\"../../x.css\"")
    })
    .unwrap();
    assert!(has(&b, diag::OPF_HREF));
    let b = variant(OPF, |t| t.replace("<item id=\"c2\"", "<item id=\"c1\"")).unwrap();
    assert!(has(&b, diag::OPF_DUPLICATE_ID));
    let b = variant(OPF, |t| t.replace("idref=\"c1\"/>", "idref=\"css\"/>")).unwrap();
    assert!(has(&b, diag::OPF_SPINE_TYPE));
    let e = variant(OPF, |t| t.replace("</package>", "")).unwrap_err();
    assert!(e.diagnostics.has(diag::OPF_XML));
}

#[test]
fn content_document_rules() {
    let b = variant(C2, |t| t.replace("</figure>", "")).unwrap();
    assert!(errs(&b) && has(&b, diag::XHT_XML));
    assert_eq!(
        b.chapters.len(),
        1,
        "the broken chapter is reported, the rest read"
    );
    let b = variant(C2, |t| {
        t.replace("Dimna and", "Dimna&nbsp;and")
            .replace("names two", "names&nbsp;two")
    })
    .unwrap();
    assert!(has(&b, diag::XHT_ENTITY));
    let b = variant(C2, |t| t.replace("ornament.png", "missing.png")).unwrap();
    assert!(has(&b, diag::XHT_RESOURCE));
    let b = variant(C2, |t| {
        t.replace(
            "</section>",
            "<table><tr><td>a</td><td>b</td></tr></table><svg xmlns=\"http://www.w3.org/2000/svg\"/></section>",
        )
    })
    .unwrap();
    assert!(b.is_ingestible(), "unsupported content is a warning");
    assert_eq!(
        b.diagnostics
            .iter()
            .filter(|d| d.code == diag::XHT_UNSUPPORTED)
            .count(),
        2
    );
    let b = variant(C1, |t| {
        t.replace("href=\"#n1\"", "href=\"elsewhere.xhtml#n1\"")
    })
    .unwrap();
    assert!(has(&b, diag::XHT_LINK));
}

#[test]
fn nav_rules() {
    let b = variant("OEBPS/nav.xhtml", |t| {
        t.replace("epub:type=\"toc\"", "epub:type=\"lot\"")
    })
    .unwrap();
    assert!(has(&b, diag::NAV_TOC));
    assert!(b.toc.is_empty());
}

#[test]
fn not_an_epub() {
    let e = epub::intake(b"%PDF-1.7", &Limits::default()).unwrap_err();
    assert!(e.diagnostics.has(diag::OCF_ZIP));
}

#[test]
fn zero_width_no_break_space_is_kept_as_word_joiner() {
    // Standard Ebooks: `said\u{FEFF}—` keeps the dash off the next line.
    let b = variant(C2, |t| t.replace("names two", "names&#xFEFF;—two")).unwrap();
    let p = find(&b, |x| {
        x.kind == BlockKind::Paragraph && x.text().starts_with("The Arabic title")
    })[0]
        .text();
    assert!(p.contains("names\u{2060}—two"), "{p}");
    assert!(
        !b.diagnostics
            .iter()
            .any(|d| d.code == diag::NRM_INVISIBLE && d.path == C2 && d.message.starts_with('2'))
    );
}
