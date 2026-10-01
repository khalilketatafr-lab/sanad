# Canary book (server-side fixture)

`book.json` is a tiny test edition whose paragraphs embed every phrase in
`fixtures/canary/canaries.json`. Atelier-side tooling (the Folio fixture
generator in `crates/folio`) turns it into encrypted Folio chunks. The P1
harness then loads the reader on that edition and asserts that no canary
ever reaches the client.

This directory is **never** served to, bundled into, or copied into any
client build.
