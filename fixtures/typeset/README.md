# Typesetting fixtures

Inputs for Atelier's golden pages (`cargo run -p sanad-atelier --example golden_pages`)
and the Compositor reflow benchmark. Paragraphs are separated by blank lines.
These are **server-side inputs**: nothing here ships to the client, which only
ever receives permuted glyph ids and shredded atlases (P1).

| File | Text | Source | Status |
|---|---|---|---|
| `latin.txt` | Jane Austen, *Pride and Prejudice* (1813), chapter I | Project Gutenberg eBook #1342 | Public domain |
| `arabic.txt` | Ibn al-Muqaffaʿ, *Kalīla wa-Dimna*, "باب الحمامة المطوقة" (opening) | Arabic Wikisource, `كليلة ودمنة/باب الحمامة المطوقة` | Public domain (8th century) |
| `mixed.txt` | Original Arabic/English text written for these fixtures, plus a vocalized sentence adapted from *Kalīla wa-Dimna* | Sanad | CC0 |

Edits:

- **`latin.txt`:**
  - illustration captions removed;
  - `--` set as em dashes;
  - Gutenberg's `_italic_` markers dropped (the spike has no italic face).
- **`arabic.txt`:**
  - the source's spaces *before* punctuation (`كلمة :`) removed, per Arabic
    typographic convention;
  - two transcription typos fixed (`م نفس` → `من نفس`, `وأنظرو` → `وأنظر`);
  - the chapter's long source paragraphs split at sentence boundaries into
    ten paragraphs.

Fonts (SIL Open Font License 1.1, license files alongside):

- `../fonts/literata/Literata-VF.ttf`: Literata, the variable font
  `Literata[opsz,wght].ttf` from google/fonts.
- `../fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf`: Noto Naskh Arabic,
  `NotoNaskhArabic[wght].ttf` from google/fonts.
