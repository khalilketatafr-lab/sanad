# Typographic QA checklist

Every edition passes this checklist before it ships (roadmap §10.7). The
automated half lives in `crates/atelier/src/qa.rs`, and each check carries
the id of its row below. `crates/atelier/tests/qa_checklist.rs` keeps the two
in sync: if an id or severity disagrees, the test fails. The manual
half is done by an editor, on the edition as readers will see it.

```sh
cargo run -q --release -p sanad-atelier --example qa_report -- \
    --lang en|fr|ar [--unwrap] [--json report.json] <source.txt>
```

The source is plain text with paragraphs separated by blank lines.
`--unwrap` reads Project Gutenberg plain text. The report counts findings per
check and prints located examples. It exits 1 while any blocker or major
finding remains. Reports quote the text, so they are ingest tooling and never
leave the server (P1).

**Sign-off.** A title moves to `ready` (see `catalog/launch/README.md`) when
all three of these hold:

- there are no blocker findings;
- every major finding is fixed or waived in writing, with the reason;
- an editor has ticked every manual check that applies to the title's
  `features`.

## Automated checks

Scope "all" means every paragraph. Arabic checks fire on Arabic context,
whatever the book's language, so an Arabic quotation in an English book is
checked too.

Layout checks set each paragraph with the Compositor at three reading
measures: `narrow` (15 em, a phone at the largest text size), `phone` (19 em)
and `wide` (32 em, a tablet column). The output depends only on the
measure-to-em ratio, so these cover every font size.

### Source

| Id | Check | Scope | Severity | Pass when |
|---|---|---|---|---|
| TQ-S1 | Unicode NFC; no presentation forms (U+FB00–FDFF, U+FE70–FEFE) | all | blocker | Atelier normalizes on ingest, so a finding means the source bypassed it, or the source is OCR output (ﬁ, ﻻ) |
| TQ-S2 | No control or invisible characters: soft hyphen, ZWSP, BOM, U+FFFD, explicit bidi embeddings, line breaks inside a paragraph | all | blocker | The Compositor owns hyphenation and direction. U+FFFD means mojibake |
| TQ-S3 | Every character has a glyph in the face the Typesetter picks for it, fallback included | all | blocker | No `.notdef` anywhere |
| TQ-S4 | No double spaces or tabs | all | minor | Normalized on ingest |
| TQ-S5 | No markup residue: wikitext (`[[`, `{{`), HTML (`<ref`, `</`, `<br`), entities, `_emphasis_` underscores | all | major | Emphasis comes from the source's structure (Standard Ebooks or Gutenberg HTML), never from plain-text markers |

### Latin

| Id | Check | Scope | Severity | Pass when |
|---|---|---|---|---|
| TQ-L1 | No straight quotes `"` `'` | all | major | Elision apostrophes (’em, o’) are ’, not ‘ |
| TQ-L2 | No `--` and no spaced hyphen standing in for a dash | all | major | |
| TQ-L3 | No `...` for an ellipsis | all | minor | |

### French

| Id | Check | Scope | Severity | Pass when |
|---|---|---|---|---|
| TQ-F1 | Imprimerie nationale spacing: U+202F (or U+00A0) before ; ! ? », U+00A0 before :, U+00A0 after « | fr | major | A breakable space would let the punctuation start a line |
| TQ-F2 | First-level quotations in « »; “ ” only nested inside them | fr | minor | |
| TQ-F3 | œ in cœur, sœur, œuvre, œil, bœuf, vœu, nœud, œuf, mœurs, chœur… | fr | minor | Capital accents (É, À) are checked by hand under TQ-M1 |

### Arabic

| Id | Check | Scope | Severity | Pass when |
|---|---|---|---|---|
| TQ-A1 | Arabic comma, semicolon and question mark (، ؛ ؟), not Latin `,` `;` `?`, after Arabic text | all | major | |
| TQ-A2 | No tatweel (U+0640) in the source | all | major | Kashida is the Compositor's to place. A frozen tatweel cannot reflow and breaks search |
| TQ-A3 | No space before ، ؛ ؟ . : ! after Arabic; a space after ، ؛ ؟ | all | major | Arabic punctuation never starts a line |
| TQ-A4 | No Persian forms in Arabic: ک ی ہ, Extended Arabic-Indic digits | ar | major | ی has no dots in final position and reads as ى |
| TQ-A5 | One digit set across the Arabic text (Western or Arabic-Indic) | ar | minor | Book-level: reported once |
| TQ-A6 | No doubled marks; at most three marks on one letter | all | major | Shadda plus a vowel is normal |

