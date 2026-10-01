# 06 — Phased Implementation Roadmap

> Build the hard core first, prove it with free books, then monetize a
> platform readers already love. Each phase ends on **exit criteria**, not
> dates. Dates are planning estimates for a focused team of 7–9 people.

---

## 1. Timeline at a glance

```text
Month:        0   1   2   3   4   5   6   7   8   9   10  11  12  13  14  15  16+
              ├───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼───┼──▶
Phase 0       ██████  Foundations & spikes
Phase 1a            ██████████  Private alpha · free books
Phase 1b                      ████████████████  Public beta · free books
Phase 2                                       ████████████████████████████  Monetization
Phase 3                                                                   ██████▶  Scale
Gates:             G0        G1              G2                          G3
```

| Gate | Question it answers | Decided by |
|---|---|---|
| **G0** | Can the renderer and layout core deliver print-grade Latin *and* Arabic at 60 fps on a low-end phone? | Engineering + design review of golden pages |
| **G1** | Do invited readers *prefer* reading on Sanad? | Alpha metrics + qualitative interviews |
| **G2** | Is there retention and publisher pull to justify monetization? | Beta metrics + publisher commitments |
| **G3** | Is the paid model working, and where should we scale? | Revenue, churn, unit economics |

---

## 2. Phase 0 — Foundations & spikes (weeks 0–6)

**Goal:** remove the existential technical risks before building product.

| Week | Workstream | Deliverable |
|---|---|---|
| 1 | Platform | Monorepo (pnpm + Cargo + Turborepo), CI, OpenTofu baseline, preview environments |
| 1 | Security | Threat-model workshop. The **P1 CI harness** (DOM, heap, HAR and storage grep) exists before any content does. |
| 1–2 | Design | Marginalia tokens v0, type specimens (Literata, Noto Naskh), the 5 themes on real devices |
| 1–4 | **Spike S1 — Lumen** | WebGL2 MSDF renderer + glyph shredding + MAX-blend coverage + composite shader. Render one Latin and one Arabic page. Pinch zoom on a low-end Android. |
| 2–4 | **Spike S2 — Compositor** | Knuth–Plass + hyphenation + kashida + bidi L2 in Rust → WASM. Reflow benchmark. |
| 2–5 | **Spike S3 — Folio** | Atelier slice: EPUB → shape → permute → shred → atlas → encrypt → CDN → decrypt → render |
| 3–5 | **Spike S4 — Kernel** | Passkey sign-in, device keys, DPoP, lease issue/renew, ECDH-wrapped keys unwrapped by WebCrypto |
| 6 | Review | Gate **G0** |

**G0 exit criteria**

- [ ] Golden pages (Latin justified with hyphenation, Arabic with kashida, mixed bidi) approved by the designer and a native Arabic typographer
- [ ] Pinch-zoom frame time ≤ 16.7 ms p95 on the reference low-end Android. Reflow of the visible spread ≤ 16 ms on tier B.
- [ ] End-to-end open → first page ≤ 1.0 s p75 on 4G (simulated) with real encryption
- [ ] P1 harness green: no known plaintext anywhere on the client

---

## 3. Phase 1a — Private alpha: free books (months 1.5–4)

**Goal:** a complete, delightful reading loop for free books, used by real
readers every day.

