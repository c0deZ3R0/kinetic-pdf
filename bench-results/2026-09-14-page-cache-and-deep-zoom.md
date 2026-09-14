# Page cache and sharp deep zoom

- Run: 2026-09-14
- Tools: `floors pages-worker FILE SCALE` and `floors deep-zoom FILE` (examples/floors.rs)
- View: 2000 x 1240 device pixels at 200% scaling, headless

## Step 2: load each page once

The worker keeps the six most recently used pages open. Before, reading a
page's highlights, extracting its text and rendering it each loaded the page
again.

`pages-worker` shows every page in turn through the real worker, with its text
and render, as scrolling does.

| Document | Scale | Before: median per new page | After | Before: all pages | After |
| --- | ---: | ---: | ---: | ---: | ---: |
| synthetic-300p-v1.pdf (300 text pages) | 3.21 | 22.5–25.8 ms | 20.8 ms | 6.9–7.7 s | 6.4 s |
| A large drawing set | 0.80 | 216–229 ms | 163–166 ms | 6.7–7.4 s | 5.5 s |

Two runs each.

## Step 3: sharp deep zoom

Past the whole-page image cap, the part of the page in view (plus a 384-pixel
margin) is rendered on its own at full sharpness. Before, deep zoom stayed soft.

`deep-zoom` opens the file in the real app, jumps to 8x zoom in one frame, and
times how long until the sharp render arrives. Then it scrolls for 240 frames at
60 fps.

| Document | Whole-page cap | Sharp after zoom | Scroll frame median | Worst | Sharp renders while scrolling |
| --- | ---: | ---: | ---: | ---: | ---: |
| Drawing set | 32 MP | 527–541 ms | 0.17 ms | 0.94 ms | 32–33 |
| Drawing set | 8 MP | 378–380 ms | 0.17 ms | 0.76 ms | 33 |
| synthetic | 32 MP | 145–148 ms | 0.19 ms | 1.30 ms | 34 |
| synthetic | 8 MP | 50–55 ms | 0.19 ms | 1.00 ms | 34 |

The app ships with the 8 MP cap. With a sharp render over the view, the
whole-page image only fills the edges, and a smaller one is ready much sooner.

Normal scrolling through the drawing set at fit width with the 8 MP cap: frame median
0.17 ms, worst 0.86 ms.
