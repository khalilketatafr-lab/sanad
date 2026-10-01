//! P1 at the schema level: the Folio payload schema must never be able to
//! carry text. A `string` (or `[string]`) field would let Unicode travel to
//! the client inside a chunk, so its mere presence fails the build.

use std::fs;
use std::path::Path;

#[test]
fn folio_schema_declares_no_string_fields() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/folio.fbs");
    let schema =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut offenders = Vec::new();
    for (n, raw) in schema.lines().enumerate() {
        let line = raw.split("//").next().unwrap_or("").trim();
        // field declarations look like `name: type ...;`
        if let Some((_, ty)) = line.split_once(':') {
            let ty = ty.trim_start();
            if ty.starts_with("string") || ty.starts_with("[string") {
                offenders.push(format!("line {}: {}", n + 1, raw.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "P1: string fields are forbidden in folio.fbs:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn generated_bindings_match_schema_revision() {
    // Guard against editing the schema without regenerating (tools/gen-folio.sh).
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = fs::read_to_string(dir.join("src/folio_generated.rs")).unwrap_or_default();
    for symbol in [
        "kashida_max",
        "bidi_levels",
        "GlyphInstance",
        "PAYLOAD_IDENTIFIER: &str = \"FOL1\"",
    ] {
        assert!(
            generated.contains(symbol),
            "generated code is stale: missing `{symbol}`; run tools/gen-folio.sh"
        );
    }
    assert!(
        !generated.contains("&'a str"),
        "generated code exposes a string accessor; P1 violation"
    );
}
