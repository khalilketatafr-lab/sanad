//! Shows one word shaped into font glyph ids, then permuted to edition ids:
//!
//!   cargo run -q -p sanad-atelier --example shape_demo -- "العربية"
//!
//! Default word is "العربية". The permutation is seeded fixed here; a real
//! edition draws its seed from the OS CSPRNG. Server-side only (it prints the
//! source word); the edition ids are what the client would ever see (P1).

use std::str::FromStr;

use rustybuzz::{Face, Language};
use sanad_atelier::epub::Limits;
use sanad_atelier::ingest::{edition_permutation, typeset_chapter};
use sanad_atelier::permute::GlyphKey;
use sanad_atelier::shape::{FontFace, Typesetter};

const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let word = std::env::args().nth(1).unwrap_or_else(|| "العربية".into());
    let face = |d: &'static [u8]| Face::from_slice(d, 0).ok_or("unreadable font");
    let ts = Typesetter::new(
        FontFace {
            face: face(LATIN)?,
            size_px: 42.0,
            script: rustybuzz::script::LATIN,
            language: Language::from_str("en")?,
        },
        FontFace {
            face: face(ARABIC)?,
            size_px: 42.0,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar")?,
        },
    )?;

    // Reuse the intake→shape bridge by wrapping the word in a one-paragraph
    // chapter.
    let book = sanad_atelier::epub::intake(&one_paragraph_epub(&word)?, &Limits::default())?;
    let chapter = typeset_chapter(&ts, &book.chapters[0])?;
    let perm = edition_permutation(&ts, std::slice::from_ref(&chapter), [0x5A; 32])?;

    println!("word: {word}  (logical order, RTL un-reversed)\n");
    println!(
        "  {:<4} {:<8} {:<10} {:<10}",
        "pos", "font_gid", "advance", "π(edition)"
    );
    println!("  {:-<4} {:-<8} {:-<10} {:-<10}", "", "", "", "");
    for block in &chapter.blocks {
        for run in &block.shaped.runs {
            for (i, (&gid, &adv)) in run.gids.iter().zip(&run.advances).enumerate() {
                let pid = perm
                    .get(GlyphKey {
                        font: run.font,
                        gid,
                    })
                    .ok_or("glyph missing from permutation")?;
                println!("  {i:<4} {gid:<8} {adv:<10} {pid:<10}");
            }
        }
    }
    Ok(())
}

/// A minimal valid EPUB holding `word` in one Arabic paragraph.
fn one_paragraph_epub(word: &str) -> Result<Vec<u8>, Error> {
    use std::collections::BTreeMap;
    let mut files = BTreeMap::new();
    files.insert(
        "META-INF/container.xml".into(),
        br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="c.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.to_vec(),
    );
    files.insert(
        "c.opf".into(),
        br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="u"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="u">x</dc:identifier><dc:title>demo</dc:title><dc:language>ar</dc:language></metadata><manifest><item id="n" href="n.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#.to_vec(),
    );
    files.insert(
        "n.xhtml".into(),
        br#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="c.xhtml">c</a></li></ol></nav></body></html>"#.to_vec(),
    );
    files.insert(
        "c.xhtml".into(),
        format!(
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xml:lang="ar" dir="rtl"><body><p>{word}</p></body></html>"#
        )
        .into_bytes(),
    );
    Ok(sanad_atelier::epub::ocf::pack(&files)?)
}
