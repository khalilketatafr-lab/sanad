# Sanad

**A ground-up digital publishing platform where the best DRM is an unbeatable
reading experience.**

Sanad streams books as encrypted, CDN-cached fragments. A custom GPU reader
reassembles them into print-grade pages, and the text never exists on the
client as copyable characters. Readers get instant opening, fluid zoom,
beautiful typography in Latin and Arabic, sync, search and offline reading.
Scrapers get permuted, shredded glyph IDs, leases that expire, and pages that
carry their identity.

> **Status:** architecture & design blueprint (pre-implementation).
> Phase 1 launches with free books. Phase 2 adds monetization.

---

## The blueprint

| # | Document | What it covers |
|---|---|---|
| 00 | [Vision, Principles & Threat Model](docs/blueprint/00-vision-and-principles.md) | The three laws (zero friction, cost asymmetry, attribution), a testable definition of "impenetrable", adversary tiers, the Cohesion Matrix, foundational decisions |
| 01 | [Core Architecture](docs/blueprint/01-architecture.md) | **High-level architecture diagram**, the Unified Security Kernel, key hierarchy, leases, the Folio encrypted container, the Atelier ingest pipeline, data model, real-time reassembly |
| 02 | [Tech Stack & Justification](docs/blueprint/02-tech-stack.md) | Rust + TypeScript, WebGL2/WebGPU, Cedar, Postgres, Cloudflare + AWS, monorepo layout, rejected alternatives |
| 03 | [Lumen — Secure Reader Engine](docs/blueprint/03-reader-engine.md) | Layout without text, glyph shredding, MSDF rendering and shaders, zoom/pan physics, **protected search**, adaptive theming, accessibility mode, performance budgets |
| 04 | [Marginalia — Design System](docs/blueprint/04-design-system.md) | Principles, benchmarks, typography, OKLCH color system with verified contrast, motion, **Reader Settings panel**, mobile-first and RTL rules |
| 05 | [Security & DRM Integration](docs/blueprint/05-security-and-drm.md) | Vault mode (EME/Widevine/PlayReady/FairPlay), the analog hole by platform, forensic watermarking, the Sentinel velocity model, privacy |
| 06 | [Phased Roadmap](docs/blueprint/06-roadmap.md) | Phase 0 spikes → free-book alpha/beta → monetization → scale. Gates, KPIs, team, risks. |

**Suggested reading order:** 00 → 01 → 03 → 05 → 04 → 02 → 06.

---

## The system in one picture

```text
           ┌────────────────── read.sanad.app · clean-room origin ───────────────────┐
Reader ──▶ │ Reader Shell ─▶ Render Worker (WebGL2) ◀─ Vault Worker (WebCrypto+WASM) │
           └─────────────────────────────────────────────▲─────────────▲─────────────┘
                 (A) wide road · ciphertext · CDN-cached │             │ (B) narrow road · wrapped keys
                                                         │             │
                                      Encrypted storage ─┘             └── Unified Security Kernel (Rust)
                                              ▲                            identity · policy · leases
                                              │                            KMS/HSM · Postgres · Oracle
                        Atelier: EPUB / PDF → shape → shred → encrypt
```

**Content travels the wide road, keys travel the narrow road.** Security and
performance stop competing because they never share a path.

---

## Core principles

1. **Zero friction:** no protection may cost a legitimate reader a step, a wait or a prompt.
2. **Cost asymmetry:** each measure must cost attackers far more than it costs us or readers.
3. **Attribution:** what can't be prevented (the analog hole) is made traceable.
