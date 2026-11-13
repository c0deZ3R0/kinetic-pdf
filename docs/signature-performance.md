# Signature-protection performance check

This report preserves the original synchronous experiment and separately records the renewed deferred-analysis gate below. Results from different frozen candidates are not interchangeable.

**Latest outcome:** 480 deferred-candidate A/B samples + 240 A/A controls completed. Forms first-render intervals include zero, but steady render-plus-ready latency increases by about **1.59 ms (empty signature field)** / **1.48 ms (ordinary text form)**. No-form render/save positives also remain unresolved. **The overall no-measurable-regression gate is not passed**, and shipping `panic=abort` recovery is separately unqualified. See the renewed gate for full medians, p95, intervals and provenance.

## Predeclared scope and decision rule

- **No numerical absolute/relative pass/fail or equivalence budget has been agreed.** Results are descriptive. A confidence interval containing zero is not proof of no regression; no unsigned no-regression acceptance claim follows automatically.
- Compare frozen candidate against HEAD `6acefc7cb631450335fdc4462c166cc56b22dc6f`, using the same external worker harness compiled with each tree's **release** profile (`opt-level="s"`, fat LTO, one codegen unit). No quick/debug timing substitutes.
- `empty-100` is an unsigned-forms workload and remains a separate, highlighted result: positive native field hints require a bounded lopdf inventory even when all signature values are empty. Its cost must not be hidden by ordinary native-zero inputs.
- Headless engine/worker measurements only. Native library binding/initialization occurs **before** the `opened_ms` timer and is recorded separately. No GUI startup, GPU upload, frame latency, scrolling or physical/virtual-printer proof.

## Coverage and method

`src/bin/bench/main.rs` and `docs/DEVELOPMENT.md` document the existing CLI:

```text
cargo run --release --features bench --bin bench -- --label NAME --pages N --runs N --repeats N [file.pdf ...]
```

It times direct PDFium/annotation operations and can launch real-app renderer workloads. Its direct open/save calls bypass worker signature discovery and guarded save dispatch. Equal direct-engine timings would **not** establish unchanged signature-open overhead. Its normal report path is manifest-relative `bench-results/`; it is not run against the live checkout for this check.

Instead, `scripts/signatures/performance-worker.rs` is copied identically into two OS-temp snapshots and added as a developer-only binary in their temporary manifests. It uses real worker requests and a wildcard for candidate-only replies:

| Metric | Timer boundary |
|---|---|
| `opened_ms` | Request Open through Opened; candidate discovery runs before this reply |
| `first_render_ms` | Same Open request through the first complete page-0 Rendered reply, at scale 1.0 |
| `annotation_scan_ms` | Same Open request through Highlights(done=true) |
| `full_open_ms` | Both first rendering and annotation scan complete; not all app background work |
| `save_ms` | Same-source Save of one new highlight through Saved, including source conflict checks and atomic replacement |

Helpers are explicitly disabled, disk cache is None, no app update/UI lifecycle is started. Each iteration copies a generated fixture to a distinct scratch path **outside timing**, warming filesystem cache. Process-first samples and subsequent steady samples are reported separately; neither is called OS-cache-cold. No user PDFs are used or modified.

Completed collection: 12 alternating-order pairs per workload, 4 opens/saves per process (**384 samples**). Report median of process medians, paired median delta, bootstrap 95% interval of the median delta (5,000 deterministic resamples), and **separate nearest-rank p95 raw latency** per variant/phase. The paired delta need not equal the difference of separately reported medians. First-in-process has **n=12 per variant**: p95 is the maximum. Steady has **n=36 per variant** across 12 processes: its p95 is only the second-largest observation. Neither tail is well characterized by this small experiment; intervals are unadjusted for multiple metrics.

A separate 6-pair A/A control runs the **same HEAD executable on both sides**, using all four inputs and 4 iterations (**192 samples**); it is not mixed into candidate results. Its first/steady sample counts are 6/18 per side, so both p95 estimates are maxima. It ran later, not interleaved with A/B, and is a noise indicator rather than a correction factor.

Coordinator confirmed the qualification/signing worker had finished before the eligible collection and reserved a quiet window. No compiler/linker process was detected by checks before/after each subprocess. These checks cannot exclude unrelated OS, antivirus, storage, scheduling or between-check activity; no unrelated processes were killed.

## Reproducibility / execution record

