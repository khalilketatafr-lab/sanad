//! `atelier`: Sanad's ingestion pipeline, server-side.
//!
//! ```text
//! atelier inspect --input book.epub [--json report.json] [--chapter N]
//! atelier pack --input dir/ --output book.epub
//! ```
//!
//! `inspect` runs intake (OCF, OPF, nav, XHTML normalization), prints the
//! block tree and the validation report, and exits 1 if the EPUB cannot be
//! ingested. Its output contains the book's text: it is an editor's tool and
//! never runs where readers can reach it (P1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use aws_lc_rs::digest::{SHA256, digest};
use clap::{Parser, Subcommand, ValueEnum};
use rustybuzz::{Face, Language};
use sanad_atelier::atlas::AtlasParams;
use sanad_atelier::book_atlas::{PAGE0_COVERAGE, build_book_atlas};
use sanad_atelier::epub::{self, Diagnostics, Limits, Severity, ocf, tree};
use sanad_atelier::ingest::{edition_permutation, typeset_book};
use sanad_atelier::msdf::FieldParams;
use sanad_atelier::publish::seal_edition;
use sanad_atelier::shape::{FontFace, Typesetter};
use sanad_folio::seal::TitleMasterKey;
use tracing_subscriber::EnvFilter;

const DEFAULT_LATIN: &[u8] = include_bytes!("../../../../fixtures/fonts/literata/Literata-VF.ttf");
const DEFAULT_ARABIC: &[u8] =
    include_bytes!("../../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");
/// Dev signing seed; a real edition is signed with the publisher's key.
const DEV_SIGNING_SEED: [u8; 32] = [0x5A; 32];

#[derive(Debug, Parser)]
#[command(name = "atelier", version, about = "Sanad ingestion pipeline")]
struct Cli {
    /// Log format on stderr.
    #[arg(long, value_enum, global = true, default_value_t = LogFormat::Text)]
    log_format: LogFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate an EPUB and print its normalized block tree.
    Inspect {
        #[arg(long)]
        input: PathBuf,
        /// Also write the book (tree and diagnostics) as JSON.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Print only this spine position (0-based).
        #[arg(long)]
        chapter: Option<usize>,
    },
    /// Pack a directory into an OCF container (mimetype first, stored;
    /// reproducible).
    Pack {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Ingest an EPUB into sealed Folio chunks and an atlas under an
    /// object-storage layout. Dev keys by default — not for production.
    Ingest {
        #[arg(long)]
        input: PathBuf,
        /// Output root; the edition is written under `e/{hash}/v{n}` inside it.
        #[arg(long)]
        output: PathBuf,
        /// Edition UUID (default: the dev canary edition).
        #[arg(long, default_value = "00000000-0000-7000-8000-00000000ca7a")]
        edition: String,
        /// Edition version.
        #[arg(long, default_value_t = 1)]
        version: u32,
        /// Title master key: "dev" (0x42×32, the dev catalog key) or 64 hex chars.
        #[arg(long, default_value = "dev")]
        tmk: String,
        /// Latin OpenType font (default: the bundled Literata).
        #[arg(long)]
        latin_font: Option<PathBuf>,
        /// Arabic OpenType font (default: the bundled Noto Naskh Arabic).
        #[arg(long)]
        arabic_font: Option<PathBuf>,
    },
}

type Error = Box<dyn std::error::Error>;

fn init_logging(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().init(),
    }
}

fn print_diagnostics(d: &Diagnostics) {
    if d.is_empty() {
        println!("\nvalidation: clean");
        return;
    }
    println!(
        "\nvalidation: {} error, {} warning, {} info",
        d.count(Severity::Error),
        d.count(Severity::Warning),
        d.count(Severity::Info)
    );
    for x in d.iter() {
        let at = match x.line {
            Some(l) => format!("{}:{l}", x.path),
            None => x.path.clone(),
        };
        let ec = x
            .code
            .epubcheck
            .map(|e| format!(" ({e})"))
            .unwrap_or_default();
        println!(
            "  {:<7} {}{ec:<10} {at}  {}: {}",
            format!("{:?}", x.severity).to_lowercase(),
            x.code.id,
            x.code.summary,
            x.message
        );
    }
}

fn inspect(input: &Path, json: Option<&Path>, chapter: Option<usize>) -> Result<bool, Error> {
    let bytes = std::fs::read(input)?;
    let book = match epub::intake(&bytes, &Limits::default()) {
        Ok(book) => book,
        Err(rejected) => {
            println!("{}: rejected", input.display());
            print_diagnostics(&rejected.diagnostics);
            return Ok(false);
        }
    };
    match chapter {
        Some(n) => {
            let ch = book
                .chapters
                .get(n)
                .ok_or_else(|| format!("no chapter {n} (spine has {})", book.chapters.len()))?;
            print!("{}", tree::chapter(ch));
        }
        None => print!("{}", tree::book(&book)),
    }
    print_diagnostics(&book.diagnostics);
    if let Some(path) = json {
        std::fs::write(path, serde_json::to_vec_pretty(&book)?)?;
    }
    println!(
        "\n{}",
        if book.is_ingestible() {
            "ingestible"
        } else {
            "NOT ingestible"
        }
    );
    Ok(book.is_ingestible())
}

