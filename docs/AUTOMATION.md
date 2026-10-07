# Application control, MCP, and demos

Kinetic exposes the live window through an optional MCP adapter. Enable **Settings →
Assistant access → Enable for this window**, then copy its Codex connection settings
into your local Codex `config.toml`. The credential changes each time access is enabled.
Use a separate MCP server name/port for each window. Port `0` chooses a free port.
The app does not edit your Codex configuration.

The copied configuration uses Streamable HTTP with an Authorization header, supported
by [Codex's MCP configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).
The client must reach this Windows user's `127.0.0.1`; a remote/cloud client or WSL
environment is not automatically connected to this listener. No public tunnel is provided.
Treat the copied credential as a password; keep it out of repositories and recordings.

## Architecture and boundaries

```
MCP JSON/schema/auth (src/mcp)
             ↓
bounded control queue and DTOs (src/control)
             ↓
UI-thread adapter (src/app/control)
             ↓
shared application actions, Session, Arrangement, tools, PDF worker
```

The command palette and control adapter reuse `src/app/actions.rs`. Document mutations
use normal history and save paths. Transport tasks never own or edit the document.
`src/demo` owns replay format/timing; its UI adapter owns presentation. `src/experiment`
owns experimental placement resolution; its UI adapter owns proposals and commits.
None of these modules owns video capture or encoding.

The `mcp` Cargo feature is included by default and can be excluded with
`--no-default-features`. Runtime access starts disabled, is per-window, and is not
remembered. The listener binds IPv4 loopback only, checks bearer credentials on every
request, restricts Host/Origin through the official Rust MCP SDK, and accepts at most
2 MiB per request. The app queue has 32 slots and starts at most eight requests per
frame. A dropped/expired/revoked queued request cannot execute. Revocation also cancels
the current demo and discards experimental proposals. Work already started, such as
a file save, finishes through the ordinary app lifecycle; revocation is not rollback.

## Protocol and command conventions

`kinetic_inspect` takes no arguments. `kinetic_control` takes a `Request`:

```json
{"command":{"command":"go_to_page","page":2},"target":null}
```

Responses contain `state`, optional typed `data`, and optional `operation`. Errors are
MCP tool errors with `error.code` and `error.message`. The generated tool schema is the
authoritative field list (`src/control/mod.rs`); unknown fields are rejected.

Inspect returns `state.document`, containing window instance, generation, and edit,
arrangement, and file revisions. Pass it unchanged as `target` on subsequent commands.
Writes require it. It becomes stale after edits, undo, structural changes, or a new
file. An error never authorizes silently rebasing a write: inspect and reconsider it.
Preferences such as named-tool configuration do not require a document target.

Page positions and search-result indices are **one-based displayed positions**.
Annotation/layer identifiers are strings; never infer their identity from row order.
PDF points use the unrotated page's user space, with bottom-left origin and crop offsets.
Pan, viewport, and scroll use egui logical pixels, independent of screen DPI. Zoom is
an absolute scale between `0.05` and `8.0`. `view_ready` means the app considers the
view sharp and has settled its navigation targets, not that a video recorder captured it.

| Area | Commands |
|---|---|
| State/read | inspect, list_pages, read_text, list_annotations, list_layers, list_tools, tool_schema |
| File | open (absolute PDF path), save, save_as (absolute path; explicit overwrite) |
| Document | set_page_label, insert_blank_page, create_layer, rename_layer, select_layer |
| Tools | configure_tool, select_tool, select_saved_tool |
| Navigation | go_to_page, set_zoom, pan, centre_on, zoom_to_region |
| Search | find, search_results, go_to_result |
| UI | invoke, palette_query, palette_choose |
| Notes | add_highlight, edit_note |
| Replay | once, run_demo, demo_status, cancel_demo, poll_operation |
| Experiments | experiment (requires separate local Settings opt-in) |

`invoke` includes quick access (Ctrl+K), command palette (Ctrl+Shift+P), find, go-to,
fit/zoom, page navigation, panels, tool selection, undo and redo. Palette selection
uses an exact, unambiguous visible label and the normal palette handler. File writes,
quit, paste/delete, printing/export, and file-opening palette choices require an explicit
command or completion by the user in the app; they cannot bypass the file-operation API.

`configure_tool` uses the existing tool-file settings model: list tools/schema first,
patch known settings, and explicitly set `replace` to update an existing name/group.
Names/groups identify saved tools. Tool preferences are persisted by the normal UI;
document writes are persisted only by save. Read-text results are limited to 100,000
characters. Annotations and search responses report whether their lazy read is complete.

### Asynchronous work and retries

Open/save/read-text/search can return a pending operation ID. Poll `poll_operation`
without a document target until `complete` or `failed`; read text/results afterwards.
History retains 64 recent operations. A save is complete only after any structural
reload finishes. Inspect after a timeout; a started operation may have completed.
Open refuses unsaved work rather than dismissing the app's discard dialog.
Save As without overwrite uses a no-replace commit even if another process creates
the destination after the initial check.

Wrap explicit writes in `once` when the caller might retry:

```json
{
  "command": {
    "command": "once",
    "key": "caller-unique-insert-001",
    "write": {"command":"insert_blank_page","at":2,"size":[595,842]}
  },
  "target": "REPLACE WITH THE OBJECT RETURNED BY INSPECT"
}
```

Resend the **identical request and original target** with that key to obtain its original
response without applying it twice. A changed payload/target with the same key is an
error. Only successful submissions are remembered; busy/validation errors may be retried.
This is bounded session memory (last 64 successful writes, max 256 KiB each), not a
durable exactly-once guarantee. After eviction, app restart, or uncertainty, inspect
the document before repeating a write. Duplicate pending saves return the same operation ID.

## Replay and video production

Open a sample copy and prepare the desired panels/tool preferences before recording.
Send `run_demo` with a version-1 script, such as
[`examples/demo/navigation.json`](../examples/demo/navigation.json). Or run
[`scripts/play-demo.ps1`](../scripts/play-demo.ps1) with the endpoint and a SecureString
credential. It initializes MCP, plays the script, polls completion, and cancels on exit.
Use the token value from the copied settings, without the `Bearer ` prefix.

Scripts support shared commands, pauses, sharp-view waits, smoothstep zoom/pan animation,
window sizing, captions, shortcut badges, and a pointer cue relative to the viewer.
The pointer cue is visual; no global input is synthesized. Use `wait_view` after
navigation and before a hold that must show a sharp page. Delayed rendering causes
a wait or a reported timeout rather than silently skipping ahead. Zoom animation keeps
the scroll origin specified by the script; it does not imply mouse-centred zoom.

Keyboard, pointer-button, wheel, touch, text, or paste input stops playback before its
next action. Document changes outside playback also fail the sequence. Other control
commands report busy while a demo runs, except inspect/status/cancel/operation polling.
Use `cancel_demo` for an immediate stop. Scripts cannot nest demos, replay wrappers, experiments, or blocking native file
dialogs. Use explicit file commands when a sequence needs to open/save a file. They leave any intentional document edits in normal undo history.

Capture the app window with your usual recorder, then use the existing Remotion project
for editing/encoding if desired. Start recording before playback and stop after
`demo_status` reports complete. Pin display scaling, fonts, window size, document,
preferences, and capture frame rate for repeatable footage. The player is clock-driven,
but page preparation and OS capture make this live playback rather than offline,
frame-exact rendering. Encoding and automated narration are outside this interface.

## Experimental markup placement

Enable **Allow experimental markup proposals for this window** locally. Version 1
supports drawing tools (pen, rectangle, ellipse, line, arrow) and highlights. Preview
accepts PDF points, displayed-page fractions (top-left origin, rotation/crop aware), or
an exact selectable-text quote and one-based occurrence (highlights only). Scanned PDFs
without selectable text need a geometric anchor; no OCR is implied.

Preview returns resolved geometry, an ID, and a snapshot of effective tool settings,
and displays an amber placement guide. It does not edit the document. The guide shows
placement bounds/path rather than the final styled appearance. A saved tool may supply
the proposal's configuration. Commit requires the exact preview ID and a current
document target; changed document/active layer, locked/hidden layer, invalid extent,
or a replaced/discarded proposal is rejected. A successful commit consumes the proposal,
uses normal Session history, and can be saved/undone. Discard abandons it.

Keep this API experimental while testing real drawings: compare coordinate approaches,
text accuracy, rotations/crop boxes, scanned/vector pages, zoom/DPI, preset/layer behaviour,
undo/redo, and saved appearance in other viewers. Do not build a permanent public markup
contract on the version-1 proposal format. Measurement/text-box automation and automatic
semantic placement are intentionally open future experiments.

## Validation and Store release

```powershell
cargo test --workspace
cargo test --no-default-features --lib
cargo check --features store --bin kinetic-pdf
powershell -ExecutionPolicy Bypass -File packaging/make-msix.ps1
```

`tests/control_live.rs` exercises actual authenticated HTTP, the live egui app/queue,
PDF worker, text/search, rotated-page highlighting, blank insertion, Unicode labels,
Save As, structural reload, file reopening, and the actual PowerShell demo runner. Library tests cover stale targets,
queue bounds/expiry, revoked queued requests, credentials/origins, retries, operation
failure, demo timing/readiness/cancellation, and experimental geometry/undo.

The package remains a full-trust desktop app with its existing `runFullTrust` capability;
see [Microsoft's packaged desktop runtime explanation](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes).
No helper executable, download-and-execute mechanism, public listener, or additional
manifest capability is introduced. MakeAppx validation/build success is not Store
certification. Before submission, review the current
[Store policies](https://learn.microsoft.com/en-us/windows/apps/publish/store-policies),
publish the updated privacy policy, and give reviewers the enable/disable instructions
and a sample document/script. No submission or installed-app replacement is part of
this implementation.

### Local validation record (8 October 2026)

- Workspace tests: 660 passed; five existing tests ignored.
- Library build with MCP excluded: 363 passed; two existing tests ignored.
- Live authenticated HTTP/PDF roundtrip and Windows PowerShell demo runner: passed.
- MCP transport/revocation tests: passed.
- Store release executable and unsigned MSIX: built and validated by MakeAppx.
- Dependency notices regenerated with the repository's cargo-about workflow.

The existing renderer warning in `src/app/gpu.rs` remains. Physical recording,
installed-package smoke testing, and Microsoft Store certification have not been
performed by these automated checks. The locally built package has not been installed
or submitted.