### Layout

| Id | Check | Scope | Severity | Pass when |
|---|---|---|---|---|
| TQ-P1 | No overfull line: the emergency pass was needed | all | major | Every measure |
| TQ-P2 | No loose line: a line's glue stretched past ratio 2 (TeX badness > 800, the relaxed pass) | all | minor | Reviewed against the justification default (Q-004) |
| TQ-P3 | At most two consecutive hyphenated lines | all | minor | |
| TQ-P4 | The last full line of a paragraph does not end in a hyphen | all | minor | |
| TQ-P5 | The last line is at least 1.5 em long, the next paragraph's indent | all | minor | |
| TQ-P6 | At most three kashida elongations per line | all | minor | Naskh favors a few long elongations over many short ones |
| TQ-P7 | No kashida on a paragraph's ragged last line | all | blocker | A justification bug if it ever fires |

## Manual checks

| Id | Check | Applies to |
|---|---|---|
| TQ-M1 | Collation: 20 random pages against the base edition, ≤ 1 error per 10,000 characters. Includes capital accents (É, À) and ligatures | `collation` sources |
| TQ-M2 | Italics, small caps, foreign-language spans and letter-spaced emphasis preserved from the source structure | all |
| TQ-M3 | Front matter belongs to the work, not to the source edition (Gutenberg headers, an edition's half-titles). Headings follow one hierarchy | all |
| TQ-M4 | Verse: one line per source line, turnovers hang-indented, no stanza split across pages, shared lines continued at the right column | `verse`, `drama` |
| TQ-M5 | Hemistichs: ṣadr and ʿajuz aligned in two columns across the poem. On narrow measures, stacked and indented consistently | `verse-hemistich` |
| TQ-M6 | Vocalized text: no mark clipped or colliding at any size, in every theme. Quranic quotations in ﴿ ﴾ | `vocalized` |
| TQ-M7 | Bidi: Latin words, numbers and units inside Arabic (and the reverse) read in the right order; brackets mirror | mixed-script titles |
| TQ-M8 | Notes: every anchor opens its own note, and notes follow reflow | `footnotes` |
| TQ-M9 | Illustrations and fixed figures: placement, captions, alt text for accessibility mode | `illustrations`, `shaped-text` |
| TQ-M10 | Block roles (epigraphs, datelines, signatures, speaker labels, space breaks) are styled consistently and survive page breaks | `epigraphs`, `epistolary`, `drama`, `section-breaks`, `numbered-sections` |
| TQ-M11 | Device pass: three chapters on three phones in all five themes, using the theme review page (`tools/theme-review`) | all |
| TQ-M12 | Pagination at the default profile: no widows or orphans, no heading at a page foot, no space break lost at a page boundary | all |

## First pass: fixtures and two Gutenberg sources

This run used `qa_report`, release build, Literata and Noto Naskh Arabic. The
French sources were unwrapped Gutenberg plain text, hyphenated with
`hyph-fr`. Blank cells mean no findings.

| Check | Fixture: P&P ch. I (en) | Fixture: Kalīla, Ring-Dove (ar) | Fixture: mixed (ar) | Candide, PG #4650 (fr) | Swann, PG #2650 (fr) |
|---|---|---|---|---|---|
| TQ-S5 |  |  |  | 190 | 2 |
| TQ-L2 |  |  |  |  | 834 |
| TQ-L3 |  |  |  | 3 | 75 |
| TQ-F1 |  |  |  | 1309 | 4196 |
| TQ-F3 |  |  |  | 67 |  |
| TQ-P1 |  |  |  | 13 | 32 |
| TQ-P2 | 36 |  | 3 | 644 | 1724 |
| TQ-P3 | 2 |  |  | 185 | 1030 |
| TQ-P4 | 13 |  |  | 264 | 753 |
| TQ-P5 |  |  |  | 14 | 4 |
| TQ-P6 |  | 359 | 22 |  |  |
| paragraphs | 34 | 10 | 4 | 385 | 1032 |

Layout counts are per paragraph per measure (three measures each). Swann is
992,963 characters; all checks, including three layouts per paragraph, run
in 1.4 s.

## Findings log

### Q-001: Latin text was hyphenated with English patterns whatever its language (fixed)

`Typesetter::new` always loaded en-US patterns, so French would have broken
at English points. Patterns now follow the Latin face's language
(`shape::patterns_for`): en-US is embedded, and French (`hyph-fr`, MIT) is
vendored in `crates/atelier/dictionaries/`. Any other language is set
unhyphenated rather than wrongly. On *Candide*, French patterns cut overfull
lines (TQ-P1) from 174 to 13 and loose paragraphs at the wide measure from
196 to 36.

### Q-002: Kashida is spread over every word of the line (open: Compositor)

`build_items` already limits a word to one kashida, at its best opportunity.
`justify` then gives every word on the line the same elongation ratio, so
the number of elongations equals the number of words: 4–8 per line on the
Arabic fixture (TQ-P6: 359 line-measures). Naskh practice is the opposite: a
few long elongations at the best positions on the line.

**Proposal:** a line's kashida stretch fills opportunities in priority-class
order. The best class takes its full `kashida_max` before the next class
starts, with a cap of three per line. Any remainder goes to glue. This
changes `justify` and the line-stretch sums in `break_paragraph`, so the
golden pages, the WASM parity fixture and the reflow digests are
regenerated in the same change.

### Q-003: No final-hyphen or consecutive-hyphen control (open: Compositor)

`BreakParams` has flagged demerits, but no counterpart to TeX's
`\finalhyphendemerits` and no hard ladder limit. On *Swann* at the phone
measure: 357 ladders of three or more lines (TQ-P3) and 270 hyphens on the
last full line (TQ-P4).

**Proposal:** add `final_hyphen_demerits` (TeX default 5000) and
`max_consecutive_hyphens = 2`, enforced by dropping candidate breaks.

### Q-004: Justified text at phone measures is loose by construction (open: product)

Boxes never stretch (no letterspacing; `crates/compositor/src/item.rs`), so
glue absorbs all slack. Paragraphs needing the relaxed pass:

| Source | narrow | phone | wide |
|---|---|---|---|
| P&P ch. I | 24 / 34 | 12 / 34 | 0 / 34 |
| Swann | 851 / 1032 | 767 / 1032 | 106 / 1032 |

**Recommendation:** Lumen defaults to ragged-right with hyphenation below
about 22 em and justifies at tablet measures. Readers can still turn
justification on. Arabic is unaffected because kashida absorbs the slack.

### Q-005: Gutenberg French lacks French typography (open: ingest normalizer)

- No French spacing anywhere (TQ-F1: *Candide* 1,309, *Swann* 4,196).
- `--` for dashes (TQ-L2: *Swann* 834).
- `OEUVRES` for Œ (TQ-F3).

Spacing and dashes are mechanical: insert U+202F or U+00A0 per TQ-F1, and
turn `--` into an em dash (a dialogue dash takes U+00A0 after it). Ligatures
and capital accents are restored during collation (TQ-M1).

### Q-006: Plain-text emphasis markers (open: ingest source format)

Gutenberg plain text marks italics as `_…_` (*Candide*: 190 TQ-S5 findings).
Ingest Gutenberg's HTML instead, where emphasis is structural. TQ-S5 stays to
guard plain-text sources and Wikisource wikitext.

## Per-title record

Once a title enters `qa`, record the run with this template:

```text
Title:        <id> @ <source revision>
qa_report:    <date>, <blocker>/<major>/<minor>, report JSON archived with the ingest run
Waivers:      <check id> × <n>: <reason>
Manual:       TQ-M1 … TQ-M12 as applicable: <editor>, <date>, notes
Devices:      <phone models> (TQ-M11)
```
