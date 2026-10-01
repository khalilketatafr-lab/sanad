# @sanad/tokens

Marginalia's five reading themes (Paper, Linen, Dusk, Night, OLED) in one
W3C Design Tokens file (`tokens.json`, DTCG 2025.10). Style Dictionary 5
(`sd-config.js`) builds them into:

| Output | Path | Committed |
|---|---|---|
| CSS custom properties | `build/css/themes.css` (`@sanad/tokens/css`) | no |
| TS constants (`THEMES`, `ThemeShaderUniforms`) | `build/ts/tokens.ts` (`@sanad/tokens/ts`) | no |
| Rust `ThemeUniforms` struct | `crates/tokens/src/theme_uniforms.rs` | yes, so Cargo never needs Node |

```sh
pnpm --filter @sanad/tokens build      # all outputs
pnpm --filter @sanad/tokens check      # CI: the committed Rust file is fresh
pnpm --filter @sanad/tokens test       # pipeline, guarantees, blueprint agreement
```

## Changing a color

1. Edit the OKLCH `components` in `tokens.json`. OKLCH is authoritative.
2. Run the build. It fails and prints the hex the OKLCH value rounds to:
   `hex #… is not the 8-bit rounding of its OKLCH value (expected #…)`.
   Copy that hex into the token.
3. Update the matching row in `docs/blueprint/04-design-system.md` §4.2–4.3.
   The tests compare every table with the tokens.
4. Rebuild, then commit `tokens.json`, the docs and
   `crates/tokens/src/theme_uniforms.rs`.

## What the build and tests refuse

- A hex that is not the exact rounding of its OKLCH value, or an out-of-gamut
  color.
- A missing or unknown token, or a derived token (`glass-tint`, `hairline`,
  `focus-ring`) whose color no longer equals its `derivedFrom` base.
- Shader parameters that break the composite's contract:
  - a translucent shader color;
  - a dark theme with positive `weight` (or a light theme with negative weight);
  - a `lumaCeil` below any palette color's luminance, which would dim the text
    and break the page/chrome match;
  - in dark themes, a `lumaCeil` above the ink's luminance rounded up to 0.01.
- Contrast below the floors:

  | Pair | Floor |
  |---|---|
  | ink / paper | 7:1 |
  | ink-2 / paper | 4.5:1 |
  | accent / paper | 4.5:1 |
  | ink / highlights | 6.5:1 |
  | ink / surfaces | 7:1 |
  | ink / glass, worst-case backdrop | 7:1 |
  | ink-2-on-glass / glass and surfaces | 5.4:1 |
  | focus ring / every background | 3:1 |

Shader colors are the linearized **shipped hex**, not the unrounded OKLCH, so
Lumen's page produces exactly the CSS chrome pixels. A GPU test in
`packages/lumen/test/webgl.test.ts` renders every theme and checks this.
