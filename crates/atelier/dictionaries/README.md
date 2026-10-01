# Hyphenation dictionaries

Knuth–Liang dictionaries that the `hyphenation` crate does not embed. Its
`embed_en-us` feature covers English only, and `embed_all` would build every
language into Atelier. These are loaded with `Standard::from_reader`
(`crates/atelier/src/shape.rs`).

| File | Language | Source | Licence |
|---|---|---|---|
| `fr.standard.bincode` | French (`fr`) | `hyphenation` 0.8.4 `dictionaries/fr.standard.bincode`, built from hyph-utf8 `hyph-fr.tex` V2.13 (2016/05/12) | MIT: see `LICENSE-hyph-fr.txt` |

The bincode layout belongs to `hyphenation` 0.8. Regenerate on a major
upgrade by copying the file from the new crate's `dictionaries/`. The unit
test `french_hyphenation_loads` fails if the format drifts.
