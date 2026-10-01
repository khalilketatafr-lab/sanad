# Launch catalog

`titles.toml` lists the first 50 titles (roadmap §10.7): 20 English, 12 French
and 18 Arabic. They are public-domain classics that readers already love. The
set is also chosen to cover the typographic structures Sanad has to handle,
from Proust's longest paragraphs to the Muʿallaqāt's two-hemistich verse.
`crates/atelier/src/catalog.rs` parses the manifest and enforces the rules
below. `cargo test -p sanad-atelier --test launch_catalog` fails if any title
breaks one.

## Rights

Each title must be free to publish in the EU, the US and the MENA markets in
`rights_as_of` (2026), the first year any title ships.

| Rule | Test | Why |
|---|---|---|
| Life + 70 | Every creator died ≤ `rights_as_of` − 71 | EU term; it runs to 31 December. It also covers the life + 50 MENA laws. Creators include translators, illustrators and the editors of the base edition. |
| *Mort pour la France* | That creator died ≤ `rights_as_of` − 101 | France adds 30 years (CPI L123-10). None of the 50 qualifies; the rule stops a later addition from slipping through. |
| Anonymous | Published ≤ `rights_as_of` − 71 | EU term for anonymous works. |
| US 95 years | `published` ≤ `rights_as_of` − 96 | For a translation, `published` is the translation's own date (Garnett's *Anna Karenina* counts from 1901, not 1878). |
| Base edition | `base_edition.year` ≤ `rights_as_of` − 96 | A modern critical edition (*taḥqīq*) can carry its own rights. We collate only against a print edition that is itself free. |

**Licences.** We accept a source only if it attaches no conditions:

- **Standard Ebooks:** CC0.
- **Project Gutenberg:** public domain in the US. The Gutenberg trademark and
  licence text are dropped on ingest.
- **Wikisource:** only the public-domain text itself. Wiki-original
  annotations and translations are CC BY-SA and never used.

Texts under CC BY or CC BY-SA 4.0 are excluded outright. Their "No downstream
restrictions" clause (BY §2(a)(5)(B), BY-SA §2(a)(5)(C)) forbids applying
effective technological measures, and P1 delivery (permuted glyphs, shredded
atlases, leases) is one. Gallica scans are used only to collate, never as a
source: BnF charges a licence fee for commercial reuse.

## Status

Each title moves through these stages in order:

| Status | Meaning | Count |
|---|---|---|
| `sourcing` | No complete, clean text yet. It needs transcription or proofreading from scans. | 4 (Arabic) |
| `collation` | A complete text exists. It still has to be collated against a public-domain print edition, which is recorded as `base_edition`. | 26 (12 French, 14 Arabic) |
| `typesetting` | The text is final, ready for Atelier ingest and design. Standard Ebooks texts start here because SE collates against page scans. | 20 (English) |
| `qa` | The automated checks pass and the manual checklist is in progress (`docs/qa/typographic-qa.md`). | 0 |
| `ready` | Signed off. | 0 |

