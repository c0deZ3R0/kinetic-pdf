# gpu-lines

A prototype: drawing a PDF page's stroked lines on the GPU instead of with
pdfium, kept apart from the app until it proves itself.

pdfium rasterises every path on the CPU at a few microseconds each, whatever
its size, so a page of Bluebeam stamps or CAD linework (about a million line
segments) takes the best part of a second to draw at any zoom, and again at
every new zoom. A GPU draws the same lines as instances of one quad: the page
is uploaded once, and each pan or zoom only changes a transform.

## What it does

- `extract` reads paths out of pdfium, through nested forms, into shapes in
  page space, in the order the page paints them: straight pieces of stroked
  lines with their widths and colours, curves flattened to within 0.05 pt, and
  triangles covering filled areas, tessellated by lyon with the path's fill
  rule (a path both filled and stroked is filled first, as PDF paints it).
- `Renderer` uploads them to one OpenGL buffer and draws them with one
  instanced draw call, so the page's painting order holds: every shape is the
  same six vertices, a line's making a quad widened in the vertex shader and
  anti-aliased in the fragment shader (hairlines one pixel wide at any zoom),
  a triangle's using three for its corners. It needs OpenGL 3.3 or OpenGL ES
  3.0 (WebGL 2), and runs inside an egui paint callback with eframe's glow
  backend. The viewer turns on 4x multisampling for the triangles' edges.

Not drawn yet, so left to pdfium: text, images, shadings. Drawn without their
effect: clip paths, dash patterns, transparency and blend modes, line caps and
joins.

## Trying it

```
cargo run --release -p gpu-lines --example viewer -- file.pdf [page]
```

The viewer makes the same copy the app draws from (annotations on hidden
layers left out, stamps' lines merged), flattens the page's annotations so
pdfium places them, reads their lines, and shows:

- **GPU lines over page**: the lines on the GPU, over pdfium's drawing of the
  page without its annotations.
- **GPU lines only**, on white.
- **pdfium**: pdfium's drawing of the whole page, 4096 px wide, stretched.
- **Compare**: the GPU lines over a faded pdfium drawing, to check they line up.

Scroll to zoom around the pointer, drag to pan. **Sweep zoom** zooms in and
out continuously and reports frame times and how long the GPU took to draw
(waiting for it to finish). Run with `GPU_LINES_VSYNC=0` so frames aren't held
to the screen's refresh rate.
