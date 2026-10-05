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
`.exe`. Save writes into the current file; Save As chooses a new destination.

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
cargo run --profile quick        # the same, built in seconds rather than minutes
cargo test                       # selection and search logic
```

The finished app is `target\release\kinetic-pdf.exe`, and that one file is all
there is to ship.

`--release` builds with fat LTO in one codegen unit, which re-optimises the
whole dependency graph however little changed: a one-line change to the app
took 1 m 34 s measured, against 7.4 s for the same change under `quick`. Use
`quick` to run the app and see a change; use `--release` for anything shipped,
and for anything timed, since `quick` is not the code that ships.

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

The Store build is the same app with pdfium.dll shipped in the package beside
the exe instead of embedded, and updates from the Store rather than GitHub
(the `store` cargo feature). The GitHub download code isn't compiled into it:
Store policy lets only the Store install a Store app's updates. The Store
does that in the background while the app is closed, which some people never
let happen, so the app also asks the Store itself (`StoreContext`, in
`src/update/store.rs`) and shows an **Update available** button. The Store
reports which installed packages have updates, without the destination version;
a nonempty update list is enough to offer the update. Clicking it
asks about unsaved work, since installing closes the app, then hands over to
the Store's own install dialog. Outside a real Store install (a `-Test`
package, or `cargo run --features store`) the Store has nothing to compare
against and no button appears, so the full path can only be tried once a
version is live and a newer one is published. Microsoft signs Store packages, so it
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
   nothing; the GitHub build contacts GitHub, and the Store build only asks
   the Store for updates, through Windows).

Every later version needs its `Cargo.toml` version raised, its notes in
`packaging/store-changes-<version>.txt`, and a new submission. Paste those
notes into the submission's "What's new in this version"; the app shows the
same text as **What's new** at the first start of that version (build.rs
compiles every notes file in, and a test fails if this version has none or
they pass the Store's 1500 characters). The benchmark and examples don't build with `--features store`,
since they time the embedded pdfium copy.

### Benchmark

```
cargo run --release --features bench --bin bench -- --label baseline
cargo run --release --features bench --bin bench -- --label baseline some.pdf other.pdf
```

The `bench` feature is what builds the benchmark at all. Without it the
binary is left out, so its thousand lines don't rebuild with every change to
the app.

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
| New PDF | **File > New PDF** or `Ctrl+N`; choose a standard or custom size, portrait/landscape, and page count. Save asks where to put the untitled document |
| Open Recent | **File > Open Recent**; the last ten successfully opened or saved PDFs, remembered between runs. Full paths distinguish similarly named files. Clear Recent Files clears only the history |
| Default units | **File > Settings… > Units**; metric or imperial, remembered between runs. Page dimensions use mm or inches; new scales use m/m²/m³ or ft-in/ft²/yd³. Existing drawing units are preserved |
| Highlight | Take up the **Highlighter** in the tool row (`H`), drag across text, pick a colour, type a note, **Highlight**. Letting go also copies the selected text. With any other tool in hand, dragging leaves the text alone |
| Highlight a box | With the **Highlighter**, hold `Ctrl` and drag a box: everything inside it is selected and copied, even one column of a table |
| Select | The **Select** tool (`V` or `Esc`) is in hand whenever no other tool is. Click a measurement, a markup or a highlight to pick it out; `Ctrl`-click adds one or takes it back out. Drag a box across bare page: dragged rightwards it picks out only what is wholly inside, leftwards everything it touches; with `Ctrl` held it adds to what is picked out. `Ctrl+A` picks out everything on the page. **Delete** removes everything picked out, and dragging one of them moves them all (highlights stay with their text, and markups already saved stay put) -- each as one step to undo |
| Edit a note | Click the highlight, or click its entry in the notes panel |
| Delete | **Delete** in the popup, or the `×` in the notes panel |
| Undo and redo | `Ctrl+Z` undoes the last highlight, markup, note change or deletion; `Ctrl+Y` or `Ctrl+Shift+Z` redoes it. Also **Undo** and **Redo** at the start of the tool row. It works across saves, and while typing in a note the keys undo the typing instead |
| Set a page's scale | **Scale** in the tool row. Click each end of something whose real length you know (or drag along it) and type the length, or pick a printed ratio. **Check it** measures a second known dimension and says how far out the scale is. **Use on every page** gives them all the same scale | Lines snap to the drawing's corners, crossings and middles; hold Ctrl to place a point freely, Shift to keep it square |
| See what has been measured | **Quantities** in the tool row opens a table across the bottom: a row per measurement, with its description, kind, page, what it measures and a column each for length, area, perimeter, depth, volume and count. Click a heading to sort by that column, again to turn it round. Double-click **Description** to name a quantity, or **Depth** against an area to price it by volume (Escape abandons what was typed); click a row to go to it and pick it out; × deletes it. **Group by** gathers rows by description, page or kind, with a subtotal each. **This page** narrows it to the page in view; **Export CSV…** saves the table for a spreadsheet | Totals leave out anything with no scale or a shape that can't be measured, and say how many |
| Measure | **Length**, **Polylength**, **Area**, **Count**, **Angle**, **Radius** or **Diameter** in the tool row, once the page has a scale. Click each point; double-click or press Enter to finish, Ctrl+Z or Backspace to take one back (Ctrl+Y puts it down again), Esc to stop. Points snap to the drawing; hold Ctrl to place one exactly where the pointer is. With the **Select** tool, press a measurement to pick it out: a corner moves it, the middle of an edge adds a corner there, anywhere else moves the whole thing. Delete removes the corner picked out, or the measurement. **Cutout** takes a hole out of an area. **Count** adds a mark per click to the count in hand, Esc starts a new one; **Angle** is arm, corner, arm; **Radius** is middle then edge; **Diameter** is two clicks straight across. A radius and a diameter show the circle they measure, and either end of the line drawn moves it |
| Clip | Take up **Clip** in the tool row (`C`) and drag a box over any part of a page, or click round a shape (double-click, Enter or a click on the first corner finishes it; Backspace takes back a corner, Esc drops it): what's drawn there -- the page, and the markups, measurements and highlights over it -- is copied as a vector drawing. `Ctrl+V` in this window or any other Kinetic PDF window puts it down as a markup under the pointer, the right way up and at the size it was. Drag it to move it, drag a corner to resize it (it keeps its shape), Delete removes it, and `Ctrl+C` with one picked out copies it again. It's a picture of the markups it covers, not the markups themselves: nothing in it is measured again |
| Cut and Erase | **Cut** (`X`) and **Erase** (`D`), beside Clip, take an area the same way -- a box or a shape clicked round. **Erase** takes the page's own drawing out of it; **Cut** copies that drawing, as Clip does, and then erases it, to paste it somewhere else with `Ctrl+V`. Markups, measurements and highlights over the area stay where they are. The area shows as paper at once and undoes like anything else until the next save, which takes it out of the page itself; after that it's the file's |
| Copy and paste | Pick out measurements, clips or markups drawn since the last save and press `Ctrl+C`; `Ctrl+V` puts copies under the pointer, in this window or another. `Ctrl+Shift+V` puts them where they were on the sheet they came from: the same place on a sheet the same size, and the same place across and down a larger or smaller one. Either way they keep their size on paper, and measure by the scale of the sheet they land on. A clip is copied the moment it's taken, placed where it was taken from |
| Text boxes | **Text box** (`T`) and **Text box with arrow** (`Shift+T`) are beside the highlighter. Drag a box, or click for one a usual size, and type; for an arrow, drag from what it points at to where the box goes. Press on the page elsewhere, Esc or Ctrl+Enter to finish; a box left empty goes again. While typing, pick out words and style them from the details panel -- font, size, colour, bold, italic, underline -- or with Ctrl+B, I and U; with nothing picked out, the style is for what is typed next. Double-click one with the Select tool to type in it again, drag a corner to resize it (the words wrap, and with Fit on grow or shrink to fill it), and drag the arrow's tip to point it elsewhere. Not being typed into, the details panel sets the whole box: the font (any installed), size, bold, italic, underline, colour, alignment across and down, padding, fit, border, background and arrow; the tool creator makes kept text tools the same way. Fonts are embedded in the saved PDF, so it looks the same everywhere; opened where a font is not installed, a box is set in the copy the file carries, which the font list shows as "(from a file)" |
| Turning and stretching | With the Select tool, what is picked out gets a frame with eight handles: drag one to stretch it (Shift from the middle, Ctrl keeping the proportions). Click what is picked out again, or click a handle, and the handles become turning ones at the corners: drag one to turn it about the middle (Ctrl in 15-degree steps); click again for the stretching ones. Several things picked out turn and stretch together. One text box, clip, rectangle or ellipse gets a frame of its own shape, turned with it. Measurements keep their quantities as they turn; stretching one changes them. A clip keeps its proportions, and a text box types upright. Drawings already saved into the file stay as they are |
| Kinetic Compare | **Tools → Kinetic Compare…** lays another revision of the open drawing set beside it: after saving anything unsaved, pick the other PDF. Three columns show the original, the compared set, and the two laid over each other -- red where only the original draws, blue where only the compared one does, dark where both do. Row by row the sheets are paired; drag a sheet up or down its column to pair it with the right one, right-click one to put a blank sheet before or after it or leave it out, Delete leaves out what is picked, and Ctrl+Z / Ctrl+Y undo and redo. Neither file is changed. The view moves freely: drag it with the middle button, scroll with the wheel (Shift for across), and Ctrl+wheel zooms keeping whatever sheet is under the pointer -- in any column -- under it. **Fit** puts the columns back across the middle. The other tools are off until **Exit compare**. **Generate** writes the overlay, paired as it is, to a PDF you name and opens it in a new window: an ordinary PDF to mark up and measure, each set on a layer of its own that any viewer can turn off. Opened here, it has a slider at the foot of the pages: all the way left shows the original alone, all the way right the compared set alone, and the middle both. With the slider all the way to one end, Clip, Cut and Erase work on that set alone -- a clip lifts only it, and an erase takes out only its drawing, the other showing through; anywhere between, they work on both |
| Save | **Save** or `Ctrl+S` writes into the current file |
| Save As | **File > Save As…** or `Ctrl+Shift+S` writes all current edits to the chosen file; subsequent saves use that file |
| Print | **File > Print…** or `Ctrl+P`; preview printer paper, margins, range, copies, scaling, pages per sheet, orientation, colour and duplex. Include or exclude annotation markups and measurements. Includes unsaved edits without changing the file |
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

Printing uses the installed driver's supported paper sizes and printable
margins. **Match each document page's size** handles mixed drawing sets when
the printer supports their sizes. **Print as image** is a fallback for complex
PDFs: 150/300/600 DPI controls the detail and job size, and image bands keep
bitmap memory bounded. Grayscale bands use 8-bit data; pure black-and-white
bands use 1-bit data. Normal printing retains vector detail. The print window
shows clipping and lets the job be cancelled. Flattened markups cannot be
excluded separately from the underlying drawing.

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
  - **Document revisions:** the worker owns an immutable byte snapshot shared
    with the GPU reader, measurements and overlay probe. Helpers stream from
    a temporary backing file of that same snapshot, so an external edit cannot
    make rendering and annotation reads describe different PDFs. The scheduler
    and any merge job keep that backing file alive. This adds temporary disk
    I/O on open and save when helpers run; with no helpers, none is written.
  - **Saving:** a replacement is parsed before committing it, so a readback
    failure leaves the original file and worker document usable. A save checks
    for external changes before preparation and immediately before replacement;
    a conflict keeps the edits open for Save As. A deleted destination is still
    recreated. The committed snapshot and cache identity go to the UI and
    helpers together. This check does not lock out another program's rename.
    Structural saves block document edits until their refresh finishes, while
    ordinary saves continue to preserve edits made during saving. Lifecycle
    transitions are kept in `app/lifecycle.rs`; snapshots are in `document.rs`.
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
  and screen goes through it (`PageGeometry` in `domain.rs`). `tests/rotation.rs`
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
- **Scales are read only when something needs them.** A page's scale lives in
  the file as a /VP viewport with a /Measure, which pdfium can't see, so
  reading it means parsing the whole file with lopdf: 0.5 s and about 390 MB
  on a 221 MB drawing set. Opening a file doesn't do that. The first time the
  scale panel opens, `Request::ReadMeasurements` parses the file on a thread
  of its own and the panel fills in when it lands. Scales then live in the
  session with highlights and markups, so setting one undoes like anything
  else and is written by the same save, which rewrites the /VP of only the
  pages whose scales changed. `tests/scales.rs` sets a scale, saves it and
  reads it back from the file.
- **Snapping comes from the shapes the GPU reads.** Reading a page for the GPU
  already gives every line as a flat segment in the page's own space, so the
  measurement tools snap to those rather than parsing the page again: corners
  and ends first, then crossings, middles, and the nearest point along a line,
  with markup corners winning over all of them. The index is a uniform grid
  built on the reader's thread (29 ms for 400,000 segments, 8 MB, 3.4 µs a
  query), kept for the pages near the view within 64 MB, and built only while
  a measurement tool is in use -- which reads the page once more. Fills are
  triangles by then, so only strokes are snapped to, and pages pdfium draws
  offer nothing but markup corners.
- **Measurements are markups with quantities.** A length, run, area, count,
  angle, radius or diameter is a markup in the model (`crates/markup-model`), measured from its points
  and the page's scale, so a recalibration updates every number on the page at
  once. They live in the session with everything else, so they undo and save
  together, and are written as standard measurement annotations
  (`crates/pdf-io`) with an appearance for other viewers -- with a standard
  dimension intent where ISO 32000 has one, and otherwise as the annotation
  whose shape the measurement has, with what it measures in /KPDF. The app draws them
  itself, live; pdfium and the GPU renderer leave annotations named `KPDF-`
  alone, or every quantity would show twice. Changing one is written as a
  removal and a write under the same name, since the name is the markup's own
  ID. `tests/scales.rs` draws one, saves it, moves it, and takes it out again
  through the worker.
- **Erasing takes the drawing out of the page itself.** An erasure is part of
  a page's own drawing -- a polygon in its user space -- held in the session
  as a change like any other, shown at once as paper over the page and under
  what's marked up on it, and undone as any change is. A save writes it
  (`gpu_lines::erase_page`) with the same reading of the content stream a clip
  uses, the other way round: what paints wholly within the area is taken out
  of the stream, and a clip round the page, less the area by the even-odd
  rule, stops what crosses its edge there. So a line through it is cut exactly
  at the edge, and what was inside is gone from the file rather than covered
  up; layers that are off, and annotations, are left as they were. Once
  written it's the file's, and goes from undo. Cut is Clip without the
  markups over the page, followed by Erase. `tests/erase.rs` erases a line of
  text, saves, and finds pdfium no longer has it, and its highlight still
  there.
- **A blank sheet is a new page of the document.** Inserting one adds a page
  to the document's own page table (`Arrangement::add_page`), numbered on
  from the file's pages -- a file of 85 pages gets page 85 -- with a size and
  a geometry like any other, and the sheet shows it. So everything that works
  on a page works on it before it's ever saved: selecting, measuring,
  drawing, calibrating, clipping. Only what reads the file skips it
  (`Doc::sheet_file_page`): it has no image, text or shapes to load, and
  draws as paper. A save puts the new pages after the file's own first
  (`arrange::append_pages`), so each lands at the number it already had;
  then what's on them is written as onto any page, and the sheets are put
  in order last, as for any rearrangement. One save, and nothing is moved
  from one page number to another on the way. `tests/new_pages.rs` draws a
  length on a new page, saves, and finds it there.
- **A clip is the page's own drawing, cut down to the box.** Letting go of
  the Clip tool's box asks the thread that reads pages into shapes -- which
  has the file parsed already -- to lift it (`gpu_lines::clip_page`). The
  page's content stream is read once, operator by operator, following the
  transform and line width: paths, text, images and forms that paint wholly
  outside the box are left out, forms are cut down the same way, and a clip
  that shuts the box out takes everything under it with it. The rest is kept
  exactly as it was written, and only the fonts and images it still uses are
  copied across, byte for byte. Content on a layer that's off is left out,
  since the clip carries no layers. Annotations shown on the page are cut
  down alike, and the markups, measurements and highlights the app draws
  itself go over the top from their appearance streams (`src/app/clip.rs`).
  All of it becomes a one-page PDF placed so the box's bottom left is the
  origin and its sides run the way the sheet was seen. Text stays text and
  photos stay the photos they were. Reading a dense sheet's 400,000
  operators takes about 15 ms; lifting a detail takes 30-120 ms and a whole
  sheet 60-170 ms, where drawing the page into shapes and writing those out
  took 1-4 s. The clip goes on the Windows clipboard in a format of its own,
  which only this app asks for, so any window of it can paste it.
  - **As a markup:** a pasted clip is a markup like a measurement, kind
    `Clip`, its geometry its four corners, the drawing's bottom left first;
    the PDF it carries is shared between undo steps rather than copied. It's
    saved as a stamp whose appearance is that PDF's page, placed by the
    corners, with the corners in /KPDF, so any viewer shows it and it reads
    back placed as it was.
  - **Drawing it:** its PDF is read into shapes once, on a thread of its own,
    and sent up to the GPU a piece a frame, then drawn through a matrix from
    its corners. Moving or resizing it only changes the matrix. Drawings not
    shown lately are let go past 256 MB, and each clip's images are kept as
    sharp as fits in 128 MB. Without a GPU a clip shows as its outline.
- **A text box is set once, the same way on screen and in the file.**
  `crates/text-layout` finds the fonts installed here (the Windows and the
  user's font folders, reading only each file's name and OS/2 tables:
  about 270 families in 35 ms) and lays a box's words out -- wrapping,
  alignment, justifying, top/middle/bottom, and fitting: the largest
  scale the words fit at, found by doubling up from 1 then halving, so a
  few words fill the box and more shrink it -- from the font's own glyph
  widths. Each word is measured once: text scaled by s breaks into lines in
  width W as the text as set does in W/s, so trying a scale is only line
  breaking (300 words fit in about 0.3 ms). The installed fonts are found
  on a thread of their own as the window opens. Each part of the words (a run) has its own format; an edit or a
  restyle works on them a character at a time (`TextBox::edited`,
  `restyled_range`) and runs are joined again where their formats match. The box
  is a markup of kind `Text`: its geometry its four corners, the text's
  bottom left first, as a clip's are; its words, their formats, padding,
  alignment and arrow tip in `Extras::text`; its border and background
  the markup's line and fill.
  - **On screen** (`src/app/text.rs`) each word is drawn by egui where the
    layout put it, in the same font file, loaded into egui the first time
    a box uses it (a frame in egui's own font until then), turned with the
    sheet. A box's layout and egui's galleys for it are kept until the box,
    the zoom or egui's glyph atlas changes (a scrap of text laid out each
    frame shows when egui has started its glyphs again), so a frame only
    places them: 200 boxes cost about 0.3 ms. Typing happens in an editor
    laid over the box at the zoom in view; each change is a `ChangeMeasure`
    merged into one step to undo.
  - **In the file** (`crates/pdf-io/src/text.rs`) it's a FreeText
    annotation -- FreeTextCallout with /CL and a closed arrow when it has
    one -- whose appearance is the same layout written as glyph ids in
    each font embedded whole as Type0 / Identity-H, with widths and a
    ToUnicode map so text can be found and copied. Each font program is
    tagged with its name and length and written once per file, reused by
    later saves. /KPDF keeps the corners and the words as typed (JSON), so
    it reads back editable. Each font's descriptor names its family and weight
    (/FontFamily, /FontWeight), so a machine without the font takes the
    file's copy in as it reads the box (`read::text_box_fonts`,
    `Catalogue::take_in`) and sets it, and saves it, in that; one that has
    it uses its own. A bold or italic the font hasn't got is
    stroked or slanted.
- **Turning and stretching** (`src/app/reshape.rs`) works out one map of
  the page (`markup_model::Affine`) from where a handle was taken hold of to
  the pointer, and puts it on everything picked out as it was when the drag
  began, so nothing creeps and the drag is one step to undo. Points are
  mapped; a text box or clip stays a box (`box_mapped`: its middle mapped,
  turned as its bottom edge is, stretched along each side, a clip the same
  both ways); an ellipse measurement can only stay square to the page. A
  drawn rectangle or ellipse keeps four corners once turned rather than
  the two ends of its drag (`markup::box_corners`), and is written as a path
  through them. A drawn markup takes new points by `Command::Reshape`.
- **Kinetic Compare** (`src/app/compare.rs`) reads both files itself, each on
  a thread of its own, straight into shapes for the GPU, so the open
  document -- and the one pdfium thread and render helpers, which hold one
  document at a time -- are left alone. Each side's pairing is an
  `Arrangement`, blanks and all, never written. A page is sent up a piece a
  frame, and each cell -- a sheet, or an overlay -- is drawn into an image of
  its own a few milliseconds a frame (`Renderer::paint_some_tinted`, whose
  overlay draws the second page onto the first without clearing it, each
  tinted and multiplied), at sizes stepped by about a fifth, then only
  shown: redrawing three columns of heavy sheets every frame couldn't keep up
  with a scroll. Pages and images not shown lately are let go past 1.5 GB
  and 512 MB. Sheets start top left to top left. **Align all…** and
  **Align page…** open a centred overlay with a fixed original and a green
  compared sheet: drag it to move, or drag a corner to resize proportionally
  around its centre. **Keep proportions** is on for a new alignment; untick
  it to stretch width and height independently with the corner handles or
  percentage fields. Turning it back on locks the current ratio without
  changing the sheet. Ctrl+wheel zooms, the middle button pans, and arrow keys
  nudge the sheet (Shift for a larger step). The size field, Centre sheets,
  Reset, and Fit view allow precise adjustment and recovery. Apply commits
  the draft; Cancel or Escape leaves the comparison unchanged.
  A document alignment is the default; page overrides are keyed by both
  source page numbers, not the row, and survive changes to that default.
  **Use document alignment** removes a page override. Applied alignment and
  pairing changes share Ctrl+Z / Ctrl+Y history. Alignment stays in the open
  comparison session and never modifies either source PDF.
  `compare_align.rs` handles the editor. Its cached green sheet is multiplied
  over the original, so paper shows through while moving and resizing without
  redrawing the geometry. The same placement calculation in `overlay.rs`
  supplies the comparison preview and exported PDF, including bounds that
  contain both sheets when one is moved beyond the other's edge. Offsets are
  fractions of the original sheet size, so document defaults adapt to mixed
  paper sizes; horizontal and vertical scale are stored separately. Export
  remains in the standard red/blue colours.
- **Generating an overlay** (`src/overlay.rs`) writes each set's sheets as forms
  in an optional content group -- a layer -- of their own, their content
  recoloured (each colour set followed by its tint, as much of it as the
  colour is dark) and every graphics state made to multiply, so the GPU
  renderer draws it rather than falling back on pdfium. The catalogue's
  `/KPDFCompare /Layers` says which layer is which set. Opening a file, a
  thread looks for that marker in its bytes (`probe_overlay`), parsing only
  a file that has it. The renderer tags each run with the layer it was drawn
  on (`Run::layer`, from the `/OC` marked content it's inside) and fades
  each by `Renderer::set_layer_fades`, so the slider needs no re-reading:
  squares of heavy pages are drawn again at each new setting, the page drawn
  straight until they're all there, and no thumbnail stands in under an
  overlay, since its layers multiply over whatever is under them.
  With the slider at an end, Clip and Cut lift that layer alone
  (`ClipOptions::only_layer`: any other group is as good as off), and an
  erase is kept with its layer (`Erasure::layer`): written, it takes out
  what's wholly inside only on that layer and clips inside that layer's
  marked content rather than round the page (`erase_page`); unsaved, it's
  not painted as paper but masked out of that layer by the renderer, which
  keeps a mask in the stencil's top bit, the clips counting in the rest
  (`set_layer_masks`).
  left.
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
src/app/lifecycle.rs opening, saving, refresh and recovery states
src/app/render.rs    per-document render resources and committed-revision cache invalidation
src/app/pages.rs     the page viewer: what to load, textures and zoomed-in squares, drawing
src/app/layout.rs    laying pages out, zoom, going to a page or a match
src/app/drag.rs      the highlighter's drags across text, and boxes of it with Ctrl
src/app/picked.rs    what is picked out, and the Select tool: clicks, boxes, moving and deleting it all
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
src/app/scale.rs    the scale panel, calibrating, checking and the dialog
src/app/quantities.rs the quantities table: rows, descriptions, grouping, totals, CSV
src/app/measure.rs  the length, polylength and area tools, and drawing them
src/app/clip.rs     Clip, Cut and Erase: taking an area of a page, lifting it, erasing it, and drawing clips
src/app/copying.rs  copying and pasting markups and clips, in place or under the pointer, on the clipboard
src/app/text.rs     text boxes: putting them down, typing into them, resizing, pointing arrows, drawing them
src/app/reshape.rs  the frame round what is picked out: stretching and turning it by its handles
src/app/compare.rs  Kinetic Compare: two drawing sets side by side, paired sheet by sheet, and laid over each other
src/overlay.rs       writing a Kinetic Compare overlay out as a PDF, each set on a layer of its own
src/document.rs      immutable document bytes and their cache identity
src/domain.rs        document geometry, annotations and changes, independent of UI and worker messages
src/protocol.rs      UI/worker requests and replies
src/raster.rs        shared page tiling, image utilities and texture payloads
src/model.rs         compatibility exports for existing library consumers
crates/markup-model  measurement markups as data: geometry, scales, units, quantities (see docs/design-log.md)
crates/pdf-io        measurement markups and scales to and from PDF: /Measure, /VP, dimension annotations, /KPDF
crates/text-layout   the fonts installed here, and laying a text box's words out in them
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
