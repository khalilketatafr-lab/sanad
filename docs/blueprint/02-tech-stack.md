# 02 — Tech Stack & Justification

> Selection rule: **every tool must serve both security and premium UX, or
> it must be essential to one at no cost to the other.** Two languages, one
> layout engine, one policy brain.

---

## 1. Stack at a glance

| Layer | Choice | Security guarantee | UX guarantee |
|---|---|---|---|
| Security Kernel | **Rust** (Axum, Tokio, `aws-lc-rs`) | Memory-safe key handling. FIPS-validated crypto module available. No GC pauses with keys sitting in heap copies. | p95 `open` ≤ 150 ms, `renew` ≤ 80 ms |
| Policy | **Cedar** (Rust-native) | Declarative, analyzable authorization. Policies are code-reviewed and testable. | Policy changes (quotas, offline days) ship without redeploys |
| Ingest / typesetting | **Rust**: `rustybuzz`/HarfBuzz, `unicode-bidi`, `hyphenation`, `msdfgen`, MuPDF/PDFium | Shaping happens server-side, so Unicode never ships (D1) | Print-grade typography: kerning, ligatures, Arabic shaping, kashida |
| Layout core | **Rust → WASM** (Compositor) | Layout runs on glyph IDs only, so no text is needed on the client | Instant reflow. Server and client layout are byte-identical. |
| Search | **Tantivy** (Oracle service) | Index never leaves the server (D4) | Millisecond search with Arabic root and stem awareness |
| Reader renderer | **TypeScript + WebGL2** (GLSL ES 3.00) → WebGPU (WGSL) later | No DOM text. Shredded atlas. | 60/120 fps zoom, shader themes, page curl |
| Reader shell | **Vite + React 19 + React Aria Components** | Static, hash-pinned assets. Strict CSP without nonces. SRI. | Accessible, RTL-correct chrome out of the box |
| Marketing + library site | **Next.js** (App Router, RSC, ISR) | No content keys or protected text on this origin | SEO for every free book: the Phase 1 acquisition engine |
| Styling | **Tailwind CSS v4** (logical properties) + generated design tokens | — | One palette for the chrome and the shaders. RTL by construction. |
| Reader state | **XState** (reader lifecycle) + **Zustand** (settings) | Lease and policy states are explicit and testable: no "half-authorized" UI states | Predictable transitions, no spinners in impossible states |
| Database | **PostgreSQL 17** | Row-level security for publisher tenancy. TMKs stored only wrapped. | — |
| Ephemeral state | **Valkey** (open-source Redis fork) | Leases expire by TTL, so revocation is automatic | Sub-ms lease lookups |
| Object storage + CDN | **Cloudflare R2 + CDN**, with WAF and Turnstile | Ciphertext only. Opaque names. Invisible human attestation. | Global edge cache. Zero egress fees keep "free books" cheap at scale. |
| Keys | **AWS KMS** (FIPS 140-3 validated HSMs) | Root of trust never leaves the HSM. Audited `Decrypt`. | — |
| Compute | **AWS** (EKS or ECS Fargate), **OpenTofu** IaC | Private networking between Kernel, KMS and Postgres | Autoscaling |
| Schemas / RPC | **Protobuf + buf**. Connect protocol to the browser, gRPC (`tonic`) internally. | One typed contract across Rust and TS. No drift. | Small binary payloads |
| Multi-DRM *(Phase 2)* | Multi-DRM provider (e.g. castLabs DRMtoday, Axinom, EZDRM, DoveRunner) + Shaka Packager + Shaka Player | Widevine / PlayReady / FairPlay licensing without running three license servers | One player abstraction across browsers |
| Native shells *(Phase 2)* | **Capacitor** (iOS/Android), **Tauri 2** (desktop) | OS capture flags: `FLAG_SECURE`, `WDA_EXCLUDEFROMCAPTURE` | Same reader, native feel, store presence |
| Observability | **OpenTelemetry** → Grafana stack | Kernel audit trail. Anomaly dashboards. | Real-user frame-time and first-page metrics |

