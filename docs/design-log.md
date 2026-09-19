# Design log

Significant decisions on measurement markups and scales, each with its
source: a section of ISO 32000-2 (the PDF 2.0 specification) or our own
reasoning.

Newest last. Each entry: date, decision, why, source.

---

## 2026-09-17 — The model is its own crate: `crates/markup-model`

Markups, geometry, scales, units and quantity maths live in a crate that
depends on no PDF library, pdfium or egui, so it can be tested alone and every
other layer reads it. Named `markup-model` to match the workspace's other
crates (`gpu-lines`, `pdf-content`); its Rust name is `markup_model`.

Source: own reasoning; build instructions section 2.

## 2026-09-17 — The `Measure` trait holds only what the model can do

Each kind of markup is measured and picked through `Measure`
(`quantities`, `hit_test`). The instructions also list `tessellate`, `to_pdf`
and `from_pdf` on that trait; those need lyon and a PDF library, so they will
be traits of their own in the render and PDF crates, implemented per kind
there. The model stays free of them, as section 2 requires.

Source: own reasoning.

## 2026-09-17 — Quantities take an optional scale

`quantities(markup, Option<&Scale>)`. With no scale, kinds that need one
return `uncalibrated: true` and no scaled numbers, and totals leave them out.
Counts and angles don't need a scale and always measure. Shape errors (a
crossed outline, a stray cutout) are reported with or without a scale, so the
user sees the problem before calibrating.

Source: own reasoning; instructions section 4, "Safety checks".

## 2026-09-17 — What each quantity means

- **Area**: shoelace formula in points², times metres per point in x and in y,
  less the cutouts. The outline must be a simple polygon, and each cutout
  simple, inside the outline, and neither crossing nor inside another cutout,
  or there is no number at all. Repeated consecutive points (a double click)
  are dropped first. The check compares every pair of edges after a box test:
  fine for hand-drawn outlines of hundreds of points; revisit with a sweep if
  imported outlines of many thousands of points show up in benchmarks.
- **Perimeter**: the outline only. Cutouts are voids, not edges to price.
- **Slope**: multiplies plan lengths and areas by 1/cos θ, stored as rise over
  run. Not applied to perimeters, since an outline's edges run in every
  direction across the slope.
- **Volume**: plan area times depth, with depth measured vertically. That is
  the volume whatever the slope, so slope doesn't change it.
- **Angle**: measured on the real shape (points scaled per axis), so a section
  with an exaggerated vertical scale gives the real angle, not the drawn one.
  With no scale, measured as drawn.
- **Radius / diameter**: from centre and edge, three points on an arc (the
  circle through them, on the real shape), or a circle's box. A box that isn't
  square to within 1% once scaled isn't a circle and has no number.

Source: own construction knowledge and reasoning.

## 2026-09-17 — Scale resolution and the whole-page viewport

Order: an override on the markup; the last viewport in the page's list whose
box holds the markup's first point; the page's whole-page viewport; none. The
"last wins" rule follows ISO 32000-2 §12.9 (viewports, /VP): where viewports
overlap, the viewer uses the last in the array.

The instructions call for a "page default viewport". To identify it,
`Viewport` has a `whole_page` flag: the default is used for points outside
every other box, including points drawn off the page's edge. On import, a /VP
entry whose box covers the page's crop box counts as whole-page.

Copying a page's scale to other pages points their whole-page viewports at the
same `ScaleId`, not a copy, so one recalibration covers them all, and the file
gets one /Measure object.

Source: ISO 32000-2 §12.9; own reasoning.

## 2026-09-17 — Identity: random 128-bit IDs with a kind prefix in /NM

Markups, scales and viewports have random UUID v4 IDs, written to /NM as
`KPDF-<uuid>`, `KPDF-SC-<uuid>` and `KPDF-VP-<uuid>`. The prefix means
one kind can't be read as another, and a foreign /NM is never taken for ours:
only the exact form written parses. An annotation from another program gets a
fresh ID and keeps its own /NM in `Extras::foreign_nm`, which is what gets
written back.

Source: ISO 32000-2 §12.5.2 (/NM, a text string naming the annotation);
own reasoning.

## 2026-09-17 — /GeomHash hashes 32-bit reals, because that's what lopdf writes

lopdf 0.45, which the app already uses to write annotations, holds PDF real
numbers as `f32` and writes each as the shortest text that reads back to the
same `f32`. So coordinates and /Measure factors survive a save only to
32-bit precision: at worst about 0.001 pt at the 14,400 pt page-size limit,
and a relative error around 6·10⁻⁸ on a scale factor (about 0.001 m² on
10,000 m²). That's far below anything a tender cares about.

