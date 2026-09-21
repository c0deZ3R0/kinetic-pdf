# Comparing and overlaying pages without rendering them first

Two prototypes on the `vector-compare` branch, both built on the `Shapes`
`gpu-lines` already produces. Nothing in the app calls either yet.

- `examples/compare.rs` -- comparing two pages by their geometry.
- `crates/gpu-lines/examples/overlay.rs` -- overlaying pages as a way of
  drawing them, with `Renderer::paint_tinted`.

The question behind both was whether a vector PDF needs to be rasterised at
all. Bluebeam's Compare Documents renders both pages and diffs the pixels,
which is why it needs `CompareRenderDPI`, `CompareSensitivity`,
`CompareDensity` and `CompareHollowThreshold`; its Overlay Pages writes a new
PDF with a layer per source, which is why it needs a dialogue asking what to
do with the layer captions when you flatten it. Neither is necessary.

## Comparing

1. Fingerprint every primitive by its *relative* geometry and its style,
   keeping its position apart, so identical linework hashes the same wherever
   it sits.
2. Pair up the fingerprints seen exactly once on each side. On this sheet that
   is 15,957 of 117,869 primitives -- the rest are identical hairline pieces
   that know nothing about where the page has moved.
3. Vote for one offset from those pairs, then take the mean of the winning
   bucket so the answer isn't stuck to the vote grid.
4. Match A to B through a grid at that offset, checking neighbouring cells
   too. Matches are verified against the real relative geometry, so a hash
   collision can't invent one.
5. Fit a similarity -- rotation, one scale, translation -- to the matched
   pairs in closed form (Horn). That is what OpenCV's `estimateRigidTransform`
   / `estimateAffinePartial2D` solves for, in about twenty lines and no
   dependency.
6. Cluster what's left with a union-find over cells, which is where clouds
   would go.

The fingerprint is deliberately **not** invariant to rotation or scale.
Making it so costs far more than it is worth: two revisions of a sheet from
the same CAD system share an origin exactly. The cases that don't are few and
discrete, so `candidates` tries them -- right angles, and the scale read off
how far the linework spreads -- scored by the vote alone, which skips the
matching pass entirely.

### Numbers

Page 1 of a civil works set: 117,869 primitives, 2,235 styles,
2,450 image pieces. Release build, one thread.

(At the 0.1 pt cell the real files later forced; see the last section.)

| Case | Matched | Candidates | Compare |
|---|---|---|---|
| The page against itself | **100.00%**, 0 left | 1 of 4 | 60 ms |
| Moved `[12, -5]` | **100.00%**, 0 left | 1 of 4 | 71 ms |
| Turned 90° and moved | **100.00%**, 0 left | 4 of 4 | 84 ms |
| Turned 0.35°, `--sweep 1 0.05` | **100.00%**, 0 left | 18 of 44 | 193 ms |
| Page 1 against page 2 | 7.67% | 8 of 8 | 229 ms |

Reading the two pages costs 160-300 ms each and dominates everything:

```
Fingerprinting A               3.1 ms  117869 primitives, cell 0.1 pt
Searching   1 of   4            8.8 ms  best: B as it is
Matching                      41.7 ms  117869 pairs
Clustering                     0.0 ms  0 differences
```

### What the cases show

- **Identical pages match completely**, residual 0.00000 pt mean and 0.00101
  pt worst, which is f32 noise rather than disagreement. No raster comparison
  can make that claim at any sensitivity.
- **Different sheets do not false-match.** Page 1 against page 2 matched only
  7.67% -- the shared border and title block -- and found a consistent offset
  for exactly that shared geometry, residual 0.00402 pt. The clustering then
  reports the whole drawing area as one difference, which is right.
- **The fine sweep works but costs**, ~11 ms a candidate. A fallback, not a
  default path.

### Three things the first cut got wrong

1. **Rotation and scale broke it entirely** -- 0.46% matched on a 0.35° turn,
   because the fingerprint is quantised relative geometry and so isn't
   similarity-invariant. Fixed with the candidate search above rather than by
   chasing an invariant fingerprint, which a single stroke segment can't carry
   anyway.
2. **A right angle left 32 stragglers** of 117,869, because `cos(90°)` in f32
   is -4.4e-8 rather than 0. `Sim::sin_cos` now returns exact values on the
   quarter turns: 0 left.
3. **A pure translation left 14**, because `(p1 + t) - (p0 + t)` differs from
   `p1 - p0` in its last bits, so a shape's own geometry rounded into the next
   hash bucket and its partner was never looked at. The hash now goes on a
   grid `COARSE` times wider than the check does: 0 left.