- Coordinator released Cargo after full workspace (675 passed, 5 ignored), all-targets bench check, and separate Store check passed. Those correctness checks are not performance evidence.
- Temp experiment root: `C:\Users\flowcost-home\AppData\Local\Temp\kinetic-signature-performance-_3duum_v`.
- Baseline is `git archive HEAD`; candidate overlays current production source/manifest changes, including the new output-safety module. No branch/worktree/new agent cwd is created. Only the existing local `pdfium.dll` is copied in addition to archived/overlaid sources.
- `metadata.json` records changed production-file SHA-256 values, identical harness SHA-256, fixture sizes/hashes, PDFium hash, environment and the predeclared decision rule. Builds use explicit manifest paths and a shared **temp** target directory. Release-cache seeding copied the existing 1.1 GB release cache, never hard-linked it; shared dependency caching affects build time, not the measured runtime input paths.
- Machine: AMD Ryzen 9 5950X (16 cores/32 logical CPUs), Windows 11 build 26200, Rust 1.100.0 / LLVM 23.1.5, `x86_64-pc-windows-msvc`; repository static-CRT rustflags applied to both builds.
- HEAD build: **171.3 s**, executable SHA-256 `4bbceff7732dc96a3d199504e69bec5f4b67f38f29e346cc087ec9aa101ef8a0`. Candidate build: **100.8 s**, executable SHA-256 `9499ed778bdc6bf45548adf5fab8c1bfd9dc5733e07df858419f3e215f139a0a`. Identical harness SHA-256: `b5660b6406c7212f814ce4ee79af7cce90783aff2b7663eefd41854c77c7c466`.
- A separate stdlib ctypes process preflighted the existing DLL outside timing: **plain-1/24/100 each count 0; empty-100 count 1**. No Widget/P/Annots change was needed. DLL SHA-256: `79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118`; input hashes and counts are in `native-hints.json`. Candidate Save succeeds for empty-100, consistent with the positive-hint empty-field path remaining editable.
- `qualify` verifies actual executable hashes against `builds.json`, fixture hashes against metadata/preflight, both copied/current harnesses, manifests, DLLs and frozen candidate overlay. `qualification.json` binds those records to the current driver hash; `run` repeats validation and rejects stale driver/provenance. In-memory hash-substitution negative checks successfully rejected executable, fixture, copied-harness and driver mismatches without altering the actual experiment files.
- Driver: installed Python 3.12.15 at `C:\Users\flowcost-home\AppData\Roaming\uv\python\cpython-3.12.15-windows-x86_64-none\python.exe`. The `python`/`py` PATH aliases were unavailable; they were not installed or changed.

```text
<python> scripts/signatures/performance.py prepare
<python> scripts/signatures/performance.py build --root <temp-root> --cargo-approved --seed-cache --build-timeout 900
<python> scripts/signatures/performance.py probe --root <temp-root>
<python> scripts/signatures/performance.py qualify --root <temp-root>
<python> scripts/signatures/performance.py run --root <temp-root> --pairs 12 --iterations 4 --quiet-window-confirmed
<python> scripts/signatures/performance.py run --root <temp-root> --pairs 6 --iterations 4 --aa --quiet-window-confirmed
```

Both builds use `cargo build --release --locked --offline -j2 --manifest-path ... --target-dir ... --bin signature-performance-worker`. Build timeout is bounded per variant. `builds.json` records compiler version, commands, durations and executable hashes; logs, fixtures, scratch outputs, raw samples and summary remain in OS temp.

## Results: measurable unsigned-forms regression

**The user's broad no-measurable-unsigned-regression requirement is not established. Empty unsigned forms show a measurable regression in this workload.** Steady `empty-100` metadata-open median rises **1.307 → 2.972 ms (+127.5%)**; paired delta **+1.655 ms [1.442, 1.759]**. Cumulative first-render median rises **12.347 → 14.393 ms (+16.6%)**. This is consistent with the positive-hint inventory cost, not an isolated timing of lopdf itself. First-process empty-form save also has a positive paired interval; it is retained below, not dismissed.

Native-zero opening deltas have intervals spanning zero at both phases. This experiment did not resolve a reliable metadata-open regression for those three small synthetic fixtures; **that is not equivalence, zero-overhead, cold-open or whole-app proof**. Some render/save estimates are lower, but A/A and large save outliers make blanket speedup claims inappropriate.

