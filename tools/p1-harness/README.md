# P1 harness

Enforces **P1: protected text never exists as Unicode on the client.**

Canary phrases (`fixtures/canary/canaries.json`) live only in server-side
fixture books. The harness fails CI if any canary, or any 4-word shingle of
one, is observed anywhere a client could hold it.

| Layer | What is scanned | Why it is needed |
|---|---|---|
| `static` | Every file in `apps/reader/dist`, including decompressed `.gz`/`.br`. Raw bytes (UTF-8, UTF-16LE, Latin-1, base64 at 3 alignments) **and** decoded text (JS `\uXXXX`, HTML entities, percent-encoding undone). | Bundlers escape non-ASCII. A naive grep misses `"يع…"`. |
| `dom` | `outerHTML`, `innerText` and open shadow roots of every frame, plus a CDP `DOM.getDocument(pierce)` walk. | The pierce walk reaches closed shadow roots and cross-frame documents. |
| `aria` | Full accessibility tree (CDP) and Playwright aria snapshot | What assistive technology (and scrapers using it) can read |
| `storage` | localStorage, sessionStorage, every IndexedDB store (strings, bytes, Blobs), Cache Storage bodies, OPFS, cookies | Offline caches must hold ciphertext only |
| `network` | HAR with embedded bodies plus live capture of every request and response body, URL and header, from the page and its workers | Plaintext on the wire |
| `console` | Console output | Debug logging of decoded content |
| `heap` | V8 heap snapshots of the main thread and every dedicated worker (CDP Target auto-attach) | Attribution: tells you *which* isolate holds a live string |
| `procmem` | Raw `/proc/<pid>/mem` of every browser process, before any GC | The authoritative check. It sees ArrayBuffers, WASM linear memory, freed-but-not-overwritten strings, Latin-1/UTF-16 string storage and long strings that heap snapshots truncate. |

Text matching normalizes both sides (NFKC, Arabic diacritics/tatweel stripped,
Arabic letter variants folded, invisibles and punctuation removed, case
folded), so re-encoded, re-shaped or partially copied canaries still match.

## Usage

```sh
pnpm --filter @sanad/reader build
node tools/p1-harness/src/cli.ts selftest --proc-mem require   # negative controls
node tools/p1-harness/src/cli.ts scan --config tools/p1-harness/p1.config.json --proc-mem require
```

The `procmem` layer needs permission to read browser memory. Run as root (CI
uses `sudo`), or with `kernel.yama.ptrace_scope ≤ 1` for descendant processes.
With `--proc-mem require`, a skipped sweep is a failure, never a pass.

Exit codes: `0` clean · `1` P1 violation · `2` harness integrity failure. An
integrity failure means the reader never became ready, a CSP or Trusted Types
violation occurred, the page was not cross-origin isolated, or a required
layer could not run.

## Negative controls

`selftest` serves a deliberately leaky site and asserts that each layer
catches its planted canary: DOM text and attributes, closed shadow root, ARIA
label, console, localStorage, IndexedDB (string and bytes), Cache Storage,
response body, request body, short and long JS strings, a Latin-1 one-byte
string, a worker heap, WASM linear memory and a UTF-16 ArrayBuffer. It also
asserts that a clean page produces **zero** findings. CI runs it before every
scan.

## Adding canaries

Add phrases to `fixtures/canary/canaries.json` and embed them in a fixture
book. Choose phrases that cannot occur naturally: invented proper nouns plus
unusual word sequences, in every script the platform supports. Never reference
canary text anywhere else in the repository.
