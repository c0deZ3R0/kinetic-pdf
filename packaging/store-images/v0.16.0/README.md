# v0.16.0 LinkedIn assets

Saved drafts for review and testing; these have not been posted to LinkedIn.

## Ready-to-use files

- `linkedin-post.txt`: the performance challenge post, awaiting measurements from the testing computer.
- `release-image.png`: 1080 × 1350 release card with the original faint drafting grid.
- `ctrl-k-post.txt`: personal development-update caption for the Ctrl+K video.
- `ctrl-k-personal-update.mp4`: 13.5-second, 1080 × 1920 video with music, captions, and actual app footage.
- `ctrl-k-cover.png`: the video's first frame, showing only “s” in the search field.

The video holds that first-letter result, continues from the same recorded frame, selects the saved tool, and draws a new area measurement on a cleared sample plan. It ends on the completed measurement. The original sample PDF was not saved over.

The performance card covers v0.16.0 navigation, cache loading, and temporary memory improvements. The Ctrl+K search improvements shipped in v0.15.0 and are demonstrated here using v0.16.0. No numerical performance claims have been added yet.

## Edit and render

From `remotion/`:

```powershell
npm ci
npm run dev
npx remotion render CtrlKPromo out/kinetic-ctrl-k-linkedin.mp4 --codec=h264 --crf=18
npx remotion still CtrlKPromo out/kinetic-ctrl-k-linkedin-cover.png --frame=0
npx remotion still Release16 out/kinetic-pdf-v0.16.0-linkedin.png --frame=0
```

`remotion/out/` is ignored by Git. Copy approved exports into this directory when saving a revision for another computer.

Source compositions are in `remotion/src/Promo/CtrlKPromo.tsx` and `Release16.tsx`. The recording is `remotion/public/ctrl-k-tools.mp4`; the generated drawing image is `remotion/public/ctrl-k-drawing.png`. `ToolsRecording` previews the plain capture, and `QuickTools` preserves the earlier illustrated draft.

## Re-recording

`packaging/store-images/ctrl-k-demo.ps1` prepares a separate demo profile and a copy of the supplied PDF. Supply the sample PDF and corresponding tools JSON using `-Pdf` and `-Tools`, then record that app window with stream-recorder and run the script with `-Take`. Wait for “Demo complete” before stopping the recorder. The script uses a 1920 × 1080 client area and coordinates specific to the Kestrel Lane ground-floor drawing. It takes focus and drives the mouse and keyboard during the take.

## Testing on another computer

The remote branch is `store-images` (the author's local branch is named `worktree/store-images`). After fetching and checking out the remote branch, build the app from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File get-pdfium.ps1
cargo build --release --locked --bin kinetic-pdf
```

Rust, the MSVC build tools, and the Windows SDK are required; see `docs/DEVELOPMENT.md`. The executable is `target/release/kinetic-pdf.exe` and is not committed.

Record machine specs, PDF size/page count, whether the cache is cold or warm, and the version/commit used alongside any timings. Keep measured results separate from the draft copy until reviewed. The existing benchmark entry point is documented in `docs/DEVELOPMENT.md`.