It does matter for /GeomHash. Hashing the 64-bit values would make a markup
read back from our own save look edited elsewhere. Rounding to a fixed number
of decimals would still flip at rounding boundaries. Instead the hash
(XXH3-64, a fixed published algorithm) is taken over each number converted to
`f32`, with both zeros as one. Saved and read back, the hash is identical; any
visible move changes it. The byte layout carries a version, and a test pins a
known hash, so it can't change by accident.

Follow-up for the PDF crate: snap committed geometry to `f32` when a markup is
committed, so the numbers shown before a save are exactly those after reopening.

Source: lopdf 0.45 source (`Object::Real(f32)`, writer uses `{value}`);
own reasoning.

## 2026-09-17 — Units, precision and parsing

Quantities are held in metres, m² and m³. Display units and precision apply
only when formatting. Fractions (`6 1/2"`) are available for lengths; areas
and volumes fall back to two decimal places under a fraction precision.
Feet-and-inches rounds the inches before splitting, so 11.999" carries into
the next foot. Thousands are grouped with commas; localised separators are
left for later.

Typed input accepts `25 m`, `2,500 mm`, `12' 6 1/2"`, `12'-6"` and so on, plus
scales as `1:100` or as `paper = real` (`1/4" = 1'-0"`, `10.58 cm = 100 m`).

Source: own reasoning.

## 2026-09-17 — Sheet sizes include US sheets

A printed ratio is only trustworthy on a PDF at true paper size. The warning
checks against ISO A0–A4 and also ANSI A–E and ARCH A–E1, to within 2 mm each
way, so imperial drawing sets don't warn on every page. Imperial units at
launch is still an open question; recognising the sheets costs nothing either way.

Source: ISO 216; ANSI/ASME Y14.1; own reasoning.

## 2026-09-17 — Separate x and y scales map through the page's rotation

A two-axis calibration is taken along the axes the user sees. On a page with
/Rotate 90 or 270, what looks horizontal runs along user-space y, so the two
factors are swapped before storing. /Measure's /X and /Y are in user space.

Source: ISO 32000-2 §12.9 (/X and /Y number formats); own reasoning.

## 2026-09-17 — `PageTransform` lives in the model

Converting user space ↔ view ↔ screen, with /Rotate and a crop box off the
origin, is pure maths that tools, snapping and rendering all need, so it sits
in the model in `f64`. The app's existing `PageGeometry` (`src/model.rs`,
`f32`, fractions of the page) does the same job for highlights. Merge the two
when the app moves onto the model (milestone 4).

Source: own reasoning.

---

## 2026-09-17 — Where the build instructions follow the app instead

Accepted on 2026-09-17. The instructions were written against a generic
architecture. The app had already settled, with benchmarks, on these
different choices.

1. **Writing PDF structures: lopdf rather than a patched pdfium.**
   Milestone 1 plans a patched pdfium with a generic dictionary API, built
   from source on a self-hosted runner. The app already writes annotations,
   including ones pdfium can't create, as incremental updates with lopdf
   (`src/markup.rs`), with appearance streams. lopdf can write any dictionary,
   indirect object or array, so /Measure, /VP, /IT and /KPDF need no C
   changes. Nobody publishes a static pdfium for Windows (see
   `docs/DEVELOPMENT.md`), and building one needs a full Chromium toolchain.
   So the milestone 1 spike proves lopdf instead (a /PolygonDimension
   with a shared indirect /Measure and a page /VP, appended incrementally, read
   back by pdfium). Cost: the 32-bit reals above.
2. **Rendering: stay on glow (OpenGL), not wgpu.** eframe's glow backend was
   chosen for a much smaller binary, and `crates/gpu-lines` already draws
   annotations on the GPU inside egui paint callbacks with glow and lyon.
3. **Threading: keep render helper processes.** pdfium-render serialises
   every pdfium call behind one lock, so one pdfium per worker thread in one
   process doesn't draw in parallel. The app draws with up to three helper
   processes, each with its own pdfium (`src/pool.rs`, `src/helper.rs`).
4. **Tile cache: already built.** 512 px squares at quarter-power-of-two zoom
   steps, in memory and in a compressed disk cache keyed by file fingerprint
   (`src/cache.rs`), matching section 6 closely. Extend it rather than rebuild.

## 2026-09-17 — Milestone 1: measurement structures written with lopdf