All table entries are milliseconds. Median/p95 columns are `HEAD → candidate`; delta intervals are candidate-minus-HEAD. Full annotation-scan/full-open metrics remain in `summary.md` and all raw timings in `samples.jsonl`.

### Steady iterations (n=36 per variant, 12 paired processes)

| Workload | Metric | Median | Relative median change | p95 | Paired delta [95% interval] |
|---|---|---:|---:|---:|---:|
| plain-1 | Opened | 0.435 → 0.374 | −14.1% | 0.772 → 0.651 | −0.025 [−0.134, +0.072] |
| plain-1 | Open → render | 4.416 → 4.094 | −7.3% | 5.979 → 5.899 | −0.301 [−0.672, +0.097] |
| plain-1 | Save | 7.166 → 6.373 | −11.1% | 8.813 → 8.506 | −0.404 [−1.059, +0.265] |
| plain-24 | Opened | 0.659 → 0.626 | −4.9% | 0.840 → 0.905 | −0.032 [−0.111, +0.009] |
| plain-24 | Open → render | 6.334 → 5.912 | −6.7% | 8.187 → 6.937 | −0.185 [−0.588, −0.031] |
| plain-24 | Save | 10.878 → 10.027 | −7.8% | 13.610 → 12.353 | −1.257 [−1.722, −0.145] |
| plain-100 | Opened | 1.314 → 1.283 | −2.4% | 1.729 → 1.846 | +0.020 [−0.183, +0.117] |
| plain-100 | Open → render | 12.272 → 12.376 | +0.8% | 13.609 → 13.267 | +0.148 [−0.467, +0.792] |
| plain-100 | Save | 17.400 → 17.551 | +0.9% | 36.578 → 20.113 | +1.057 [−2.636, +1.754] |
| **empty-100** | **Opened** | **1.307 → 2.972** | **+127.5%** | **1.689 → 3.259** | **+1.655 [+1.442, +1.759]** |
| **empty-100** | **Open → render** | **12.347 → 14.393** | **+16.6%** | **13.677 → 15.431** | **+1.717 [+1.413, +2.665]** |
| empty-100 | Save | 18.549 → 18.320 | −1.2% | 23.555 → 22.875 | +0.036 [−0.271, +1.414] |

### First in process, native already initialized (n=12 per variant; p95=max)

| Workload | Metric | Median | Relative median change | p95 | Paired delta [95% interval] |
|---|---|---:|---:|---:|---:|
| plain-1 | Opened | 0.476 → 0.509 | +7.0% | 0.851 → 0.675 | +0.024 [−0.036, +0.079] |
| plain-1 | Open → render | 7.102 → 6.978 | −1.8% | 9.525 → 9.013 | +0.238 [−0.371, +0.858] |
| plain-1 | Save | 17.025 → 17.077 | +0.3% | 20.315 → 19.925 | +0.575 [−1.529, +2.414] |
| plain-24 | Opened | 0.675 → 0.772 | +14.4% | 0.874 → 0.940 | +0.050 [−0.087, +0.178] |
| plain-24 | Open → render | 8.369 → 8.351 | −0.2% | 10.255 → 11.506 | −0.082 [−1.118, +1.119] |
| plain-24 | Save | 20.145 → 20.186 | +0.2% | 24.756 → 22.094 | −0.619 [−2.749, +1.947] |
| plain-100 | Opened | 1.292 → 1.298 | +0.4% | 1.681 → 1.535 | +0.032 [−0.084, +0.094] |
| plain-100 | Open → render | 14.516 → 14.661 | +1.0% | 16.812 → 15.419 | +0.127 [−0.733, +0.473] |
| plain-100 | Save | 27.244 → 26.789 | −1.7% | **210.239 → 31.869** | −0.188 [−2.332, +2.611] |
| **empty-100** | **Opened** | **1.272 → 3.217** | **+152.9%** | **1.480 → 3.410** | **+1.870 [+1.733, +2.025]** |
| **empty-100** | **Open → render** | **14.547 → 16.952** | **+16.5%** | **15.428 → 18.459** | **+2.349 [+2.047, +2.699]** |
| empty-100 | Save | 27.433 → 29.877 | +8.9% | 30.258 → 32.849 | +2.276 [+0.043, +4.062] |

Native initialization is outside these timers: across 48 process starts per variant, median/p95 **15.137/17.596 ms HEAD**, **15.192/16.615 ms candidate**. This covers embedded-DLL check/bind, not window/application startup.

### A/A control and noise