| Component | Scope |
|---|---|
| **Atelier v1** | EPUB 3 reflowable (Latin + Arabic), Standard profile, 2 A/B variants per chunk, visible bookplate, image classification, visual-regression QA |
| **Kernel v1** | Passkeys + magic link, **anonymous sampling**, `free` and `sample` entitlements, leases, DPoP, device caps, audit log, Cedar policies v1 |
| **Lumen v1** | Paged + scrolled, 5 themes + auto, size, spacing, margins, justify and hyphenation, slide transitions, pinch-to-resize, TOC, progress sync through anchors, render-on-demand, context-loss recovery, print blocking, custom context menu |
| **Oracle v1** | In-book search, Arabic normalization + light stemming, typeset snippets, query budgets |
| **www v1** | Catalog, SEO book pages with `schema.org/Book`, library, account |
| **Content** | **200–500 curated titles**: high-quality public-domain editions (Standard Ebooks' CC0 English catalog, Arabic and French public-domain classics) plus a handful of partner free titles. Curation *is* the product: beautiful editions of books people already love. |
| **Alpha cohort** | 200–500 invited readers, half on mid-range Android |

**G1 exit criteria**

- [ ] Reader SLOs met in production (first page, page turn, zoom, crash-free ≥ 99.5%)
- [ ] ≥ 60% of alpha readers rate the reading experience *better than their current app* (survey + interviews)
- [ ] Median session ≥ 15 min. Week-4 retention of active readers ≥ 35%.
- [ ] Zero P1 violations. Internal red team cannot extract a chapter in under 1 week of effort.

---

## 4. Phase 1b — Public beta: free books at scale (months 4–8)

**Goal:** an acquisition engine plus everything a serious reader expects,
and the publisher-facing proof.

| Track | Scope |
|---|---|
| **Reading features** | Highlights and notes (anchor-based, synced), **quote cards**, metered copy with citation, library-wide search, bookmarks, reading-time estimates |
| **Accessibility** *(launch-blocking)* | Screen-reader mode ([03 §8.4](03-reader-engine.md#84-accessibility-mode)), full keyboard operation, dyslexia preset, external audit against EN 301 549 / WCAG 2.2 |
| **Offline** | PWA install, offline leases, page-level offline search, storage persistence |
| **Fixed layout** | PDF ingest → text layer + art tiles, vector-crisp zoom to 800%, **guided column and panel view** |
| **Ex Libris v1** | Session micro-typography watermark, leak-drill tooling, first drill |
| **Sentinel v1** | Rules-based velocity model with the response ladder |
| **Quality** | Hinted rest atlases for DPR < 1.5, curl transition (tier A), device tiering |
| **Publisher portal v1** | Upload, QA preview, policy presets, aggregate analytics (cohort ≥ 20) |
| **Growth** | Quote-card sharing loop, SEO at scale, referral ("gift a sample"), email digests that are opt-in and calm |
| **Assurance** | External penetration test before public launch |

**Phase 1 KPIs**

| Metric | Target at G2 |
|---|---|
| Landing → first page rendered | ≥ 70% of "Start reading" taps |
| Sample → 10 minutes read (activation) | ≥ 40% |
| Sample → account (passkey) | ≥ 15% |
| D30 retention (accounts) | ≥ 25% |
| Weekly reading minutes per active reader | ≥ 90 |
| Quote cards shared per 100 weekly readers | ≥ 8 |
| Publisher commitments for Phase 2 catalog | ≥ 10 publishers, ≥ 2,000 paid titles signed |

---

## 5. Phase 2 — Monetization (months 8–15)

**Goal:** turn love into revenue without adding a single step to the reading flow.

| Track | Scope |
|---|---|
| **Commerce** | Purchase, subscription and rental entitlements. Web checkout (Stripe plus regional payment providers where readers are). Localized pricing. EU VAT through OSS. Receipts. |
| **Entitlement UX** | A sample ends in a typeset, one-tap purchase sheet. Purchased books open at the exact sentence where the sample ended. |
| **Vault mode** | Multi-DRM provider integration, Atelier Vault packaging (CMAF/CENC `cbcs`), License Gateway, lazy EME probe, pre-rendered themes, geometry overlay for selection, device-lab matrix |
| **Native shells** | Capacitor (iOS/Android) and Tauri (Windows/macOS) hosting Lumen, with `FLAG_SECURE`, capture-state curtains and `WDA_EXCLUDEFROMCAPTURE`. Store compliance for in-app purchase and reader-app rules. |
| **Ex Libris v2** | q-ary Tardos codes (2–4 variants per chunk), image marks, monitoring partner, leak-response runbook with due process |
| **Sentinel v2** | Gradient-boosted risk scoring, attestation on renewals |
| **Publisher portal v2** | Sales and royalty reports, per-title policy (profile, quotas, offline days, Vault fallback), payout integration |
| **Assurance** | Private bug bounty, second external pentest, red-team time-to-plaintext benchmark |

**G3 exit criteria:** paid conversion and churn targets (set at G2 from beta
data), positive contribution margin per paying reader, zero unattributed
leaks in drills.

---

## 6. Phase 3 — Scale & differentiation (months 15+)

- **WebGPU backend** for Lumen: compute-shader layout for very large fixed pages, lower CPU use at 120 Hz.
- **Read-aloud** with server-side neural TTS, streamed audio, sentence-synced highlighting. Text never reaches the client.
- **Institutional / B2B:** universities and libraries, SSO (SAML/OIDC), library lending entitlements, citation export. This is Perlego's territory, entered with a better reader.
- **Social reading:** shared highlights in private groups, book clubs, author annotations.
- **Discovery:** recommendations built on aggregate signals only (privacy-preserving), editorial collections.
- **Multi-region:** Kernel replicas per region, data residency options for institutions.

---

## 7. Team plan

| Role | Phase 0–1a | Phase 1b | Phase 2 |
|---|---|---|---|
| Tech lead / architect | 1 | 1 | 1 |
| Rust engineers (Kernel, Atelier, Compositor, Oracle) | 2 | 3 | 4 |
| Graphics / frontend engineers (Lumen, shell) | 2 | 2 | 3 |
| Product designer (Marginalia, typography) | 1 | 1 | 2 |
| Platform / SRE | 0.5 | 1 | 1 |
| Security engineer | 0.5 (+ external red team) | 1 | 1 |
| Native shells (Capacitor/Tauri) | — | — | 1 |
| Content and publisher partnerships | 0.5 | 1 | 2 |

Specialist contracts: a native Arabic typographer (Phase 0–1a), an
accessibility auditor (Phase 1b), a multi-DRM integration consultant (Phase 2).

---

## 8. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Canvas text looks worse than native text on some screens | Med | High | MSDF + hinted rest atlases. Golden-page reviews per release. Typography is never tiered down. |
| R2 | Arabic layout quality (kashida, bidi, diacritics) slips | Med | High | Arabic in spike S1 from day one. Native typographer sign-off at G0. Mixed-bidi golden tests. |
| R3 | Accessibility non-compliance blocks EU launch | Med | High | Screen-reader mode is launch-blocking in 1b. External EN 301 549 audit. |
| R4 | Over-securing hurts UX ("DRM feel") | Med | High | The L1/L2/L3 gate in every design review. UX SLOs are release blockers equal to security. |
| R5 | Phase 1 content not compelling enough | Med | High | Curated, beautifully typeset public-domain editions. Early partner free titles. Quote-card loop. |
| R6 | EME/Vault behavior varies by platform | High | Med | Vault limited to fixed-layout titles. Device lab. Policy fallbacks. Lazy probing. |
| R7 | Free-tier delivery costs scale with success | Low | Med | Zero-egress object storage. Ciphertext CDN-cached. A tiny Kernel footprint. |
| R8 | App-store payment rules constrain native commerce | Med | Med | Web-first commerce. Native shells follow store and reader-app rules. |
| R9 | A key or TMK compromise | Low | High | KMS/HSM, audit, P5 blast radius, re-encryption runbook |
| R10 | Browser platform shifts (storage eviction, GPU driver bugs) | Med | Med | Backend abstraction (WebGL2 ↔ WebGPU ↔ Canvas2D). Real-user monitoring. Fast rollback. |

---

## 9. Definition of Done (every feature, every phase)

- [ ] Passes the **L1 · L2 · L3** review ([00 §1](00-vision-and-principles.md#1-the-thesis-made-operational)). The Cohesion Matrix is updated if the feature adds a security mechanism.
- [ ] **P1 harness** green
- [ ] Performance budgets ([03 §9](03-reader-engine.md#9-performance-budgets)) met on the reference devices
- [ ] Golden-page visual tests pass in **Latin, Arabic and mixed bidi**, in all 5 themes
- [ ] Keyboard and screen-reader paths verified
- [ ] Telemetry contains no content, and the privacy review is checked off

---

## 10. The first two weeks: concrete next steps

1. Create the monorepo skeleton exactly as in [02 §3](02-tech-stack.md#3-monorepo-layout). Wire CI with the P1 harness stub.
2. Write `crates/folio` schemas (FlatBuffers) and the chunk header codec with fuzz tests.
3. Generate the first shredded MSDF atlas for Literata and Noto Naskh Arabic. Render a static page in a bare WebGL2 harness.
4. Implement Knuth–Plass in `crates/compositor` against a 10-paragraph Latin and Arabic fixture. Compile to WASM. Benchmark.
5. Stand up the Kernel skeleton (Axum) with passkey registration, device-key registration and a stub lease that wraps a fixed test key.
6. Produce token JSON and the five theme swatches. Review them on three physical phones in a dark room and in daylight.
7. Select the first 50 launch titles and begin the typographic QA checklist.
