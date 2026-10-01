# 04 — Marginalia: The Sanad Design System

> Marginalia is designed for **deep reading**. Its success metric is how quickly
> the reader forgets it exists. The chrome is quiet, the typography is
> excellent, and every control answers the question the reader was about to ask.

---

## 1. Core principles

| # | Principle | What it means in practice |
|---|---|---|
| 1 | **The page is the product** | While reading, chrome takes up 0 px. Tap the center to summon it, and it auto-hides after 4 s of inactivity. No floating buttons, badges or toasts over the text, ever. |
| 2 | **Calm by default** | No streaks, nags or notifications inside the reader. Achievements live in the library, never on the page. Motion is slow, eased and purposeful. |
| 3 | **Typography is the interface** | Hierarchy comes from type (size, weight, small caps, spacing) before color, borders or boxes. If a screen needs a divider line, the spacing is probably wrong. |
| 4 | **One gesture, one meaning** | Tap zones, swipes and long-press behave identically in every book, on every device. They mirror for RTL books and never change for any other reason. |
| 5 | **Progressive disclosure** | The settings sheet shows 6 controls. Everything else sits one level deeper. Defaults are tuned so most readers never open it. |
| 6 | **Respect the body** | Thumb-reachable controls, anti-glare defaults, extra-dim for night, no pure black on white. Ergonomics are a design input, not an accessibility afterthought. |

### 1.1 Reducing cognitive load

- **Recognition over recall.** Theme swatches render *in* their theme, and font
  chips are set *in* their face. The reader sees the outcome, not a label.
- **Live preview, no Apply button.** Every setting changes the page behind the
  translucent sheet instantly. Reversible actions need no confirmation.
- **Time, not percentages.** "14 min left in chapter" is computed from *this
  reader's* measured pace. "62%" is only shown on request.
- **Stable spatial memory.** Controls never move between screens. The settings
  sheet opens to the same scroll position it was closed at.
- **Hick's law budget.** At most 5 choices per row, and at most 3 segments per
  segmented control.

---

## 2. Benchmarks and how we go further

| Dimension | Apple Books | Kindle (Cloud Reader / apps) | Perlego | **Sanad** |
|---|---|---|---|---|
| Platform reach | Apple devices only, no web reader | Everywhere. The web reader is functional but plainer than the apps. | Web-first plus apps | **Web-first PWA with the native-grade feel of Apple Books**, plus native shells in Phase 2 |
| Typography | Excellent themes and fonts | Good on devices, more limited on the web | Utilitarian | **Knuth–Plass justification, hyphenation, optical margins, kashida, dark-mode weight compensation**, on the web |
| Page feel | Best-in-class curl | Slide | Functional | GPU slide/curl at 120 Hz, gesture physics, reduced-motion aware |
| Theming | Several themes plus auto night | Several themes | Basic options | **5 calibrated themes, warmth, extra-dim, image-aware dark mode** |
| Search | In-book | In-book plus X-Ray | In-book, strong academic tooling | **In-book plus whole-library, Arabic root-aware, typeset snippets** |
| Quoting | Copy with citation | Clipping limits | Citation tools | **Quote cards** (designed, shareable, linked) plus metered copy with citation |
| RTL / Arabic | Supported | Supported | Not a focus | **First-class**: kashida justification, RTL progression, mirrored gestures, Arabic UI typography |
| Fixed layout | Good | Panel view for comics | PDF-style | Vector-crisp text at 800%, **guided column and panel view**, tile-streamed art |

We take Apple Books' **craft**, Kindle's **ubiquity and sync**, Perlego's
**research tooling**, and add what none of them offer on the open web:
typeset-quality layout, GPU fluidity and RTL excellence, in a reader that
opens from a link in under a second.

---

## 3. Typography

### 3.1 Typefaces

All typefaces are SIL Open Font License. Atlases are generated from them, and
no font file is ever shipped to the client, which also removes licensing
ambiguity.

| Role | Latin | Arabic | Why |
|---|---|---|---|
| **Reading, default** | **Literata** (variable, `opsz` + `wght`) | **Noto Naskh Arabic** | Literata was designed for long-form screen reading, and its optical-size axis keeps small text sturdy and large text refined. Noto Naskh pairs well in color and rhythm. |
| Reading, alternate serif | Source Serif 4 (`opsz`) | Amiri (classical Naskh) | A crisper modern serif. Amiri suits literary and classical Arabic. |
| Reading, accessible | **Atkinson Hyperlegible Next** | IBM Plex Sans Arabic | Highly distinguishable letterforms for low vision and dyslexia presets |
| Reading, original | Publisher's embedded fonts (atlas-converted at ingest, if licensed) | Same | Respects the publisher's design |
| UI | **Inter** (variable) | **IBM Plex Sans Arabic** | Neutral, excellent at small sizes, tabular figures. Plex Arabic matches Inter's proportions. |