Same baseline executable, later quiet window, separate `samples-aa.jsonl` / `summary-aa.md`. Steady metadata-open paired deltas:

| Input | Paired A/A delta [95% interval], ms | A/B delta for comparison, ms |
|---|---:|---:|
| plain-1 | −0.014 [−0.153, +0.097] | −0.025 |
| plain-24 | +0.001 [−0.073, +0.123] | −0.032 |
| plain-100 | −0.070 [−0.259, +0.030] | +0.020 |
| empty-100 | +0.012 [−0.245, +0.132] | **+1.655** |

The forms-open effect exceeds the observed A/A metadata noise; this is still not a production-population causal/equivalence test. Tail instability is substantial: A/A empty-100 steady save maximum/p95 is **489.606 ms** on one side; plain-24 first-save maximum/p95 is **150.579 ms**. A/B includes a **210.239 ms** HEAD first-save maximum for plain-100. All remain in raw data/p95; no trimming or winsorization was performed. Their causes were not isolated.

### Failures, exclusions and remaining limits

- Both release builds and every eligible worker open/render/save completed. No compiler-idle or provenance mismatch occurred during eligible collection. Driver mismatch negative checks were separate and deliberately simulated.
- An earlier collection completed before the explicit backend-quiet confirmation and stronger provenance enforcement. It is retained only under `initial-unqualified/`, excluded for those methodological reasons—not because of its timings.
- The combined 300-second outer A/B+A/A command completed A/B but interrupted A/A after 128 samples. That incomplete control remains under `partial-aa-timeout/`; **only A/A** was rerun under a separate 360-second bound to completion. No partial control is treated as complete, and A/B was not rerun to select a favorable result.
- Fixtures are only 3 KB / 70 KB / 293 KB uncompressed text/vector PDFs, not representative large drawing/image sets. CPU/IO contention outside the checks, allocator/process-cache state, power/frequency behavior and antivirus are uncontrolled. OS-cache-cold, real-window, GPU, large-file, adversarial-PDF and provider/crypto workloads are unmeasured.
- No crypto/network activity is introduced by this ordinary-worker workload. In this original synchronous candidate, native-zero opens had no extra structural parse; empty-field forms did. The subsequent form-category fix below also inventories native-zero AcroForms. Neither structural reasoning nor these timings establish zero overhead.
- **Do not declare the overall no-measurable-unsigned-regression requirement passed.** Report the measured forms regression and seek an agreed latency budget/next decision before any follow-up production change. Production remained frozen during this experiment; no stage/commit/push was performed.

## Transition to deferred inventory (separate experiment below)

After the measured regression, a separately assigned follow-up moved bounded inventory to a private latest-job service, kept pending worker/UI state Unknown, and added checked form-category discovery at native count zero (the signature API misses nested/inherited fields). Worker renders release analysis. The app sends a one-shot scheduling-only release after the first usable matching helper render while pending; final discovery ends scheduling. Headless/no-render opens retain a 100 ms fallback. This moves work and delays editability; it does not eliminate the parse.

The original snapshots, binaries, raw data and summaries above remain evidence for the **synchronous** candidate. Its exact qualified driver and harness source were also archived under `performance-driver-qualified.py` and `performance-worker-measured.rs`, with matching original qualification hashes, before instrumentation changed. They are not performance proof for the new patch.

The follow-up harness (subsequently built and measured in the renewed gate below) uses a shared Rust workload with separately generated/hashed protocol adapters: HEAD readiness is Opened (no analysis API), whereas the candidate requires final, matching, mutation-allowed `SignatureDiscovery`. When supported, its adapter sends only the scheduling-release request after first render if still pending. These adapters—and therefore the complete compiled inputs—are **not identical**, even though the central workload is shared. Per-adapter kind/hash and build-time manifest/harness/adapter hashes are recorded and enforced.

New metrics are `analysis_ready_ms` (Open request → actual headless gate readiness) and `post_render_analysis_wait_ms` (remaining readiness delay after first render). Save timing starts only after actual readiness plus scan/first-render completion; there are no fixed sleeps. These are not real-window time-to-editability measurements. Fresh frozen snapshots and a renewed correctness/Cargo/quiet-window gate are required; legacy experiments deliberately fail the revised driver's stale-experiment checks. Python CLI/adapter-generation checks preceded the renewed gate below. The original synchronous results above remain unchanged; no no-regression acceptance claim is made.

