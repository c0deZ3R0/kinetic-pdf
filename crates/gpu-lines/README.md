# gpu-lines

A prototype: drawing a PDF page's annotations on the GPU instead of with
pdfium, kept apart from the app until it proves itself.

pdfium rasterises every path on the CPU at a few microseconds each, whatever
its size, so a page of Bluebeam stamps or CAD linework (about a million line
segments) takes the best part of a second to draw at any zoom, and again at
every new zoom. A GPU draws the same shapes as instances of one quad: the page
is uploaded once, and each pan or zoom only changes a transform.

## What it does

- `annotation_shapes` finds the annotations shown on a page -- leaving out
  highlights (the app draws those), popups, hidden ones, and ones on layers
  that are off -- and places each appearance in its rectangle the way PDF
  says, relative to the page's visible area as pdfium draws it.
- `Interpreter` reads each appearance's content stream with `pdf-content`,
  following the graphics state: saving and restoring it, transforms, line
  widths, colours in gray, RGB, CMYK, ICC-based and indexed spaces, spot inks
  in Separation and DeviceN spaces through their tint transforms (PDF
  functions of all four types), alpha and Multiply blending from graphics
  states, every path and painting operator,
  clips set by paths and by forms' boxes, forms inside forms, and marked
  content on layers, left out while the layer is off.
- Text is drawn from the fonts embedded in the PDF, simple or Type 0 with
  Identity-H encoding: TrueType and OpenType read with skrifa (Google Fonts'
  Fontations, which Chrome uses), and bare CFF and Type 1 with hayro-font. Each
  glyph's outline is tessellated once, in ems, and its triangles placed by
  the text state -- size, spacing, scaling, rise and the text matrix -- in
  every rendering mode, clipping ones included. Like the page's other
  shapes, glyphs stay sharp at any zoom without being drawn again; editors
  such as Zed draw glyphs into a texture at each size shown instead, which
  suits text at a few sizes rather than a continuous zoom.
- Images are decoded once however often they're drawn -- Flate, LZW, ASCII
  and run-length data, JPEG by zune-jpeg (YCCK ones turned into CMYK as
  pdfium reads them), samples of 1 to 16 bits through
  their colour space and decode ranges, soft masks as alpha, image masks in
  the fill colour -- and packed into 2048-pixel atlas pages, their edge
  pixels repeated around them. Each draws as one quad in painting order,
  sampling its place from the pages, which are one texture array.
- What's painted becomes `Shapes` in painting order: straight pieces of
  stroked lines, curves flattened to within 0.05 pt, cut into their dash
  patterns, with triangles for their caps and joins (miter, round or bevel,
  and the miter limit); and triangles covering filled areas, tessellated by
  lyon with the path's fill rule. A stroke with round caps and round joins,
  as Bluebeam draws markups, needs no triangles: its lines are drawn as
  capsules, reaching half their width past each end, which the shader rounds.
- `Renderer` uploads them to one OpenGL buffer and draws a run of the same
  blend at a time: every shape the same six vertices, a line's making a quad
  widened in the vertex shader and anti-aliased in the fragment shader
  (hairlines, and lines thinner than a pixel, a full pixel wide at full
  strength at any zoom, as pdfium draws them), a triangle's using three for its
  corners. Clips are stored once however often they're used. A clip whose
  shapes are all convex -- boxes, mostly -- becomes a handful of edges, kept
  in a float texture, that the fragment shader fades each shape's coverage
  across, so clipped shapes still draw together. Any other clip is drawn
  into the stencil buffer first, and its run shows only where all its shapes
  overlap: on the Bluebeam overlay, drawing all 39 of its clips that way,
  each change of clip clearing the stencil, took GPU draws from 1.7 ms to
  52 ms. A run whose shapes, or clip, fall outside the view isn't drawn at
  all, and a clip is cleared and drawn only where it could show: a civil
  drawing whose hatches are each clipped to an outline of their own came to
  58,000 runs, and drawing it went from a median 128 ms a frame to 5 ms. It needs OpenGL 3.3 or OpenGL ES 3.0 (WebGL 2) with a stencil
  buffer, and runs inside an egui paint callback with eframe's glow backend.
  The viewer asks for a stencil buffer, and turns on 4x multisampling for the
  triangles' edges.

Tiling patterns fill areas when their cells set their own colours and pattern
space isn't turned: the cell is read into shapes once, cut to its box, and
copied into every tile the area reaches, within the area, as Bluebeam fills
its area markups.

Not drawn yet, and counted in `Shapes::not_drawn`: shadings, shading
patterns, uncoloured or turned tiling patterns, strokes in patterns, soft
masks in graphics states, transparency groups, blend modes other than
Multiply, rotated pages; text in fonts that aren't embedded, Type 3 fonts,
or other CMaps; and inline images, images in JPEG 2000, JBIG2
or fax encodings, or with colour key or stencil masks. Small text is anti-aliased only by multisampling, so
it's rougher than pdfium's at small sizes. Where a see-through stroke's pieces
overlap, at its joins, it's drawn darker than it should be.

## Trying it

```
cargo run --release -p gpu-lines --example viewer -- file.pdf [page]
```

The viewer reads the page's annotations into shapes, and has pdfium draw the
page from the copy the app draws from (annotations on hidden layers left out,
stamps' lines merged). It shows:

- **GPU over page**: the shapes on the GPU, over pdfium's drawing of the page
  without its annotations.
- **GPU only**, on white.
- **pdfium whole page**: pdfium's drawing of the whole page, 4096 px wide,
  stretched.
- **pdfium sharp**: pdfium drawing exactly the part of the page in view, once
  the view holds still, as the app does when zoomed in.
- **Compare**: the GPU shapes over a faded pdfium drawing, to check they line
  up.

Scroll to zoom around the pointer, drag to pan. **Sweep zoom** zooms in and
out continuously and reports frame times and how long the GPU took to draw
(waiting for it to finish). Run with `GPU_LINES_VSYNC=0` so frames aren't held
to the screen's refresh rate, and `GPU_LINES_BENCH=1` to benchmark and take
screenshots without a hand on the mouse.