Each of those was only visible because the synthetic moves (`--shift`,
`--rotate`, `--scale`) are checked against a known answer.

## Overlaying

`Renderer::paint_tinted` draws a page in one colour instead of its own,
multiplied into what's already there. Each shape takes as much of the tint as
it is dark, so paper and pale linework keep out of the way and black linework
takes it whole. Painting two pages one after another, each with its own tint,
*is* the overlay: coincident linework goes darker than either page, and what
only one page draws keeps that page's colour.

There is no render-to-texture, no new document, and nothing to flatten. It is
the same draw the viewer already does, once per page, so on screen it costs
nothing more at any zoom and the layers can be toggled instantly.

| | |
|---|---|
| Pages 1 and 2, 510k primitives, into 1800 x 1391 | **43 ms** |
| Page 1 alone, 118k primitives, into 1200 x 927 | 21 ms |

Reading the two pages took 966 ms and 485 ms; the overlay itself is the 43 ms.

The example writes a PNG so the result can be looked at without a viewer
(`tmp/overlay/`), but that is the example's doing -- `overlay_to_image` is
`paint_tinted` into an offscreen framebuffer, and on screen the same calls go
straight into the viewport.

### Changes this needed in `gpu-lines`

- `u_tint` in the shape fragment shader, off when its alpha is 0, so the
  normal path is untouched. All 78 crate tests and the app's 67 still pass.
- `Renderer::paint_tinted`, with `paint` delegating to it. A tinted page
  multiplies every run, since the blends its own shapes asked for say nothing
  once it is all one colour. Highlighter marks are drawn untinted.
- `Renderer::overlay_to_image`, and `draw_to_image` refactored onto a shared
  `onto_paper`.
- `Tint`, with `Tint::WHEEL` -- red, blue, green, in that order, because that
  is what every overlay uses and what everyone reads without being told.
- `page_size`, since `placed_page` already worked the page's size out and
  nothing could get at it.

## What neither does yet

- **No clouds are written.** Compare's output is a list of boxes on stdout,
  not annotations through `pdf-io`.
- **No UI.** Neither is reachable from the app.
- **Images are excluded from the fingerprint**, because an image's style
  colour is its place in the texture atlas. 2,450 image pieces on this page,
  so a mixed sheet still wants a raster pass confined to those boxes. That
  tier doesn't exist.
- **No raster baseline was measured.** That this beats rendering both pages
  and diffing them is argued, not benchmarked.
- **Text is treated as geometry.** Diffing extracted runs as a sequence would
  say "REV 3 → REV 4" instead of clouding glyph outlines.
- **Moved geometry reads as added plus deleted.** The matched-pair transform
  spots a whole-page move; a local one is still two differences.
- **A mid-tone image takes a heavy tint** in an overlay and can bury what's
  under it -- the aerial photo on this sheet does. Bluebeam has the same
  problem and answers it with `OverlayPagesAdvancedColorShading`.
- **One thread, one page.** No `rayon`, no use of the helper pool in
  `pool.rs`.

## Tried on two real revisions

Rev A of a road intersection set against rev B2 of the same set, which also
adds three sheets. This is the first time either prototype saw files it wasn't written
against, and it found a real mistake.

**The first run matched 0.00%.** Not a crash, not a bad alignment -- the vote
found a clean offset from 1,626 agreeing pairs -- just nothing matched at all.

The cause was `QUANT`, the matching cell, at 0.01 pt. That number was chosen
for what a draughtsman would call a change: a hundredth of a point is far
below anything anyone draws. But the number that matters is how exactly *two
exports of the same drawing write the same coordinate*, and the example now
measures it:

```
offset [-0.0215, -0.0424]
they agree to within   0.0215 / 0.0424 / 0.3385 pt (median/p99/worst)
```

Identical linework sits a median 0.02 pt and a worst 0.34 pt from where the
offset says it should be -- two to thirty cells away. It was never looked at.

Sweeping the cell by hand shows where the truth is:

| cell | page 9 | page 1 |
|---|---|---|
| 0.01 | 0.27% | 0.00% |
| 0.05 | 66.42% | -- |
| 0.10 | 86.40% | 12.69% |
| 0.25 | 87.85% | 13.18% |
| 0.50 | 87.20% | 14.40% |
| 1.00 | 86.67% | 8.53% |