## Renewed deferred-analysis release gate: completed, no-regression not established

The coordinator handed over frozen async sources after app/lib 385 passed (2 ignored), full workspace 685 passed (5 ignored), signature safety 7/7, all-targets/bench and Store checks passed, with 21 production hashes unchanged during those checks. These are correctness results, not performance evidence.

Predeclared decision rule remains **no numerical absolute/relative pass/fail or equivalence budget agreed**. Report all workloads, tails and paired intervals; do not infer no regression from an interval containing zero. Collection plan remains 12 alternating A/B pairs × 4 iterations per process, followed by a separate 6-pair baseline A/A control. Each build and collection gets a separate bounded outer timeout; no Cargo/compiler/signing run may overlap collection.

Fresh root: `C:\Users\flowcost-home\AppData\Local\Temp\kinetic-signature-performance-2q6j_yy5`. Baseline remains HEAD `6acefc7cb631450335fdc4462c166cc56b22dc6f`; old synchronous evidence stays in `_3duum_v`. Manifests/sources, copied DLL/harness, generated adapter kinds/hashes, executable hashes and driver qualification are recorded and checked before each collection. Shared workload source is common; API adapters differ explicitly (HEAD ready at Opened; candidate ready only on final matching discovery, with scheduling-only release if needed). No fixed readiness sleeps.

Separate native preflight confirmed:

| Generated workload | Native signature count | Native form type |
|---|---:|---:|
| plain-1 / plain-24 / plain-100 | 0 | 0 (None) |
| empty-100 | 1 | 1 (AcroForm) |
| text-100: ordinary populated text field/widget, no signature field | 0 | 1 (AcroForm) |

The first four fixtures retain the original hashes. New text-100 SHA-256: `cf8b636e4dcf0c423baeecdcc11dc3e5ccc859da2cf12b66bccd3cd2d91cbf0a`. Count/form category expectations and actual DLL/input hashes are enforced by provenance checks.

Metrics additionally include `analysis_ready_ms` and `fully_interactive_ms = max(first_render_ms, analysis_ready_ms)`, plus `post_render_analysis_wait_ms`. Fully-interactive is a **headless render-plus-edit-gate proxy**, not GUI interaction/frame latency. Annotation-scan/full-open completion remains separate. Save starts after actual readiness and scan/first-render completion, so delayed cost cannot disappear into a fixed sleep or the save timer.

### Latest execution/provenance

- Same Windows 11 / Ryzen 9 5950X / Rust 1.100.0 machine and release profile as above; native binding remains outside Opened timing. Cache/helper settings and warm-filesystem/process-first distinctions are unchanged.
- Separate bounded builds: HEAD **156.6 s**, candidate **89.4 s** (`--release --locked --offline -j2`, explicit snapshot manifests/shared temp target, 900 s internal limit and separate 1,000 s outer calls). Same copied release-dependency cache strategy; no live cache hard links.
- HEAD exe SHA-256: `3373bdb3aed62dcbdd2dc4ef67869f79bf59f534a413df31b8d59bab8359f333`; candidate: `50956529bb5dd15bc48597478135668a0e01b8400f0ea40fb9216736e8e8064f`.
- Shared workload SHA-256: `6c32a3fcac4b0503c438d91327f309366f15a86f7fb32ec46992ea5f9fe70096`. HEAD adapter: `df2805da0f6092e958f8dcf66ed1f92a4d980c8f4643f71e8f80ba10c73335ad`; candidate adapter: `41639787df63a64a191a5b5ae056710b2cc50e16f2a6c87d087253f639010f52`. Adapter/API differences are intentional and recorded, not claimed identical.
- Qualified driver and shared workload are archived in the new root as `performance-driver-qualified.py` and `performance-worker-template.rs`. `metadata.json`, `builds.json`, `native-hints.json`, `qualification.json`, and `run-provenance{,-aa}.json` retain full inputs, commands, environment, hashes and sample configuration.
- A/B completed **480 samples**, with 12 paired processes per workload/variant and 4 iterations each. A/A completed **240 separate baseline-only samples**, 6 pairs per workload. Runs used separate **600 s / 420 s** outer timeouts; neither timed out. No interrupted/excluded run is part of this renewed gate.
- Provenance and compiler-idle checks passed before both collections and between sample processes; post-collection production/input/executable/adapter/harness validation also passed. Coordinator confirmed no competing signing/native builds during the reserved quiet window. No production/Cargo source was changed. CPU/Cargo priority was released immediately after collection.