`crates/pdf-io` writes and reads them; `tests/measure_pdf.rs` opens the result
in pdfium, which reports each annotation's subtype and /NM and draws it from
its appearance stream. `cargo run --example measure_sample` writes
`tmp/measure-sample.pdf`, a sample sheet to look at by hand.

- **Annotations**: Length as /Line with /IT /LineDimension and /L;
  Polylength as /PolyLine /PolyLineDimension; Area, Perimeter and Volume as
  /Polygon /PolygonDimension with /Vertices. Cutouts have no place in the
  standard, so they're in /KPDF /Holes, and the appearance fills even-odd so
  other viewers show the voids. Source: ISO 32000-2 §12.5.6.7 (line), §12.5.6.9
  (polygon and polyline), §12.9.
- **/Measure** is written once per scale as an indirect object. Number formats
  are in the scale's display units, so a viewer that only knows the standard
  shows our units: /X's /C is display units per point, and /D, /A and /T
  convert from there. Feet and inches are two formats, feet truncated (/F /T)
  then 12 inches to the foot. A non-uniform scale adds /Y and /CYX 1, both axes
  being in the same unit. Read back, metres per point is /C times the unit's
  metres. Source: ISO 32000-2 §12.9, tables on rectilinear measure and number
  format dictionaries.
- **No /V in /Measure.** The build instructions list /V, but the standard's
  rectilinear measure dictionary has no volume format; the volume unit goes in
  the /Measure's own /KPDF, with the scale's ID.
- **/VP**: each entry has /Type /Viewport, /BBox, /Name, /Measure and our /NM;
  the whole-page viewport is marked with /KPDF /WholePage.
- **Which scale on reading**: a markup's /Measure is compared with what its
  page's viewports would give it. If they agree, it follows the page. If not,
  or /KPDF /Override says it was chosen, it keeps its own scale. That way a
  plain ISO annotation from another program, which has only its own /Measure, still
  measures.
- **Unknown keys**: annotation keys and /KPDF keys this doesn't read are kept in
  `Extras::raw` / `raw_kpdf` and written back unless we now write that key.
- **/GeomHash on load**: compared against the geometry read and the scale
  resolved; a mismatch sets `changed_externally`. A test moves a line's /L in
  the file and checks only that markup is flagged.
- **Appearance**: path stroked (and filled even-odd) under an ExtGState for
  opacity, then the quantity label in Helvetica with WinAnsiEncoding (so ², ³
  and ° print), at full strength, just above lines and centred in areas. /Rect
  and the form's /BBox cover the path, the stroke and the label. Follow-up:
  labels on steep segments should sit beside the segment, not above it.
- **Not yet**: changing or deleting annotations already in the file, the
  temp-file-and-rename save, and the other kinds (count, angle, radius,
  diameter). Those are milestones 8 and 10.

Source: ISO 32000-2 as cited; own reasoning; pdfium rendering our own files.

## 2026-09-17 — Milestone 4: a session of commands, with undo

`src/session.rs` owns the open document's highlights and markups. The window
hands it commands (`AddHighlights`, `AddMarkup`, `EditNote`, `Remove`); each
is applied and kept as a step that undo and redo replay backwards and
forwards, up to 1,000 steps.

- **What to save is derived, not recorded.** The session remembers which
  uids are in the file, at which key, with which note. New annotations are
  those without a key, deletions are removed uids still in the file, and
  edits are notes that differ from the file's. Undoing back to the file as
  saved therefore leaves nothing to save, with no bookkeeping to unwind.
- **Uids survive a save.** A save deletes (highest index first) and appends
  new highlights, then new markups, so after it each page's annotations are
  those kept, in their old order, followed by those added. The session
  records that expected order when the save begins and matches the pages read
  back to it, giving each its new key under its old uid. If the counts don't
  match, the pages are shown as read and history is cleared, never guessed.
  This also fixes changes made while a save ran being lost on the pages it
  touched.
- **Undo after a save** works on the file as it now is: undoing an addition
  that was saved deletes it; undoing a deletion that was saved adds the
  highlight back as new. A markup read from the file has no shape of its own
  to write back, so undoing its saved deletion shows it but can't save it.
- **Saved colours stay.** As before, a note edit on an annotation in the file
  keeps its colour, since another program may have given it an appearance in
  the old one; the session enforces that rather than the popup.
