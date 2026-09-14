# To do

## Before publishing the exe as a download

Parked until we decide to upload. None of this is needed to publish the source
code; it's for handing out the compiled exe, which has pdfium and egui's fonts
built in.

- [ ] **THIRD-PARTY-LICENSES.txt** next to the exe in the download, containing:
  - [ ] pdfium's `LICENSE` and all 16 files in its `licenses/` folder (abseil,
        agg23, fast_float, freetype, icu, lcms, libjpeg_turbo, libopenjpeg,
        libpng, libtiff, llvm-libc, pdfium, simdutf, zlib). They come in the
        pdfium-binaries archive; `get-pdfium.ps1` only copies the DLL today, so
        have it copy `LICENSE` and `licenses/` too.
  - [ ] Every Rust package in the exe with its licence text, generated with
        `cargo-about` (MIT, Apache-2.0, BSD, Zlib, ISC, Boost, Unicode).
  - [ ] egui's default fonts: SIL Open Font Licence and Ubuntu Font Licence.
- [ ] **Credits in the README**, which the licences require in the
      documentation:
  - [ ] "Portions of this software are copyright © The FreeType Project
        (www.freetype.org). All rights reserved."
  - [ ] "This software is based in part on the work of the Independent JPEG
        Group."
  - [ ] pdfium (BSD-3-Clause) is used, and Google's name is not used to
        promote the app.
- [ ] Optional: a "Licences" link in the app that shows the file.
- [ ] Optional: add `LICENSE-MIT` and `LICENSE-APACHE` to
      `vendor/pdfium-render/`. Its `LICENSE.md` refers to them, but the
      published crate doesn't include them.
- [ ] Choose a licence for this project (MIT or Apache-2.0 fit everything used).
- [ ] Before the first commit: check `test-preserve.html` (it has a real name
      as a test author) and that `target/` stays out of the repo.

### Practical, not legal

- [ ] Unsigned exe: Windows SmartScreen warns on new downloads. A code-signing
      certificate removes that.
- [ ] Antivirus false positives: the app writes pdfium.dll into
      `%LOCALAPPDATA%` and loads it on first run, which some scanners dislike.
      Publish a checksum with the download, or ship the DLL beside the exe in
      a zip instead of embedding it.

## App

- [ ] A "Clear page cache" option, with the space it uses. `Cache::clear()`
      and `Cache::bytes()` already exist; the app has no settings or menu to
      put it in yet.

## Running beyond Windows

The app is meant to be platform agnostic, but these parts only work on Windows
today. Starting the render helpers already builds anywhere (`pool.rs`
`background()`), so these are what's left:

- [ ] **pdfium library**: `worker.rs` embeds `pdfium.dll` and writes it out to
      `%LOCALAPPDATA%`; `build.rs` and `get-pdfium.ps1` only fetch the Windows
      build. Other platforms need `libpdfium.so` / `libpdfium.dylib` and a
      cache folder of their own.
- [ ] **Memory readings**: `worker::private_bytes` and `pool::free_memory` call
      Windows APIs (`windows-sys`), used for the open-page budget, helper
      count and texture budgets. Needs a fallback, e.g. `/proc/self/status`
      and `sysinfo` on Linux, `mach` on macOS, or a fixed budget.
- [ ] **Sharing the open file**: `helper::open_shared` opens it with Windows
      share flags, so saving can replace it while helpers read it. On Unix a
      plain open already allows that.
- [ ] **Exe resources**: `build.rs` embeds the icon and version info with
      `winresource`; skip that off Windows.