A title cannot move past `collation` without a `base_edition` unless its source
is already collated. For French, Gutenberg's plain text is the starting point
only. *Candide* (#4650) has 1,309 TQ-F1 findings because French spacing is
missing throughout. That is restored mechanically. Accents on capitals and
the œ ligature (`OEUVRES`) are restored by hand during collation.

## Features and capabilities

`features` records the structures a title needs beyond running prose. The
table below shows what each one needs from the engine and where that stands.
The test `features_table_matches_the_manifest` checks that the title lists
match the manifest.

| Feature | Needs | Status | Titles |
|---|---|---|---|
| `verse` | Verse mode: one source line per line, no justification, turnovers hang-indented, stanzas never split across pages | Not built | `en-carroll-alice-in-wonderland`, `en-shakespeare-hamlet`, `en-khayyam-rubaiyat`, `fr-baudelaire-les-fleurs-du-mal`, `fr-rostand-cyrano-de-bergerac` |
| `verse-hemistich` | Arabic two-hemistich layout: ṣadr and ʿajuz as two equal columns (kashida-justified) with a fixed gap. Stacked, second indented, when the measure is too narrow | Not built | `ar-ibn-hazm-tawq-al-hamama`, `ar-jahiz-al-bukhala`, `ar-hamadhani-maqamat`, `ar-hariri-maqamat`, `ar-mutanabbi-selected-poems`, `ar-al-muallaqat`, `ar-maarri-risalat-al-ghufran`, `ar-shawqi-masraa-kliyubatra` |
| `drama` | Speaker labels, stage directions, and verse lines shared between speakers | Not built | `en-melville-moby-dick`, `en-shakespeare-hamlet`, `fr-rostand-cyrano-de-bergerac`, `ar-shawqi-masraa-kliyubatra` |
| `footnotes` | Note anchors in the glyph stream, notes as a sheet; anchors survive reflow | Not built | `en-melville-moby-dick`, `fr-voltaire-candide` |
| `illustrations` | Atelier image classification and art tiles (Phase 1a) | Not built | `en-carroll-alice-in-wonderland` |
| `shaped-text` | A fixed figure: a positioned block that never reflows | Not built | `en-carroll-alice-in-wonderland` |
| `epigraphs` | Block roles: epigraph and attribution (indent, size, alignment) | Not built | `en-eliot-middlemarch`, `en-fitzgerald-great-gatsby`, `fr-stendhal-le-rouge-et-le-noir` |
| `epistolary` | Block roles: dateline, salutation, signature, enclosed document | Not built | `en-shelley-frankenstein`, `en-stoker-dracula` |
| `dialect` | Text only: elision apostrophes stay ’ (TQ-L1, manual review) | Ready | `en-e-bronte-wuthering-heights`, `en-stoker-dracula`, `en-dickens-great-expectations`, `fr-zola-germinal` |
| `nested-quotation` | Text only: quotes reopened per paragraph, nesting alternates “ ‘ (manual review) | Ready | `en-conrad-heart-of-darkness` |
| `long` | Chunking and lease windows (Folio, Kernel) exist; TOC depth and sync anchors at 300k+ words are untested | Partial | `en-melville-moby-dick`, `en-eliot-middlemarch`, `en-tolstoy-anna-karenina`, `en-dostoevsky-crime-and-punishment`, `fr-stendhal-le-rouge-et-le-noir`, `fr-hugo-notre-dame-de-paris`, `fr-zola-germinal`, `fr-dumas-les-trois-mousquetaires`, `fr-proust-du-cote-de-chez-swann`, `ar-ibn-khaldun-muqaddima`, `ar-ibn-battuta-rihla` |
| `long-paragraphs` | Compositor reflow of a ~1,800-word paragraph within the 16 ms frame budget (bench before ship) | Untested | `fr-proust-du-cote-de-chez-swann` |
| `vocalized` | Dense harakat: shaping, MSDF marks and Arabic leading exist (mixed golden page); full-page vocalized verse is untested | Partial | `ar-hamadhani-maqamat`, `ar-hariri-maqamat`, `ar-mutanabbi-selected-poems`, `ar-al-muallaqat`, `ar-maarri-risalat-al-ghufran`, `ar-shawqi-masraa-kliyubatra` |
| `section-breaks` | Block role: space break, kept visible at page boundaries (ornament) | Not built | `en-woolf-mrs-dalloway` |
| `numbered-sections` | Block roles: book and section numbers as headings | Not built | `en-aurelius-meditations` |

Most of the "Not built" rows need one missing piece: **block roles** in
Folio. These are paragraph-level roles (verse line, hemistich pair, speaker,
stage direction, epigraph, attribution, dateline, signature, space break,
numbered heading) that the Compositor lays out and Lumen styles. Only text
and inline structure can be expressed today.

## First wave

These 17 titles are running prose with no special structure, so the Phase 1a
pipeline can set them as it stands:

- `en-austen-pride-and-prejudice`: Pride and Prejudice
- `en-c-bronte-jane-eyre`: Jane Eyre
- `en-wilde-dorian-gray`: The Picture of Dorian Gray
- `en-doyle-sherlock-holmes-adventures`: The Adventures of Sherlock Holmes
- `en-gibran-the-prophet`: The Prophet
- `fr-flaubert-madame-bovary`: Madame Bovary
- `fr-balzac-eugenie-grandet`: Eugénie Grandet
- `fr-verne-le-tour-du-monde`: Le Tour du monde en quatre-vingts jours
- `fr-colette-cheri`: Chéri
- `ar-ibn-al-muqaffa-kalila-wa-dimna`: كليلة ودمنة
- `ar-ibn-tufayl-hayy-ibn-yaqzan`: حي بن يقظان
- `ar-alf-layla-sindbad`: السندباد البحري
- `ar-ghazali-al-munqidh`: المنقذ من الضلال
- `ar-gibran-al-ajniha-al-mutakassira`: الأجنحة المتكسرة
- `ar-gibran-damaa-wa-ibtisama`: دمعة وابتسامة
- `ar-manfaluti-al-nazarat`: النظرات
- `ar-kawakibi-tabai-al-istibdad`: طبائع الاستبداد ومصارع الاستعباد

## Adding a title

1. Check the rights rules against the edition you will actually use. For a
   translation, that includes the translator's death date and the
   translation's publication date.
2. Pin the source: a Standard Ebooks URL, a Gutenberg ebook number, or a
   Wikisource page. Confirm it exists and note any incompleteness in `notes`.
3. Tag `features` and add the id to the table above. The test fails until
   you do.
4. Run `cargo test -p sanad-atelier --test launch_catalog`.
