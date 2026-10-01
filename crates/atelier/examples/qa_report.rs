//! Runs the automated half of the typographic QA checklist
//! (`docs/qa/typographic-qa.md`) over a plain-text source:
//!
//!   cargo run -q -p sanad-atelier --example qa_report -- \
//!       --lang en|fr|ar [--unwrap] [--no-layout] [--json <out.json>] <source.txt>
//!
//! The source uses the fixtures' model: paragraphs separated by blank lines.
//! `--unwrap` first joins hard-wrapped lines (Project Gutenberg plain text)
//! and drops the Gutenberg header and licence. Prints a summary per check
//! with located examples; exits 1 when a blocker or major finding remains.
//!
//! Faces are the Phase 0 pair (Literata, Noto Naskh Arabic). Latin text is
//! hyphenated only when patterns for its language exist (see
//! `Typesetter::new`).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::{env, fs, path::PathBuf, process::ExitCode};

use rustybuzz::{Face, Language};
use sanad_atelier::qa::{Check, Finding, Lang, QaOptions, Report, check_book};
use sanad_atelier::shape::{FontFace, Typesetter};

const LATIN_FONT: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC_FONT: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");
/// Examples printed per check.
const EXAMPLES: usize = 3;

type Error = Box<dyn std::error::Error>;

struct Args {
    lang: Lang,
    unwrap: bool,
    layout: bool,
    json: Option<PathBuf>,
    source: PathBuf,
}

fn parse_args() -> Result<Args, Error> {
    let mut lang = None;
    let mut unwrap = false;
    let mut layout = true;
    let mut json = None;
    let mut source = None;
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => {
                let tag = it.next().ok_or("--lang needs a value")?;
                lang = Some(Lang::from_tag(&tag).ok_or("--lang is en, fr or ar")?);
            }
            "--unwrap" => unwrap = true,
            "--no-layout" => layout = false,
            "--json" => json = Some(PathBuf::from(it.next().ok_or("--json needs a path")?)),
            _ if a.starts_with("--") => return Err(format!("unknown option {a}").into()),
            _ => source = Some(PathBuf::from(a)),
        }
    }
    Ok(Args {
        lang: lang.ok_or("--lang is required")?,
        unwrap,
        layout,
        json,
        source: source.ok_or("a source file is required")?,
    })
}

fn typesetter(lang: Lang) -> Result<Typesetter<'static>, Error> {
    let face = |data: &'static [u8]| Face::from_slice(data, 0).ok_or("unreadable font");
    // An Arabic book's Latin quotations are set as English.
    let latin_lang = if lang == Lang::Fr { "fr" } else { "en" };
    Ok(Typesetter::new(
        FontFace {
            face: face(LATIN_FONT)?,
            size_px: 36.0,
            script: rustybuzz::script::LATIN,
            language: Language::from_str(latin_lang)?,
        },
        FontFace {
            face: face(ARABIC_FONT)?,
            size_px: 42.0,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar")?,
        },
    )?)
}

/// Project Gutenberg plain text → the paragraph model: body only, wrapped
/// lines joined, blank-line paragraphs kept.
fn unwrap_gutenberg(raw: &str) -> String {
    let raw = raw.replace("\r\n", "\n");
    let body = raw
        .find("*** START OF")
        .and_then(|s| raw[s..].find('\n').map(|n| s + n + 1))
        .map_or(raw.as_str(), |s| &raw[s..]);
    let body = body.find("*** END OF").map_or(body, |e| &body[..e]);
    body.split("\n\n")
        .map(|p| p.split('\n').map(str::trim).collect::<Vec<_>>().join(" "))
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 1-based line and column (in characters) of a byte offset.
fn locate(source: &str, byte: usize) -> (usize, usize) {
    let before = &source[..byte];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rfind('\n')
        .map_or(before, |n| &before[n + 1..])
        .chars()
        .count()
        + 1;
    (line, col)
}

/// Up to 24 characters either side of `byte`, on one line.
fn context(source: &str, byte: usize) -> String {
    let start = source[..byte]
        .char_indices()
        .rev()
        .nth(23)
        .map_or(0, |(b, _)| b);
    let end = source[byte..]
        .char_indices()
        .nth(24)
        .map_or(source.len(), |(b, _)| byte + b);
    source[start..end].replace('\n', "⏎")
}

fn print_report(source: &str, report: &Report) {
    let mut by_check: BTreeMap<Check, Vec<&Finding>> = BTreeMap::new();
    for f in &report.findings {
        by_check.entry(f.check).or_default().push(f);
    }
    println!(
        "{} paragraphs, {} characters, hyphenation: {}",
        report.paragraphs,
        report.characters,
        report.hyphenation.as_deref().unwrap_or("none")
    );
    for check in Check::ALL {
        let Some(found) = by_check.get(&check) else {
            continue;
        };
        println!(
            "{} {:<7} {:>6}",
            check.id(),
            format!("{:?}", check.severity()).to_lowercase(),
            found.len()
        );
        for f in found.iter().take(EXAMPLES) {
            let (line, col) = locate(source, f.byte);
            let measure = f.measure.map(|m| format!(" @{m}")).unwrap_or_default();
            println!(
                "    {line}:{col}{measure}  {}  «{}»",
                f.note,
                context(source, f.byte)
            );
        }
    }
    println!(
        "{}: {} blocker, {} major, {} minor",
        if report.passes() { "PASS" } else { "FAIL" },
        report.count(sanad_atelier::qa::Severity::Blocker),
        report.count(sanad_atelier::qa::Severity::Major),
        report.count(sanad_atelier::qa::Severity::Minor),
    );
}

fn run() -> Result<bool, Error> {
    let args = parse_args()?;
    let raw = fs::read_to_string(&args.source)?;
    let source = if args.unwrap {
        unwrap_gutenberg(&raw)
    } else {
        raw
    };
    let ts = typesetter(args.lang)?;
    let report = check_book(
        &ts,
        args.lang,
        &source,
        QaOptions {
            layout: args.layout,
        },
    )?;
    print_report(&source, &report);
    if let Some(path) = args.json {
        fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(report.passes())
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("qa_report: {e}");
            ExitCode::from(2)
        }
    }
}