### Latest conclusion

**Deferral moves cost; it does not satisfy the overall no-measurable-unsigned-regression requirement.** For both unsigned-form workloads, Opened and first-render paired intervals include zero in this run, unlike the old synchronous form-render result. But final readiness and the render-plus-ready proxy are measurably delayed:

- Empty signature form, steady: ready **1.107 → 12.862 ms**; render-plus-ready **11.283 → 12.862 ms (+14.0%)**, paired **+1.594 ms [1.437, 1.726]**.
- Ordinary non-signature text form, steady: ready **1.172 → 12.961 ms**; render-plus-ready **11.495 → 12.961 ms (+12.8%)**, paired **+1.479 ms [1.311, 1.631]**.
- Candidate post-render analysis wait: steady median/p95 **1.530/1.657 ms** for empty-100 and **1.528/1.796 ms** for text-100; baseline is zero. First-process waits are **1.632/1.824 ms** and **1.636/1.845 ms**. This is actual deferred readiness cost, not a hidden sleep.
- Additional potentially regressed no-form metrics remain visible: plain-100 first-process render **+0.125 ms [0.039, 0.235]** and steady Save **+0.926 ms [0.255, 1.473]**. A/A shows larger uncertainty/noise for these metrics, so causal attribution is unresolved; the positive A/B intervals are not ignored or declared passes.

All following entries are ms, with `HEAD → candidate` median/p95. Paired delta is the median of per-process differences, not necessarily the difference of the displayed medians. Bootstrap intervals are unadjusted across 80 metric/phase/workload combinations; they are descriptive, not simultaneous equivalence bounds.

### Latest steady results (n=36 raw samples per variant; 12 paired processes)

p95 is nearest rank 35/36 (second largest), not a confidence interval; repeated samples within a process are not independent tail replicates.

| Input | Metric | Median | p95 | Paired delta [95% interval] |
|---|---|---:|---:|---:|
| plain-1 | Opened | 0.282 → 0.288 | 0.335 → 0.410 | +0.009 [−0.025, +0.026] |
| plain-1 | Open → render | 3.344 → 3.388 | 3.697 → 3.824 | −0.026 [−0.104, +0.185] |
| plain-1 | Analysis ready | 0.282 → 0.289 | 0.335 → 0.411 | +0.010 [−0.024, +0.027] |
| plain-1 | Render + ready | 3.344 → 3.388 | 3.697 → 3.824 | −0.026 [−0.104, +0.185] |
| plain-1 | Save after ready | 4.292 → 4.296 | 4.591 → 4.922 | −0.019 [−0.113, +0.101] |
| plain-24 | Opened | 0.513 → 0.516 | 0.585 → 0.585 | +0.001 [−0.047, +0.057] |
| plain-24 | Open → render | 5.303 → 5.239 | 5.598 → 5.574 | −0.052 [−0.110, +0.052] |
| plain-24 | Analysis ready | 0.513 → 0.518 | 0.585 → 0.587 | +0.003 [−0.046, +0.058] |
| plain-24 | Render + ready | 5.303 → 5.239 | 5.598 → 5.574 | −0.052 [−0.110, +0.052] |
| plain-24 | Save after ready | 7.331 → 7.387 | 8.307 → 8.050 | −0.033 [−0.229, +0.337] |
| plain-100 | Opened | 1.111 → 1.155 | 1.240 → 1.304 | +0.036 [−0.022, +0.062] |
| plain-100 | Open → render | 11.310 → 11.286 | 11.879 → 11.747 | +0.008 [−0.128, +0.129] |
| plain-100 | Analysis ready | 1.110 → 1.157 | 1.240 → 1.306 | +0.038 [−0.020, +0.064] |
| plain-100 | Render + ready | 11.310 → 11.286 | 11.879 → 11.747 | +0.008 [−0.128, +0.129] |
| plain-100 | **Save after ready** | **14.283 → 15.072** | **16.468 → 17.803** | **+0.926 [+0.255, +1.473]** |
| empty-100 | Opened | 1.107 → 1.121 | 1.384 → 1.329 | +0.006 [−0.042, +0.065] |
| empty-100 | Open → render | 11.283 → 11.344 | 11.732 → 11.982 | +0.076 [−0.090, +0.261] |
| empty-100 | **Analysis ready** | **1.107 → 12.862** | **1.384 → 13.536** | **+11.725 [+11.624, +11.839]** |
| empty-100 | **Render + ready** | **11.283 → 12.862** | **11.732 → 13.536** | **+1.594 [+1.437, +1.726]** |
| empty-100 | Save after ready | 15.818 → 14.733 | 16.905 → 17.682 | −0.643 [−1.667, +0.578] |
| text-100 | Opened | 1.172 → 1.115 | 1.276 → 1.271 | −0.070 [−0.112, +0.021] |
| text-100 | Open → render | 11.495 → 11.428 | 12.153 → 11.774 | −0.067 [−0.222, +0.122] |
| text-100 | **Analysis ready** | **1.172 → 12.961** | **1.276 → 13.572** | **+11.813 [+11.666, +11.947]** |
| text-100 | **Render + ready** | **11.495 → 12.961** | **12.153 → 13.572** | **+1.479 [+1.311, +1.631]** |
| text-100 | Save after ready | 15.047 → 14.817 | 16.589 → 17.071 | −0.259 [−1.028, +0.821] |

