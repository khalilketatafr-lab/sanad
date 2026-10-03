# Atelier intake: EPUB 3 → semantic block tree

Stage ① of the ingest pipeline (`crates/atelier/src/epub`). It turns a
publisher's EPUB 3 into a validated, normalized block tree that shaping
(stage ②) can consume. This is the last stage that holds Unicode text: past
shaping, nothing keeps it (P1).

```text
bytes ─ ocf::read ─▶ Container ─ opf::parse ─▶ Package ─┬─ nav::toc ─▶ TOC
        (limits,      (inflated    (metadata,           └─ xhtml::chapter × spine
         DRM, paths)   files)       manifest, spine)        ─▶ Chapter { blocks }
```

```sh
atelier inspect --input book.epub [--json book.json] [--chapter N]
atelier pack --input dir/ --output book.epub      # reproducible OCF container
```

`inspect` prints the block tree and the validation report. It exits 1 when
the EPUB cannot be ingested and 2 on I/O errors. Its output quotes the book's
text, so it is an editor's tool and never runs where readers can reach it.

## Validation

These rules are native to Atelier, not a wrapper around the Java EPUBCheck. The
EPUBCheck column gives the message id for the same condition, so a publisher
can match our report against theirs. EPUBCheck checks far more (full schemas,
CSS, accessibility metadata). These are the conditions that decide whether
Atelier can build an edition correctly and safely.

**Severity semantics:**

- **error:** the EPUB is not ingestible.
- **warning:** the edition is built, but something was dropped, flattened or
  guessed.
- **info:** a normalization was applied.

If the archive or package document is unreadable, intake stops with
`Rejected` and reports everything found up to that point. Otherwise
`intake` returns the whole `Book` with every diagnostic, so a single
report lists all the problems.

| Code | Severity | EPUBCheck | Condition |
|---|---|---|---|
| OCF-001 | error | — | not a readable ZIP archive |
| OCF-002 | error | PKG-006 | `mimetype` missing or not the first entry |
| OCF-003 | error | PKG-007 | `mimetype` is not exactly `application/epub+zip` |
| OCF-004 | error | — | `mimetype` is compressed |
| OCF-005 | error | — | `META-INF/container.xml` missing, unreadable or without a package rootfile |
| OCF-006 | error | — | entry name escapes the container (absolute, `..`, backslash, NUL) |
| OCF-007 | error | — | entry uses a compression method other than stored or deflate |
| OCF-008 | error | — | archive exceeds size, entry-count or compression-ratio limits |
| OCF-009 | error | — | content is encrypted (DRM); only font obfuscation is accepted |
| OCF-010 | error | — | two entries with the same name |
| OPF-001 | error | RSC-005 | package document is not well-formed |
| OPF-002 | error | — | not an EPUB 3 package (`version` must be 3.x) |
| OPF-003 | error | OPF-030 | `unique-identifier` does not reference a `dc:identifier` |
| OPF-004 | error | RSC-005 | required metadata missing (`dc:title`, `dc:language`) |
| OPF-005 | error | RSC-001 | manifest item's file is not in the container |
| OPF-006 | error | OPF-049 | spine `idref` not found in the manifest |
| OPF-007 | error | RSC-005 | not exactly one manifest item with the `nav` property |
| OPF-008 | error | — | spine item is not XHTML |
| OPF-009 | error | — | duplicate manifest `id` |
| OPF-010 | error | — | spine has no linear items |
| OPF-011 | error | — | manifest `href` is not a resolvable relative path |
| XHT-001 | error | RSC-005 | content document is not well-formed XML |
| XHT-002 | error | RSC-005 | undeclared entity (HTML named entities are not XML) |
| XHT-003 | warning | — | element not supported in Phase 1a; content dropped or flattened |
| XHT-004 | warning | — | right-to-left text with no `dir` in its ancestry; direction detected |
| XHT-005 | error | — | referenced image is not in the manifest |
| XHT-006 | warning | — | internal link target is not in the spine |
| NAV-001 | error | — | navigation document has no `toc` nav |
| NRM-001 | info | — | text normalized to NFC |
| NRM-002 | info | — | invisible characters removed (soft hyphen, ZWSP, bidi controls) |

## Container safety (OCF)

An archive is bounded before any XML is read:

| Limit | Default |
|---|---|
| Entries | 10,000 |
| Inflated size per entry | 64 MiB |
| Inflated size in total | 512 MiB |
| Inflation ratio (entries over 1 MiB) | 200 : 1 |

- Declared sizes are checked first. Inflation is then capped with `take`, so
  an entry whose header lies cannot exceed the limit.
- Entry names are rejected if they are absolute, contain `..` or `.`
  segments, backslashes, NUL bytes or a drive letter.
- Only the stored and deflate compression methods are accepted.
- ZIP-level encryption, and any `encryption.xml` algorithm other than font
  obfuscation (IDPF, Adobe), means DRM. Such an EPUB is refused rather than
  guessed at.
- XML parsing never fetches external entities or expands DTD declarations, so
  an EPUB cannot make Atelier read a file or blow up memory. Nesting deeper
  than 256 elements is refused.