### 3.2 Reading typography rules

| Rule | Latin | Arabic |
|---|---|---|
| Measure (characters per line) | 60–72, target **66** | 50–65, target **58** |
| Leading (line height) | **1.5** (Compact 1.38, Airy 1.7) | **1.75** (tall ascenders, descenders and diacritics) |
| Paragraphs | Book convention: 1 em indent, no extra space. No indent after headings and breaks. | Indent 1 em or block spacing, per publisher style |
| Justification | On by default, Knuth–Plass, with hyphenation | On by default, kashida first, then inter-word |
| Hyphenation | Max 2 consecutive hyphenated lines. Min word length 6, min 2 characters before and 3 after the break. | Never (not part of the script tradition) |
| Figures | Old-style numerals in running text, lining figures in tables | Arabic-Indic or Western digits per book locale; the reader can override |
| Chapter openers | Small-caps lead-in on the first 3–5 words, generous sink (≈ ⅓ page) | Larger heading size plus sink. No small caps (not applicable). |
| Emphasis | True italics. No faux slanting, ever. | Weight or color shift (italics are not native to Arabic) |
| Letter-spacing | 0 for body text. +5% for small caps and all-caps UI labels. | Always 0 (letter-spacing breaks joining) |

### 3.3 Reader type-size steps

Twelve detents give the size slider haptic ticks and stable, predictable
results:

```text
14 · 15 · 16 · 17 · 18 · 20 · 22 · 24 · 27 · 30 · 34 · 40   (CSS px)
Default: 18 on phones · 20 on tablets and desktop
Arabic books render at +2 px effective size (Arabic glyphs are optically
smaller at equal nominal size), so the slider position stays comparable
across languages.
```

### 3.4 UI type scale

A modular scale of **1.2 (minor third)** on a 16 px base, using Inter or Plex
Sans Arabic:

| Token | Size / line height | Weight | Use |
|---|---|---|---|
| `type.caption` | 12 / 16 | 500 | Progress labels, timestamps |
| `type.label` | 14 / 20 | 500 | Controls, chips, segmented controls |
| `type.body` | 16 / 24 | 400 | Sheets, dialogs, library metadata |
| `type.title-s` | 19 / 26 | 600 | Sheet titles, book titles in lists |
| `type.title-m` | 23 / 30 | 600 | Section headers |
| `type.title-l` | 28 / 34 | 650 | Book detail page title |
| `type.display` | 33 / 38 | 700 | Marketing only, never in the reader |

Arabic UI text is set one step larger at the same token, because of its optical size.

---

## 4. Color system

### 4.1 Color theory for long-form reading

1. **Avoid the extremes.** Pure `#000` on `#FFF` (21:1) causes halation and
   glare in long sessions. The target is **12–17:1** for light themes and
   **9.5–12:1** for dark themes. Every combination clears WCAG AAA (7:1) by a
   wide margin.
2. **Warm neutrals.** Paper and ink sit at a hue of about 80–90° (warm) with
   very low chroma (≤ 0.02 OKLCH). It reads as paper, not as a "beige theme".
