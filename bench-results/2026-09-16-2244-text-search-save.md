# Kinetic PDF benchmark: text-search-save

- Run: 2026-09-16 22:44
- Build: release
- Logical CPUs: 16
- App exe: 15.4 MB
- Open runs per document: 3

## Startup

| Step | Time |
| --- | ---: |
| First launch: unpack pdfium.dll (hash and write) | 11.9 ms |
| Later launches: check the unpacked copy (hash and size) | 6.04 ms |
| Load pdfium.dll | 21.4 ms |
| Initialise pdfium | 1.84 ms |
| **Later launch, pdfium ready** | **29.3 ms** |

Window creation and the first egui frame come on top and aren't measured here.

Memory: 15 MB now, 15 MB peak so far.

## drawing-set-a.pdf, 85 pages

81.6 MB

### Opening, step by step as the app does it

Everything up to the first page appearing. The other pages' highlights are read afterwards, in the background, and don't hold up the first page.

| Step | First run | Best | Median |
| --- | ---: | ---: | ---: |
| Read the file into memory | 16.5 ms | 16.5 ms | 18.8 ms |
| Copy it for pdfium | 7.92 ms | 7.56 ms | 7.92 ms |
| Parse the document | 4.40 ms | 4.40 ms | 4.41 ms |
| Every page's size | 7.53 ms | 7.44 ms | 7.53 ms |
| Read page 1's highlights | 15.9 ms | 12.1 ms | 12.6 ms |
| Render page 1 (125% zoom, 200% display) | 449 ms | 438 ms | 449 ms |
| Make page 1's texture (worker thread) | 5.64 ms | 5.46 ms | 5.64 ms |
| **Time to first page** | **507 ms** | **496 ms** | **507 ms** |
| Then, in the background: every other page's highlights | 2470 ms | 2470 ms | 2485 ms |

85 pages, 0 highlights.

Memory: 21 MB now, 475 MB peak so far.

### Rendering

Measured over 12 pages spread through the document. Removing a page's highlights before its first render: 14.0 ms median.

| View | Pixels | Texture | Render best | Render median | Render p95 | Make texture (worker thread) | Memory for 9 kept pages |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 125% zoom, 100% display | 3365 × 2377 | 30.5 MB | 30.3 ms | 274 ms | 1725 ms | 5.56 ms | 275 MB |
| 125% zoom, 200% display | 3365 × 2377 | 30.5 MB | 29.1 ms | 282 ms | 1795 ms | 5.86 ms | 275 MB |
| 300% zoom, 200% display | 3365 × 2377 | 30.5 MB | 31.1 ms | 296 ms | 1803 ms | 5.66 ms | 275 MB |

Memory: 176 MB now, 876 MB peak so far.

### Text extraction

| Pages sampled | Characters per page | Best | Median | p95 |
| ---: | ---: | ---: | ---: | ---: |
| 30 | 4974 | 2.09 ms | 6.38 ms | 72.9 ms |

The app extracts text for each page it shows, in a separate request from the render.

### Search, the whole document as the app does it

| Query | Matches | First match after | Whole document | Pages per second |
| --- | ---: | ---: | ---: | ---: |
| Common word: "the" | 746 | 9.18 ms | 2789 ms | 30 |
| Rare word: "yellow" | 1 | 2386 ms | 2714 ms | 31 |
| No matches: "qzxjvkw" | 0 | none | 2630 ms | 32 |

The app searches in 30 ms slices between other work, so on screen it takes a little longer than this.

Memory: 389 MB now, 876 MB peak so far.

### Saving

Adding 10 highlights, then everything the app does after Save.

| Step | Time |
| --- | ---: |
| Parse, apply changes, serialise | 614 ms |
| Write the file | 60.6 ms |
| Re-open the saved bytes | 24.5 ms |
| Re-read highlights on the 10 changed pages (10 found) | 249 ms |
| **Save round trip** | **949 ms** |

Memory: 320 MB now, 876 MB peak so far.

## Zooming in

From 10% to 800% in one step, on the last page, after resting 6 s so pages can be drawn ahead. The app runs it in a window of its own; a cold cache has nothing kept from before, a warm one has what the cold run left.