### Latest first-in-process results (native already bound; n=12, p95=max)

These remain warm-filesystem first-document samples, not OS-cold or window-startup timings.

| Input | Metric | Median | p95 | Paired delta [95% interval] |
|---|---|---:|---:|---:|
| plain-1 | Opened | 0.359 → 0.362 | 0.489 → 0.422 | −0.001 [−0.012, +0.020] |
| plain-1 | Open → render | 5.566 → 5.643 | 6.780 → 5.860 | +0.002 [−0.140, +0.157] |
| plain-1 | Analysis ready | 0.355 → 0.363 | 0.480 → 0.423 | +0.003 [−0.007, +0.025] |
| plain-1 | Render + ready | 5.566 → 5.643 | 6.780 → 5.860 | +0.002 [−0.140, +0.157] |
| plain-1 | Save after ready | 12.661 → 12.620 | 16.261 → 16.561 | +0.034 [−0.284, +0.646] |
| plain-24 | Opened | 0.557 → 0.553 | 0.592 → 0.846 | −0.004 [−0.020, +0.007] |
| plain-24 | Open → render | 7.535 → 7.485 | 7.883 → 9.918 | 0.000 [−0.276, +0.365] |
| plain-24 | Analysis ready | 0.554 → 0.555 | 0.589 → 0.848 | 0.000 [−0.016, +0.012] |
| plain-24 | Render + ready | 7.535 → 7.485 | 7.883 → 9.918 | 0.000 [−0.276, +0.365] |
| plain-24 | Save after ready | 15.904 → 15.999 | 16.841 → 17.079 | +0.280 [−0.462, +0.521] |
| plain-100 | Opened | 1.178 → 1.172 | 1.480 → 1.361 | −0.010 [−0.036, +0.032] |
| plain-100 | **Open → render** | **13.746 → 13.871** | **14.070 → 14.299** | **+0.125 [+0.039, +0.235]** |
| plain-100 | Analysis ready | 1.175 → 1.173 | 1.477 → 1.362 | −0.004 [−0.031, +0.037] |
| plain-100 | Render + ready | 13.746 → 13.871 | 14.070 → 14.299 | +0.125 [+0.039, +0.235] |
| plain-100 | Save after ready | 22.510 → 23.625 | 29.467 → 25.690 | +0.536 [−0.246, +1.876] |
| empty-100 | Opened | 1.160 → 1.184 | 1.296 → 1.247 | −0.004 [−0.016, +0.029] |
| empty-100 | Open → render | 13.765 → 13.812 | 14.263 → 14.587 | +0.047 [−0.137, +0.222] |
| empty-100 | **Analysis ready** | **1.157 → 15.435** | **1.294 → 16.238** | **+14.274 [+14.171, +14.431]** |
| empty-100 | **Render + ready** | **13.765 → 15.435** | **14.263 → 16.238** | **+1.687 [+1.528, +1.901]** |
| empty-100 | Save after ready | 23.568 → 24.924 | 27.915 → 26.619 | +0.160 [−0.687, +3.370] |
| text-100 | Opened | 1.169 → 1.180 | 1.215 → 1.232 | +0.010 [−0.012, +0.033] |
| text-100 | Open → render | 14.041 → 14.054 | 15.086 → 14.393 | +0.171 [−0.286, +0.265] |
| text-100 | **Analysis ready** | **1.165 → 15.704** | **1.213 → 15.992** | **+14.529 [+14.331, +14.727]** |
| text-100 | **Render + ready** | **14.041 → 15.704** | **15.086 → 15.992** | **+1.790 [+1.379, +1.890]** |
| text-100 | Save after ready | 24.359 → 24.934 | 24.798 → 39.757 | +0.606 [−0.718, +2.332] |

