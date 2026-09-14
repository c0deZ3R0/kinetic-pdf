# To do

## Before publishing the exe as a download

Parked until we decide to upload. None of this is needed to publish the source
code; it's for handing out the compiled exe, which has pdfium and egui's fonts
built in.

- [x] **Third-party notices**, compiled into the exe and shown under About
      (`assets/THIRD-PARTY-NOTICES.txt`, written by `make-notices.ps1`):
  - [x] pdfium's `LICENSE` and every file in its `licenses/` folder, which
        `get-pdfium.ps1` now copies to `licenses/pdfium`.
  - [x] Every Rust package in the exe with its licence text, generated with
        `cargo-about`.
  - [x] egui's default fonts: SIL Open Font Licence and Ubuntu Font Licence.
- [x] **Credits**, which the licences require in the documentation: FreeType,
      the Independent JPEG Group, and pdfium without Google's name used to
      promote the app. In the README and at the top of the notices.
- [x] A "Licences" link in the app that shows the file (About).
- [ ] Optional: add `LICENSE-MIT` and `LICENSE-APACHE` to
      `vendor/pdfium-render/`. Its `LICENSE.md` refers to them, but the
      published crate doesn't include them.
- [x] Choose a licence for this project: MIT OR Apache-2.0.
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