---

## 2. Justifications in depth

### 2.1 Why Rust for every security-critical and typography-critical path

1. **One layout engine, two targets.** The Compositor (line breaking,
   pagination, bidi reordering, anchor math) is a single crate compiled to
   `wasm32-unknown-unknown` for Lumen and to native for the server, where
   Oracle uses it to compute highlight geometry, quote cards and visual
   regression tests. A highlight made on a phone lands on exactly the same
   glyphs on a laptop, because both ran the same code.
2. **The typography ecosystem is excellent.** `rustybuzz` (a HarfBuzz port),
   `unicode-bidi`, `unicode-linebreak`, `hyphenation` (Knuth–Liang patterns,
   many languages), `swash`, `ttf-parser`. Arabic, Hebrew, Devanagari and CJK
   all work from day one.
3. **Memory safety where keys live.** The Kernel holds TMKs in memory. Rust
   eliminates the whole class of memory-corruption bugs that turn into key
   disclosure, and `zeroize` gives deterministic wiping.
4. **Predictable latency.** No GC pauses on the lease path.
5. **A smaller WASM attack surface.** WASM is not a security boundary against
   the device owner. Still, auditing a compact Rust-compiled module is a far
   higher bar than reading minified JS, and it costs us nothing.

### 2.2 Why TypeScript for everything else

- The web *is* TypeScript. React Aria, Next.js, Playwright and Shaka Player are
  all first-class there.
- **Shared types** come from the same Protobuf schemas the Rust services use
  (`buf generate` → `@bufbuild/protobuf` and `prost`).
- The Product API (catalog, library, annotations CRUD) is classic product code
  where iteration speed matters more than raw performance: **Hono on Node
  22** for portability, with Drizzle ORM. It never touches keys, so it carries
  no security-critical burden.

### 2.3 Why the reader is a Vite SPA, not part of Next.js

| Concern | Next.js reader | Vite SPA reader on `read.sanad.app` |
|---|---|---|
| Strict CSP | Needs per-request nonces, which forces dynamic rendering | `script-src 'self' 'wasm-unsafe-eval'`, fully static |
| Subresource Integrity | Partial | Every asset content-hashed and SRI-pinned |
| COOP/COEP for `SharedArrayBuffer` | Applies site-wide, which breaks third-party embeds (checkout, OAuth popups) | Isolated to the reader origin only |
| Third-party scripts | Marketing pages inevitably add some | **Zero**, enforced by CSP. First-party telemetry only. |
| Bundle | Framework runtime + RSC | Shell ≤ 150 KB gz + WASM ≤ 400 KB gz |

Next.js does what it is best at: SSR/ISR catalog pages with `schema.org/Book`
structured data, social cards, the library and account pages.

### 2.4 Why WebGL2 now and WebGPU next

WebGL2 runs on effectively every browser in use, including older iOS and
low-end Android. WebGPU reached Baseline across major browsers in early 2026,
but Android and Linux coverage is still uneven. Lumen is therefore written
against a **thin render-backend interface** (`createPipeline`, `drawInstances`,
`renderPass`). The WebGL2 backend ships first. The WebGPU backend lands in
Phase 3 for compute-based layout of very large fixed-layout pages and lower
CPU overhead on 120 Hz displays.

### 2.5 Why Cloudflare at the edge and AWS at the core

- **Free books mean high reads and zero revenue per read.** R2's zero egress
  pricing turns content-delivery cost into roughly zero, which makes Phase 1
  economics work.
- **Turnstile** and Private Access Tokens attest humans *invisibly*. That
  satisfies L1, where a CAPTCHA would break it.
- **AWS KMS** has the most mature HSM-backed envelope encryption with audit
  (CloudTrail) and tight IAM, and RDS/Aurora Postgres is the safe default.
- **Single-cloud alternative:** CloudFront + S3 + AWS WAF works with the same
  design. Only egress cost and attestation UX change.

### 2.6 Why Cedar for policy

