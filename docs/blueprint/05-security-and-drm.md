# 05 — Security & DRM Integration: The Magician's Touch

> A magician's best trick is the one the audience never notices. Sanad's
> protection works the same way: the reader experiences speed, beauty and
> personalization, and a scraper runs into a wall at every step.

---

## 1. Defense in depth, by layer

| Layer | Mechanism | Defeats | Reader-visible cost |
|---|---|---|---|
| Edge | WAF, rate shaping, invisible human attestation (Turnstile, Private Access Tokens) | Bulk bots, credential abuse | None |
| Identity | Passkeys, device-bound DPoP tokens, device caps | Token theft, account sharing at scale | None (faster sign-in) |
| Transport | TLS 1.3, HSTS preload, opaque variant-specific URLs | Interception, variant enumeration | None |
| Content at rest | AES-256-GCM per chunk, TMKs wrapped by KMS/HSM | Storage or CDN compromise | None |
| Key delivery | Sliding leases, ECDH-wrapped keys, non-extractable `CryptoKey`s | Key exfiltration, bulk decryption | None (prefetch hides it) |
| Representation | No Unicode on the client: permuted, shredded glyphs | Copy, scrape, DOM/heap dumps | None (better typography) |
| Rendering | GPU-only pipeline, custom context menu, print blocking | T0/T1 capture paths | None (better menu) |
| Behavior | Sentinel velocity model | Automated capture farms | None at human speed |
| Hardware DRM | Vault mode through EME (Phase 2) | Software screen capture, HDMI capture | Reduced flexibility, on Vault titles only |
| Forensics | Ex Libris: bookplate, A/B variants, session micro-typography | Anonymous leaking | None (or a bookplate the reader enjoys) |
| Legal & ops | Leak-response workflow, takedowns, publisher reporting | Distribution of leaks | None |

---

## 2. Vault mode — EME woven in, not bolted on

### 2.1 What EME can and cannot do

Encrypted Media Extensions connect a web page to a **Content Decryption
Module** (Widevine, PlayReady, FairPlay) for **audio and video played through
`<video>` / `<audio>`**. On platforms with *hardware-backed* CDMs, decrypted
frames travel a protected path from the TEE to the display controller. Screen
capture APIs then see black, and HDCP guards the external outputs.

What EME **cannot** do is decrypt arbitrary data for JavaScript. No EME API
returns plaintext to the page, and that restriction is its security model. So
"using Widevine to protect a book" means exactly one thing: **render the
pages as encrypted video frames.** That is Vault mode.

### 2.2 Pages as video

```text
Atelier (Vault packaging)
  page renders ─▶ intra-only encode ─▶ CMAF fMP4 ─▶ CENC 'cbcs' ─▶ object storage
  (per theme × rendition)  (every frame a keyframe)   (one KID per chapter)

Playback (Lumen Vault backend)
  Shaka Player + MSE ─▶ <video> paused at t = page × frame_duration
  page turn = seek to the next keyframe (one decode) · two <video> elements
  ping-pong so the next page is already decoded
```

| Decision | Choice | Reason |
|---|---|---|
| Content scope | **Fixed-layout titles** (textbooks, comics, premium reports). Reflowable titles only at a few pre-set typesetting profiles. | Video frames can't reflow, and fixed layouts are already page images in spirit |
| Encryption scheme | **CENC `cbcs`** | One encrypted package plays under Widevine, PlayReady 4+ and FairPlay |
| Codecs | H.264 High (universal), HEVC (Safari), AV1 (where hardware decode exists) | Hardware decode is required for the secure path |
| Key granularity | **One KID per chapter** | Licenses are scoped to the lease window, so EME licenses *are* leases |
| Renditions | Phone, tablet and desktop resolutions, plus a "zoom ladder" up to about 2.5× | Zoom switches to a higher rendition. Beyond that, Vault trades zoom for protection. |
| Text quality | High-quality intra frames. Text content is luma-dominant, so 4:2:0 chroma is acceptable for dark-on-light. | Fine colored text is the weak spot, which is another reason Vault targets fixed layout |