3. **Dark is not inverted light.** Dark themes lower ink lightness
   (L ≈ 76–88%) instead of using white, and compensate stroke weight in the
   shader ([03 §7.4](03-reader-engine.md#74-optical-weight-compensation)).
4. **Elevation in dark themes is lightness, not shadow.** Each surface step is
   about +3% L in OKLCH.
5. **One accent.** Links, the progress indicator, the focus ring and search
   matches share a single accent per theme. Color is never the only signal:
   links are also underlined, and matches also get a shape cue.
6. **Defined in OKLCH, shipped as hex.** Perceptual uniformity makes
   interpolation (theme crossfades) look even. Hex values feed the shaders.

### 4.2 Reading themes

Contrast ratios are computed with the WCAG 2.x relative-luminance formula.

| Theme | Paper | Ink | Ink 2 (secondary) | Accent | Ink / Paper | Ink 2 / Paper | Accent / Paper |
|---|---|---|---|---|---|---|---|
| **Paper** | `#FBFAF7` · oklch(98.5% 0.004 91) | `#1D1C1A` · oklch(22.7% 0.004 85) | `#5E5A53` | `#2F5D8A` | **16.3:1** | 6.6:1 | 6.6:1 |
| **Linen** | `#F3EBDD` · oklch(94.3% 0.021 82) | `#2E261D` · oklch(27.5% 0.020 70) | `#6A5D4C` | `#8A4B2A` | **12.6:1** | 5.4:1 | 5.7:1 |
| **Dusk** | `#2A2A2D` · oklch(28.6% 0.005 286) | `#DAD6CE` · oklch(87.7% 0.012 85) | `#A29E96` | `#9DB8D9` | **9.9:1** | 5.4:1 | 7.0:1 |
| **Night** | `#141414` · oklch(19.1% 0 0) | `#CFCAC1` · oklch(84.0% 0.014 82) | `#8F8A82` | `#8FA9C9` | **11.3:1** | 5.4:1 | 7.6:1 |
| **OLED** | `#000000` | `#B5B0A6` · oklch(75.8% 0.015 85) | `#7F7A72` | `#7E96B3` | **9.7:1** | 4.9:1 | 6.9:1 |

Per-theme shader parameters:

| Theme | `weight` (MSDF offset) | `covGamma` | `lumaCeil` | Image policy |
|---|---|---|---|---|
| Paper | +0.010 | 0.90 | 1.00 | natural |
| Linen | +0.010 | 0.90 | 0.92 | natural |
| Dusk | −0.020 | 1.10 | 0.55 | dimmed / smart-invert |
| Night | −0.030 | 1.15 | 0.45 | dimmed / smart-invert |
| OLED | −0.035 | 1.20 | 0.38 | dimmed / smart-invert |

These are starting values. Final tuning happens per font with golden-page
reviews on real devices.

### 4.3 Highlight colors

Highlights are underlays beneath the ink. Ink-on-highlight contrast is listed:

| Theme | Yellow | Green | Blue | Rose |
|---|---|---|---|---|
| Paper | `#F7E8A4` 13.8 | `#CFEBC9` 13.3 | `#CFE0F5` 12.7 | `#F5D3DC` 12.4 |
| Linen | `#EED89A` 10.6 | `#CFE0B8` 10.6 | `#C9D8E6` 10.2 | `#ECCBC4` 9.9 |
| Dusk | `#48442A` 6.8 | `#2E4634` 7.1 | `#2E3E52` 7.5 | `#4C333C` 7.8 |
| Night | `#3D391C` 7.1 | `#233A28` 7.5 | `#223142` 8.1 | `#3F242A` 8.6 |
| OLED | `#2E2A12` 6.7 | `#18291C` 7.1 | `#16222F` 7.5 | `#2F191E` 7.6 |

Each highlight color also has a distinct **margin glyph shape** in the notes
list (● ▲ ■ ◆), so meaning never relies on hue alone.

### 4.4 Tokens: one source for chrome and shaders

`packages/tokens/tokens.json` uses the W3C Design Tokens format and is built
by Style Dictionary into three outputs: **CSS custom properties** for the
chrome, **TS constants** for logic, and **shader uniform blocks** for Lumen.
The settings sheet and the page behind it can never drift apart.

```css
:root,
[data-theme="paper"] {
  --paper: #FBFAF7;  --ink: #1D1C1A;  --ink-2: #5E5A53;  --accent: #2F5D8A;
  --surface-1: #FFFFFF;  --surface-2: #F4F2EE;  --hairline: rgb(29 28 26 / 0.10);
  --glass-tint: rgb(251 250 247 / 0.88);  --ink-2-on-glass: #4F4B45;
  --focus-ring: 0 0 0 3px rgb(47 93 138 / 0.45);
}
[data-theme="night"] {
  --paper: #141414;  --ink: #CFCAC1;  --ink-2: #8F8A82;  --accent: #8FA9C9;
  --surface-1: #1C1C1E;  --surface-2: #232325;  --hairline: rgb(207 202 193 / 0.12);
  --glass-tint: rgb(28 28 30 / 0.88);  --ink-2-on-glass: #B3AEA5;
  --focus-ring: 0 0 0 3px rgb(143 169 201 / 0.55);
}
/* linen, dusk, oled: same shape; generated, never hand-edited */
```

---

## 5. Space, shape, elevation and motion

### 5.1 Spacing and shape

- **4 px base unit.** Scale: `4 · 8 · 12 · 16 · 24 · 32 · 48 · 64`.
- **Radii:** `8` (chips) · `12` (controls) · `20` (cards) · `28` (sheets and
  popovers). Concentric nesting: inner radius = outer radius − padding.
- **Icons:** 24 px grid, 1.5 px stroke, rounded joins, a Lucide-derived set.
  Directional icons mirror under RTL. Media and clock icons never mirror.

### 5.2 Page margins

| Breakpoint | Width | Narrow | Normal | Wide | Layout |
|---|---|---|---|---|---|
| Compact | < 600 | 16 | 24 | 32 | Single page |
| Medium | 600–1023 | 32 | 48 | 64 | Single page; spread in landscape ≥ 900 |
| Expanded | 1024–1439 | 48 | 72 | 96 | Two-page spread (Auto) |
| Large | ≥ 1440 | measure-bound | measure-bound | measure-bound | Spread, centered, measure capped |

All values are in px, added to `env(safe-area-inset-*)` on notched devices.
The measure (§3.2) always wins: on wide screens the margins grow, never the line length.

### 5.3 Motion

| Token | Duration | Easing | Use |
|---|---|---|---|
| `motion.instant` | 90 ms | `cubic-bezier(0.2, 0, 0, 1)` | Toggle states, chip selection |
| `motion.quick` | 160 ms | `cubic-bezier(0.2, 0, 0, 1)` | Reflow crossfade, chrome fade |
| `motion.standard` | 240 ms | `cubic-bezier(0.2, 0, 0, 1)` | Theme crossfade, sheet detent change |
| `motion.gentle` | 320 ms | `cubic-bezier(0.05, 0.7, 0.1, 1)` | Sheet entrance, TOC jump |
| `spring.gesture` | — | k = 300, critically damped | Page snap, rubber band, zoom settle |

`prefers-reduced-motion: reduce` replaces translation with opacity, curl with
fade, and springs with 120 ms linear fades. Nothing animates *in response to
time alone*; every motion answers a reader action.

---

## 6. The Reader Settings panel

### 6.1 Interaction model

- **Entry:** the `Aa` button in the top bar. Also `,` on keyboards and a
  two-finger long-press anywhere on the page on touch devices.
- **Form factor:** a **bottom sheet** on compact and medium widths, where it is
  thumb-reachable and has detents at 45% and 85%. On expanded and large
  widths it is a **floating popover** anchored to `Aa`.
- **Live preview:** the page stays visible above the sheet (mobile) or beside
  the popover (desktop). Every change renders behind the glass instantly.
- **Dismiss:** swipe down, tap the page, `Esc`, or tap `Aa` again. State is
  saved continuously, so there is never an Apply or Cancel button.
- **Scope:** size, theme, spacing and layout are **global**, and follow the
  reader across books and devices. "Original" font and publisher styles are
  **per book**.

### 6.2 Material: frosted vellum

```css
.settings-surface {
  background: var(--glass-tint);                 /* 88% tint: legibility floor */
  backdrop-filter: blur(24px) saturate(140%);
  -webkit-backdrop-filter: blur(24px) saturate(140%);
  border: 1px solid var(--hairline);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.06),   /* top light edge */
              0 12px 40px rgb(0 0 0 / 0.18);
  border-radius: 28px;
}
@media (prefers-reduced-transparency: reduce) {
  .settings-surface { background: var(--surface-1); backdrop-filter: none; }
}
```

- **Legibility guarantee:** with an 88% tint, the *worst case* (dark image
  under light glass, white image under dark glass) still gives primary
  labels ≥ 7.3:1 and secondary labels ≥ 5.4:1 (`--ink-2-on-glass`). The glass
  shows enough to feel alive and never compromises text.
- **Performance:** backdrop blur over a WebGL canvas is composited by the
  browser. Tier C devices get the solid surface automatically.

### 6.3 Mobile: bottom sheet

```text
┌────────────────────────────────────────────┐
│ ‹  The River Book             Search   Aa  │  top bar (chrome visible)
│                                            │
│   …and so the river kept its own counsel,  │  live page: every change
│   as rivers do, long after the town had    │  previews here instantly
│   forgotten the name it once gave it.      │
╞════════════════════════════════════════════╡
│                  ───────                   │  grabber · drag to dismiss
│                                            │
│   ( Aa )  ( Aa )  ( Aa )  ( Aa )  ( Aa )   │  theme swatches, rendered
│   Paper   Linen    Dusk   Night    OLED    │  in each theme's own colors
│                                            │
│   ○  ────────────────●───────────────  ◉   │  brightness · left of the
│                                            │  OS floor = extra-dim
│   A  ──┼──┼──┼──●──┼──┼──┼──┼──┼──┼──  A   │  text size · 12 detents
│                18 px                       │  haptic tick per step
│                                            │
│   ‹ Literata │ Source Serif │ Atkinson ›   │  font chips, each set
│                                            │  in its own face
│   Layout    [ Pages | Scroll ]             │
│   Spacing   [ Compact | Comfort | Airy ]   │
│                                            │
│   More settings                         ›  │  progressive disclosure
└────────────────────────────────────────────┘
```

### 6.4 Desktop: anchored popover

```text
                                    ┌────┐
   ‹  The River Book     Search     │ Aa │  ⋯
                                    └─┬──┘
              ┌───────────────────────┴──────────────┐
              │ Theme                     Auto  ○──● │
              │ (Aa) (Aa) (Aa) (Aa) (Aa)             │
              │                                      │
              │ Brightness   ○ ───────●──────── ◉    │
              │ Warmth    cool ───●──────────── warm │
              │──────────────────────────────────────│
              │ Text size    A− ──┼──┼──●──┼──┼── A+ │
              │ Font         Literata              ▾ │
              │ Spacing      [Compact|Comfort|Airy]  │
              │ Margins      [ Narrow|Normal|Wide ]  │
              │ Justify  ●──○        Hyphenate  ●──○ │
              │──────────────────────────────────────│
              │ Layout   [ Pages | Scroll ]          │
              │ Columns  [ Auto | 1 | 2 ]            │
              │ Turn     [ Slide | Curl | Fade ]     │
              │──────────────────────────────────────│
              │ Accessibility & more               › │
              └──────────────────────────────────────┘
```

Desktop shows more controls at the first level because a pointer is precise
and the screen is large. The hierarchy of groups is the same, so a reader who
knows one knows both.

### 6.5 Control specification

| Control | Type | Values | Default | Notes |
|---|---|---|---|---|
| Theme | Swatch row (radio group) | Paper, Linen, Dusk, Night, OLED | Auto (Paper ↔ Night) | Swatches are mini-pages in their own colors. Long-press opens the auto schedule. |
| Auto theme | Switch | on / off | on | Follows the OS, with an optional local-time schedule |
| Brightness | Slider | −60% … 0 … OS | 0 | Below 0 is in-app extra-dim (shader). Above 0 hands off to the OS. |
| Warmth | Slider | 0–100 | 0 day / 40 night | Ramps over 20 min in auto mode |
| Text size | Detented slider | 12 steps (§3.3) | 18 / 20 px | Pinch on the page does the same thing |
| Font | Chips (mobile) / menu (desktop) | Literata, Source Serif, Atkinson, Original (+ Arabic counterparts) | Literata / Noto Naskh | Arabic books show Arabic faces |
| Spacing | Segmented | Compact, Comfort, Airy | Comfort | Sets leading and paragraph rhythm together |
| Margins | Segmented | Narrow, Normal, Wide | Normal | |
| Justify | Switch | on / off | on | Off means ragged edge with optimal breaks (still Knuth–Plass) |
| Hyphenate | Switch | on / off | on | Hidden for scripts that don't hyphenate |
| Layout | Segmented | Pages, Scroll | Pages | |
| Columns | Segmented | Auto, 1, 2 | Auto | Spread on ≥ 1024 px landscape |
| Page turn | Segmented | Slide, Curl, Fade | Slide (Curl offered on tier A) | Fade is forced under reduced motion |
| **More →** | | Tap-anywhere-to-advance · Publisher styles · Dark-mode images (natural, dimmed, smart) · Number style · Screen-reader mode · Dyslexia preset · Reduce motion · Reset to defaults | | |

**Fixed-layout books** swap the typography group for **View**: Fit page, Fit
width, Zoom % · Guided view (column or panel) · Spreads (on or off).

---

## 7. Reader chrome and key screens

### 7.1 Reading view

```text
 immersive (default)                         chrome visible (tap center)
┌──────────────────────────────┐            ┌──────────────────────────────┐
│                              │            │ ‹ Back   The River Book   Aa │
│  Chapter Four                │            │                              │
│                              │            │  Chapter Four                │
│  THE TOWN HAD forgotten the  │            │                              │
│  name it once gave the river,│            │  THE TOWN HAD forgotten the  │
│  but the river had not for-  │            │  name it once gave the river,│
│  gotten the town.            │            │  …                           │
│  …                           │            │ ─────────────●────────────── │
│                              │            │ Ch. 4 · 14 min left       ☰  │
│              ·               │            │ Search    Notes    Contents  │
└──────────────────────────────┘            └──────────────────────────────┘
   a single dot of progress:                   scrubber with chapter ticks;
   no numbers unless asked                     preview bubble while dragging
```

### 7.2 Supporting surfaces

| Surface | Design |
|---|---|
| **Contents** | A sheet with a chapter list. The current chapter is marked by an accent rule, not a background fill. Each row shows the reader's notes count. |
| **Search** | A sheet with the query field on top. Results are grouped by chapter, and snippets are rendered by Lumen in the book's typography. A scope toggle switches between *This book* and *My library*. |
| **Notes & highlights** | A list grouped by chapter with color shape markers. Export as quote cards or as a citation list (metered). |
| **Library** | Covers on a calm grid. "Continue reading" is a single large card with time left. No carousels inside carousels. |
| **Book page (www)** | SSR, with cover, description, reading time, sample quote card and a **Start reading** button that opens the reader in under 1 s. No signup wall for the free sample. |
| **Onboarding** | None before reading. At the end of the sampled chapters, one calm, typeset interstitial: *"Keep reading — save your place across devices."* Passkey in one tap. |

---

## 8. Mobile-first and responsive behavior

- **Design at 360×740 first.** Every screen is designed and approved on a
  compact phone before being expanded.
- **Thumb zones.** Primary controls (settings, progress, contents) sit in the
  bottom 40% of the screen. The top bar carries only *Back* and the title.
- **Touch targets:** ≥ 44×44 pt (iOS) and ≥ 48×48 dp (Android). The page-turn
  zones are the largest targets in the app.
- **Orientation and foldables:** rotation reflows with anchor preservation.
  Two-page spreads activate when the landscape width can hold two full
  measures. On foldables, the hinge acts as the spread gutter.
- **Keyboard and pointer** (expanded and up): every action has a shortcut, and
  hover reveals the chrome edges subtly. Trackpad horizontal swipes turn pages.
- **PWA:** installable, full-screen standalone display, splash screen in the
  paper color of the reader's last theme. Installation also lifts iOS's
  7-day storage eviction for offline books.

---

## 9. RTL and internationalization

| Concern | Rule |
|---|---|
| **UI direction** | Follows the *interface* language. Tailwind logical properties (`ms-*`, `pe-*`, `start-*`) are mandatory, and lint rules ban physical left/right. |
| **Page progression** | Follows the *book*, never the UI. An Arabic UI reading an English novel turns pages LTR. An English UI reading an Arabic book turns them RTL. |
| **Gestures** | Swipe direction and tap zones follow page progression |
| **Progress scrubber** | Fills in the book's progression direction |
| **Mixed strings** | Titles and author names in UI strings are wrapped in bidi isolation (`<bdi>`, `dir="auto"`) |
| **Numerals** | Per-locale default (Arabic-Indic or Western), overridable in settings |
| **Dates and times** | `Intl` APIs throughout. "14 min left" is localized with the correct plural rules. |
| **Fonts** | UI falls back Inter → IBM Plex Sans Arabic by script. Arabic UI is one type step larger (§3.4). |

---

## 10. Accessibility standards for the chrome

- **WCAG 2.2 AA minimum** for all chrome. **AAA (7:1)** for reading text, which
  every theme exceeds (§4.2).
- React Aria Components for all interactive primitives: correct roles, focus
  management, keyboard support and RTL behavior by default.
- Visible focus rings (`--focus-ring`), never removed. Focus is trapped
  inside open sheets and restored on close.
- Respect `prefers-reduced-motion`, `prefers-reduced-transparency`,
  `prefers-contrast: more` (which raises ink and accent contrast by a further
  step) and `forced-colors` (the chrome falls back to system colors).
- The canvas page is reached through screen-reader mode
  ([03 §8.4](03-reader-engine.md#84-accessibility-mode)). Chrome is fully
  accessible at all times.
