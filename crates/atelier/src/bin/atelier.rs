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

use clap::{Parser, Subcommand, ValueEnum};
use sanad_atelier::epub::{self, Diagnostics, Limits, Severity, ocf, tree};
use tracing_subscriber::EnvFilter;

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