### 2.3 Capability negotiation

The probe runs **only when an edition offers a Vault profile**, never at
startup. On some browsers (Firefox, for example) touching EME can trigger a
DRM-enablement prompt, and that would break L1 for Standard books.

```ts
// Lazy, ordered probe. The Kernel normalizes the result into a robustness label.
const probes = [
  { ks: "com.widevine.alpha",                    robustness: "HW_SECURE_ALL",  label: "HW_SECURE_ALL" },
  { ks: "com.microsoft.playready.recommendation", robustness: "3000",           label: "SL3000" },
  { ks: "com.apple.fps",                         robustness: "",               label: "FPS_HW" }, // verified per platform
  { ks: "com.widevine.alpha",                    robustness: "SW_SECURE_DECODE", label: "SW" },
];
for (const p of probes) {
  try {
    await navigator.requestMediaKeySystemAccess(p.ks, [{
      initDataTypes: ["cenc", "sinf", "skd"],
      videoCapabilities: [{ contentType: 'video/mp4; codecs="avc1.640028"', robustness: p.robustness }],
    }]);
    return p.label;
  } catch { /* try next */ }
}
return "NONE";
```

Typical outcomes, all verified in the device lab and never assumed:

| Platform | Typical result | Software screen capture of Vault pages |
|---|---|---|
| Android Chrome (most devices) | Widevine L1 → `HW_SECURE_ALL` | Black |
| ChromeOS | Widevine L1 | Black |
| Windows Edge | PlayReady SL3000 (on supported hardware) | Black |
| Safari, macOS and iOS | FairPlay | Black on supported configurations |
| Desktop Chrome and Firefox | Widevine **L3** (software) | **Not protected.** Policy decides: Standard fallback or "open in app". |

