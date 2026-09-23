# Arranging pages — where this got to

Branch: `page-arrange`. Deleting, copying, pasting, duplicating, turning and
dragging sheets into a new order, scaffolded far enough to use and to build on.

The shape of it: pulled back to 20% zoom or further, a drawing set on screen
stops being pages to read and becomes sheets to sort, so that is where the
clicks change meaning. No mode button, no separate window — the zoom the user
is already at says which of the two they want, because at that size there is
nothing on a sheet to read or draw on anyway.

## What is built

**`src/arrange.rs` — the model**, with no egui and no pdfium in it. An
`Arrangement` is a list of `Sheet::Page { page, turns }` /
`Sheet::Blank { size, turns }` plus what is picked out, a clipboard and an undo
stack. Every operation is one method: `click(at, ctrl, shift)`, `select_all`,
`delete`, `cut`, `copy`, `paste(at)`, `duplicate`, `rotate(quarters)`,
`insert_blank`, `move_selected(to)`, `undo` / `redo`, `discard`. Plus
`rearrange(&mut Document, &[Sheet])`, which rewrites the page tree with lopdf.
28 unit tests.

**Turning is a quarter-turn on the sheet, not on the page.** `turns` is 0–3
clockwise and is counted *from how the file already has the page*, so a
drawing the file itself turns comes up turned and one more quarter-turn is one
more from there; `rearrange` writes it by adding to the page's own `/Rotate`.
Because the turn is the sheet's, turning one duplicate leaves the other
standing. Nothing is re-rendered to show it: the sheet view hands the
thumbnail's corners round a four-vertex quad (`image_turned`), so turning a
whole set costs four vertices a sheet.

**`src/app/arrange.rs` — the sheet view.** At 20% zoom and further out
(`SHEET_ZOOM`), `draw_sheets` stands in for `draw_pages`: sheets are drawn
from their thumbnails, clicks pick them out, Ctrl and Shift extend, Delete
takes them out, Ctrl+X/C/V/D cut, copy, paste and duplicate, right-click
offers the same plus a quarter-turn either way and a blank sheet, and
dragging shows a caret in the gap the
sheets will land in. `handle_input` sends the keys to the sheet view *or* to
the measure and tool keys, never both, so Delete means a sheet at 5% and a
measurement at 100%. Undo goes through the existing `undo_step`, which now
gives the sheet order first refusal.

**`tests/arrange.rs`** opens what the writer wrote with **pdfium**, not with
the library that wrote it: the order is honoured, a page used twice comes back
twice, a blank sheet is the size it was asked for, a turned sheet comes back
reported the other way round — including a page the file had already turned,
which comes back upright — and the highlights are still on the pages they
belong to after a reorder.

All green, along with the rest of the suite, and clippy is clean on the new
files.

**The design choice worth knowing: an edit changes a list, not the file.**
Taking twenty sheets out of an 85 MB set costs twenty `usize`s, and a
duplicate shares the original's thumbnail — nothing is rewritten, redrawn or
re-read.

## The sheet/page split

The column the user scrolls is the **arrangement's sheets**; everything kept
about a page — its images, squares, thumbnail, text, shapes, annotations — is
kept against the **page of the file**. Two sheets showing one page share all
of it, and taking a sheet out throws none of it away.

The crossing between the two is `Doc::sheet_page`, `Doc::sheet_turns` and
`Doc::sheet_geometry`, and that is the only place it is made. `layout()` is
fed `sheet_sizes` at every zoom, so `layout.tops` and `layout.scales` are
indexed by sheet; `current_page`, `page_rects` and every `Drag` are sheets
too, which is why `Drag`'s fields are named `sheet`. The compiler cannot tell
a sheet from a page — both are `usize` — so the naming is the only guard
there is. **Keep it.**

Turning a sheet is one line of that: `PageGeometry::turned` adds the
quarter-turns to the geometry everything drawn over a page already goes
through, so text, highlights, markups, measurements and hit testing all come
round with the sheet for free.

**An edit still changes a list, not the file.** Taking twenty sheets out of an
85 MB set costs twenty `usize`s, and a duplicate shares the original's
thumbnail — nothing is rewritten, redrawn or re-read until the user saves.

## Saving

There is no "apply" and no second file. A changed order is unsaved work like a
markup is (`App::has_unsaved_work`), so the save button lights up for it and
closing asks about it. **Ctrl+S writes the markups and the order together**,
into the open file, in place, in one atomic write: `annots::save` puts the
annotations down against the pages as the file still holds them, and
`rearranged_bytes` moves the pages afterwards, which carries each page's
annotations with it. The document is then opened again, since everything held
against where a page used to be now describes a file that no longer exists.

Rotated sheets use the GPU transform and sharp tiles. Tile requests,
zoom preloading and highlight texture sampling map through the sheet's
rotation. The GPU paint call must run for rotated sheets too: skipping it
leaves blank paper once the GPU takes over from the page texture.

Arrangement saves must not rekey cached drawings to the new fingerprint:
page positions and rotations have changed. The worker starts fresh page
state, cancels the helpers' old renders, and the UI reopens without applying
old annotation keys to the new order. The cache fingerprint namespace was
changed to bypass entries contaminated by earlier arrangement saves.

The post-save refresh preserves the live zoom, scroll and layout reference
size rather than applying the defaults for opening a different document.
Visible texture previews bridge the refresh without entering the page cache;
a two-second "Saved" toast confirms completion.

## What was deliberately left

- **The GUI has not been run by eye.** This is compile-, clippy- and
  test-verified only. The drag caret, the selection ring and the hit testing
  want a look before they are trusted.
- No grid layout — sheets stay in the single column the viewer already uses.
  No auto-scroll when a drag reaches the edge of the view. No pasting sheets
  between documents.

## Where to pick it up

1. Run it and watch it: the drag caret, the selection ring, and whether a drag
   that leaves the window still lands where it should.
2. **Sheet-versus-page is not type-checked.** Anywhere a `usize` crosses
   between the column and the file without going through `Doc::sheet_page`,
   it is a bug that shows only on a rearranged document — never on one nobody
   has touched, where the two indices are equal. A newtype for each would end
   that class of mistake outright, and is the single most valuable thing left.

The reasoning is written up in [the design log](design-log.md), under
2026-09-21.
