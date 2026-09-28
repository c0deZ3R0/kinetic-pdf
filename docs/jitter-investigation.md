# Scrolling jitter investigation — 25 September 2026

The reproducer is the local 137-sheet civil drawing PDF. This machine has Intel graphics and an NVIDIA RTX 5070 Ti Laptop GPU.

## Findings

1. **Backward jumps depended on the forced discrete-GPU path.** Matching baseline builds at 94fa864, with and without the high-performance GPU exports, produced 69 vs 0 detected reversals in 999 consecutive screen-capture pairs each. The logs confirmed NVIDIA vs Intel OpenGL respectively. Windows now chooses the adapter; normal OpenGL vsync remains enabled. The experimental DwmFlush call inside App::ui was removed.
2. **Page loading could stall the UI independently of presentation.** The existing work moves CPU upload preparation to the reader, splits uploads, reads thumbnails asynchronously, and caches heavy page drawing in GPU tiles. This review also bounds tile allocation and culled-run scanning by elapsed time, counts multisample attachments in tile memory, and gives stationary views more drawing time.
3. **Replacing page geometry discarded sharp tiles prematurely.** An 800% screenshot showed a sharp upper region and a blurry lower region immediately after a higher-density upload. Tile keys now include the upload ID. Complete tiles from the previous upload remain visible until replacement tiles cover the view, with deterministic fallback ordering. Before/after images are in tmp/jitter/fixed-shots/800.png and tmp/jitter/handover-shots/800.png.
4. **Readiness reporting ignored unfinished GPU tiles.** paint_page now returns completeness and contributes it to view_sharp.

## Validation

- cargo test --workspace --lib --offline --quiet: **438 passed, 3 ignored, 0 failed**.
- Release build and git diff --check pass. An existing unused-assignment warning in wait_for_shapes remains.
- Final release presentation check: **0 backward jumps in 1,499 capture pairs**, with Intel OpenGL and vsync enabled.
- Final full-document release scroll: 88.7 seconds, 5,405 frames, median 16.7 ms, p95 19.6 ms, worst 78 ms, **2 frames over 50 ms**. Upload handling p95 3.1 ms, worst 33 ms. Trace logging and capture diagnostics were enabled; peak process memory was 4,051 MB. Occasional stalls remain.
- Earlier Intel baseline: 75 frames over 50 ms, worst 207 ms. That baseline used the quick profile, so this is not a controlled quantitative speedup comparison.
- The heavy-sheet zoom/pan benchmark completes all 12 steps; screenshots were inspected at 200% and 800%. The final handover correction removes the observed blurry replacement region.

## Reproduction artifacts

Local diagnostics are under tmp/jitter/:
- run.ps1 launches only its own test process, captures scrolling, and reports detected reversals.
- bench.ps1 runs the full scroll or sheet work benchmark and writes timing logs/screenshots.
- baseline-nvidia.tsv, baseline-intel.tsv, final-release.tsv contain capture measurements.
- final-release.log, fixed-work.log, handover-work.log contain runtime measurements.

The capture uses screen-image correlation, not presentation frame IDs. It now prefers zero movement when correlation ties, avoiding a false reversal on blank/repeated content. Capture counts are not dropped-frame counts, and this investigation does not establish the underlying driver/compositor defect. The adapter result is specific to the tested machine; Windows or a user graphics preference can choose differently elsewhere.


The verified standalone executable was target/release/kinetic-pdf.exe. The fixes are included in version 0.9.3; the Store package is built separately with packaging/make-msix.ps1.