The Cedar policy ([01 §3.5](01-architecture.md#35-policy-as-code)) maps
*(edition profile, robustness label)* to the representation served.

### 2.4 Vault UX constraints, and how we hide them

| Constraint | How it is hidden |
|---|---|
| Shaders can't touch protected frames, so live theming is impossible | Themes are **pre-rendered renditions** (Paper and Night). Switching swaps the track behind a crossfade. |
| No glyphs, so no selection | The Folio for a Vault title still carries **geometry without identity**: block and glyph boxes with no glyph IDs. Selection, highlights and search highlights draw on a transparent overlay canvas above the video. |
| Seek latency | Ping-pong video elements keep the next and previous pages decoded, so a page turn is an element swap |
| Transitions | Limited to what each platform's protected video path composites reliably (opacity fades by default). Validated per device. |
| L3-only desktops | Per publisher policy: a Standard fallback (still glyph-protected and watermarked) or a prompt to continue in the native app |

### 2.5 One policy, two representations

The Kernel's License Gateway gives the multi-DRM provider an **upfront
entitlement token** derived from the *same* lease decision:

```text
Lease (Standard)                          License (Vault)
───────────────────────────────           ───────────────────────────────────
window: chunks 20–25                 ═▶   KIDs: chapters 4–5
TTL 15 min, renew at 50 %            ═▶   license duration 15 min, renewal enabled
offline: 30 days                     ═▶   persistent license, 30 days (where supported)
device: DPoP jkt 9fQ…                ═▶   bound to the CDM session of that device
robustness: n/a                      ═▶   HW_SECURE_ALL / SL3000 + HDCP ≥ 1.4 on outputs
watermark codeword                   ═▶   A/B segment variant selection (same Ex Libris code)
```

That is why Vault feels native rather than bolted on. The reader, the Kernel,
the audit log and the watermark all see one concept, the lease, rendered
through two technologies.

---

## 3. The analog hole: honest engineering

### 3.1 What each platform actually allows

| Platform | Block screenshots? | Detect capture? | Mechanism |
|---|---|---|---|
| Web, Standard profile | **No.** No web API exists. | No reliable signal | Watermarks (§4) plus the velocity model (§5) |
| Web, Vault on hardware DRM | **Yes** for software capture of the video plane | — | Protected media path. HDCP blocks HDMI capture cards. |
| Android app (Capacitor shell) | **Yes**: `FLAG_SECURE` blocks screenshots and recording and blanks the app-switcher thumbnail | Android 14+: screenshot detection callback | Native window flag |
| iOS / iPadOS app | **No** public API to block | Yes: screen recording and mirroring (capture state notifications), plus screenshots after the fact | On recording or mirroring, show a privacy curtain. On a screenshot, log a forensic event and show a gentle, non-accusatory note. |
| Windows app (Tauri) | **Yes**, for capture APIs: `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (Windows 10 2004+) | — | Window affinity |
| macOS app | **Not reliably.** Since macOS 15, ScreenCaptureKit ignores `NSWindow.sharingType = .none`, and there is no public replacement. | Limited | Watermarks |
| Any device + camera | **Never** | — | Watermarks, attribution, legal |

The native shells exist mainly to unlock the rows marked **Yes**. They host
the same Lumen engine in a WebView, so the reading experience stays
identical while the OS adds a capture barrier. For high-value titles,
publishers can require "app or hardware DRM only" by policy.

### 3.2 Web-level measures that respect L1

- **App-switcher privacy:** on `visibilitychange → hidden`, Lumen draws a
  paper-colored frame. Task-switcher thumbnails then show a calm blank page
  instead of the text. This is best-effort, because OS snapshot timing varies.
- **No blur-on-focus-loss.** Focus moves for countless innocent reasons
  (notifications, password managers, devtools). Punishing those fails L1 and
  barely slows an attacker.
- **No "screenshot detected" accusations on the web.** Without a reliable
  signal, guessing would create false accusations.

### 3.3 The strategic position

We cannot stop a camera. We can make sure that:

1. **Capture is slow.** It happens one page at a time, at human pace, under
   lease windows and the velocity model.
2. **Capture is lossy.** The output is an image that needs OCR, so the
   attacker must clean up layout, hyphenation, footnotes and RTL ordering.
3. **Capture is attributable.** Every pixel carries the reader's identity (§4).
4. **The legitimate product is better than the capture.** Sync, search,
   notes, typography, offline access, quote cards. A pirated OCR'd PDF is a
   visibly worse book.

---

## 4. Ex Libris — forensic watermarking

### 4.1 Watermark layers

| Layer | Where it is applied | Robust against | Removable by | Capacity |
|---|---|---|---|---|
| **Visible bookplate** | Title page / first page: *"Ex libris · Leila K. · Oct 2026"*. An optional discreet footer mark for premium titles, per publisher. | Everything visual | Cropping, or a patched client (T2+) | Identity, explicitly shown |
| **A/B chunk variants** | Server-side, at ingest. Each chunk exists in 2–4 micro-typographic variants, and the reader's codeword picks one per chunk. | Screenshots, photos, a patched client (the client never receives the sibling variants) | Only OCR plus re-typesetting | 1–2 bits per chunk: ≈ 400 symbols (400–800 bits) per typical edition |
| **Session micro-typography** | Client-side, in the Compositor: the lease's seed modulates inter-word glue and baseline micro-shifts | Screenshots, JPEG compression, phone photos | A T2 attacker who patches the WASM | Full 64-bit session ID per page, with error correction |
| **Image marks** *(Phase 2)* | Spread-spectrum marks in image tiles, per variant | Screenshots, recompression | Heavy filtering | Low |
| **Accessibility marker** | Code-point variants (apostrophes, quotes, spaces) in a11y text | Copy-paste | Unicode normalization | Moderate |

The design pairs **dense but removable** (session micro-typography: every page
carries the full ID) with **sparse but unremovable** (A/B variants: the client
cannot see what it never received). A casual leak of one screenshot is
attributed by the first. A sophisticated leak of a whole book by a patched
client is attributed by the second.

### 4.2 Micro-typographic encoding

```text
carrier selection:  PRNG(seed = HMAC(K_wm, session_id ‖ page)) picks ~40 carrier words per page
modulation:         inter-word glue  Δ = ±0.6 % of the line's natural space
                    baseline shift   Δ = ±0.08 em on selected word starts
payload:            64-bit session id ─▶ Reed–Solomon ECC ─▶ spread across carriers
imperceptibility:   justified text already varies inter-word space by 10–30 % from line to
                    line; a 0.6 % delta is far below the just-noticeable difference
decoding:           register the leak against a reference re-render (typesetting state
                    estimated from measure and size), measure glue and baselines,
                    demodulate, run ECC
```

### 4.3 Collusion resistance

Two or more leakers could compare copies to find and scramble the
differences. The A/B variant vectors are therefore **q-ary Tardos
fingerprinting codes**. Tardos codes come with provable bounds: any coalition
of up to *c* colluders can be traced, with a quantified false-accusation
probability ε. Required code length grows with *c²·log(n/ε)*. With 2–4
variants per chunk and about 400 chunks per typical edition, we target
tracing **coalitions of 2–3** among about 10⁶ readers per edition, with
ε ≤ 10⁻⁶. The dense session layer adds evidence for partial leaks.

### 4.4 What watermarking cannot do

An OCR'd, re-typeset *text-only* leak carries no layout watermark. For that
case:

- **Cost** (P3) and the **velocity model** are the defenses.
- **Attribution** falls back to Sentinel's capture-pattern evidence and the
  accessibility ledger.
- **Lexical canaries** (per-variant wording changes) exist **only** as an
  explicit publisher opt-in, typically for non-literary content. We do not
  alter literature by default.

### 4.5 Leak response workflow

```text
① detect     web monitoring partner · publisher report · reader report
② extract    tools/leak-drill: decode bookplate / micro-typography / A/B vector
③ match      restricted "forensics" DB role → codeword → session → account
              (every lookup audited; two-person approval)
④ assess     report includes the false-accusation probability from the decoder
⑤ act        ladder: revoke device keys & offline leases → suspend pending human review
              → takedown notices to hosts → publisher-led legal action (by contract)
⑥ learn      feed capture patterns back into Sentinel; adjust edition policy
```

**Due process** is part of the design. No automated permanent ban. A human
reviews every case, the account holder can respond, and evidence and
probability travel together.

---

## 5. Anti-automation: the Sentinel velocity model

### 5.1 Signals

| Source | Signals (no content, ever) |
|---|---|
| Lease renewals | Cadence, window overruns, jump frequency and intents |
| Client digest | Page-dwell histogram (log-normal for humans), input modality mix (touch, pointer, keys), visibility ratio, frame-timing regularity, GPU renderer class (software rasterizers suggest headless), `navigator.webdriver` |
| Oracle | Query entropy, vocabulary-coverage rate, inter-query timing |
| Network | IP/ASN reputation, datacenter ranges, concurrent sessions per account |
| Account | Device age, payment history (Phase 2), past flags |

### 5.2 Response ladder

Escalation is gradual, and invisible until it has to be:

| Level | Trigger (illustrative) | Response | A human reader would notice… |
|---|---|---|---|
| 0 | Normal | Nothing | — |
| 1 | Sustained cadence > 1 page / 2 s over 50+ pages with low dwell variance | `ahead` window shrinks to 1 chunk | Nothing (prefetch still beats human pace) |
| 2 | Level 1 persists, or automation signals appear | Invisible attestation on each renewal | Nothing in practice |
| 3 | Attestation fails, or the signal is strong | Renewals paced to a human ceiling | Pages arrive at reading pace |
| 4 | Clear automation | Session paused, human review, account notified | A calm message with a support path |

**Calibration rule:** thresholds are set from real reading distributions so
that **< 0.1% of human sessions ever reach level 2**. Skimming and flipping
to find a figure are short bursts with high dwell variance, and they are
explicitly allowed.

---

## 6. Client hardening, the low-friction kind

- **Clean-room origin:** `read.sanad.app` serves only first-party, static,
  SRI-pinned assets.
  - CSP: `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self' https://kernel.sanad.app https://cdn.sanad.app; img-src 'self' blob: data:; style-src 'self'; font-src 'self'; manifest-src 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'`
  - `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Embedder-Policy: require-corp`
  - `Permissions-Policy` disables every feature the reader doesn't use
- **Framing denied** (`frame-ancestors 'none'`), so the reader can't be embedded
  and scraped through a hostile parent page.
- **Lightweight obfuscation** only on key-orchestration code. The render and
  layout hot paths stay fast and readable, because obfuscating them costs
  frames and buys minutes.
- **No anti-debug traps** (see [00 §3.2](00-vision-and-principles.md#32-what-we-explicitly-do-not-do)).
- **Extensions** can inject into any page, and we cannot prevent that. P1
  makes it moot: there is no text for them to find.

---

## 7. Platform and data security

- **Network:** TLS 1.3 everywhere, HSTS preload, mTLS between internal
  services, private subnets for the Kernel, Postgres and KMS endpoints.
- **Keys:** KMS key policies grant the `Decrypt` action to the Kernel's role
  only. TMK unwraps are logged and rate-alarmed. Key export follows a
  two-person rule. Rotation: re-wrap TMKs under a new KMS key yearly, and
  re-encrypt editions on demand (for example after a suspected leak).
- **Data:** Postgres encrypted at rest. Encrypted, immutable backups.
  Restricted roles for forensics and publisher data.
- **Software supply chain:** pinned dependencies, `cargo-deny`, signed
  container images, SBOMs, reproducible WASM builds (the hash is published per release).
- **Assurance:** an external penetration test before public beta, a private
  bug bounty from Phase 2, and a standing red-team exercise (§9).

---

## 8. Privacy by design

What someone reads can reveal their beliefs, health and politics. Trust is
part of the product.

- **Minimal telemetry:** performance metrics plus behavioral *digests*
  (histograms, not event streams). No content, no keystrokes, no
  third-party analytics.
- **Retention:** raw digests 90 days. Watermark codeword mappings 24 months
  after session end, then deleted. Reading progress and notes kept until the
  reader deletes them, with export always available.
- **Publisher analytics** are aggregated with minimum cohort sizes (no metric
  for fewer than 20 readers). A publisher never sees an individual's reading.
- **Compliance posture:** GDPR-grade by default (lawful basis, data-subject
  rights, DPA with processors), designed to satisfy stricter regimes wherever
  readers are.

---

## 9. Security KPIs and red-team program

| KPI | Target | Measured by |
|---|---|---|
| P1 violations (plaintext found on the client) | **0**, enforced in CI | Heap, DOM, HAR and storage grep job on every build |
| Time-to-plaintext, T2 attacker, 300-page Standard title | ≥ 1 week of skilled effort for the *first* edition. Each later edition still costs a full OCR pass. | Annual external red team |
| Scrape throughput under Sentinel | ≤ human reading pace after level 3 | Synthetic bot harness in staging |
| Leak-drill attribution rate (single screenshot, phone photo) | ≥ 95% / ≥ 80% correct session | Quarterly drill with `tools/leak-drill` |
| Human sessions reaching Sentinel level 2 | < 0.1% | Production metrics |
| Vault capture test (hardware DRM platforms) | Black frames on 100% of the supported device matrix | Device-lab run per release |
