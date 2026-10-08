# MCP merge performance check — 2026-10-08

No meaningful navigation or save slowdown was observed in this comparison with
assistant access disabled. This is evidence for the tested workloads, not a
guarantee for every PDF or for an assistant issuing expensive commands.

## Builds and scope

- Baseline: `48dfab4`. Its `src`, `crates`, `Cargo.toml`, `Cargo.lock`, and `tests`
  match `origin/main` at `a6b256f`.
- Current runtime: `32b6b55`, including the three review fixes and the early
  return for completed, cancelled, or failed demos. Terminal demo progress is
  retained without constructing a control-state snapshot each frame.
- Both navigation executables were built with `cargo build --release --features
  store --bin kinetic-pdf`. Both use the same PDFium DLL. Engine harnesses use
  `cargo test --release --features store --test mcp_performance --no-run`.
- The local `review/mcp-only` branch starts at current `origin/main` and contains
  the MCP commits, fixes, and benchmark evidence. It excludes the older Store
  screenshots, drawing generators, and Remotion project inherited by the
  original feature branch. Its runtime source matches the measured current
  build. Use this narrower branch for an MCP-only PR. The original branch is
  retained, and this check does not merge either branch into main.

## Method

Windows, Ryzen 7 8845HS (16 logical CPUs), NVIDIA RTX 4060 Laptop GPU, driver
616.64, 2040×1208 viewer pixels at 150% scaling, paced frames.

The runner alternates baseline/current launch order and isolates settings and
cache directories. No builds ran during the recorded comparison. Navigation uses
the existing `KINETIC_PDF_WORK_BENCH=0` sequence: 12 zoom/pan steps from 100%
through 800% on the middle page, waiting for three sharp frames at each step.
It measures time to the app reporting a sharp view, not input-to-photon latency.
Each document gets one excluded warm-up pair and five measured pairs.

Documents: a generated 300-page text PDF with 50 lines per page, and the
303-page Heron Ridge civil drawing set. Both builds completed every sequence
without timeout or renderer fallback.

The engine harness runs three processes per build. Each operation discards its
first sample and retains 11, for 33 samples. Editing samples average 1,000
highlight-add/undo pairs on an otherwise empty session, with 300 or 3,000 labels
loaded into the new session model. Before MCP, labels lived outside Session.
Save samples reset the 300-page PDF, open it, then time the real worker's save
request through its acknowledgement, including atomic file replacement. The
structural case reverses the page order as well as adding an annotation.

An initial exploratory navigation comparison had one slow current text run and
used the baseline app produced by `cargo test`. The table below uses the complete
repeat with the baseline explicitly rebuilt by the normal release command; it
does not remove any measured runs from that repeat.

## Results

All timings are milliseconds. Editing rows are batch averages per add/undo pair.
Navigation rows pool 60 step timings per build; the steps are not independent
identical workloads. Frame rows report the median of five per-run medians.

| Operation | Baseline median | Current median | Baseline p95 | Current p95 |
| --- | ---: | ---: | ---: | ---: |
| Text zoom/pan step | 3.8 | 3.7 | 11.3 | 11.4 |
| Drawing zoom/pan step | 6.1 | 6.0 | 13.5 | 11.2 |
| Text frame interval | 0.92 | 0.90 | — | — |
| Drawing frame interval | 1.45 | 1.37 | — | — |
| Annotation save, 300 pages | 9.333 | 8.962 | 10.571 | 10.481 |
| Reorder and save, 300 pages | 12.799 | 12.674 | 15.067 | 15.489 |
| Edit/undo, 300 labels | 0.000282 | 0.001890 | 0.000491 | 0.002100 |
| Edit/undo, 3,000 labels | 0.000272 | 0.017112 | 0.000288 | 0.019115 |

The repeat supports no material navigation/save regression, not a speedup claim.
Label equality checks do introduce a real linear editing cost: about 0.0016 ms
at 300 labels and 0.0168 ms at 3,000 labels in this test. It is small relative to
frame budgets, but it is not zero overhead and could merit caching for documents
with substantially more pages.

Private memory at navigation completion was comparable, with overlapping ranges:
text median 845 → 847 MiB (baseline range 830–852, current 841–852), drawing
793 → 801 MiB (786–805 versus 793–805). This is not a peak-memory measurement.
The Store executable grew from 11,040,768 to 12,774,912 bytes (+1,734,144 bytes).
The MCP listener/runtime starts only when enabled in Settings.

Active MCP traffic, active demo playback, cold-start latency, and other hardware
are not covered by these numbers. The full workspace suite on the MCP-only branch
passed 664 tests, with five existing tests ignored, before adding the separately
ignored performance harness. The existing `gpu.rs` unused-assignment warning
remains unrelated to this change.

## Reproduction and evidence

Use PowerShell 7 and `scripts/compare-mcp-performance.ps1`. It takes
`-BaselineApp`, `-CurrentApp`, `-BaselineHarness`, `-CurrentHarness`, `-Drawing`,
`-OutputDirectory`, and optional `-Repeats` (default 5). Supply a fresh output
directory. Copy `tests/mcp_performance.rs` into the baseline checkout before
building its harness; its conditional label setup accommodates the older model.
The baseline emits an expected warning because its Cargo features predate `mcp`.
Snapshot each normal release app before building test harnesses, and place
`pdfium.dll` beside each executable. Keep builds out of measurement runs.

Raw timing samples are in [2026-10-08-mcp-performance.json](2026-10-08-mcp-performance.json).
This session's detailed logs and private benchmark checkouts are retained under
`target/mcp-benchmark/`; the recorded repeat is `comparison-2`.

SHA-256 inputs:

- Baseline app: `A6ADC49241D0B50B47393F7502D6401B3BD69560824AC32837BF201DC243B315`
- Current app: `DCDAB3A6F2EDD5F165D07D57FE6D82AFFF7E77893CFF00EE6153E3A90095F9D4`
- Heron Ridge PDF: `D11907D0C2459706246D936728E405B04B0C486FF85F8F5E882AE748CC40BB49`
- Generated text PDF: `7F84E5BEF585FDD2CF71B4B831B342E87125374709CE84D3B67EA76FF030105E`
