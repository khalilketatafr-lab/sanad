# @sanad/theme-review

Roadmap §10.6: one self-contained page for the physical-device theme review.

```sh
pnpm --filter @sanad/theme-review build
# dist/theme-review.standalone.html  open on a phone (any static host, or AirDrop/file://)
# dist/theme-review.html             same body without the document shell (artifact hosting)
```

The build runs `cargo run -p sanad-atelier --example golden_pages`, which uses
the same pipeline as the golden tests. It then inlines the shredded atlas as a
PNG, the three pages' `GLYPH_INSTANCE` buffers, and the bundled Lumen renderer.
The phone draws the pages with the production WebGL2 path at its own DPR. The
page never contains any text from the book: only permuted glyph ids and atlas
fragments (P1).

What a reviewer checks, per phone and per lighting condition (dark room at
minimum brightness, then daylight at full brightness):

- every theme on the Latin, Arabic and mixed pages;
- Night with the warmth and extra-dim sliders at the shipped maximum;
- the settings sheet over the page, using the worst-case glass contrast;
- the checklist at the bottom of the page.

The checklist is stored per phone, in `localStorage`. Record the phone model
from the device line under the page next to each finding.