It plateaus. Everything from 0.1 pt up agrees on roughly the same answer, so
the remainder is genuine difference rather than something the cell is still
hiding -- which is the only reason to believe any of these numbers.

Three changes came out of it:

1. `QUANT` is now **0.1 pt**, chosen from that plateau rather than from
   instinct.
2. The cell **widens itself** when the files ask: the scatter of the
   distinctive pairs is measured, and if it is wider than the cell, both sides
   are fingerprinted again at one that fits. Page 9 re-reads at 0.166 pt.
   `--cell` overrides it.
3. Matching takes the **nearest** verified candidate rather than the first.
   A cell wide enough to absorb the rounding is wide enough to hold several
   primitives, and taking whichever came first spent partners on the wrong
   ones -- it cost 3.14% on a page compared against itself, which is how it
   was noticed.

### Where it lands

Pages line up by index across the two revisions.

| | A p1 | p2 | p4 | p9 | p10 | p12 |
|---|---|---|---|---|---|---|
| matched | 12.80% | 46.29% | 61.37% | **88.80%** | **88.82%** | 84.37% |

Page 9 in full: 57,029 against 64,385 primitives, read in 16 ms each, compared
in 58 ms, 202 differences. The largest are seven bands about 640 x 128 pt at
regular spacing -- the cross-section panels -- each mostly *added* rather than
deleted, which is rev B2 filling them in.

Page 1 at 12.80% is not a failure either: it is the drawing index, and B2 has
three more sheets to list.

The overlay of the same sheet (`tmp/overlay/rev-A-vs-B2-p9.png`, 2600 px, 28
ms) reads exactly as it should. The frame, grid and labels go dark because
both revisions draw them; the design and existing surface lines split into red
and blue where the levels changed; and in the revision block "ISSUED FOR
CONSTRUCTION" is dark while "REVISED TO RSA COMMENTS" is blue alone, with rev
A's date under rev B2's.

The synthetic cases are all still exact: a page against itself, moved, and
turned a right angle each match 100.00% with nothing left over.

## Writing the overlay out as a PDF

`examples/overlay_pdf.rs`. The GPU overlay is the right thing while you're
working, but an overlay you mean to send someone has to be a document, and it
should still be a drawing when it arrives -- vectors that zoom and print, not
a picture of a drawing.

The obvious way to tint a page in PDF is a luminosity soft mask over a solid
fill: elegant, needs no rewriting at all, and wrong here. `interpret.rs:777`
counts soft masks among the things it can't draw, and `:270` transparency
groups, so every overlay written that way would fall off our own renderer onto
pdfium -- for the one feature whose whole argument is that it's fast.

So it recolours the vectors and blends with `/BM /Multiply`, which
`interpret.rs:767` already understands. Recolouring sounds like the bigger
job, but a drawing has a handful of pen colours -- 26 to 33 a sheet here --
and the colour operators say what they are. Each one is left where it is and
a replacement written straight after it, so no operand is touched and nothing
else in the stream moves. Each source becomes a form XObject in its own OCG,
so the layers toggle in any viewer.

Page 2 of the two revisions: **1.4 MB, two layers, 2384 x 1684 pt**, written
in about 150 ms after the pages are read.

### What the two-renderer check found

The point of rendering the written file with pdfium as well as with ours is
that pdfium has never seen any of this code. Checking it with the renderer
that wrote it only proves it agrees with itself. It earned its keep three
times, and each fault was invisible in the numbers alone -- they only showed
up as two pictures side by side:

1. **Every sheet came out upside down.** Both renderers agreed, so it wasn't
   a disagreement -- it was the file. `/Rotate 270` on a portrait MediaBox,
   and the quarter-turn matrices for 1 and 3 were swapped: `/Rotate` is how
   far clockwise the *page* turns, so the content turns with it. A mistake
   that is invisible on a square page.
2. **The tint was being wiped.** `53.79 -> 42.71` apart. Choosing a colour
   space resets the colour to that space's default, so `0 0 0 RG` then
   `/DeviceGray CS` threw the tint away, and the two renderers disagreed
   about exactly when. Tinting the space's default as well fixed it.
3. **Ours wasn't multiplying.** `40.31 -> 27.00`. Every ExtGState in the
   drawing says `/BM /Normal`, so the sheet's own `gs` calls switched the
   outer multiply back off partway through and the second layer painted over
   the first instead of darkening it. Forcing `/BM /Multiply` into every
   imported graphics state means nothing inside the page can turn it off.

### Knowing when to stop

