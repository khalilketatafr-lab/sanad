# Folio fuzzing

Two `cargo-fuzz` targets keep the client-facing decoding path total over
hostile input:

| Target | Exercises | Invariants |
|---|---|---|
| `chunk_header` | `ChunkHeader::parse`, `SealedChunk::parse` | No panics or OOB. `encode(parse(h)) == h`. AAD, sealed-body, ciphertext and tag slices tile the input exactly. |
| `payload` | `decompress` (zstd, bomb-bounded) → `verify_payload` (FlatBuffer verifier + structural validation), for flow and page kinds | No panics. Output ≤ `MAX_PAYLOAD_LEN`. |

## Setup (once)

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
```

## Run

From `crates/folio`:

```sh
cargo +nightly fuzz run chunk_header -- -max_total_time=300
cargo +nightly fuzz run payload      -- -max_total_time=300 -max_len=65536 -rss_limit_mb=2048
```

Seed the payload corpus with real chunk payloads to reach deep FlatBuffer
paths quickly:

```sh
cargo test -p sanad-folio --all-features   # sanity
mkdir -p fuzz/corpus/payload               # then copy decompressed fixture payloads here, prefixed with one selector byte
```

Crashes are written to `fuzz/artifacts/<target>/`. Reproduce with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`, then turn
each one into a regular unit test in `src/codec.rs` or `src/payload.rs` before
fixing it.

`fuzz/corpus` and `fuzz/artifacts` are git-ignored. CI runs both targets for
60 seconds on every PR that touches `crates/folio` (see `.github/workflows/ci.yml`).