| Document | Cold cache | Warm cache | Drawn ahead |
| --- | ---: | ---: | ---: |
| drawing-set-a.pdf, 85 pages | 15 ms | 14 ms | 64 squares, 64 MB |

## Showing a sheet for the first time

12 sheets spread through the document, shown one after another at 100% zoom, each timed from asking for it to the app reporting it sharp. Sheets are spread out so drawing ahead hasn't already done the work. `KINETIC_PDF_GPU=0` is pdfium doing all of it, which is what most PDF software does. 1 runs a renderer, taking turns to go first, after a pair thrown away; each run in a cache folder of its own. Sheet times are pooled across runs.

| Document | pdfium median sheet | Ours | pdfium p90 sheet | Ours | Sheets timed a renderer | Sheets ours gave to pdfium | pdfium memory | Ours |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| drawing-set-a.pdf, 85 pages | 241 ms | **232 ms** | 1.2 s | **821 ms** | 11 / 11 | 4 | 1021 MB | 1558 MB |

## Working on a sheet

Twelve steps on the middle sheet, after it is already up: zoom to 200%, pan down twice, zoom to 400%, pan down twice, zoom to 800%, pan down twice, then back out to 400%, 200% and 100%. Each step is timed to the app reporting the view sharp: everything in view drawn at the zoom in view. That is not proof the frame reached the screen. A step ends on the third sharp frame, so no step can measure shorter than about three frame intervals; the frame interval is reported so results at that floor can be seen for what they are. Getting the sheet up isn't counted. 1 runs a renderer, taking turns to go first, after a pair thrown away. Step times are pooled across runs.

| Document | pdfium, 12 steps (median run) | Ours | pdfium slowest run | Ours | Ratio of medians | pdfium median step | Ours | pdfium p90 step | Ours | Frame interval, pdfium / ours | Sheets ours gave to pdfium |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| drawing-set-a.pdf, 85 pages | 8.8 s | **4.2 s** | 8.8 s | **4.2 s** | 2.1× | 368 ms | **78 ms** | 1.4 s | **733 ms** | 4.2 / 4.2 ms | 5 |

- drawing-set-a.pdf, 85 pages: 0 of our 12 steps were within 20% of the three-frame floor (13 ms). Pictures of the view at 200% and 800%: `shots/drawing-set-a-pdf-85-pages/`.

### Our pictures against pdfium's

The view pictured by each renderer once sharp, at the same place and zoom. Mean difference is per channel, out of 255. Pixels drawn differently are those off by more than 64 in some channel, which antialiasing alone rarely reaches.

| Document | Zoom | Mean difference | Pixels drawn differently |
| --- | ---: | ---: | ---: |
| drawing-set-a.pdf, 85 pages | 200% | 1.50 | 0.27% |
| drawing-set-a.pdf, 85 pages | 800% | 0.47 | 0.09% |

### Measured on

| | |
| --- | --- |
| Processor | AMD Ryzen 7 8845HS w/ Radeon 780M Graphics, 16 logical |
| Memory | 31 GB |
| Graphics | AMD Radeon 780M Graphics (ATI Technologies Inc., OpenGL 3.3.0 Core Profile Context 25.10.30.14.251216) |
| Page area of the window | 2100x1215 pixels |
| Display scaling | 150% |
| Vsync | on |
| Build | release |

## Summary

- Startup to pdfium ready (later launch): 29.3 ms
- drawing-set-a.pdf, 85 pages: time to first page 507 ms (median)
- drawing-set-a.pdf, 85 pages: page render 282 ms + making its texture 5.86 ms, both on the worker thread (median, 125% zoom on a 200% display)
- drawing-set-a.pdf, 85 pages: search for a common word 2789 ms
- drawing-set-a.pdf, 85 pages: save round trip 949 ms
- drawing-set-a.pdf, 85 pages: zoom to 800% sharp after 14 ms (warm cache)
- drawing-set-a.pdf, 85 pages: a sheet first shown sharp in 232 ms with our renderer, 241 ms with pdfium (median)
- drawing-set-a.pdf, 85 pages: zooming and panning about a sheet, 4.2 s against pdfium's 8.8 s (median run)
- Peak memory over the whole run: 876 MB