- **Not a separate crate.** The build instructions split `app`, `render` and
  a binary crate. The session sits in the app's library crate instead: it
  depends only on `model.rs`, has no UI code, and is tested on its own, which
  is what the split is for, without moving the window and worker code. The
  measurement tools (milestones 6 and 7) add their markups as further command
  kinds.
- **Performance.** Nothing runs per frame. Loading appends and sorts once per
  batch, as before. Finishing a save updates keys in one pass rather than a
  search per annotation. On 40,000 annotations over 500 pages (release
  build): a command 0.03 ms, an undo 0.07 ms, starting a save 2.2 ms, matching
  its read-back 2.7 ms.

Source: own reasoning; the worker's save order in `src/annots.rs`, checked by
`tests/session.rs` against real saves.

## 2026-09-18 — Milestone 6: the scale tool

A page's scale is set from the **Scale** button in the tool row: measure a
known dimension and type its real length, or pick a printed ratio. The panel
shows what the page measures at, warns when the page isn't a standard sheet
(so a printed ratio may not hold), offers a check against a second known
dimension, and can give every page the same scale.

- **Scales live in the session**, beside highlights and markups, so setting
  one is an undoable step, counts as unsaved work, and is written by the same
  save. `Command::SetScales` carries the whole set: the caller changes a copy
  and hands it back, so calibrating, picking a ratio, copying to other pages
  and changing units all undo the same way. They are small -- a few scales and
  viewports -- so a step holds a copy rather than a description of the change.
- **What a save writes** is the difference from the scales as read: only the
  pages whose viewports changed get a new /VP, and a scale whose numbers
  changed rewrites every page that uses it, since they share one /Measure.
- **Read on demand.** Scales and measurements need a pass over the whole file
  with lopdf, which pdfium can't do: on a 221 MB drawing set that's 0.5 s and
  about 390 MB while it parses (measured). Doing that on every open would cost
  every reader who never measures anything, so `Request::ReadMeasurements`
  runs on a thread of its own the first time the scale panel opens, and
  opening a file is untouched. Until it arrives the panel says it's reading,
  and nothing may change a scale, or undo could step back to "no scales" and a
  save would strip the file's.
- **Recalibrating is in place.** Measuring a known dimension again changes the
  scale the page already has rather than making another, so every page sharing
  it follows, and the file keeps one /Measure. The panel says how many pages
  share it, and the message after calibrating says how many followed.
- **Accuracy warnings**: calibrating with the two ends less than 20 pixels
  apart on screen says so and suggests zooming in; Shift keeps the line square
  while dragging; the line shows what it measures at the scale set so far.

Source: own reasoning; ISO 32000-2 §12.9 for what's written.

## 2026-09-18 — Snapping, from the lines the GPU already reads

Measurements are only as good as where their points land, so the tools snap
to the drawing: corners and ends of lines, where two lines cross, middles,
and the nearest point along a line. Corners of markups already made win over
anything in the drawing.

- **The lines come free.** The GPU reader already turns each page into flat
  line segments in the page's own space, with curves flattened and transforms
  applied. Snapping indexes those rather than parsing the page again. The
  index is built on the reader's thread, where the page is already being read,
  so the UI thread never sees the cost.
- **A grid, not a tree.** Drawing linework is dense and evenly spread, which
  is where a uniform grid beats an R-tree: about four segments to a cell, and
  a query reads the few cells within reach. Measured on 400,000 segments:
  29 ms to build, 8 MB held, 3.4 µs a query (`snap::timing`). Long lines are
  walked cell by cell rather than filling the box around them, so one diagonal
  across a sheet doesn't land in every cell.
- **Crossings are worked out at the pointer**, among the few dozen lines
  within reach, rather than precomputing millions that nobody will use.
- **Strokes only.** A fill reaches the GPU as tessellated triangles whose
  inner edges are artefacts of the tessellation; snapping to those would catch
  nothing anyone drew. Pages pdfium draws have no lines to offer, so those
  snap to markup corners only.
- **Kept for the pages near the view**, within 64 MB, the furthest let go
  first. A page's lines are indexed only while a measurement tool is in use:
  the page is read again once for them, which costs one read (20-260 ms on
  the reader thread) and nothing thereafter.
- **In the hand**: the snapped point gets a mark saying what it caught -- a
  square on an end or a markup's corner, a cross on a crossing, a triangle on
  a middle, a circle for a point along a line -- because snapping is only
  worth having if you can see what it did. Alt places a point freely, Shift
  holds the line square or to 45°, and Shift wins, since a snap off the line
  would undo it.
