# Kinetic PDF — development notes

Building, releasing, and how the app works inside. The short version is in the
[README](../README.md).

Kinetic PDF (formerly PDF Annotate) is a native Windows port of an earlier
browser version, built
with [egui](https://github.com/emilk/egui) (glow/OpenGL backend) and
[pdfium-render](https://github.com/ajrcarey/pdfium-render). It does one thing:
highlight text and attach a note to it. Highlights are written into the PDF as
real `/Highlight` annotations, so they open in Edge, Preview, or
anything else — and highlights made elsewhere show up here.

No browser, no local web server, nothing to install. The app is a single
`.exe`. Save writes straight into the file you opened.

The interface is light throughout: a white toolbar and side panels around a
light grey reading area. The palette and the light theme live in
`src/app/style.rs`, and every button is drawn by one helper in
`src/app/widgets.rs` (`paint_button`), so restyling is a matter of editing a few
constants.

## Building it

You need, once:

1. **Rust** — install from <https://rustup.rs>.
2. **Visual Studio Build Tools** with the *Desktop development with C++*
   workload — Rust on Windows links with the MSVC linker, and the Windows SDK
   it installs provides `rc.exe`, which embeds the icon.
3. **pdfium.dll** — run `powershell -ExecutionPolicy Bypass -File get-pdfium.ps1`
   in this folder. It downloads the build pdfium-render 0.9.4 targets
   (chromium/7881, from bblanchon/pdfium-binaries). The build refuses to start
   without it, because it is compiled into the exe.

Then:

```
cargo run --release              # or: cargo run --release -- some.pdf
cargo test                       # selection and search logic
```

The finished app is `target\release\kinetic-pdf.exe`, and that one file is all
there is to ship.

### Releases and updates

Pushing a tag like `v0.2.0` runs `.github/workflows/release.yml`, which builds
the exe on GitHub and publishes it as a release. The tag must match `version`
in `Cargo.toml`, or the workflow stops:

```
# set version = "0.2.0" in Cargo.toml, commit, then:
git tag v0.2.0
git push origin main v0.2.0
```

A few seconds after it starts, the app checks the latest release
(`src/update.rs`). If it's newer, the toolbar shows **Update to v0.2.0**.
Clicking it downloads the new exe and swaps it in for the running one, which
moves aside to `kinetic-pdf.<pid>.old` and is deleted at a later start.
**Restart to update** then reopens the app, and the file you had open, in the
new version. Unsaved changes are asked about first, as when closing. Debug
builds don't check; `KINETIC_PDF_UPDATE=0` turns checking off, and `=1` turns
it on in a debug build. The check needs the repository to be public.

The exe isn't code-signed, so Windows SmartScreen warns the first time a
downloaded copy runs.

### Microsoft Store

The Store build is the same app without the in-app updater, since the Store
updates it, and with pdfium.dll shipped in the package beside the exe instead
of embedded (the `store` cargo feature). Microsoft signs Store packages, so it
runs where Smart App Control blocks the unsigned download.

`packaging/make-msix.ps1` builds `target\msix\KineticPDF_<version>_x64.msix`
from `packaging/AppxManifest.xml`, the logos in `packaging/Assets` (drawn by
`assets/make-icon.ps1`) and the identity in `packaging/store-identity.json`.
The package also puts Kinetic PDF under "Open with" for PDFs.

To try it on this PC before the Store has it:

```
powershell -ExecutionPolicy Bypass -File packaging\make-msix.ps1 -Test
```

It prints the two commands that trust its test certificate (once, as
administrator) and install the package.

To publish:

1. Create a free individual developer account, starting at
   [storedeveloper.microsoft.com](https://storedeveloper.microsoft.com) (Get
   started for free > Individual developer) with a personal Microsoft account.
   Other entry points, Partner Center's own sign-in included, ask for a work
   (Entra ID) account. Then reserve the app's name in Partner Center
   (Kinetic PDF, reserved by Corymbia Software).
2. Copy the three values under *Product management > Product identity* into
   `packaging/store-identity.json`, with the reserved app name as
   `DisplayName`, and commit. The app is listed as Kinetic PDF by Corymbia
   Software.
3. Build the package: run `make-msix.ps1` without `-Test`, or push a release
   tag, which also builds it and keeps it with the workflow run as
   `store-package`.
4. Create a submission and upload the `.msix`. The app needs the restricted
   `runFullTrust` capability, like every desktop program in the Store; say it's
   a desktop app that opens and saves PDFs the user picks. Add screenshots, a
   description, and a privacy policy link if asked for one (the app collects
   nothing; it only contacts GitHub, and not at all in the Store build).

Every later version needs its `Cargo.toml` version raised and a new
submission. The benchmark and examples don't build with `--features store`,
since they time the embedded pdfium copy.

### Benchmark

```
cargo run --release --bin bench -- --label baseline
cargo run --release --bin bench -- --label baseline some.pdf other.pdf
```

`src/bin/bench.rs` times the work behind what you feel in the app, running the
app's own engine code: startup, each step of opening a file up to the first
page appearing, page renders at typical and high zoom, text extraction, a
whole-document search, and a save round trip, with memory after each stage.
With no PDF given it generates a long text document with highlights (once, then
reuses it); PDFs you pass are measured as they are and repeated to `--pages`
long (default 300). Results are saved to `bench-results/` as Markdown, so run it
before and after a change and compare. Frame time on the UI thread isn't
covered, since that needs a real window.

### Why pdfium is embedded rather than statically linked

Nobody publishes a static pdfium library for Windows — bblanchon/pdfium-binaries
and pdfium-lib both ship only the DLL — and building one means a full Chromium
toolchain checkout. So the DLL is embedded in the exe instead. Windows can only
load a DLL from disk, so on first launch the app writes it to
`%LOCALAPPDATA%\kinetic-pdf\pdfium-<hash>.dll` and loads it from there. Later
launches reuse that file. The hash in the name means a newer build of the app
never trips over an older one that is still running. The cost is about 7 MB of
exe size and one small file in the user's app data.

pdfium is BSD-licensed, and pdfium.dll also contains FreeType, ICU, libjpeg-turbo
and other libraries under their own licences. They're all in `licenses/pdfium`
and in the app's third-party notices (see [Licence](#licence)).

### The icon

`assets/make-icon.ps1` draws it — a page with a highlighted line on a blue
tile — and writes `assets/icon.ico` (every size from 16 to 256 px, each drawn
separately so small sizes stay crisp) plus `assets/icon-128.rgba`. `build.rs`
embeds the `.ico` in the exe, so Explorer, shortcuts and "Open with" show it;
`main.rs` sets the raw RGBA as the window and taskbar icon. Edit the drawing in
the script and re-run it to change the icon. Its outputs are checked in, so
building doesn't need to run it.

## Using it

| Action | How |
| --- | --- |
| Open a PDF | **Open PDF…**, `Ctrl+O`, drag a file onto the window, or pass a path on the command line |
| Highlight | Drag across text, pick a colour, type a note, **Highlight**. Letting go also copies the selected text |
| Select a box | Hold `Ctrl` and drag a box: everything inside it is selected and copied, even one column of a table |
| Edit a note | Click the highlight, or click its entry in the notes panel |
| Delete | **Delete** in the popup, or the `×` in the notes panel |
| Undo and redo | `Ctrl+Z` undoes the last highlight, markup, note change or deletion; `Ctrl+Y` or `Ctrl+Shift+Z` redoes it. Also **Undo** and **Redo** at the start of the tool row. It works across saves, and while typing in a note the keys undo the typing instead |
| Save | **Save** or `Ctrl+S` — writes into the original file |
| Find | `Ctrl+F`, type; `Enter` / `F3` for the next match, `Shift+Enter` / `Shift+F3` for the previous, `Esc` to clear |
| See every match | **Results** toggles a side panel listing them; click one to go there |
| See all notes | **Notes** toggles the side panel |
| Zoom | **Fit width** (`Ctrl+0`) and **Fit page** follow the window; `Ctrl` `+` / `Ctrl` `-` step the zoom; `Ctrl` + mouse wheel zooms in on the spot under the pointer |
| Move around | Scroll, or hold the middle mouse button and drag the document like grabbing a page |
| Next or previous page | `Page Down` / `Page Up`: the same spot on the next or previous page, at the zoom you're at. `Home` / `End` go to the start and end |
| Oversized pages | Pages much wider than the rest, such as long drawing sheets, are shrunk to the usual page width. The label on the page switches to actual size and back. Zoom in and scroll across to see detail |

`Ctrl+Enter` saves the popup, `Esc` cancels it. Nothing is written to disk
until you hit Save; closing or opening another file with unsaved work asks
first.

Search ignores case, and a space in the query matches any whitespace in the
page, including a line break — so a phrase that wraps onto the next line is
still found. Matches show in orange, the current one darker and outlined. The
first match shown is the first one from the page you're on.

The **Results** panel shares the right-hand side with **Notes**; opening one
closes the other. It lists every match with its page number and the text
around it, the match picked out in orange, and a count of matches and pages in
its header. The current match is marked, and the list follows along as you step
through with `Enter` or `F3`. Rows are drawn only when scrolled into view, so a
search with thousands of matches stays quick.

## How it works

- **A worker thread, and helper processes to draw pages.** The UI thread (egui)
  never touches pdfium. A worker thread owns pdfium and the open document for
  text, highlights, search and saving (`worker.rs`). pdfium is not thread-safe
  — pdfium-render serialises every call behind one lock — so more threads
  would only queue behind each other. Pages are drawn instead by up to three
  copies of the app started as render helpers, each with its own pdfium
  (`helper.rs`, `pool.rs`). Page renders go to them most wanted first, so the
  pages in view and the pages ahead draw at the same time, and a render for a
  page that has left the view is stopped in the helper. A helper reads the PDF
  from disk as it needs it instead of holding a copy, and closes each page as
  soon as it is drawn, because pdfium keeps a page's decoded images (up to a
  few hundred MB on a large drawing) until the page closes. It also reopens
  the file once its caches pass 256 MB. On a dense drawing set, after scrolling to
  the end and loading ahead, the app's own memory fell from about 740 MB to
  260 MB, and all four processes together peaked at about 730 MB, down from
  900 MB for the app on its own. The look-ahead finished 0.26 s sooner.
  - **How many:** fewer helpers start on machines with fewer than five logical
    processors or under 4 GB of free memory, none under 2 GB, and if none
    start, the worker draws pages itself.
  - **Saving:** helpers open the file with delete sharing, so saving can still
    replace it; they carry on reading the old file until told to open the new
    one.
  - **Shutdown:** a helper quits as soon as its input pipe closes, so helpers
    never outlive the app, even if it crashes. Killed mid-scroll in testing,
    all three were gone within 75 ms.
  - **Cost:** sending pixels between processes adds a few ms a page. Showing a
    300-page text document one page at a time went from 20.5 to 23.4 ms a page.
  - **Setting it:** `KINETIC_PDF_HELPERS=0` turns helpers off, and any other
    number sets how many.
- **Slow pages are cached, drawn ahead, and redrawn sparingly.** pdfium spends
  about 3–4 µs on every drawing object, at almost any size drawn, so a page
  with a few hundred thousand of them takes 1–2 s to draw every time. That
  covers dense drawings, and markup overlays made of stamps. Nothing inside
  pdfium changes that: turning off anti-aliasing and flattening the stamps into
  the page were both tried and didn't help. So the app avoids drawing them
  again (`cache.rs`).
  - **Page cache:** a page that took 150 ms or more to draw is kept on disk,
    compressed, in the user's cache folder (`%LOCALAPPDATA%\kinetic-pdf\pages`
    on Windows). It's keyed by a fingerprint of the file's contents, the page
    and the drawing scale; a quick page gets an empty marker instead. The same
    file under any name finds its pages, and a file changed by anything else
    no longer matches. Saving notes moves the file's pages to its new
    fingerprint, since highlights are never drawn into a page. The cache keeps
    to 1 GB by deleting the images used longest ago, with 60% of that set
    aside for zoomed-in squares so they never push out whole pages. A page
    read back shows in tens of milliseconds: flinging to the end of a dense
    drawing set opened a second time had the pages in view and ahead all
    showing by 0.48 s, against 1.6 s the first time.
  - **Storage:** images are stored as PNG, with a palette when a drawing
    uses 256 colours or fewer, and a square of one colour (blank paper) as a
    20-byte marker. Two background threads write them, and new images are
    skipped rather than queued once 256 MB are waiting. Whole pages are
    compressed quickly, to read back fast: a markup overlay page takes
    5.3 MB and reads back in 17 ms, and a dense drawing about 1 MB in 15 ms.
    Squares are compressed hard, since they're written in the background and
    read back just as fast (under 1.5 ms each): 64 squares of the overlay at
    800% take 0.59 MB against 2.7 MB with plain zlib, and those of a dense
    drawing 0.17 MB. That's four times as much zoomed-in detail kept in the
    same space. Every square is kept, not only slow ones, so a zoom never
    waits on pdfium for a part already seen.
  - **Drawing ahead:** once the view and the pages ahead are drawn, free
    helpers draw the rest of the document into the cache, nearest pages first,
    at the zoom in use. They stop as soon as the view moves or needs them. In
    a test that opened a 49-sheet drawing set, waited 6 s and flung to the end,
    every page had been drawn or marked quick by the time the test finished,
    about 14 s after opening. The cost on first opening is that a helper busy
    loading a dense page when the view lands can't stop until the load is done:
    the pages ahead of the landing finished about 0.24 s later than with no
    cache.
  - **Zooming:** drawing scales step in quarter powers of two, so small zoom
    and window changes keep the same image and find it in the cache. A slow
    page already on screen is redrawn only once a zoom has stayed put for
    0.3 s, stretching the old image meanwhile.
  - **The part in view first:** zoomed in on a slow page so that less than
    half of it shows, the area in view is drawn on its own, ahead of the whole
    page. pdfium skips most objects outside the area, so on a dense drawing the
    view at twice fit width draws in about 0.2 s against 1 s for the page. At
    four times, the overlay's view arrived 2.3 s after zooming, against 3.8 s
    for the page. Overlays gain little at twice, because their embedded images
    cost more the more pixels are drawn, and the view plus its margin is
    nearly as many pixels as the page.
  - **Zooming back is instant.** The zoomed-in view is drawn as 512-pixel
    squares on a grid at each zoom step, and the squares are kept: up to
    384 MB in memory, with the ones used longest ago let go first, and on disk
    too. A page's earlier whole-page images are kept in memory as well, up to
    256 MB. Both budgets shrink on a machine with less memory free when the
    app starts (a twelfth and an eighteenth of it), because a texture costs
    the process about two and a half times its pixels: the graphics driver
    keeps copies of its own. Opening a drawing set and letting the app draw
    176 MB of squares ahead of a zoom took the process from 250 MB to
    740 MB. Zooming the markup overlay to 800%
    the first time took 3.3 s until everything was sharp, whole page included.
    Every zoom back out, and in again, was then sharp within a millisecond.
    Opened again later, the sheet was sharp at fit width in 37 ms and at 800%
    in 0.33 s, from the disk cache.
  - **Drawing ahead of a zoom.** Ctrl+scroll zooms towards the pointer, and
    Ctrl +/- towards the middle of the view. Once the view is sharp and the
    pointer has rested half a second, helpers with nothing else to do draw
    that spot as 800% would show it. First come 2048-pixel blocks of squares
    around the spot, nearest first: the screen the zoom would land on, then half
    a screen further each way. Then comes the whole page at the size it would
    need. It stops when the view moves or needs a helper. It only covers a few
    screens, because a whole sheet at 800% is over a billion pixels. On the
    way in, squares from a deeper zoom stand in at any zoom until that zoom's
    own arrive. Zooming the markup overlay straight to 800% was sharp after
    0.77 s with no rest, 68 ms after a 1 s rest, and at once after 2 s.
  - **Stamps drawn from a merged copy.** Markup overlays and CAD exports
    often draw every line as a path of its own, and pdfium's cost is per
    path, not per pixel: a sheet of six stamps holding 382,000 one-line paths
    took 1.5 s at fit width and 1.2 s at an eighth of that size. So when a
    file opens, a process of its own (`--merge-copy`, so a PDF that trips up
    the reader can't take the app down) rewrites a copy in which back-to-back
    strokes inside annotation appearances become one path with many parts
    (`merge.rs`): only the `S` between them goes, at most 256 paths to one,
    never across a clip, fill or state change, and never where strokes are
    see-through or blended. The helpers then draw from the copy, kept in the
    cache next to the pages and moved along with them when notes are saved;
    the worker keeps the file itself for text, notes and saving. On that
    sheet the copy took 0.2 s to make, 344,000 of 364,000 strokes merged, and
    the pixels differ from the original's by under 0.02%. In the app the
    first draw of the page went from 1.9 s to 1.4 s, each view at 800% from
    about 370 ms to 215 ms, and the whole page at 800% from 3.1 s to 2.4 s.
    Pages' own drawing is left alone: on a dense drawing set, merging it made
    views at 800% slower, since pdfium skips lines outside the area being
    drawn one by one, and a merged path can't be skipped.
  - **Annotations on hidden layers aren't drawn.** An annotation can belong to
    a layer (optional content), and a viewer that honours layers, as many
    do, shows it only while that layer is on. pdfium draws a page's
    annotations whatever their layer, so a markup overlay that kept its
    earlier stamps on layers switched off showed both versions, one out of
    line with the other. The same copy leaves out annotations whose layers
    are off when the file opens (groups, membership dictionaries and
    visibility expressions all count). On that overlay 5 of its 6 stamps were
    hidden, which also halved the lines to draw: the page's first draw took
    0.7 s. A file whose bytes show layers, or objects packed where their names
    can't be seen, has its helpers wait up to 2 s for the copy the first time
    it opens, and nothing drawn from the file itself is kept in the cache
    until the copy is ready. Images cached before this change could show
    hidden layers, so the cache's file names changed and the old ones are
    deleted when it opens.
  - **Setting it:** `KINETIC_PDF_CACHE=0` turns the cache off, and any other
    value is the folder to keep it in. `KINETIC_PDF_MERGE=0` draws from the
    file itself, without a merged copy.
- **Virtualised pages.** On open the worker reports every page's size without
  loading the pages, so the whole document lays out at once. The UI asks only
  for the pages in view and a few ahead (see below), and textures for pages
  that scroll well away are dropped. The
  worker also turns each rendered page into its texture, so the UI thread only
  receives a finished texture; for a large drawing that conversion was about
  20 ms per page, a visible hitch while scrolling. The texture's upload to the
  graphics card still happens on the UI thread, where egui does it.
- **Highlights load a page at a time.** Reading a page's annotations means
  loading the page, so reading them all before showing anything kept a long
  document blank for a moment (330 ms for 300 pages in the benchmark; the first
  page now appears in about 25 ms). Instead each page's highlights are read just
  before it is first drawn, so they're on screen with its pixels, and the rest
  are read in the background in 20 ms slices between other work. The notes panel
  says when it is still reading. `tests/worker.rs` checks this end to end.
- **Page geometry.** Text and annotations live in a page's unrotated user
  space, but pdfium draws the page turned by its `/Rotate` and trimmed to its
  crop box. The worker reports each page's rotation and visible box, from the
  same page load that reads its highlights, and every conversion between page
  and screen goes through it (`PageGeometry` in `model.rs`). `tests/rotation.rs`
  renders rotated pages and checks the text maps to where the ink really is.
- **Layout and zoom.** The page most of the document shares sets the fit, so a
  drawing set fits its A1 sheets rather than one oversized sheet. Pages much
  wider than that are shrunk to its width unless switched to actual size, and
  every page is centred in one column. A zoom change remembers the page spot
  under the pointer (or the middle of the view) and scrolls it back under the
  same point. Page images are limited to what's in view plus one page either
  side, then kept within a 512 MB budget.
- **Deep zoom renders only what's in view.** A whole-page image is capped at
  8 megapixels. Zoomed in past that, the part of the page in view, plus a
  384-pixel margin, is rendered again on its own at full sharpness and drawn
  over it. The cost follows the pixels on screen, not the size of the sheet, so
  an A1 drawing at 8× is sharp about 380 ms after the zoom, and a text page is
  sharp in about 50 ms. The whole-page image fills the edges while you scroll
  until the next sharp render arrives. `tests/region.rs` checks the region's
  pixels line up exactly with a whole-page render, on rotated pages too.
- **Selection is pdfium's character list.** Each page's text layer is the list
  of characters pdfium extracts, with their boxes. A drag picks the caret
  nearest each end, and the selection is the characters between — so a
  highlight always snaps to the text (`selection.rs`). Character boxes are
  merged into one band per line, keeping columns apart, and stored as
  `/QuadPoints`.
- **Search runs in slices.** The find box waits for typing to pause, then asks
  the worker to search. The worker goes through the pages in order, about 30 ms
  at a time, sending each batch of matches as it goes. Between slices it serves
  renders and other requests, so the viewer stays responsive during a long
  search. A new query replaces the old search, and it stops at 5,000 matches.
  Each page's text is kept once it has been extracted (up to 256 MB), so
  searching again, or scrolling back to a page, doesn't load the page again.
- **Pages stay loaded briefly.** Loading a page parses its content, about 9 ms
  for a large drawing, and reading its highlights, extracting its text and
  rendering it each used to load it again. The worker now keeps the six most
  recently used pages open, so a new page is loaded once and a zoom re-render
  doesn't load it at all. Showing each page of a large drawing set in turn
  went from about 220 ms to 165 ms a page. Open pages are also held to 512 MB:
  pdfium keeps the images it decoded to draw a page until the page closes,
  which is a few MB for text but 150–400 MB for some drawings. The worker
  measures what loading and drawing each page took and closes the oldest pages
  when over. The background highlight scan reads each page and closes it
  again. On a dense drawing set this cut memory after scrolling from about
  900 MB to about 740 MB.
- **Loading follows the view.** The UI shares with the worker the pages it
  wants, most wanted first: the pages in view from the middle out, then pages
  to load ahead. The worker queues page work and always takes the most wanted
  job next. Work for a page that has left the view is dropped, and a render
  already under way stops within about 40 ms, because pdfium draws in steps
  and checks between them. While the view moves faster than eight screens a
  second -- a fling or a scroll bar drag, faster than steady wheel scrolling --
  nothing new is asked for, and the background highlight scan waits.
  A fast scroll to the end of a long document therefore goes straight to the
  last page instead of loading every page it passed; on a dense drawing set
  that page used to arrive about 3 s after the scroll stopped, and now arrives
  in under 100 ms. Once everything in view is drawn, the next pages load
  ahead: two the way you were scrolling, then one back the other way, or all
  three back from either end of the document. A page that takes longer than a
  quarter of a second to draw shows as it draws. `tests/scheduling.rs` checks
  the order, the skipping and the stopping. Set `KINETIC_PDF_TRACE=1` to see
  every loading decision on stderr.
- **Highlights are see-through.** The browser version relied on
  `mix-blend-mode: multiply`. Here each highlight band redraws that part of the
  page texture tinted by the highlight colour. A tint multiplies, so the white
  page turns yellow while black glyphs stay black. Search matches are drawn the
  same way.
- **Highlights are drawn by the app, not by pdfium.** Before a page is first
  rendered, its highlight annotations are removed from the worker's display copy
  of the document. Otherwise a deleted highlight would linger in the pixels.
  Every other kind of annotation still renders.
- **The quoted text on a note is exactly what was highlighted.** For a fresh
  highlight, that's the selected characters. For one loaded from a file, it is
  recovered by finding the first and last characters whose centres fall
  inside each band.
- **Saving** starts from the bytes as last read, applies edits, then deletes
  (highest index first), then additions, and writes to a temporary file that
  replaces the original only once complete. The worker then re-reads the pages
  the save changed, so their highlights carry their new positions on disk. A save
  doesn't move annotations on any other page, so those are left as they are,
  which cut the save round trip from 448 ms to 177 ms in the benchmark.
- **Every change is a command.** The window never edits highlights and
  markups itself: adding, removing and changing a note each go to the
  document's session (`session.rs`) as a command, which it applies and keeps,
  up to 1,000 of them, for undo and redo. What a save writes -- new
  annotations, deletions, changed notes -- is worked out from the session's
  state, so undoing back to the file as saved leaves nothing to save.
  - **Across a save:** a save moves annotations within their pages. The
    session knows the order the file will hold them in, kept ones by position
    then added ones, and matches the pages read back to the same highlights
    and markups, so selection and undo carry on, as do changes made while the
    save ran. If what comes back doesn't match, as when something else changed
    the file, those pages are shown as read and undo history is cleared.
    `tests/session.rs` saves through the worker and checks the session against
    a fresh read of the file each time.
  - **Cost:** nothing per frame. On 40,000 annotations over 500 pages, a
    command takes 0.03 ms, an undo 0.07 ms, starting a save 2.2 ms and
    matching its read-back 2.7 ms (`cargo test --release --lib session::timing
    -- --ignored --nocapture`).
- There is no sidecar file and no database. The PDF is the store. Your name
  for new notes is kept in `%APPDATA%\kinetic-pdf\author.txt`.

## Files

```
Cargo.toml           dependencies and a size-tuned release profile
build.rs             checks pdfium.dll is present; embeds the icon
vendor/pdfium-render pdfium-render 0.9.4 with a crash fix (see its PATCHES.md)
tests/               end-to-end tests of the worker thread against real pdfium
examples/probe.rs    crash triage: runs each pdfium call on a PDF, step by step
examples/floors.rs   takes each part of the app apart to estimate its fastest possible time
src/bin/bench.rs     the benchmark
get-pdfium.ps1       downloads pdfium.dll
assets/make-icon.ps1 draws the icon
assets/icon.ico      the exe icon
assets/icon-128.rgba the window icon
src/main.rs          window setup
src/app/mod.rs       the window's state, opening and saving, replies from the worker, keys
src/app/pages.rs     the page viewer: what to load, textures and zoomed-in squares, drawing
src/app/layout.rs    laying pages out, zoom, going to a page or a match
src/app/drag.rs      selecting text, and boxes of it with Ctrl
src/app/notes.rs     the highlight popup, notes panel, colours, author name
src/app/search.rs    the find box and results panel
src/app/toolbar.rs   the toolbar, save status, toasts
src/app/style.rs     the palette and light theme
src/app/widgets.rs   buttons, swatches, quote cards
src/worker.rs        the pdfium thread: text, highlights, saving, search; draws pages if no helper can
src/pool.rs          starts the render helpers and hands them pages, most wanted first
src/helper.rs        a render helper process, and the messages it exchanges with the app
src/cache.rs         the disk cache of pages that were slow to draw
src/merge.rs         merging stamps' back-to-back strokes, for a copy that's only drawn
src/annots.rs        reading and writing annotations, rendering, text extraction
src/selection.rs     carets, line bands, quoted text, search matching (with unit tests)
src/session.rs       the open document's highlights and markups, changes to them as commands, undo, what to save
src/model.rs         data passed between the two threads
crates/markup-model  measurement markups as data: geometry, scales, units, quantities (see docs/design-log.md)
crates/pdf-io        measurement markups and scales to and from PDF: /Measure, /VP, dimension annotations, /KPDF
tests/measure_pdf.rs measurement markups written by pdf-io, opened and drawn by pdfium
examples/measure_sample.rs  writes tmp/measure-sample.pdf, a sample sheet of measurements
```

## Differences from the browser version

- **No appearance stream is written** for new highlights. pdf-lib let the
  browser version build one; pdfium-render doesn't expose that. Edge,
  Chrome, Firefox and Preview all draw highlights from `/QuadPoints` and `/C`
  anyway, but a viewer that relies solely on appearance streams would show
  nothing.
- Recolouring a saved highlight still doesn't stick, for the same reason as
  before: another viewer may have given it an appearance stream in the old
  colour.
- Search is new; the browser version relied on the browser's own find.

## Known limits

- **Scanned PDFs have no text to select or search.** The app says so on any
  page with no text. Making those selectable needs OCR, which this doesn't do.
- **Search matches characters as pdfium reports them.** A word split by a
  hyphen at a line break, or typeset with a ligature character such as "ﬁ",
  won't match the plain spelling.
- **Loading a very dense page can't be interrupted.** A drawing with hundreds
  of thousands of objects can take up to a second to load before pdfium starts
  drawing it, and the load can't be stopped part-way the way drawing can. If
  the view moves on just as one starts loading, that helper is tied up until it
  finishes, though the other helpers carry on. Drawing such a page takes
  another second or so; it shows as it draws. A helper drawing one of these
  pages briefly holds a few hundred MB until it closes the page.
- **The page cache has no "clear" button yet.** It clears itself to stay
  within 1 GB. To empty it by hand, close the app and delete
  `%LOCALAPPDATA%\kinetic-pdf\pages`.
- **Scrolling fast at deep zoom shows soft edges briefly.** The sharp render
  covers the view plus a margin. Scroll past it and the softer whole-page image
  shows until the next sharp render arrives, a fraction of a second on a large
  drawing.
- **Encrypted/password-protected PDFs** aren't handled.
- If the file is open in another program that locks it, Save reports that it
  could not replace the file, and your changes stay unsaved in the app.

## Licence

Kinetic PDF is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](../LICENSE-APACHE))
- MIT license ([LICENSE-MIT](../LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution you
submit for inclusion in the work, as defined in the Apache-2.0 license, is
dual licensed as above, without any additional terms or conditions.

The exe includes other open-source software: the Rust crates it depends on,
a patched copy of pdfium-render (`vendor/pdfium-render`, see its `PATCHES.md`),
and pdfium with the libraries built into it. Their licences are collected in
`assets/THIRD-PARTY-NOTICES.txt`, which is compiled into the app and shown
under **About**. After changing dependencies or the pdfium build, regenerate it
and commit the result:

```
cargo install cargo-about --locked --features cli   # once
powershell -ExecutionPolicy Bypass -File make-notices.ps1
```

The release workflow regenerates it before every build.