Native initialization/check/bind remains outside the tables: 60 process starts per variant, median/p95 **13.121/13.892 ms HEAD**, **13.142/13.895 ms candidate**. No real-window startup is measured.

### Latest controls, remaining uncertainty and decision

A/A uses the same HEAD executable on both sides; n=6 first/n=18 steady per side (both nearest-rank p95 estimates are maxima). It ran separately after A/B, not as a simultaneous adjustment. All 80 A/A rows, with medians/p95 and intervals, remain in `summary-aa.md`.

| Metric / phase | A/B paired delta [95% interval], ms | A/A paired delta [95% interval], ms |
|---|---:|---:|
| empty-100 render, steady | +0.076 [−0.090, +0.261] | −0.112 [−0.677, +0.302] |
| empty-100 render + ready, steady | **+1.594 [+1.437, +1.726]** | −0.112 [−0.677, +0.302] |
| text-100 render, steady | −0.067 [−0.222, +0.122] | −0.035 [−0.233, +0.111] |
| text-100 render + ready, steady | **+1.479 [+1.311, +1.631]** | −0.035 [−0.233, +0.111] |
| plain-100 render, first | **+0.125 [+0.039, +0.235]** | −0.023 [−1.356, +0.663] |
| plain-100 save, steady | **+0.926 [+0.255, +1.473]** | +0.835 [−1.797, +3.209] |
| text-100 save, steady | −0.259 [−1.028, +0.821] | **+1.635 [+0.509, +3.093]** |

A/A itself has a positive steady text-save interval despite identical code, demonstrating drift/noise/false-positive risk. A/A plain-24 steady save maximum/p95 is **102.535 ms**; A/B maximum Save is candidate text-100 first-process **39.757 ms**. No tails were trimmed. This does not erase observed A/B positive intervals; it limits causal/no-regression conclusions.

Every eligible open/render/readiness wait/save completed, including ordinary text forms and empty signature fields. No fixed sleeps, Unknown masking, compiler-check failures or timed-collection timeouts occurred. Original synchronous, unqualified and partial-control evidence is unchanged in its separate root.

Limits remain: tiny generated files, warm OS cache, no real UI/GPU/helper process or user interaction measurement, small/clustered samples, no cold-open/large-drawing/provider/crypto benchmark, and uncontrolled OS/power/storage/antivirus effects. The app's helper scheduling hook has correctness-test evidence, not a helper-enabled latency measurement here. The 100 ms no-render scheduling fallback was not exercised by this render-triggered benchmark; no claim is made that all headless/no-render edit-readiness latencies equal these values.

### Release panic/recovery qualification limitation

The root shipping release profile uses **`panic = "abort"`**. Therefore the service's `catch_unwind` does **not** convert a shipping parser panic into Unknown: the process would abort. The debug/unwind unit test demonstrates only that build's behavior; source/header claims that all panics become Unknown overstate release recovery. No parser panic occurred in this benchmark, and ordinary Result errors/pending states remain conservatively locked.

These successful timings do not qualify release crash containment, hostile-parser resilience or preservation of unrelated unsaved work on process abort. A **process-isolation/release-recovery gate remains open**. The frozen binaries, panic profile and service implementation were not changed during collection; header/documentation narrowing is a separately coordinated follow-up, not a global profile change or helper refactor hidden in this experiment.

After collection, the coordinator narrowed only the service header/catch-site comments
about unwind versus abort semantics. No executable statements or panic profile changed.
The recorded snapshots/hashes above remain the measured source; this comment-only cleanup
is not a new performance qualification or a crash-isolation fix.

**Gate result: overall no-measurable-unsigned-regression remains unproven.** Deferred inventory improves the specific first-view critical path descriptively but adds measurable forms edit-readiness/render-plus-ready latency, with additional unresolved positive no-form metrics and an open release-recovery limitation. Report and agree the acceptance decision before any further production work; do not silently reinterpret readiness as free or successful measurements as blanket release readiness. Production stayed frozen, no signing scope was added, and no stage/commit/push was performed.
