# PDF Annotate benchmark: lazy-highlights

- Run: 2026-09-14 09:54
- Build: release
- Logical CPUs: 16
- App exe: 12.2 MB
- Open runs per document: 3

## Startup

| Step | Time |
| --- | ---: |
| First launch: unpack pdfium.dll (hash and write) | 10.2 ms |
| Later launches: check the unpacked copy (hash and size) | 5.77 ms |
| Load pdfium.dll | 9.93 ms |
| Initialise pdfium | 1.99 ms |
| **Later launch, pdfium ready** | **17.7 ms** |

Window creation and the first egui frame come on top and aren't measured here.

Memory: 15 MB now, 15 MB peak so far.

## Generated text document, 300 pages, a highlight on every 5th page

`target\release\bench-data\synthetic-300p-v1.pdf`, 5.1 MB

### Opening, step by step as the app does it

Everything up to the first page appearing. The other pages' highlights are read afterwards, in the background, and don't hold up the first page.

| Step | First run | Best | Median |
| --- | ---: | ---: | ---: |
| Read the file into memory | 1.46 ms | 1.37 ms | 1.44 ms |
| Copy it for pdfium | 0.54 ms | 0.50 ms | 0.52 ms |
| Parse the document | 1.73 ms | 1.48 ms | 1.48 ms |
| Every page's size | 2.79 ms | 2.69 ms | 2.76 ms |
| Read page 1's highlights | 4.53 ms | 2.59 ms | 2.78 ms |
| Render page 1 (125% zoom, 200% display) | 11.6 ms | 11.3 ms | 11.6 ms |
| Convert page 1 to a texture (UI thread) | 2.58 ms | 2.58 ms | 2.58 ms |
| **Time to first page** | **25.3 ms** | **22.6 ms** | **24.7 ms** |
| Then, in the background: every other page's highlights | 300 ms | 289 ms | 291 ms |

300 pages, 60 highlights.

Memory: 18 MB now, 79 MB peak so far.

### Rendering

Measured over 12 pages spread through the document. Removing a page's highlights before its first render: 0.93 ms median.

| View | Pixels | Texture | Render best | Render median | Render p95 | Convert to texture (UI thread) | Memory for 9 kept pages |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 125% zoom, 100% display | 744 × 1052 | 3.0 MB | 3.84 ms | 3.91 ms | 4.38 ms | 0.61 ms | 27 MB |
| 125% zoom, 200% display | 1488 × 2105 | 11.9 MB | 10.2 ms | 10.4 ms | 12.1 ms | 2.56 ms | 108 MB |
| 300% zoom, 200% display | 3572 × 5051 | 68.8 MB | 70.8 ms | 73.0 ms | 74.8 ms | 15.0 ms | 619 MB |

Memory: 27 MB now, 164 MB peak so far.

### Text extraction

| Pages sampled | Characters per page | Best | Median | p95 |
| ---: | ---: | ---: | ---: | ---: |
| 30 | 5824 | 1.65 ms | 1.90 ms | 2.13 ms |

The app extracts text for each page it shows, in a separate request from the render.

### Search, the whole document as the app does it

| Query | Matches | First match after | Whole document | Pages per second |
| --- | ---: | ---: | ---: | ---: |
| Common word: "method" | 3935 | 1.63 ms | 591 ms | 508 |
| Rare word: "zephyrine" | 6 | 13.8 ms | 514 ms | 584 |
| No matches: "qzxjvkw" | 0 | none | 524 ms | 572 |

The app searches in 30 ms slices between other work, so on screen it takes a little longer than this.

Memory: 49 MB now, 164 MB peak so far.

### Saving

Adding 10 highlights, then everything the app does after Save.

| Step | Time |
| --- | ---: |
| Parse, apply changes, serialise | 143 ms |
| Write the file | 1.66 ms |
| Re-open the saved bytes | 2.00 ms |
| Re-read highlights on the 10 changed pages (20 found) | 30.1 ms |
| **Save round trip** | **177 ms** |

Memory: 39 MB now, 164 MB peak so far.

## Summary

- Startup to pdfium ready (later launch): 17.7 ms
- Generated text document, 300 pages, a highlight on every 5th page: time to first page 24.7 ms (median)
- Generated text document, 300 pages, a highlight on every 5th page: page render 10.4 ms + texture conversion 2.56 ms (median, 125% zoom on a 200% display)
- Generated text document, 300 pages, a highlight on every 5th page: search for a common word 591 ms
- Generated text document, 300 pages, a highlight on every 5th page: save round trip 177 ms
- Peak memory over the whole run: 164 MB