- **Calibration snaps too**, which is where it matters most: a pixel of error
  in a calibration spreads into every quantity on the page.

Source: own reasoning; the shapes `crates/gpu-lines` already produces.

## 2026-09-18 — Milestone 7: the length, polylength and area tools

Three tools in the tool row. Each point is placed by clicking, snapping to the
drawing as it goes, and the quantity shows while the shape is still being
drawn, so the number is there before the last click. Length takes two clicks;
polylength and area take as many as you like, finished by double-clicking,
pressing Enter, or (for an area) clicking the first point again. Backspace
takes back a point, Esc drops what's half-drawn and then puts the tool down.
Clicking a measurement picks it out; dragging a vertex moves it, Delete
removes it.

- **Measurements live in the session** as `markup_model` markups, so they
  undo, count as unsaved work and are written by the same save. A vertex
  dragged across the page is one step to undo, not one per frame: the session
  merges consecutive changes to the same measurement while a drag lasts, but
  never merges a change into the "add" step, or two separate drags together.
- **What a save writes** is worked out by comparing what's shown with what the
  file holds: new and changed measurements are written, and those gone, or
  about to be written again, are taken out. Since a measurement's /NM is its
  own ID, changing one is a remove and a write under the same name; no index
  bookkeeping is needed, unlike highlights.
- **Drawn once.** A saved measurement carries an appearance stream for other
  viewers, so pdfium and the GPU renderer would draw it as well as the app,
  showing every quantity twice. Both now leave annotations named `KPDF-` to
  the app, as pdfium already left highlights to it.
- **Quantities are the model's**, worked out from geometry and the page's
  scale, so recalibrating a page updates every number on it at once, and
  undoing the recalibration puts them back.

Source: own reasoning; instructions sections 3 and 5.

## 2026-09-18 — Two fixes from the first real takeoff

Drawing areas on a real drawing showed two faults.

- **An area was filled outside itself.** A drawing library's polygon fill
  makes a fan of triangles from the first corner, which is only right while
  the shape stays convex: a shape with a notch was filled across the notch,
  and thin spikes shot out of it. Areas are now cut into triangles by ear
  clipping (`geom::triangulate`), which is right for any simple outline. The
  triangles are worked out once when the measurement changes, beside its
  quantities, rather than every frame. An outline that crosses itself has no
  triangles: it shows as an outline alone, which is honest, and its quantity
  already says what's wrong.
- **A measurement stretched across the page.** A press near an existing
  measurement's corner took hold of that corner, so the next click dragged it
  away. A measurement tool now only places points; picking one out and moving
  its corners is for the Select tool. Placing also happens where the button
  goes down rather than where it comes up, since a click that slips a pixel
  counts as a drag and used to place nothing.

Snapping was the other suspect, so it now has property tests: whatever a snap
catches it never moves a point further than its reach, and a crossing lies on
both lines. Neither found a fault, which is what ruled snapping out.

Also: an area's label sits inside it (the middle of its largest triangle)
rather than at the average of its corners, which for a shape with a notch can
fall outside it; a run's label sits half way along by length.

Source: two bugs found by the project owner, drawing a real takeoff.

## 2026-09-18 — Spikes at sharp corners, and a length that drew nothing

Two more from drawing a real takeoff.

- **A corner that doubles back grew a spike.** Measured with the tessellator:
  an outline 200 points across, with one corner turning back on itself,
  reaches 800 points across when drawn as a **closed** path -- the mitre
  reaches out by one over the cosine of half the angle, and a closed path puts
  no limit on it. An **open** path keeps to itself. So a closed shape is drawn
  a piece at a time with a round patch at each joint, while open runs stay one
  path, which is quicker. Tests pin both: what we draw stays within the shape,
  and a closed path on its own still spikes -- if that ever stops being true,
  the joint drawing can go.
- **A length drew nothing.** A line's two ends are held as one ring each,
  which is how a vertex is addressed for dragging, and the drawing took only
  the first ring: one point, nothing to see, though the quantity showed. The
  drawing now asks for the points in the order they join up.

Both were only visible on screen, so the tests measure the triangles a shape
really comes to, not its bounding box -- a shape's box is worked out from its
points, which is exactly what hides a spike.

Source: two bugs found by the project owner, drawing a real takeoff.

## 2026-09-18 — The faint spikes: smoothing a sliver

The last of the drawing faults, and the subtlest. A filled shape is smoothed
at its edges by spreading its corners outwards, by one over the sine of half
the angle at each corner -- so a sliver's sharp corner spreads without limit.
Measured: a sliver 200 points across, filled as a shape, reaches 660 points
across, in a long half-transparent spike. Cutting an area into triangles
gives slivers wherever the outline doubles back on itself, which is exactly
where they appeared.