## Normalization

Rules in the order they apply (`epub/xhtml.rs`):

1. **Block or inline.**
   - An element with block-level children becomes a container. Loose text
     beside those children becomes an anonymous paragraph, as with CSS
     anonymous block boxes.
   - Any other element becomes a leaf with inline content.
   - A leaf holding nothing but an image becomes an `Image` block.
   - Empty leaves are dropped.
2. **Direction** (`dir` and `dir_source` on every block):
   - `attr`: `dir="ltr|rtl"` on the element itself.
   - `inherited`: `dir` on an ancestor.
   - `detected`: `dir="auto"`, or no `dir` anywhere; the first strong
     character decides (UAX #9 P2–P3).
   - `default`: no strong character at all; left to right.

   A browser would show an undirected Arabic paragraph left to right. That is
   a source bug, so Atelier detects the direction and reports XHT-004.
   `page-progression-direction` is page order, never text direction.
3. **Language.**
   - Taken from `xml:lang` or `lang`, inherited, falling back to the first
     `dc:language`.
   - A run records a language only where it differs from its block's.
4. **Whitespace.**
   - Collapsed as with CSS `white-space: normal`, and trimmed at block and
     line edges.
   - `<pre>` keeps its whitespace, with newlines turned into breaks.
5. **Unicode.**
   - **Removed** (NRM-002): soft hyphens, ZWSP, and explicit bidi controls.
     The Compositor owns hyphenation and markup owns direction.
   - **Kept:** U+FEFF inside text becomes U+2060 WORD JOINER. Standard Ebooks
     uses it before em dashes (544 times in *Pride and Prejudice*) to keep a
     line from starting with the dash.
   - **NFC** (NRM-001): text is NFC-normalized.
6. **Quotations.** `<q>` gets the quotation marks of its language and
   nesting depth: `“ ”` and `‘ ’` for English, `«   »` for
   French, `« »` for Arabic.
7. **Unsupported content** is never dropped silently:
   - tables are flattened to paragraphs;
   - ruby annotations are dropped and the base text kept;
   - SVG, MathML, media and form controls are dropped.

   Each case is reported as XHT-003 with a count.

No publisher CSS is applied. `class` and `epub:type` tokens are kept on
blocks and runs so edition design can map them to Marginalia styles. The only
class read is `small-caps`/`smcap`, which sets the small-caps style.

## The block model

`epub/blocks.rs`:

| Block | Source |
|---|---|
| `Section` | `section`, `article`, `div`, `header`, `body`, `nav`… (containers) |
| `Heading(n)` | `h1`–`h6` |
| `Paragraph` | `p`, `dt`, `dd`, `td`, `th`, `summary`, `address`, anonymous text |
| `List{ordered,start}` / `ListItem(n)` | `ol`/`ul` and `li`, ordinals from `start` and `value` |
| `Blockquote`, `Figure`, `Caption` | `blockquote`, `figure`, `figcaption`/`caption` |
| `Image{src,alt}` | `img` standing alone (src resolved to a container path) |
| `Note` / `Aside` | `aside` with `epub:type` footnote, endnote, rearnote or note (or role `doc-footnote`/`doc-endnote`); any other `aside` |
| `SectionBreak` | `hr` |
| `Preformatted` | `pre` |

Inline content is a sequence of `Run`s, `Break`s (`<br/>`) and inline
`Image`s. A `Run` carries:

- `text`;
- `style`: italic, bold, small caps, superscript, subscript, code,
  underline, strike;
- `lang` and `dir` where they differ from the block;
- `link`: internal, external, or a note reference (`noteref`);
- `classes` and `epub_type`.

## Sample

`fixtures/epub/sanad-sample` is the source of `sanad-sample.epub`: 2
chapters and 10 paragraphs of Arabic and Latin text. It includes an English
epigraph inside an RTL chapter, a noteref with its footnote, a list, a
section break, a figure, a vocalized quotation with a `<br/>`, an
undirected Arabic paragraph, a soft hyphen and a decomposed `ī`.

`fixtures/epub/sanad-sample.tree.txt` is its tree, and
`crates/atelier/tests/epub_intake.rs` holds the snapshot plus one broken
variant per rule. Regenerate both with:

```sh
cargo run -p sanad-atelier --bin atelier -- pack --input fixtures/epub/sanad-sample --output fixtures/epub/sanad-sample.epub
UPDATE_SNAPSHOT=1 cargo test -p sanad-atelier --test epub_intake
```

**Real books** (release build):

| EPUB | Spine | Paragraphs | Intake | Report |
|---|---|---|---|---|
| Standard Ebooks, *Pride and Prejudice* | 65 | 2,057 | 56 ms | clean |
| Gutenberg EPUB3, *Candide* (#4650) | 36 | 364 | 14 ms | 2 warnings: one SVG cover wrapper, one table |

Chapter I of *Pride and Prejudice* normalizes to 34 paragraphs, the same
count as the hand-prepared `fixtures/typeset/latin.txt`.
