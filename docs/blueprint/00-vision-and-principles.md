# 00 — Vision, Principles & Threat Model

> **Sanad** — a ground-up, unified digital publishing platform whose reading
> experience is so good that a pirated file feels like a downgrade.

---

## 1. The thesis, made operational

"The best DRM is an unbeatable user experience" is a good slogan and a bad
engineering requirement. We turn it into three laws. Every component in this
blueprint is checked against them.

| Law | Statement | Test applied to every feature |
|---|---|---|
| **L1 · Zero friction** | No protection may add a step, a wait, a prompt or a failure mode for a legitimate human reading at human speed. | *Can a reader on a mid-range Android phone with a weak 4G signal notice it?* If yes, redesign it. |
| **L2 · Cost asymmetry** | Each measure must raise the attacker's cost by orders of magnitude more than it raises ours or the reader's. | *What does this cost a scraper per book, and what does it cost us per reader?* |
| **L3 · Attribution** | What we cannot prevent, we make traceable. | *If this page leaks, can we name the account, device and session that saw it?* |

The corollary: **security features are product features in disguise.** Rendering
through the GPU is the reason zoom is smooth. Streaming encrypted chunks is the
reason a 900-page book opens in under a second. A personalized watermark is
an *ex libris* bookplate. When a measure has no UX dividend, it gets extra
scrutiny before we accept it (see §4, the Cohesion Matrix).

---

## 2. Defining "impenetrable" honestly

Any system that shows readable text to a human can be captured. A phone camera
pointed at a screen beats every DRM ever shipped, and no web API changes that.
A blueprint that promises otherwise is selling something.

So "impenetrable" is not a slogan here. It is five properties we can test and
measure:

| # | Property | Meaning | Verified by |
|---|---|---|---|
| **P1** | *No client plaintext* | Protected text never exists as Unicode on the client: not in the DOM, the JS heap, the network, or storage. | Heap snapshots and network captures in CI; red-team audits |
| **P2** | *No portable artifact* | The reader can never hold a file they can copy elsewhere and open. Offline caches are ciphertext bound to one device key. | Storage inspection; device-transfer tests |
| **P3** | *Extraction requires recognition* | The cheapest way to get text out is to render pages and OCR the pixels, at a human-bounded rate, while being watched. | Velocity model; scrape-cost benchmark |
| **P4** | *Every page is unique* | Every rendered page carries forensic marks that identify (user, device, session). | Leak drills: recover the ID from photos and screenshots |
| **P5** | *Bounded key compromise* | One leaked key unlocks one chunk of one variant of one edition, nothing more. | Key-hierarchy review |