Areas are now filled as a mesh, which is drawn as it is, with no smoothing:
the fill's edges are a shade harder, which nobody can see under the outline
drawn over them, and it is one shape a page rather than one a triangle.

Both this and the mitre spike before it were invisible to a test that asks a
shape for its bounds, since a shape works those out from its points. The
tests tessellate and measure the triangles, and each fault has a test beside
it pinning the behaviour that caused it, so the workaround can go if that
behaviour ever changes.

Source: a bug found by the project owner; measured with the tessellator.

## 2026-09-18 — Ctrl+Z takes back a point while a shape is being drawn

Half way round an area, Ctrl+Z means "not that point", not "undo the
measurement I finished a minute ago". So while a shape is being placed, undo
and redo work on its points -- as Backspace already did -- and only reach the
document's own changes once it is finished or dropped. The Undo button says
which it will do. Taking back every point leaves the tool in hand with
nothing drawn, rather than putting the tool down, since the next click should
start the shape again.

Source: asked for by the project owner, drawing a real takeoff.

## 2026-09-18 — Ctrl places a point where the pointer is

Snapping is on while a tool is in use, and Ctrl turns it off for as long as
it's held, for the times the drawing's own lines are in the way of what's
being measured. (Alt does the same, which is what it was before.) The mark
showing what a point would catch is larger and heavier, since it sits under
the pointer and has to be read at a glance.

Source: asked for by the project owner.

## 2026-09-18 — Editing what's been measured

A measurement can now be changed rather than redrawn. With no tool in hand,
pressing on one takes hold of what's under the pointer: a corner moves it,
the middle of an edge puts a new corner there and moves that, and anywhere
else moves the whole thing. Delete takes out the corner picked out, or the
whole measurement if it can't spare one -- an area keeps three corners, a run
two. The handles show what can be grabbed: filled squares at the corners, the
one picked out larger, and a hollow square in the middle of each edge.

Every change goes through the session as one step, and a whole drag is one
step to undo, as a vertex drag already was.

**Cutouts.** The Cutout tool draws a ring inside an area already measured,
and its size comes off that area: the smallest area holding the ring's first
point takes it, so a cutout inside a cutout's area goes to the right one.
The model and the file format already carried cutouts; what was missing was
drawing the fill with a hole in it.

- **Filling a shape with holes** needs the cutouts bridged into the outline
  first: the outline is joined to each cutout by a pair of coincident edges,
  making one ring that walks in around the hole and back out, which ear
  clipping then handles. The bridge runs from the cutout's rightmost point to
  the nearest corner of the outline it can see without crossing an edge, and
  cutouts are bridged rightmost first so a bridge never crosses one still to
  come. Tests check the triangles come to the area less its cutouts, and that
  none of them lies inside a cutout.
- The ear test had to stop counting a corner that lies *on* a triangle's edge
  as inside it, since a bridged ring runs along itself: every ear was
  rejected and nothing was filled at all.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — Count, angle, radius and diameter

The rest of the measurements an estimator takes off a drawing:

- **Count** is one markup that grows a mark at a time: each click adds a
  mark to the count in hand, so a count of ninety doors is one line in the
  quantities rather than ninety. Each mark is its own step, so Ctrl+Z takes
  back the last one; Esc lets go of the count so the next click starts
  another. The marks are drawn as crosses, not joined up, and have no edges
  to add a corner to.
- **Angle** is three clicks -- along one arm, the corner, then along the
  other -- and finishes on the third. An arc across the corner shows which
  of the two angles is the one measured. The angle is worked out on the real
  shape, so a section drawn with an exaggerated vertical scale gives the real
  slope rather than the drawn one.
- **Radius** is the middle then the edge; **diameter** is two clicks straight
  across. Both are held as the line drawn, which is the line measured, and
  both show the circle they come off: around the first point for a radius,
  around the middle of the line for a diameter. The circle grows as the line
  is drawn, so its size reads before the second click, and the appearance
  written to the file shows it too.
- A diameter was first held as the circle itself, a box with no direction in
  it, which meant it couldn't be drawn back across the way it was measured
  and had no corners to drag. Holding the line instead keeps the direction,
  gives both ends as handles, and leaves the circle as something drawn rather
  than stored. A circle still reads back from a file, since one made
  elsewhere arrives as a /Circle: its rim is dragged to resize it.

