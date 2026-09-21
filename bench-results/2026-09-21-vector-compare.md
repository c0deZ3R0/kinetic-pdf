# Comparing pages by geometry rather than pixels

A prototype on the `vector-compare` branch: `examples/compare.rs`, built on the
`Shapes` that `gpu-lines` already produces. Nothing in the app calls it yet.

The question was whether a vector PDF can be compared without rendering it --
so without a DPI, a sensitivity, a density or a threshold, which is what every
raster comparison needs and what all of Bluebeam's `CompareSensitivity`,
`CompareDensity`, `CompareHollowThreshold` and `CompareRenderDPI` settings are
for. It can.

## How it goes

1. Fingerprint every primitive by its *relative* geometry and its style,
   keeping its position apart, so identical linework hashes the same wherever
   it sits.
2. Pair up the fingerprints seen exactly once on each side. On this sheet that
   is 15,957 of 117,869 primitives -- the rest are identical hairline pieces
   that know nothing about where the page has moved.
3. Vote for one offset from those pairs, then take the mean of the winning
   bucket so the answer isn't stuck to the vote grid.
4. Match A to B through a grid at that offset, checking the neighbouring cells
   too, so a coordinate that rounds across a boundary still finds its partner.
   Matches are verified against the real relative geometry, so a hash collision
   can't invent one.
5. Fit a similarity -- rotation, one scale, translation -- to the matched pairs
   in closed form (Horn). That is what OpenCV's `estimateRigidTransform` /
   `estimateAffinePartial2D` solves for, in about twenty lines and no
   dependency.
6. Cluster what's left with a union-find over cells, which is where clouds
   would go.

The fingerprint is deliberately **not** invariant to rotation or scale. Making
it so costs far more than it is worth: two revisions of a sheet from the same
CAD system share an origin exactly. The cases that don't are few and discrete,
so `candidates` tries them -- right angles, and the scale read off how far the
linework spreads -- scored by the vote alone, which skips the matching pass.

## Numbers

All on page 1 of a civil works set: 117,869 primitives, 2,235
styles, 2,450 image pieces. Release build, one thread.

| Case | Matched | Compare | Note |
|---|---|---|---|
| The page against itself | 100.00% | 54 ms | 0 differences, residual 0.00000 pt |
| Moved `[12, -5]` | 100.00% | ~50 ms | offset recovered exactly |
| Turned 90° and moved | 99.97% | 61 ms | 32 stragglers of 117,869 |
| Turned 0.35°, `--sweep 1 0.05` | 100.00% | 540 ms | 44 candidates searched |
| Page 1 against page 2 | 5.43% | 200 ms | control: different sheets |

Reading the two pages costs 160-300 ms each and dominates everything. The
comparison itself is 54 ms on a page of 118k primitives, and most of that is
the candidate search, not the work:

```
Fingerprinting A               2.0 ms  117869 primitives
Searching   4 candidates      30.3 ms
Matching                      19.4 ms  117869 pairs
Clustering                     0.0 ms  0 differences
```

### What the cases show

- **Identical pages match completely.** The residual is 0.00000 pt mean,
  0.00101 pt worst, which is f32 noise, not disagreement. A raster comparison
  cannot make this claim at any sensitivity.
- **A right angle leaves 32 stragglers** out of 117,869, because `cos(90°)` in
  f32 is -4.4e-8 rather than 0, and a handful of primitives round across a
  quantisation boundary. They cluster into 4 tiny boxes, the largest 7.8 x 24.8
  pt. Real, and small enough to ignore or to fix by snapping exact right angles
  instead of going through `sin_cos`.
- **The fine sweep works but costs.** 44 candidates at ~11 ms each. It is a
  fallback, not a path anything should take by default.
- **Different sheets don't false-match.** Page 1 against page 2 matched only
  5.43% -- the shared border and title block -- and found a consistent offset
  for exactly that shared geometry (residual 0.00095 pt). The clustering then
  reports the whole drawing area as one difference, which is correct.

## What this doesn't do yet

- **Nothing renders.** There are no clouds written, no overlay, no UI. The
  output is a list of boxes on stdout.
- **Images are excluded from the fingerprint.** An image's style colour is its
  place in the texture atlas, which has nothing to do with the drawing. 2,450
  image pieces a page here, so a mixed-content sheet still needs a raster pass
  confined to those boxes. That tier doesn't exist.
- **No raster baseline was measured.** The claim that this beats rendering both
  pages and diffing them is not benchmarked here, only argued.
- **Text is treated as geometry.** Diffing extracted runs as a sequence would
  say "REV 3 → REV 4" instead of clouding glyph outlines.
- **Moved geometry reads as added plus deleted.** The matched-pair transform
  exists to spot a whole-page move; a local move is still two differences.
- **One thread, one page.** No `rayon`, no use of the helper pool in `pool.rs`.

## Next, if it's worth continuing

1. Early-out when the identity candidate already has near-total agreement, so
   the usual case pays for one fingerprint rather than four. That alone should
   take the 54 ms to about 25 ms.
2. Overlay as a render-time composite -- two page textures, one tint each,
   multiply -- rather than a baked PDF. It is a fragment shader in `gpu-lines`
   and costs nothing at any zoom.
3. Write the difference boxes as clouds through `pdf-io`, matching what
   `markup-model` already stores.