There are two deliberate, metered exceptions to P1, accessibility mode and
quota-limited quoting. Both are covered in
[03 §8](03-reader-engine.md#8-selection-annotations-quoting--accessibility).
They are server-metered, rate-bound and attributed, and they expose nothing
that OCR does not already expose.

---

## 3. Threat model

### 3.1 Adversary tiers

| Tier | Adversary | Typical tooling | What stops them | Residual risk |
|---|---|---|---|---|
| **T0** | Casual reader | Copy/paste, right-click → Save, Print, "Save page as" | P1: there is nothing to copy. Print stylesheet blanks the reader. There are no files. | None |
| **T1** | Tool user | Browser extensions, "download as PDF" sites, DevTools network tab, generic scrapers | Ciphertext on the wire, no Unicode anywhere, keys never cross the network unwrapped | None for generic tools |
| **T2** | Scripter | Hooks WebGL/WebCrypto, dumps memory, reads the WASM | Gets **permuted, shredded glyph IDs** with no Unicode mapping. Must reconstruct glyph composition and then OCR. | Hours to days per edition. Output is attributable through the A/B variant vector. |
| **T3** | Capture farm | Headless browsers, automated page turns, screenshot plus OCR | Sentinel velocity model, lease windows, invisible human attestation, device caps | Throttled to human speed, and the output carries a forensic watermark |
| **T4** | Hardware / analog | HDMI capture card, camera on a tripod | Vault mode with HDCP on hardware-DRM devices stops capture cards. For a camera, only watermarks help. | Accepted. Mitigated by attribution and legal response. |
| **T5** | Insider / supply chain | Stolen master keys, compromised publisher upload, rogue operator | KMS/HSM root, envelope encryption, per-edition keys, audit log, two-person rule for key export | Bounded by P5 and audit |

### 3.2 What we explicitly do not do

These measures fail L1 or L2. They hurt readers more than attackers.

- **Anti-debugger traps** (infinite `debugger` loops, DevTools detection that
  freezes the page). They produce false positives on real users and a
  determined attacker defeats them in minutes.
- **Blocking right-click or keyboard shortcuts globally.** These break
  accessibility and expected platform behavior. There is nothing to protect
  anyway: P1 already holds.
- **CAPTCHAs in the reading flow.** Human attestation stays invisible (see
  [05 §5](05-security-and-drm.md#5-anti-automation-the-sentinel-velocity-model)).
- **Lexical watermarking** (changing the author's words) unless a publisher
  explicitly opts in. We do not alter literature.
- **Download-and-install "secure reader" plugins.** The web reader is the product.

---

## 4. The Cohesion Matrix

Every security mechanism, and the reader-facing feature it pays for.

| Security mechanism | What it defends | UX dividend the reader actually feels |
|---|---|---|
| GPU glyph rendering (no DOM text) | P1, T0–T2 | 120 fps pinch-zoom, Knuth–Plass justification, page curl, and shader themes no CSS reader can match |
| Server-side shaping + client-side line breaking | P1 | Reflow on font-size change or rotation with no network call, under one frame |
| Encrypted, chunked streaming | P2, P5 | The first page of a 900-page book renders in under 1 s. There are no downloads and no file management. |
| Per-edition encryption, per-user keys | P5 | Ciphertext is CDN-cached worldwide, so it is fast everywhere and costs little to serve |
| Sliding lease windows | P3, T3 | Reading position syncs across devices on every renewal ("continue on laptop") |
| Passkey + device-bound sessions | T1, T3 | No passwords. One-tap sign-in with Face ID or fingerprint. |
| Server-authoritative search | P1 | Searches whole libraries at once, Arabic root-aware and accent-insensitive. Fast even on huge books. |
| Visible *ex libris* watermark | P4, social deterrent | "This copy belongs to Leila", a bookplate that makes the book feel owned |
| Invisible micro-typographic watermark | P4, T3–T4 | None. The reader can't see it. |
| Quote quotas | P1 | **Quote cards**: well-designed shareable images with a deep link back, which double as a growth loop |
| Isolated reader origin, zero third-party JS | T1, supply chain | No trackers and no ad scripts. The reader is fast and private by construction. |
| Offline ciphertext + device-bound keys | P2 | Real offline reading, on a plane, for weeks |

---

## 5. Protection profiles

The reader never sees a "DRM mode". The publisher, through policy, assigns each
*edition* a **protection profile**. The Kernel then negotiates the strongest
representation the reader's device can display smoothly.

| Profile | Used for | Representation | Client requirements |
|---|---|---|---|
| **Open** | Public-domain promo, samples, SEO excerpts | Folio glyph stream, no watermark, generous leases | Any modern browser |
| **Standard** *(Phase 1 default)* | Free and most paid books | Folio glyph stream, shredded atlas, A/B + micro-typographic watermark, velocity limits | WebGL2 (≈ every browser since 2021) |
| **Vault** *(Phase 2)* | High-value fixed-layout titles (textbooks, comics, premium reports) | Pages as CENC-encrypted video frames through EME (Widevine, PlayReady, FairPlay) | A hardware-backed CDM for full strength. Otherwise policy falls back to Standard or denies, per publisher setting. |

Details: [05 — Security & DRM Integration](05-security-and-drm.md).

---

## 6. Component glossary

| Name | Role | Language | Spec |
|---|---|---|---|
| **Atelier** | Ingestion and typesetting pipeline: EPUB or PDF in, shaped, encrypted Folio out | Rust | [01 §5](01-architecture.md#5-atelier--the-ingestion--typesetting-pipeline) |
| **Folio** | The encrypted, chunked content container format | Rust (spec, encoder, decoder) | [01 §4](01-architecture.md#4-folio--the-encrypted-content-container) |
| **Kernel** | The Unified Security Kernel: identity, entitlements, policy, leases, keys, watermark assignment, telemetry | Rust | [01 §3](01-architecture.md#3-the-unified-security-kernel) |
| **Lumen** | The Secure Reader Engine: GPU renderer, workers, gestures | TypeScript + GLSL/WGSL | [03](03-reader-engine.md) |
| **Compositor** | Line breaking, pagination, bidi reordering and anchor math. One crate compiled to native and to WASM. | Rust | [03 §3](03-reader-engine.md#3-the-compositor-layout-without-text) |
| **Oracle** | In-book and cross-library search | Rust (Tantivy) | [03 §6](03-reader-engine.md#6-protected-search) |
| **Ex Libris** | Visible bookplate plus forensic watermark encoding and decoding | Rust | [05 §4](05-security-and-drm.md#4-ex-libris--forensic-watermarking) |
| **Sentinel** | Behavioral telemetry, the velocity model and anomaly detection | Rust | [05 §5](05-security-and-drm.md#5-anti-automation-the-sentinel-velocity-model) |
| **Vault mode** | EME-backed protection profile | Rust (Kernel), TS (Lumen) | [05 §2](05-security-and-drm.md#2-vault-mode--eme-woven-in-not-bolted-on) |

---

## 7. Foundational decisions

Each of these is load-bearing. Reversing one later is expensive, so each is
recorded with its rationale.

| ID | Decision | Rationale | Rejected alternative |
|---|---|---|---|
| **D1** | Shaping (Unicode → glyphs) happens **only on the server**. Line breaking and pagination happen **on the client**. | Unicode never ships (P1), and reflow stays instant (L1) | Server-paginated pages: every font-size change is a round trip. Client shaping: requires Unicode on the client. |
| **D2** | **WebGL2 baseline**, WebGPU as progressive enhancement | WebGL2 runs on effectively all devices. WebGPU is now Baseline but young on Android and Linux. | Canvas2D: no shader theming and weak zoom performance |
| **D3** | **Two languages only: Rust and TypeScript** | The Compositor and the Folio decoder are the same Rust code on server and client, so layout is byte-identical everywhere | Go or Node for the Kernel: we would lose the shared layout core |
| **D4** | Search is **server-authoritative**. Offline search is page-level only. | A client-side index of the text *is* the text (see [03 §6](03-reader-engine.md#6-protected-search)) | "Encrypted" client-side full index: theatre against the device owner |
| **D5** | Encrypt **per edition and chunk**. Authorize **per user, through keys**. | Content takes the CDN (cacheable), keys take the Kernel (tiny). Security costs no latency. | Per-user encryption: kills CDN caching, so costs ×N and latency rises |
| **D6** | The reader runs on an **isolated origin** with zero third-party code | No analytics script or compromised dependency can read the page. COOP/COEP unlocks `SharedArrayBuffer` and WASM threads. | Reader inside the marketing site: shares an origin with tag managers |
| **D7** | **Accessibility is a first-class, metered channel** | It is a legal duty (the European Accessibility Act has applied to e-books since 28 June 2025) and a moral one. Metering keeps it from becoming a loophole. | "Canvas only, no screen-reader support": illegal in the EU and wrong |
| **D8** | **Bidi/RTL and complex scripts are first-class from day one** | Retrofitting Arabic shaping, kashida justification and RTL page progression into a custom renderer later costs months | "Latin first, RTL later" |
