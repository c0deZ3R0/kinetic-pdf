# Local patches to pdfium-render 0.9.4

This is pdfium-render 0.9.4 exactly as published on crates.io, with the changes
below. `rust/Cargo.toml` substitutes it for the crates.io release through
`[patch.crates-io]`. The `include/` headers (only used by the `bindings`
feature) and `test/` files were left out.

## 1. Crash reading or setting the colour of an annotation with an appearance stream

In `src/pdf/document/page/annotation/private.rs`, `fill_color_impl`,
`set_fill_color_impl`, `stroke_color_impl` and `set_stroke_color_impl` call
`FPDFAnnot_GetColor` / `FPDFAnnot_SetColor`. Pdfium deliberately returns false
from both when the annotation has a normal appearance stream. The upstream code
then fell back to `FPDFPageObj_GetFillColor` and friends, passing the
**annotation** handle cast to `FPDF_PAGEOBJECT`. An annotation is not a page
object, so Pdfium reads (or writes) the wrong structure: an access violation
(0xC0000005) inside pdfium.dll that takes the whole process down.

Highlights made in Acrobat, Edge or Bluebeam all carry appearance streams, so
any PDF annotated in those crashed PDF Annotate on opening. Found with
`examples/probe.rs` on a PDF highlighted in Bluebeam.

The fallbacks now return `Err(PdfiumInternalError::Unknown)` instead, and the
now-unused `FPDF_PAGEOBJECT` import is gone.

## 2. `PdfPageAnnotationCommon::remove_appearance(mode)`

A new method wrapping `FPDFAnnot_SetAP(annot, mode, NULL)`, which removes the
appearance stream for that mode (for the normal mode, the whole `/AP` entry).
Pdfium reports `/C` and `/IC` only for annotations without a normal appearance
stream, so this is the way to read the real colour of one that has one. PDF
Annotate calls it on its display copy of the document, which deletes a page's
highlights before rendering it anyway, and never on the copy it saves.

Declared on the trait in `src/pdf/document/page/annotation.rs`, implemented as
`remove_appearance_impl` in `private.rs`.

## 3. Unused-code warnings silenced

`Cargo.toml` gains `[lints.rust] dead_code = "allow"`. Upstream has a few unused
items; Cargo hides warnings from crates.io dependencies but shows them for a
path dependency, where they would bury warnings from PDF Annotate itself.

## 4. `PdfPage::render_into_bitmap_in_steps(bitmap, config, step, keep_going)`

A new method in `src/pdf/document/page.rs`: the same render as
`render_into_bitmap_with_config`, but through Pdfium's progressive API
(`FPDF_RenderPageBitmap_Start`, `FPDF_RenderPage_Continue`,
`FPDF_RenderPage_Close`) with an `IFSDK_PAUSE` that pauses every `step`. At each
pause `keep_going` sees the bitmap drawn so far and can abandon the render. A
page with hundreds of thousands of drawing objects takes over a second to draw;
this lets PDF Annotate drop one that has scrolled away and show one in view as
it draws. Form fields are drawn with `FPDF_FFLDraw` once the page is complete;
custom matrices and form highlight colours aren't supported.

## 5. A Rust lib only, not a staticlib and cdylib as well

`Cargo.toml`'s `[lib] crate-type` was `["lib", "staticlib", "cdylib"]`, for
using the crate from C and from WebAssembly. Kinetic PDF binds pdfium.dll at
runtime and wants none of that, and building all three put
`libpdfium_render.rlib`, `pdfium_render.lib` and `pdfium_render.dll` in
`target/<profile>/deps` under names that collide. `cargo build` got away with
it; `cargo test` builds the crate twice, the copies clobbered each other, and
the rlib went missing, so every integration test failed to compile with
``can't find crate for `kinetic_pdf` ``. It is now `["lib"]`.

## Updating

The changes are small. To move to a newer pdfium-render: check whether the
fallbacks above are fixed upstream and whether an appearance-stream setter now
exists, then either drop this folder and the `[patch.crates-io]` entry, or copy
the new release here and reapply what's still needed.
