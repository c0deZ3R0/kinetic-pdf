# Arranging pages — where this got to

Branch: `page-arrange`. Deleting, copying, pasting, duplicating and dragging
sheets into a new order, scaffolded far enough to use and to build on.

The shape of it: pulled back to 20% zoom or further, a drawing set on screen
stops being pages to read and becomes sheets to sort, so that is where the
clicks change meaning. No mode button, no separate window — the zoom the user
is already at says which of the two they want, because at that size there is
nothing on a sheet to read or draw on anyway.

## What is built

**`src/arrange.rs` — the model**, with no egui and no pdfium in it. An
`Arrangement` is a list of `Sheet::Page(n)` / `Sheet::Blank([w, h])` plus what
is picked out, a clipboard and an undo stack. Every operation is one method:
`click(at, ctrl, shift)`, `select_all`, `delete`, `cut`, `copy`, `paste(at)`,
`duplicate`, `insert_blank`, `move_selected(to)`, `undo` / `redo`, `discard`.
Plus `rearrange(&mut Document, &[Sheet])`, which rewrites the page tree with
lopdf. 20 unit tests.

**`src/app/arrange.rs` — the sheet view.** At 20% zoom and further out
(`SHEET_ZOOM`), `draw_sheets` stands in for `draw_pages`: sheets are drawn
from their thumbnails, clicks pick them out, Ctrl and Shift extend, Delete
takes them out, Ctrl+X/C/V/D cut, copy, paste and duplicate, right-click
offers the same plus a blank sheet, and dragging shows a caret in the gap the
sheets will land in. `handle_input` sends the keys to the sheet view *or* to
the measure and tool keys, never both, so Delete means a sheet at 5% and a
measurement at 100%. Undo goes through the existing `undo_step`, which now
gives the sheet order first refusal.

**`tests/arrange.rs`** opens what the writer wrote with **pdfium**, not with
the library that wrote it: the order is honoured, a page used twice comes back
twice, a blank sheet is the size it was asked for, and the highlights are
still on the pages they belong to after a reorder.

All green, along with the rest of the suite, and clippy is clean on the new
files.

**The design choice worth knowing: an edit changes a list, not the file.**
Taking twenty sheets out of an 85 MB set costs twenty `usize`s, and a
duplicate shares the original's thumbnail — nothing is rewritten, redrawn or
re-read.

## What was deliberately left

- **Applying writes a new PDF through "Save as…", not the open file.** Every
  highlight, markup and measurement is keyed to its page's place in the file
  (`AnnotKey`), so moving pages under the open document would move all of
  them. Renumbering that is the real next piece of work.
- **Pending edits show in the sheet view only.** Zoom in and you see the
  file's order; a bar at the bottom says what has changed, with *Put back* and
  *Save as…*. Making the arranged order show at every zoom means a
  display-index → file-page split through `pages.rs`, `gpu.rs`, `markups`,
  `measure` and `scale` — about 59 call sites. Worth doing, but not inside a
  scaffold.
- **The GUI has not been run by eye.** This is compile-, clippy- and
  test-verified only. The drag caret, the selection ring and the hit testing
  want a look before they are trusted.
- No grid layout — sheets stay in the single column the viewer already uses.
  No auto-scroll when a drag reaches the edge of the view. No pasting sheets
  between documents.

## Where to pick it up

1. Run it and watch the sheet view: the caret, the ring, and whether a drag
   that leaves the window still lands where it should.
2. Then either the annotation renumbering (which unlocks applying to the open
   file), or the display-index split (which unlocks seeing the new order at
   every zoom). The renumbering is the one that makes the feature finished;
   the split is the one that makes it feel finished.

The reasoning behind both is written up in
[the design log](design-log.md), under 2026-09-21.
