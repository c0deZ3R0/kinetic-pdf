# Kinetic PDF

A small, fast PDF reader for Windows that does one thing: highlight text and
attach a note to it.

- Highlights, notes and markups (pen, box, ellipse, line, arrow) are saved into
  the PDF as standard annotations, so they show up in Edge or any
  other viewer.
- Search, zoom, and quick scrolling, even through large drawing sets.
- One `.exe`: nothing to install, no account, no extra files. It updates
  itself from this repo's releases.

## Download

Get `kinetic-pdf.exe` from
[Releases](https://github.com/c0deZ3R0/kinetic-pdf/releases/latest) and run it.
It isn't code-signed yet, so Windows may warn the first time.

## Build

Needs [Rust](https://rustup.rs) and the Visual Studio C++ build tools.

```
powershell -ExecutionPolicy Bypass -File get-pdfium.ps1
cargo run --release
```

How it works, releasing, and benchmarks: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Licence

MIT or Apache-2.0, at your option. Kinetic PDF collects no data; see the
[privacy policy](PRIVACY.md). Third-party licences are in
[assets/THIRD-PARTY-NOTICES.txt](assets/THIRD-PARTY-NOTICES.txt).

Built on [pdfium](https://pdfium.googlesource.com/pdfium/) and
[egui](https://github.com/emilk/egui). Portions of this software are copyright ©
1996-2002, 2006 The FreeType Project (www.freetype.org). All rights reserved.
This software is based in part on the work of the Independent JPEG Group.