**What is written.** ISO 32000 gives a dimension intent to lengths, runs and
areas only, so an angle, a radius, a diameter or a count is written as the
annotation whose shape it has -- a polyline, a line, a circle, a polygon --
with no /IT, and what it measures in our own /KPDF and in /Contents. A count
also writes its marks to /Vertices, since a /Polygon must have them.

- A circle's box goes in /KPDF /Box rather than being read back off /Rect:
  /Rect is grown to hold the line's width and the label, so a save-and-read
  round trip off it grew the circle by about 1% each time. Caught by the test
  that reads all four kinds back and checks they still measure the same.
- An appearance draws a circle as the four Bezier curves a circle is drawn
  with, rather than the box it fills, and a count as a cross at each mark.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — A calibration line is two clicks

Calibrating and checking now work the way the measuring tools do: press once
to put an end down, move, press again to draw the line. The line follows the
pointer in between, snapping and squaring as it always did.

Dragging still works, since it costs nothing to keep: the line starts where
the button goes down, so letting go somewhere else finishes it, and letting
go without having moved leaves the first end down and waits for the second
click. Esc drops a line with one end down, as it drops any half-drawn shape.

The gesture had to move from `drag_started` to the press itself: a click that
never moves raises no drag at all, which is why a click did nothing before.

Source: asked for by the project owner.

## 2026-09-18 — The quantities panel

Measurements were only readable where they were drawn. **Quantities** in the
tool row lists every one in the file: kind, name, page and what it measures,
with the totals underneath -- length, area, perimeter, volume and count, each
shown only when there is one. Clicking a row scrolls to that measurement and
picks it out, so the list and the page are the same selection; × deletes,
through the session, so it undoes like any other change. **This page** narrows
the list to the page in view, and the totals follow it.

- **Nothing is measured here.** Each row reads the quantities the session
  worked out when the measurement last changed, and the totals come from
  `MarkupStore::totals`, so opening the panel costs a walk over the
  measurements and nothing more. A recalibration shows through it at once,
  since it remeasures the store.
- **Totals are honest.** Anything uncalibrated or with an error is counted as
  left out and said so at the foot, rather than being quietly summed as zero.
  That's `Totals` in the model, which the panel only displays.
- The store is keyed by ID, so rows are sorted for reading: by page, then by
  when each was taken.
- **CSV** writes the same rows, with the numbers in metres and square metres
  whatever the page is shown in, plus the text as the panel shows it, so a
  spreadsheet gets something it can add up and a person gets something they
  recognise. Cells with a comma or a quote are quoted.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — The quantities table, and naming what's measured

The list became a table across the bottom of the window, which is where a
take-off is read: a row per measurement with its **description**, kind, page
and what it measures, then a column each for length, area, perimeter, volume
and count. A row only fills the columns its kind has numbers for, so a count
sits under Count and an area under Area and Perimeter, and the columns add up
down the page.

- **Description** is typed straight into the row. It's the name the quantity
  is priced under, and it was already in the model and the file (/KPDF
  /Label), so naming one costs nothing new. Typing merges into one undo step
  until the box is left, the way a drag does.
