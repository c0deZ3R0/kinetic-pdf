# Code architecture review

Reviewed 2 October 2026 at `88c93f4`, after rebasing `code-hygiene` onto `origin/main`.

This is a fresh high-level review of document ownership, editing, saving, rendering, and module boundaries. The strongest existing boundaries are the command-based `Session`, the independent `markup-model` crate, and PDFium isolation from the UI thread. The main weakness is that document lifecycle rules are distributed between the UI, session, worker, and rendering services.

The original findings below describe that baseline. They came from source tracing; the implementation status records the subsequent changes and regression coverage. Priorities describe recommended order of attention: P1 for possible lost changes or broken document operation, P2 for correctness and design debt.

## Implementation on this branch

The save and revision foundation is implemented. `src/document.rs` owns immutable byte snapshots and their cache identities. The worker, GPU reader, overlay probe and lazy measurement reader share those bytes. Render helpers and the merge process stream from a temporary immutable backing file kept alive by the scheduler and any merge job.

`src/app/lifecycle.rs` replaces the independent opening status, structural-save flag and refresh generation with one state machine. Opening and structural saves block session mutations centrally, and failed writes restore editing. A failed reopen leaves the document unavailable for editing or saving until it is opened successfully again. Navigation remains available during a structural save.

Saving now prepares a readable replacement document before committing the file. A failed preparation preserves the existing worker document and original file. Saving with no matching loaded document returns a terminal failure, and a disconnected worker no longer leaves the UI waiting. Source bytes are checked before preparation and again immediately before replacement. A deleted destination is recreated, preserving the existing save contract; changed external content is rejected, with Save As available to preserve the open edits.

Save replies carry the committed snapshot. The UI advances its cache identity, restarts thumbnail reads for that identity, retries an outstanding measurement read, and rejects older measurement replies. GPU thumbnail jobs keep their source identity; jobs for pages the worker certifies visually unchanged can move to the new identity, while jobs for changed pages are cancelled. Fresh shapes replace changed pages' old thumbnail previews.

Regression coverage includes structural-save undo and failure recovery, ordinary edits during saving, stale measurement replies, disconnected and unloaded workers, source replacement, Save As recovery, render helpers reading an externally replaced source, and injected preparation failure. Existing view-preservation and destination-recreation tests continue to apply.

The contextual undo contract is preserved: committed annotation edits and sheet edits still have separate histories, and structural-save refresh resets them. Consolidating those histories requires a product decision and stable page identity.

The next pass separates rendering resources into `src/app/render.rs`. `Doc` retains document metadata, the snapshot, session and arrangement; its `RenderState` owns page images, GPU resources, thumbnail readers, pending drawing work and derived snapping caches. A committed revision enters through `RenderState::revision_committed`, which advances thumbnail identities and invalidates changed-page images. GPU rereading receives only render state and a reader for the committed snapshot. Existing render scheduling still accesses these fields within `app`; this is an ownership boundary, not a fully private renderer API.

`src/domain.rs` now owns document geometry, annotations and change sets without UI or worker-message imports. `src/protocol.rs` owns requests and replies, and `src/raster.rs` owns tiling and image utilities. Production code imports those modules directly. `src/model.rs` retains compatibility exports so existing library consumers and integration tests keep compiling. The save regression also verifies that changed pages retain a preview until their replacement arrives, unchanged pages retain their cached drawings, and later edits survive the transition.

The new backing file adds temporary disk I/O when helpers run. The user ran the save/revision build (now `29ba7b6` after rebasing) and reported that everything looked fine. The subsequent rendering/domain separation still needs an interactive check, and large-document performance needs a dedicated check. External-change detection is a best-effort check immediately before rename, not a filesystem transaction coordinated with other programs.

## Validation

- `cargo test -j 2 --workspace --locked`: 594 passed, zero failed, four existing tests ignored.
- `cargo check -j 2 --workspace --all-targets --features bench`: passed.
- `cargo check -j 2 --bin kinetic-pdf --features store --locked`: passed. The Store app is checked separately because the repository's benchmark, examples and update tests do not support combining their targets with the Store feature.
- `git diff --check`: passed.

These checks also passed after the render/domain separation. The 594-test total includes the extended save regression covering changed-page previews, unaffected cached drawings and later edits.

Before publishing on 3 October, both commits were rebased without conflicts onto `90d59dd` (the Store update notification fix). The full workspace/all-targets build passed, and the rebased workspace tests passed: 595 passed, zero failed, four existing tests ignored. The extra test came from the updated main branch.

