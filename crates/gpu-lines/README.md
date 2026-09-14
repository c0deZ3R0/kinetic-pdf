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
  widths, colours in gray, RGB, CMYK, ICC-based and indexed spaces, alpha and
  Multiply blending from graphics states, every path and painting operator,
  forms inside forms, and marked content on layers, left out while the layer
  is off.
- What's painted becomes `Shapes` in painting order: straight pieces of
  stroked lines, curves flattened to within 0.05 pt, and triangles covering
  filled areas, tessellated by lyon with the path's fill rule.
- `Renderer` uploads them to one OpenGL buffer and draws a run of the same
  blend at a time: every shape the same six vertices, a line's making a quad
  widened in the vertex shader and anti-aliased in the fragment shader
  (hairlines one pixel wide at any zoom), a triangle's using three for its
  corners. It needs OpenGL 3.3 or OpenGL ES 3.0 (WebGL 2), and runs inside an
  egui paint callback with eframe's glow backend. The viewer turns on 4x
  multisampling for the triangles' edges.

Not drawn yet, and counted in `Shapes::not_drawn`: text, images, shadings,
patterns, clips, dash patterns, soft masks, transparency groups, blend modes
other than Multiply, line caps and joins, and rotated pages.

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
