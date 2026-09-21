# Comparing and overlaying revisions: how the UI might work

An idea, not a plan. Nothing here is built. The engine behind it is on this
branch as `examples/compare.rs`, `crates/gpu-lines/examples/overlay.rs` and
`examples/overlay_pdf.rs`, with the numbers in
`bench-results/2026-09-21-vector-compare.md`.

The shape of it follows from one thing the prototypes proved: **an overlay is
a view, not an export.** Every other program makes you produce a document
before you can look at a comparison. We don't have to, and the whole thing
should be built around not having to.

## The workflow

**1. Bring in the second file.** Drag it onto an open document, or "Compare
with...". No wizard, no dialogue. The app goes from holding one document to
holding a pair.

**2. Sheets pair themselves, and can be corrected.** A strip along the bottom
showing one set's sheets against the other's, paired. This is the one
genuinely hard part: pairing by page number works until a revision inserts a
sheet, which they do.

We already have a pairing score without noticing it. The candidate search in
`compare.rs` counts how many distinctive fingerprints agree on one offset, and
it costs about 10 ms because it stops before the matching pass. That *is* a
sheet-similarity score. So pair by best score rather than by page number or by
reading title blocks -- seeded with the page number, so it is instant when the
page number is right. A wrong pair gets dragged onto its proper partner.

**3. The overlay is simply the view.** Live, at any zoom, no file written.
Layer toggles per revision, a strength slider, colour chips.

Then the thing worth building the feature around: **hold a key to blink
between them.** Hold a key for one revision, release for the other.
Astronomers found moving objects this way for a century, because the eye
catches a change that jumps far better than one drawn in another colour. It is
free once drawing is live, and nothing on the market can do it, because they
all bake a file first.

**4. A list of differences, not a hunt for clouds.** Like the markups list:
*"1,818 added / 380 deleted -- 640 x 128 pt"*, biggest first, click to fly to
it, tick when it has been looked at. Everything else clouds the sheet and
leaves you to find them; we already compute the clusters, so show them as a
worklist. This is where reading the geometry pays twice -- we know *added*
from *deleted*, which no pixel diff can tell you.

**5. Committing and exporting are deliberate, at the end.** "Write clouds"
turns the ticked differences into real annotations through `pdf-io` --
standard ones, so they open anywhere. "Save overlay" writes the PDF. Both are
things you choose, not the machinery you have to go through to see anything.

## What settings there are

Almost none, and that is the point. No DPI, no sensitivity, no density, no
hollow threshold, no preset for scanned against printed: reading the geometry
needs none of them, and the matching cell sizes itself from the two files.
What is left is proximity (how far apart two changes are still one change),
a margin to ignore for title-block churn, and the layer colours.

## Where it will hurt

- **Pairing is where this lives or dies.** The rest is mechanical.
- **Memory doubles.** `pool.rs` budgets open pages against `private_bytes`;
  an overlay holds two sheets' shapes at once and that budget has to know.
- **Reading pages dominates.** 160-300 ms a page against 27-58 ms to compare
  one. Pairing a twenty-sheet set means reading forty pages, so it wants the
  helper pool and a progress indication, not a frozen window.
- **A scanned sheet has to say so.** Today it silently matches almost
  nothing, which reads as "no changes". That is the one place a wrong answer
  is dangerous, and it needs to say "this sheet is scanned; comparing it
  needs a raster pass, which isn't built".

## What to build first

The smallest slice that is worth using: **the live overlay as a view mode,
paired by page number, with the blink key.** No differences list, no export,
no pairing strip. It reuses `paint_tinted` as it stands, and a day of using it
on real revisions would say whether the blink is as good as it looks on paper.

The differences list second, since it needs the worklist and cloud writing.
Pairing third, once pairing by page number has gone wrong often enough to say
what the correction gesture should be.

There is an obvious fourth, given where the measurement work is heading: a
difference carrying through to "measure this again". The reason anyone opens a
revision comparison is to find out what they have to price again. That last
one is a guess about who uses this, not something the prototypes showed.