The structural-save regression failed against the original code before the mutation guard was added. Validation also caught the existing deleted-destination recreation contract, which the conflict check now preserves. The compiler's existing unused-assignment warning in `app/gpu.rs` remains.

## Architecture at the review baseline

| Component | Current responsibility |
| --- | --- |
| `src/app` | egui presentation, interaction, document lifecycle coordination, rendering policy and GPU resource management |
| `src/session.rs` | Annotation and measurement state, commands, undo, dirty tracking and reconciliation after save |
| `src/arrange.rs` | Sheet order, duplication, rotation, blank pages, selection and a separate undo history |
| `src/model.rs` | Domain types, request/reply protocol, image utilities and egui texture handles |
| `src/worker.rs` | PDFium document ownership, reading, search, save orchestration, printing and fallback rendering |
| `src/pool.rs` and `src/helper.rs` | Render scheduling and separate PDFium processes |
| `src/app/gpu.rs` and `crates/gpu-lines` | Background shape reads, uploads, GPU drawing and thumbnails |
| `crates/markup-model` | UI-independent measurement geometry, identity, scales, layers and spatial indexing |
| `crates/pdf-io` | Measurement and scale persistence and appearances |
| `crates/pdf-content` | Shared PDF parsing and content operations |
| `crates/text-layout` | Text shaping and layout shared by display and persistence |

Opening starts a GPU reader and a worker request. The worker owns a byte snapshot and reports metadata; the UI creates a `Doc` with a new `Session`. Annotations arrive progressively, and measurements are read separately on demand.

Annotation edits pass through session commands. Sheet edits pass through `Arrangement`. Saving captures both, writes annotations before rearranging pages, replaces the file, then reloads the worker document. An ordinary save reconciles annotation identities in the existing session. An arrangement save instead causes the UI to reopen the document and create a new session.

## P1 Arrangement save can discard changes accepted while saving

**Evidence:** `src/app/mod.rs:931` captures the save; `src/app/mod.rs:1231` reopens after an arrangement save; `src/app/mod.rs:1014` creates a fresh session. Keyboard undo remains reachable during saving (`src/app/mod.rs:1330` and `src/app/mod.rs:1374`), and `src/app/notes.rs:295` directly changes the session. The saving guard in `src/app/arrange.rs:419` protects sheet mutations, not session undo.

**Scenario to reproduce:** add a markup, rotate a sheet, start saving, then undo the markup before the save reply arrives. The undo changes the live session after the snapshot was taken. Arrangement-save completion bypasses session reconciliation and replaces that session with the saved file. The accepted undo is lost. Ordinary saves explicitly preserve edits made during saving, making this difference particularly hard to reason about.

**Recommendation:** define one document-level rule for edits during structural saves. A focused initial fix is to reject all document mutations consistently until the structural save and refresh finish. Longer term, stable page identities and revision-aware reconciliation would permit edits to survive without reopening the entire editing session.

**Validation needed:** an application-level test that injects save replies around undo and another edit, covering both ordinary and arrangement saves. Existing view-preservation coverage in `src/app/mod.rs:1571` is a useful starting point.

## P1 Readers and saves lack a shared file revision contract

**Evidence:** the worker captures bytes at open (`src/worker.rs:685`), but lazy measurement reads reopen the path (`src/worker.rs:764`, `src/worker.rs:1177`). The GPU reader independently reads that path (`src/app/gpu.rs:538`). Save generation uses the worker's retained bytes and replaces the destination without checking whether its source changed (`src/worker.rs:807`).

**Consequence:** if another program replaces the PDF after it is opened, measurements can come from a different version than the worker's annotations and page metadata. Saving can overwrite the other program's changes with a transformation of the older snapshot. A document generation identifies an open operation; it does not establish that independent path reads saw identical content.

**Recommendation:** establish an authoritative document snapshot and revision. Give background readers that snapshot or a verified immutable backing file. Before overwriting the source, check for external changes and return a distinct conflict outcome. Preserve the performance advantages of streaming helper reads when choosing the backing-file approach.

**Validation needed:** replace the source after open, then request measurements and save. Assert a consistent snapshot and an explicit conflict instead of silent replacement. Destination overwrite policy for Save As should be specified separately.

## P1 Save failure conflates write failure with reload failure

**Evidence:** `src/worker.rs:833` completes file replacement before reloading it at `src/worker.rs:873`. If reload fails, the worker sets `loaded = None` and sends `SaveFailed` (`src/worker.rs:925`). The UI handles every `SaveFailed` by cancelling session save tracking and returning to idle (`src/app/mod.rs:1283`). Later save requests silently continue when no matching loaded document exists (`src/worker.rs:809`).