- **Group by** gathers the rows under a heading with a subtotal each:
  description (everything called the same thing, whichever page it's on),
  page, or kind. Grouping by description is what turns a page of lines into
  "external walls 142 m".
- Clicking any cell but the description goes to that measurement and picks it
  out, so the table and the page share one selection.
- The table is a bottom panel rather than a side one: rows are wide and there
  are many columns, and the pages keep the width they had.
- CSV carries the same shape -- group, description, columns as shown, then the
  raw numbers in metres and square metres whatever the page is displayed in,
  and a note for anything left out. Subtotal lines are written too, so the
  file reads like the table.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — Sorting the table, and depth for volumes

**Sorting.** Clicking a column heading sorts by it, clicking again turns it
round, and the arrow says which way. Within the sort the rows keep their
settled order -- page, then when each was taken -- so equal cells don't
shuffle about between frames. Empty cells sort to the bottom whichever way
the column runs: a measurement with nothing to say in a column is neither
the largest nor the smallest. The Measured column sorts by whatever each row's
own kind measures, so a mixed column still reads biggest to smallest.

**Depth.** An area priced by volume is an area with a depth against it, so the
depth is typed into the row rather than being a tool of its own: give an area
a depth and it becomes a volume, clear the depth and it's an area again. The
model measured this all along (plan area times a vertical depth, whatever the
slope), the file format already carried /KPDF /Depth, and the page label
follows: the shape now reads 56.4 m³.

- The depth box holds what's been typed while it has the keyboard, rather
  than reformatting from the model at each keystroke, which would rewrite the
  number under the pointer. Anything that doesn't parse is left alone, so a
  half-typed number doesn't wipe what's there.
- It's read in the units the page is shown in, so `300` at a metric scale is
  300 mm and `1'` reads as a foot.
- Depths aren't totalled: two areas a foot deep aren't two feet deep. The
  volume column is.
- A volume still takes cutouts, still fills, and still writes as a polygon
  dimension.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — The quantities table is a real table

The first table was laid out by hand with `egui::Grid`, and read like it:
columns as wide as whatever was in them, a heading row that scrolled away,
numbers ranged left against their labels, and every row drawn whether it was
in view or not.

It is now `egui_extras::TableBuilder`, which is the widget for this:

- Columns can be dragged to any width and keep it; long descriptions clip
  rather than shoving the numbers off the end.
- The heading row stays put while the rows scroll under it, and the headings
  are the sort buttons.
- Only the lines in view are drawn. The table is one flat run of lines -- a
  heading, its measurements, its subtotal, then the total -- so grouping
  doesn't cost that.
- Numbers are ranged right, so the digits line up down the page.
- A row picks out its measurement wherever it's clicked, not only on a cell,
  and the row the page has picked shows as selected in the app's own colours.

`egui_extras` is taken with no default features: the image and file loaders it
can bring have nothing to do with a table.

Source: own reasoning; asked for by the project owner.

## 2026-09-18 — Cells are text, and the table resizes like one

Three things the table got wrong, all measured against how a table is meant
to behave rather than how it was easiest to build:

- **Cells were widgets.** A description and a depth sat in boxes, and the
  delete was a button, so every row read as a row of bubbles. They are text
  now: double-click a cell to open it for typing, and it goes back to text
  when it's left. The box that appears has no frame, so only the caret says
  it's open. Escape abandons what was typed, and an edit is one step to undo
  rather than one per letter, since it applies when the cell is left.
- **Shrinking a column was slow.** `TableRow::col` grows a column's remembered
  width to whatever its widest cell used, so a cell that didn't clip fought
  the drag: the column crept back a pixel a frame. Every column clips now, and
  every cell's text is truncated rather than laid out at its full length, so a
  column goes back as fast as the pointer.
- **The table stopped short.** The scroll area shrank to its rows, so dragging
  the panel taller only added empty space under them. It fills the panel now,
  and the panel takes any height up to the whole window under the tool row.

A cell takes its own clicks, so the cells gather them for the row rather than
the row waiting for clicks nothing else took: clicking anywhere on a row still
goes to that measurement on the page.

Source: asked for by the project owner.

## 2026-09-19 — A cell's clicks belong to the cell, not to the words in it

Opening a cell for typing meant hitting the text itself. A description of one
short word left most of a 210-pixel column dead, and a double-click in the
empty part of it did nothing, so naming a quantity was a matter of aim.

The cause was the previous entry's "a cell takes its own clicks": the clicks
were taken by the `Label` drawn inside the cell, not by the cell. `Ui::new`
registers a cell's own rect before anything is drawn into it, and egui's hit
test gives a click to the nearest widget under the pointer, which is always
the widget within the cell rather than the cell around it. The label won every
click that landed on the words and nothing else won the rest.

So the text is drawn with no sense of its own (and `selectable(false)`, so
egui doesn't make it interactive for text selection either), and the response
`TableRow::col` gives back -- which covers the whole column width -- carries
the double-click that opens a cell, the tooltip that says so, and the click
that picks the row out. The row's response is a union of its cells', so it now
catches the gaps between words too and `hit` gathering clicks cell by cell is
gone.

Two things that follow:

- **The delete cross keeps a target of its own**, the only one here: deleting
  a measurement by a stray click in a 24-pixel column costs more than having
  to aim at the cross. A click beside it picks the row out as any other cell
  does.
- **A cell being typed in is left alone.** A double-click there picks a word
  out of what was typed, so the cell isn't reopened on top of it -- that would
  throw the typing away. Leaving one cell is also applied before the next
  opens, so a double-click straight from one cell to another lands in the
  second rather than being cancelled as the first commits.

Source: reported by the project owner; egui's hit test (`hit_test.rs`,
nearest-widget-wins) and `Ui::response`, checked against both by driving a
table headlessly with simulated clicks.