Two independent rasterisers will never agree pixel for pixel on a dense line
drawing: sub-pixel placement, antialiasing and hairline rules all differ. So
`--strength 0` writes a page through untinted and unblended, and the same
comparison runs on that. That is the floor.

| | pixels with ink | apart on average |
|---|---|---|
| One page written through, no tint (the floor) | 327,481 | **27.93** |
| The two-layer tinted overlay | 352,616 | **27.00** |

The overlay sits at the floor -- marginally under it. Whatever the two
renderers still disagree about is how they draw lines, not anything wrong
with the file.

### What this doesn't handle

- **Images and shadings keep their own colours**, so they multiply in their
  own hue rather than the layer's. Few on these sheets; a survey sheet full
  of photographs would look wrong.
- **Pattern fills are left alone** -- `scn` with a name is skipped, so
  hatching keeps its colour rather than taking the tint.
- **Separation and DeviceN defaults are assumed black.** True for the spaces
  a drawing uses, not in general.
- **Only pdfium and ours have seen these files.** Acrobat and Bluebeam
  haven't, and print drivers haven't.

### A whole set at once

`--all` pairs the sheets of two documents in order and writes one overlaid
page for each. Each source is loaded once and becomes one layer, so turning a
revision off turns it off on every sheet, and objects it shares between sheets
-- fonts above all -- are brought across once rather than once a page.

Two nineteen-sheet revisions of the road set, A1 sheets at 2384 x 1684 pt, on
a 24-thread machine:

| | |
|---|---|
| Reading both documents | 127 ms |
| Building 19 pages, 38 layers, 179,808 colours tinted | 160 ms |
| Deflating 68.9 MB | 332 ms |
| Writing it out | 40 ms |
| **Whole run** | **~0.7 s** |
| Written | 18.6 MB |

### Why it first took six seconds

The first version took **6.0 s**, and reporting it as "compressing and saving"
hid where it went:

```
building                    899.4 ms
deflating  68.9 MB         5402.8 ms
writing it out               75.8 ms
```

Writing the file was 76 ms. The rest was zlib, one stream after another, at
about 13 MB/s.

None of the 68.9 MB is avoidable. Recolouring a page means decompressing its
content, so unlike the images -- copied across with the filter they arrived
with and never touched -- every content stream has to go back through zlib on
the way out. A set of drawings is tens of megabytes of instructions once
unpacked.

What was avoidable was doing any of it one at a time. Breaking the building
half down showed the same shape again:

```
building                    839.8 ms
  inflating  64.8 MB        162.0 ms
  recolouring               234.1 ms
  importing                 440.6 ms   <- inflating and recolouring forms too
```

Almost all of it is per-stream work -- inflate, rewrite, deflate -- and every
stream is independent. Only copying the object graph is genuinely sequential,
because the objects a source brings across are shared between its sheets.

Three changes, in the order they paid:

1. **Deflate every stream at once** rather than through `Document::compress`,
   which walks them in order.
2. **Take the stream work out of the sequential part.** `import` no longer
   recolours the forms it copies; it notes them down and `retint_forms`
   does them all afterwards. Page content is inflated and recoloured up
   front, before anything is assembled. Importing fell from 441 ms to 4 ms,
   because what was left was only copying dictionaries.
3. **Take the next free stream, not a fixed share.** A set has a few sheets
   several times the size of the rest, so handing each thread an equal
   *count* leaves most of them idle while one grinds through the big one.
   `in_parallel` hands out the next one that is free, for the cost of an
   atomic add. That alone took preparing from 358 ms to 142 ms -- more than
   threading it in the first place had.

| | then | now |
|---|---|---|
| Reading both documents | 127 ms | 127 ms |
| Building | 899 ms | **160 ms** |
| Deflating 68.9 MB | 5,403 ms | **332 ms** |
| Writing it out | 76 ms | 40 ms |
| **Whole run** | **6.0 s** | **~0.7 s** |
| Written | 18.5 MB | 18.6 MB |

Same file: both renderers still draw it 22.40 apart over the same 629,265
inked pixels, which is what it was before any of this.

`--level 3` takes deflating to about 180 ms for 3% more bytes, and `--quick`
skips it for a file about to be read once. The default stays at 6.

### What is left

About 0.7 s, of which 0.5 s is zlib in both directions on 130 MB and 0.13 s is
parsing two documents. Both are near what the machine will do. Getting
materially below this would mean not recolouring the content at all -- which
is the method -- or writing streams uncompressed and accepting a 75 MB file.

Worth knowing before a UI calls this: a whole set is under a second.