**Consequence:** a successful disk write followed by a reload error leaves an apparently editable UI with no worker document. Retrying save can leave the UI waiting without a terminal reply. The error text acknowledges the write, but the state machine handles it like a write that never happened.

**Recommendation:** model outcomes such as write rejected, write committed, and reload required explicitly. Preserve the committed revision and transition to a recoverable state when reload fails. Every accepted save request must receive a terminal outcome, including when worker state is missing.

**Validation needed:** inject a reload failure after successful replacement, then retry. Assert that the committed state is reported correctly and recovery or rejection completes deterministically.

## P2 Save completion does not carry the new cache identity to the UI

**Evidence:** the worker updates its fingerprint after save (`src/worker.rs:848`, `src/worker.rs:869`). `Reply::Saved` carries no fingerprint (`src/model.rs:529`), and the ordinary-save handler leaves `Doc.file` unchanged (`src/app/mod.rs:1227`). GPU thumbnail completion stores images using `doc.file` (`src/app/gpu.rs:987`).

**Consequence:** thumbnails produced after an ordinary save can be written under the pre-save content identity. This misses reuse for the newly saved file and can associate changed imagery with an older document version, including the retained source after Save As.

**Recommendation:** return the committed document revision in the save result and update all consumers together. Tag outstanding rendering work with the revision it actually rendered, so merely replacing `doc.file` cannot mislabel an old in-flight result. Make cache invalidation part of that revision transition.

**Validation needed:** save a visual edit, complete old and new thumbnail jobs in both orders, and verify their cache keys. Include Save As where the original file remains unchanged.

## P2 Document undo depends on the current view

**Evidence:** `Session` and `Arrangement` have separate histories. `src/app/measure.rs:450` gives sheet history priority only in sheet mode; otherwise it routes to shape construction or session history. `src/app/arrange.rs:516` documents this behavior. Arrangement-save refresh also creates a fresh session and discards its history.

**Consequence:** drawing an annotation, rotating a sheet, then changing zoom can change which edit Ctrl+Z reverses. Saving a sheet arrangement clears annotation history even though ordinary saves preserve it. This is implemented behavior, not evidence that the original design was accidental, but it is a product-level contract that needs an explicit decision.

**Recommendation:** prefer one chronological history for committed document changes, while keeping unfinished drawing gestures local to the tool. If contextual undo is intentional, expose its scope clearly and test mixed sequences and save boundaries. Do not merge histories mechanically without defining page identity and deletion semantics first.

## P2 UI modules share too much lifecycle and rendering state

**Evidence:** `Doc` (`src/app/mod.rs:140`) combines editing state, sheet arrangement, page metadata, pending requests, CPU/GPU caches and resource lifetimes. `App` (`src/app/mod.rs:495`) coordinates saves through fields such as `status`, `rearranged_on_save` and `refreshing_save`. Feature modules implement methods on the same `App` and commonly import `super::*`. Invalidation spans `src/app/markups.rs:81`, `src/app/gpu.rs:782`, the worker, and the pool. `src/model.rs` couples session-facing data to the egui rendering protocol.

**Consequence:** splitting files has not restricted who can mutate shared state. A new save or rendering feature must preserve conventions across several modules. The findings above are concrete reasons to introduce narrower ownership boundaries; file length alone is not the reason.

**Recommendation:** first extract a document controller that owns lifecycle transitions, revisions and mutation eligibility. Then separate editing state from render state, with explicit events for committed revisions and changed pages. Separate domain types from worker/render messages when doing this work. Keep the existing pure crates and helper-process design; there is no demonstrated need to replace them.

## Implementation progress

- [x] Capture save failure scenarios, fix mutation gating and ensure terminal save outcomes.
- [x] Carry committed revisions through save results, background readers and thumbnail caches.
- [x] Establish snapshot ownership and external-change detection.
- [x] Extract document lifecycle transitions into one controller state.
- [ ] Decide whether to change contextual undo and introduce stable page identity before changing structural-save reconciliation.
- [x] Separate editing state from rendering state and separate domain data from worker/render protocol types.

The repository already has focused session, worker, helper, save, arrangement, and cache tests, and CI builds and tests the workspace. Extend those around cross-component transitions rather than starting a broad rewrite. GPU presentation and performance still require application checks and the existing benchmark workflows after implementation.

This pass does not audit every PDF parser, geometry algorithm, printing backend, helper protocol failure, or memory budget. Those are follow-up review areas rather than implied clean bills of health.
