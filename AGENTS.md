# Working in Kinetic PDF

Native Windows PDF reader/editor and measurement/takeoff app, written in Rust
with egui/eframe (OpenGL), PDFium, and lopdf. Ships as a standalone executable;
the Microsoft Store build has separate packaging and update behavior.

## Code map

- `src/main.rs`: startup/window setup; `src/lib.rs`: engine/library modules.
- `src/app/`: UI and interaction. Start with `mod.rs`; feature panels and tools
  have separate modules. Shared visuals live in `style.rs` and `widgets.rs`.
- `src/session.rs`: annotation/measurement commands, undo, dirty state, save reconciliation.
- `src/arrange.rs`: sheet order, rotation, duplication, blank pages, separate undo history.
- `src/document.rs`, `src/app/lifecycle.rs`: immutable revisions and lifecycle transitions.
- `src/worker.rs`, `src/protocol.rs`: PDFium worker and request/reply messages.
- `src/app/render.rs`, `src/app/gpu.rs`: rendering state and GPU integration;
  `src/pool.rs`, `src/helper.rs`, `src/cache.rs`: render helpers and caching.
- `src/domain.rs`: domain types/geometry; `src/model.rs`: compatibility exports.
- `crates/markup-model`: UI-independent markup geometry, measurements, scales, layers.
- `crates/pdf-io`: measurement/markup persistence and PDF appearances.
- `crates/pdf-content`, `crates/gpu-lines`: PDF content operations and GPU drawing.
- `crates/text-layout`: text layout shared by screen display and PDF output.
- `tests/`: focused integration/regression tests; modules also contain unit tests.

## Build and validate

Run from the repo root on Windows with Rust and Visual Studio C++ build tools.
PDFium is required for building and integration tests; fetch it if absent:

```powershell
powershell -ExecutionPolicy Bypass -File get-pdfium.ps1
cargo run --profile quick                 # fast local UI iteration
cargo build --workspace --all-targets     # CI build
cargo test --workspace                    # CI tests
cargo test --test scales                  # example focused integration test
cargo check --bin kinetic-pdf --features store --locked # Store checked separately
cargo run --release                      # shipping/performance profile
```

- Use release builds for benchmarks; see `docs/DEVELOPMENT.md` for commands.
- Do not combine Store checks with all targets: benchmarks/examples do not support it.
- Match surrounding formatting. CI deliberately skips rustfmt/clippy; there is no
  rustfmt config for the existing wide style. Do not blanket-format the repo.
- Add focused regression coverage; UI changes also need an interactive check.
  Report checks actually run and any checks that could not be performed.

## Guardrails

- Keep PDFium off the UI thread; preserve worker/helper-process ownership.
- Route document edits through session commands (sheet edits through arrangement).
  Preserve undo and edits made during ordinary saves; structural saves gate edits.
- Keep readers/render caches tied to the same immutable document revision; reject
  stale results. Preserve external-change detection and safe save replacement.
- Use `PageGeometry` for page/screen conversions; account for rotation and crop boxes.
- Avoid unrelated changes to `vendor/` or generated assets. PDFium is patched:
  read `vendor/pdfium-render/PATCHES.md` before changing that dependency.

## File headers

- New or modified non-trivial source files must start with a short header.
  Use Rust `//!` module comments; use the language's comment syntax elsewhere.
- Include role, purpose, repo-relative path, and primary consumers. Add inputs,
  outputs, and constraints only when useful (e.g. threading, saves, coordinates).
- Skip trivial type/constants-only files and vendored/generated code.
- Adopt this as files are touched; do not do unrelated repo-wide header rewrites.
- Before finishing an edit, re-read the header. Update it in the same change if
  purpose, consumers, interfaces, or caveats drifted. A stale header is a defect;
  an accurate header needs no change. Keep it concise, not an exhaustive API list.

```rust
//! [Name] - [Brief role]
//!
//! [Short description of purpose and any non-obvious constraints.]
//!
//! PATH: [repo-relative path]
//! CONSUMED BY: [primary modules, binaries, or test runners]
```

## Communication and contributions

- Use concise sentences and dot points in replies, issues, and PRs. Avoid walls
  of text, repeated summaries, and boilerplate; explain why when it adds context.
- Issues: a clear descriptive title, problem, and expected behavior. Include
  reproduction steps for bugs when useful; omit irrelevant sections.
- Commits: short, action-led titles, e.g. `Keep callout arrow pinned during resize`.
  No mandatory prefixes or body; add brief bullets only for non-obvious context.
- PRs: a short descriptive title, 1–3 change bullets, and checks actually run
  (or `Not run` with a reason). Include screenshots for UI changes when useful.
- Use the templates in `.github/`; keep them brief rather than filling space.

## Further reading

- `docs/DEVELOPMENT.md`: build, release, benchmarks, and implementation details.
- `docs/code-architecture-review.md`: boundaries, resolved issues, remaining caveats.
- `docs/design-log.md`: design rationale; `docs/PAGE-ARRANGE.md`: sheet behavior.
- `docs/SIGNATURES.md`: staged signing/certification plan, safety gates, and limitations.
- Docs contain historical descriptions; verify current behavior against source/tests.