Cedar is open source and written in Rust, so it embeds in the Kernel with no
sidecar. It is designed for fast, analyzable authorization: you can *prove*
properties such as "no policy grants offline rights to anonymous devices".
Publishers get policy *presets*, never raw policy code.

---

## 3. Monorepo layout

pnpm workspaces plus a Cargo workspace, orchestrated by Turborepo:

```text
sanad/
├── apps/
│   ├── web/               Next.js — www: catalog, book pages (SEO), library, account
│   ├── reader/            Vite SPA — read.: shell, settings, gestures; hosts Lumen
│   ├── publisher/         Next.js — publisher portal: upload, policy presets, analytics
│   └── api/               Hono — Product API: catalog, library, annotations, progress
├── crates/
│   ├── kernel/            Axum — Unified Security Kernel
│   ├── oracle/            Axum + Tantivy — protected search
│   ├── atelier/           ingest & typesetting pipeline (worker binary)
│   ├── compositor/        line breaking, pagination, bidi, anchors (native + wasm)
│   ├── folio/             container format: encoder, decoder, FlatBuffers schemas
│   ├── exlibris/          watermark encoder/decoder, Tardos codes
│   └── sanad-crypto/      key hierarchy, HKDF labels, lease wrapping
├── packages/
│   ├── lumen/             WebGL2 renderer, workers, shaders, gesture physics
│   ├── lumen-wasm/        wasm-bindgen output of compositor + folio (generated)
│   ├── ui/                React Aria Components + Tailwind design system
│   ├── tokens/            design tokens (JSON) → CSS vars, TS consts, shader uniforms
│   └── proto/             Protobuf schemas (buf) → TS + Rust
├── infra/                 OpenTofu modules, Helm charts, Cloudflare config
├── tools/                 visual-regression harness, leak-drill toolkit, red-team scripts
└── docs/                  this blueprint, ADRs, runbooks
```

---

## 4. Quality engineering built into the stack

| Concern | Tooling |
|---|---|
| Compositor correctness | `proptest` property tests (e.g. "every cluster is laid out exactly once", "anchors round-trip through reflow") |
| Folio decoder robustness | `cargo-fuzz` on the chunk parser. FlatBuffers verifier enabled in debug. |
| Visual fidelity | Playwright drives Lumen in headless Chromium, WebKit and Firefox. Pixel diffs against golden pages for every theme × script (Latin, Arabic, mixed bidi). |
| Device realism | Real-device farm runs on low-end Android, older iPhones and iPads, at 60 and 120 Hz |
| P1 enforcement | CI job loads a protected book, dumps the DOM, JS heap snapshot, network HAR and IndexedDB, then greps for known plaintext sentences. **Any hit fails the build.** |
| Supply chain | Lockfiles, Renovate, `cargo-deny`, `npm audit`, SBOM (CycloneDX), cosign-signed images, CSP violation reporting |

---

## 5. What we deliberately rejected

| Option | Why not |
|---|---|
| **PDF.js / any DOM-text viewer** | The text layer is in the DOM by design, so P1 fails at T0. Rendering performance is tied to CPU rasterization. |
| **EPUB.js / Readium Web (DOM-rendered)** | Excellent open readers, but they render XHTML into the DOM, so text is fully exposed. We borrow ideas (pagination UX, Readium's locator model) without the rendering model. |
| **WordPress or any CMS for reading** | Wrong security boundary and wrong performance model. A CMS may still power the *blog*. |
| **Flutter web (CanvasKit)** | Canvas rendering, but text shaping needs Unicode on the client (fails P1), and the multi-MB runtime hurts first load on mobile |
| **Shipping encrypted EPUB/PDF files to a client app** | P2 fails: a decryptable portable file is one patched binary away from a clean copy |
| **Go or Node for the Kernel** | Viable on their own, but we would lose the shared Rust layout and crypto core (D3) |
| **Per-user encryption of content** | Destroys CDN caching (cost ×N, latency up) with no gain over per-user *keys* (D5) |