fn collect(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, root, out)?;
        } else {
            let rel = path
                .strip_prefix(root)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.insert(rel, std::fs::read(&path)?);
        }
    }
    Ok(())
}

fn pack(input: &Path, output: &Path) -> Result<(), Error> {
    let mut files = BTreeMap::new();
    collect(input, input, &mut files)?;
    let bytes = ocf::pack(&files)?;
    std::fs::write(output, &bytes)?;
    tracing::info!(files = files.len(), bytes = bytes.len(), output = %output.display(), "packed");
    Ok(())
}

fn parse_tmk(spec: &str) -> Result<TitleMasterKey, Error> {
    if spec == "dev" {
        return Ok(TitleMasterKey::from_bytes([0x42; 32]));
    }
    let bytes = (0..spec.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(spec.get(i..i + 2).ok_or("odd-length tmk")?, 16).map_err(Into::into)
        })
        .collect::<Result<Vec<u8>, Error>>()?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "tmk must be 64 hex chars")?;
    Ok(TitleMasterKey::from_bytes(arr))
}

fn typesetter(latin: Option<&Path>, arabic: Option<&Path>) -> Result<Typesetter<'static>, Error> {
    let load = |p: Option<&Path>, default: &'static [u8]| -> Result<&'static [u8], Error> {
        match p {
            Some(path) => Ok(Box::leak(std::fs::read(path)?.into_boxed_slice())),
            None => Ok(default),
        }
    };
    let latin = load(latin, DEFAULT_LATIN)?;
    let arabic = load(arabic, DEFAULT_ARABIC)?;
    let face = |d: &'static [u8]| Face::from_slice(d, 0).ok_or("unreadable font");
    Ok(Typesetter::new(
        FontFace {
            face: face(latin)?,
            size_px: 36.0,
            script: rustybuzz::script::LATIN,
            language: Language::from_str("en")?,
        },
        FontFace {
            face: face(arabic)?,
            size_px: 42.0,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar")?,
        },
    )?)
}

/// A 32-byte secret seed for an edition's permutation/atlas, domain-separated.
fn seed32(edition_id: &[u8; 16], domain: &[u8]) -> [u8; 32] {
    let mut m = Vec::with_capacity(16 + domain.len());
    m.extend_from_slice(edition_id);
    m.extend_from_slice(domain);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest(&SHA256, &m).as_ref());
    out
}

#[allow(clippy::too_many_arguments)]
fn ingest(
    input: &Path,
    output: &Path,
    edition: &str,
    version: u32,
    tmk_spec: &str,
    latin_font: Option<&Path>,
    arabic_font: Option<&Path>,
) -> Result<bool, Error> {
    let edition_id = uuid::Uuid::parse_str(edition)?;
    let tmk = parse_tmk(tmk_spec)?;
    let bytes = std::fs::read(input)?;
    let book = match epub::intake(&bytes, &Limits::default()) {
        Ok(b) if b.is_ingestible() => b,
        Ok(b) => {
            print_diagnostics(&b.diagnostics);
            eprintln!("atelier: {} has errors; not ingested", input.display());
            return Ok(false);
        }
        Err(rejected) => {
            print_diagnostics(&rejected.diagnostics);
            return Ok(false);
        }
    };
    let ts = typesetter(latin_font, arabic_font)?;
    let typeset = typeset_book(&ts, &book.chapters)?;
    let perm = edition_permutation(&ts, &typeset, seed32(edition_id.as_bytes(), b"permutation"))?;
    let atlas = build_book_atlas(
        &ts,
        &typeset,
        &perm,
        &FieldParams::default(),
        &AtlasParams::default(),
        PAGE0_COVERAGE,
        seed32(edition_id.as_bytes(), b"atlas"),
    )?;
    let sealed = seal_edition(
        &ts,
        &typeset,
        &perm,
        &atlas,
        &tmk,
        edition_id,
        version,
        &DEV_SIGNING_SEED,
    )?;
    sealed.write_dir(output)?;

    let bytes_written: usize = sealed.files.values().map(Vec::len).sum();
    println!(
        "ingested {} → {}/{}",
        input.display(),
        output.display(),
        sealed.prefix
    );
    println!(
        "  {} chapters · {} sealed chunks ({} variants) · {} atlas page(s) · {} glyphs",
        typeset.len(),
        sealed.manifest.chunks.len(),
        sealed.manifest.variants,
        sealed.manifest.atlas.len(),
        atlas.glyph_count(),
    );
    println!(
        "  {} objects, {bytes_written} bytes, Ed25519-signed manifest",
        sealed.files.len()
    );
    println!(
        "  manifest: {}/{}",
        output.display(),
        sealed.manifest_path()
    );
    if tmk_spec == "dev" {
        println!(
            "  note: dev key — register the title master key with the Kernel for real serving"
        );
    }
    Ok(true)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.log_format);
    let result = match &cli.command {
        Command::Inspect {
            input,
            json,
            chapter,
        } => inspect(input, json.as_deref(), *chapter),
        Command::Pack { input, output } => pack(input, output).map(|()| true),
        Command::Ingest {
            input,
            output,
            edition,
            version,
            tmk,
            latin_font,
            arabic_font,
        } => ingest(
            input,
            output,
            edition,
            *version,
            tmk,
            latin_font.as_deref(),
            arabic_font.as_deref(),
        ),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("atelier: {e}");
            ExitCode::from(2)
        }
    }
}
