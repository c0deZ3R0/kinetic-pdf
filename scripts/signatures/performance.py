#!/usr/bin/env python3
"""OS-temp paired release worker benchmark; not GUI/startup or absence-proof testing.

PATH: scripts/signatures/performance.py
CONSUMED BY: developers/coordinator after the Cargo test gate. Prepare never runs Cargo.
Inputs are generated PDFs only. Snapshots use git archive, never worktrees/branches.
"""
import argparse
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import tarfile
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
HARNESS = Path(__file__).with_name("performance-worker.rs")
METRICS = ("opened_ms", "first_render_ms", "annotation_scan_ms", "full_open_ms", "analysis_ready_ms", "fully_interactive_ms", "post_render_analysis_wait_ms", "save_ms")
GATE = "No numerical absolute/relative pass/fail or equivalence budget has been agreed. Descriptive results only; no no-regression acceptance claim."


def require_compilers_idle():
    result = command(["tasklist.exe", "/FO", "CSV", "/NH"], capture_output=True, text=True)
    names = {row[0].lower() for row in csv.reader(result.stdout.splitlines()) if row}
    busy = names & {"cargo.exe", "rustc.exe", "cl.exe", "link.exe", "lld-link.exe"}
    if busy:
        raise SystemExit(f"Refusing measurement while compiler/linker processes exist: {sorted(busy)}")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(args, **kw):
    return subprocess.run([str(a) for a in args], check=True, **kw)


def git(*args):
    return command(["git", "-C", REPO, *args], capture_output=True).stdout


def fixture(path, pages, empty, text_field=False):
    field = 4 + pages * 2
    form = f" /AcroForm << /Fields [{field} 0 R] >>" if empty or text_field else ""
    if text_field:
        form = f" /AcroForm << /Fields [{field} 0 R] /DR << /Font << /Helv 3 0 R >> >> /DA (/Helv 10 Tf 0 g) >>"
    objects = [f"<< /Type /Catalog /Pages 2 0 R{form} >>".encode(),
               f"<< /Type /Pages /Count {pages} /Kids [{' '.join(f'{4 + p * 2} 0 R' for p in range(pages))}] >>".encode(),
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for p in range(pages):
        content = "BT /F1 10 Tf 50 740 Td 13 TL\n" + "\n".join(
            f"(Synthetic page {p + 1} row {row}: bounded unsigned workload.) Tj T*" for row in range(40)) + "\nET\n"
        content += "\n".join(f"{50 + n * 8} 50 m {50 + n * 8} 150 l S" for n in range(12))
        data = content.encode("ascii")
        annots = f" /Annots [{field} 0 R]" if text_field and p == 0 else ""
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {5 + p * 2} 0 R{annots} >>".encode())
        objects.append(f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream")
    if empty:
        objects.append(b"<< /FT /Sig /T (Empty unsigned signature field) >>")
    elif text_field:
        objects.append(b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (OrdinaryText) /V (Synthetic unsigned value) /Rect [50 50 250 75] /P 4 0 R >>")
    pdf = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for index, obj in enumerate(objects, 1):
        offsets.append(len(pdf))
        pdf.extend(f"{index} 0 obj\n".encode() + obj + b"\nendobj\n")
    xref = len(pdf)
    pdf.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        pdf.extend(f"{offset:010d} 00000 n \n".encode())
    pdf.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    path.write_bytes(pdf)


def protocol_adapter(protocol):
    header = "//! Generated protocol-only adapter for the shared headless workload.\n//! CONSUMED BY: signature-performance-worker.rs in an OS-temp snapshot.\n"
    imports = "use std::sync::mpsc::Sender;\nuse kinetic_pdf::protocol::{Reply, Request};\n"
    has_discovery = "\n    SignatureDiscovery {" in protocol
    has_release = "\n    StartSignatureAnalysis {" in protocol
    if has_discovery:
        ready = '''pub fn ready(reply: &Reply, generation: u64, file: Option<u64>) -> Result<bool, String> {
    match reply {
        Reply::SignatureDiscovery { generation: actual, file: revision, discovery }
            if *actual == generation && Some(*revision) == file => {
                if discovery.allows_document_mutation() { Ok(true) }
                else { Err(discovery.read_only_reason().unwrap_or("analysis not editable").into()) }
            }
        _ => Ok(false),
    }
}
'''
    else:
        ready = '''pub fn ready(reply: &Reply, generation: u64, file: Option<u64>) -> Result<bool, String> {
    Ok(matches!(reply, Reply::Opened { generation: actual, snapshot, .. }
        if *actual == generation && Some(snapshot.file()) == file))
}
'''
    if has_release:
        release = '''pub fn release(tx: &Sender<Request>, generation: u64, file: u64) -> Result<(), String> {
    tx.send(Request::StartSignatureAnalysis { generation, file }).map_err(|e| e.to_string())
}
'''
    else:
        release = '''pub fn release(_tx: &Sender<Request>, _generation: u64, _file: u64) -> Result<(), String> { Ok(()) }
'''
    description = "final-discovery" if has_discovery else "Opened-baseline-no-analysis-gate"
    if has_release:
        description += "+scheduling-only-release-after-first-render"
    return header + imports + ready + release, description


def prepare(args):
    root = Path(tempfile.mkdtemp(prefix="kinetic-signature-performance-"))
    archive = git("archive", "--format=tar", "HEAD")
    changed = git("diff", "--name-only", "HEAD").decode().splitlines()
    untracked = git("ls-files", "--others", "--exclude-standard").decode().splitlines()
    production = [p for p in changed + untracked if p.startswith(("src/", "crates/", "vendor/")) or p in ("Cargo.toml", "Cargo.lock", "build.rs")]
    digests = {}
    adapters = {}
    for side in ("baseline", "candidate"):
        tree = root / side
        tree.mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(tree, filter="data")
        if side == "candidate":
            for relative in production:
                source, target = REPO / relative, tree / relative
                if source.is_file():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source, target)
                    digests[relative] = sha(source)
                elif target.exists():
                    target.unlink()
        shutil.copy2(REPO / "pdfium.dll", tree / "pdfium.dll")
        shutil.copy2(HARNESS, tree / "signature-performance-worker.rs")
        adapter, description = protocol_adapter((tree / "src" / "protocol.rs").read_text(encoding="utf-8"))
        adapter_path = tree / "signature-performance-adapter.rs"
        adapter_path.write_text(adapter, encoding="utf-8")
        adapters[side] = {"sha256": sha(adapter_path), "kind": description}
        with (tree / "Cargo.toml").open("a", encoding="utf-8") as f:
            f.write('\n[[bin]]\nname = "signature-performance-worker"\npath = "signature-performance-worker.rs"\n')
    inputs = root / "inputs"
    inputs.mkdir()
    workloads = {}
    for name, pages, empty, text_field in (("plain-1", 1, False, False), ("plain-24", 24, False, False),
            ("plain-100", 100, False, False), ("empty-100", 100, True, False), ("text-100", 100, False, True)):
        path = inputs / f"{name}.pdf"
        fixture(path, pages, empty, text_field)
        workloads[name] = {"pages": pages, "empty_signature_field": empty, "ordinary_text_field": text_field,
                           "expected_form_type": 1 if empty or text_field else 0, "bytes": path.stat().st_size, "sha256": sha(path)}
    metadata = {"root": str(root), "head": git("rev-parse", "HEAD").decode().strip(), "candidate_overlay_sha256": digests,
                "harness_sha256": sha(HARNESS), "protocol_adapters": adapters, "pdfium_sha256": sha(REPO / "pdfium.dll"), "workloads": workloads,
                "platform": platform.platform(), "logical_cpus": os.cpu_count(), "prepared_at": time.time(),
                "latency_gate": GATE, "driver_sha256": sha(Path(__file__))}
    (root / "metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    print(root)


def build(args):
    if not args.cargo_approved:
        raise SystemExit("Refusing Cargo: wait for coordinator handoff, then explicitly pass --cargo-approved")
    root = args.root.resolve()
    target = root / "target"
    if args.seed_cache and not target.exists():
        target.mkdir()
        # Copies, not hard links: this cannot mutate the live repository cache.
        shutil.copytree(REPO / "target" / "release", target / "release", ignore=shutil.ignore_patterns("bench-data", "incremental"))
    output = root / "executables"
    output.mkdir(exist_ok=True)
    records_path = root / "builds.json"
    records = json.loads(records_path.read_text(encoding="utf-8")) if records_path.exists() else {}
    for side in ((args.side,) if args.side else ("baseline", "candidate")):
        cmd = ["cargo", "build", "--release", "--locked", "--offline", "-j2", "--manifest-path", root / side / "Cargo.toml",
               "--target-dir", target, "--bin", "signature-performance-worker"]
        start = time.time()
        with (root / f"build-{side}.log").open("w", encoding="utf-8") as log:
            command(cmd, stdout=log, stderr=subprocess.STDOUT, timeout=args.build_timeout)
        exe = output / f"{side}.exe"
        shutil.copy2(target / "release" / "signature-performance-worker.exe", exe)
        records[side] = {"command": [str(a) for a in cmd], "seconds": time.time() - start, "sha256": sha(exe),
                         "harness_sha256": sha(root / side / "signature-performance-worker.rs"),
                         "adapter_sha256": sha(root / side / "signature-performance-adapter.rs"),
                         "manifest_sha256": sha(root / side / "Cargo.toml")}
        print(side, records[side], flush=True)
    records["rustc"] = command(["rustc", "-Vv"], capture_output=True, text=True).stdout
    (root / "builds.json").write_text(json.dumps(records, indent=2), encoding="utf-8")


def probe(args):
    # Isolated native process, outside all worker timing. No crypto or validation.
    import ctypes
    root = args.root.resolve()
    metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
    dll = root / "candidate" / "pdfium.dll"
    library = ctypes.CDLL(str(dll))
    library.FPDF_InitLibrary.argtypes = []
    library.FPDF_InitLibrary.restype = None
    library.FPDF_LoadMemDocument64.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]
    library.FPDF_LoadMemDocument64.restype = ctypes.c_void_p
    library.FPDF_GetSignatureCount.argtypes = [ctypes.c_void_p]
    library.FPDF_GetSignatureCount.restype = ctypes.c_int
    library.FPDF_GetFormType.argtypes = [ctypes.c_void_p]
    library.FPDF_GetFormType.restype = ctypes.c_int
    library.FPDF_CloseDocument.argtypes = [ctypes.c_void_p]
    library.FPDF_CloseDocument.restype = None
    library.FPDF_DestroyLibrary.argtypes = []
    library.FPDF_DestroyLibrary.restype = None
    library.FPDF_InitLibrary()
    results = {"dll_sha256": sha(dll), "workloads": {}}
    try:
        for name, expected in metadata["workloads"].items():
            path = root / "inputs" / f"{name}.pdf"
            data = path.read_bytes()
            buffer = ctypes.create_string_buffer(data)
            doc = library.FPDF_LoadMemDocument64(buffer, len(data), None)
            if not doc:
                raise RuntimeError(f"PDFium could not load generated fixture: {name}")
            try:
                count = library.FPDF_GetSignatureCount(doc)
                form_type = library.FPDF_GetFormType(doc)
            finally:
                library.FPDF_CloseDocument(doc)
            required = 1 if expected["empty_signature_field"] else 0
            if sha(path) != expected["sha256"] or count != required or form_type != expected["expected_form_type"]:
                raise RuntimeError(f"Fixture provenance/hint mismatch: {name}: count={count}, form_type={form_type}")
            results["workloads"][name] = {"sha256": sha(path), "native_count": count, "native_form_type": form_type}
    finally:
        library.FPDF_DestroyLibrary()
    (root / "native-hints.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(json.dumps(results, indent=2))


def validate(root):
    metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
    builds = json.loads((root / "builds.json").read_text(encoding="utf-8"))
    native = json.loads((root / "native-hints.json").read_text(encoding="utf-8"))
    if metadata.get("latency_gate") != GATE:
        raise SystemExit("Missing predeclared latency-gate statement")
    if "protocol_adapters" not in metadata:
        raise SystemExit("Legacy measured experiment is preserved; prepare fresh snapshots for readiness instrumentation")
    if sha(HARNESS) != metadata["harness_sha256"]:
        raise SystemExit("Current harness differs from the one prepared/built; rebuild both snapshots")
    for side in ("baseline", "candidate"):
        if sha(root / "executables" / f"{side}.exe") != builds[side]["sha256"]:
            raise SystemExit(f"Executable differs from builds.json: {side}")
        if sha(root / side / "signature-performance-worker.rs") != metadata["harness_sha256"] or builds[side]["harness_sha256"] != metadata["harness_sha256"]:
            raise SystemExit(f"Copied snapshot harness changed: {side}")
        adapter_hash = sha(root / side / "signature-performance-adapter.rs")
        if adapter_hash != metadata["protocol_adapters"][side]["sha256"] or adapter_hash != builds[side]["adapter_sha256"]:
            raise SystemExit(f"Copied protocol adapter changed: {side}")
        if sha(root / side / "Cargo.toml") != builds[side]["manifest_sha256"]:
            raise SystemExit(f"Build manifest changed: {side}")
        if sha(root / side / "pdfium.dll") != metadata["pdfium_sha256"]:
            raise SystemExit(f"Copied DLL changed: {side}")
    for relative, digest in metadata["candidate_overlay_sha256"].items():
        if sha(REPO / relative) != digest or sha(root / "candidate" / relative) != digest:
            # The temp manifest has only the intentional extra benchmark bin.
            if relative != "Cargo.toml" or sha(REPO / relative) != digest:
                raise SystemExit(f"Production/snapshot changed: {relative}")
    suffix = '\n[[bin]]\nname = "signature-performance-worker"\npath = "signature-performance-worker.rs"\n'
    baseline_manifest = git("show", f"{metadata['head']}:Cargo.toml").decode().replace("\r\n", "\n")
    for side, original in (("baseline", baseline_manifest), ("candidate", (REPO / "Cargo.toml").read_text(encoding="utf-8"))):
        if (root / side / "Cargo.toml").read_text(encoding="utf-8") != original + suffix:
            raise SystemExit(f"Snapshot manifest has unexpected edits: {side}")
    if native["dll_sha256"] != metadata["pdfium_sha256"]:
        raise SystemExit("Native fixture probe used a different DLL")
    for name, workload in metadata["workloads"].items():
        if sha(root / "inputs" / f"{name}.pdf") != workload["sha256"] or native["workloads"][name]["sha256"] != workload["sha256"]:
            raise SystemExit(f"Fixture changed after native probe: {name}")
        if native["workloads"][name]["native_count"] != (1 if workload["empty_signature_field"] else 0) or native["workloads"][name]["native_form_type"] != workload["expected_form_type"]:
            raise SystemExit(f"Native count/form-type preflight failed: {name}")
    tracked = ["metadata.json", "builds.json", "native-hints.json", "baseline/Cargo.toml", "candidate/Cargo.toml"]
    return metadata, {p: sha(root / p) for p in tracked}


def qualify(args):
    root = args.root.resolve()
    _, files = validate(root)
    shutil.copy2(Path(__file__), root / "performance-driver-qualified.py")
    shutil.copy2(HARNESS, root / "performance-worker-template.rs")
    (root / "qualification.json").write_text(json.dumps({"driver_sha256": sha(Path(__file__)), "files": files}, indent=2), encoding="utf-8")
    print("Executable/input/harness/manifest/source hashes validated; driver qualification recorded")


def run(args):
    root = args.root.resolve()
    metadata, files = validate(root)
    qualification = json.loads((root / "qualification.json").read_text(encoding="utf-8"))
    if qualification != {"driver_sha256": sha(Path(__file__)), "files": files}:
        raise SystemExit("Driver/provenance changed since qualification; requalify before collection")
    if not args.quiet_window_confirmed:
        raise SystemExit("Wait for coordinator's quiet window, then pass --quiet-window-confirmed")
    labels = ("baseline-a", "baseline-b") if args.aa else ("baseline", "candidate")
    suffix = "-aa" if args.aa else ""
    env = dict(os.environ, KINETIC_PDF_CACHE="0", KINETIC_PDF_HELPERS="0", KINETIC_PDF_UPDATE="0", LOCALAPPDATA=str(root / "appdata"))
    require_compilers_idle()
    (root / f"run-provenance{suffix}.json").write_text(json.dumps({"driver_sha256": sha(Path(__file__)), "pairs": args.pairs,
        "iterations": args.iterations, "started_at": time.time(), "latency_gate": GATE, "aa_control": args.aa,
        "quiet_window_confirmed": True, "workload": args.workload,
        "worker_env": {k: v for k, v in env.items() if k.startswith("KINETIC_PDF_") or k == "LOCALAPPDATA"}}, indent=2), encoding="utf-8")
    records = []
    with (root / f"samples{suffix}.jsonl").open("w", encoding="utf-8") as raw:
        for name in metadata["workloads"]:
            if args.workload and name != args.workload:
                continue
            for pair in range(args.pairs):
                for side in (labels if pair % 2 == 0 else tuple(reversed(labels))):
                    require_compilers_idle()
                    scratch = root / f"scratch{suffix}" / name / str(pair) / side
                    executable = "baseline" if args.aa else side
                    result = command([root / "executables" / f"{executable}.exe", root / "inputs" / f"{name}.pdf", scratch, args.iterations],
                                     env=env, capture_output=True, text=True, timeout=120)
                    require_compilers_idle()
                    for line in result.stdout.splitlines():
                        row = dict(json.loads(line), workload=name, pair=pair, side=side)
                        records.append(row)
                        raw.write(json.dumps(row) + "\n")
                    raw.flush()
                    print(name, pair, side, flush=True)
    summarize(root, records, labels, suffix)


def summarize(root, records, labels, suffix):
    rng = random.Random(7193)
    lines = [f"# Headless RELEASE worker pairs: {labels[0]} versus {labels[1]}", "", "Warm filesystem cache; iteration 0 retained separately, not an OS-cold claim.",
             "Same generated inputs, worker source, helpers=none, disk cache=none, scale=1.0. Not GUI/GPU frame proof.",
             "Native bind/initialization finishes before opened_ms starts; native_initialization_ms is recorded separately, not a GUI/startup claim.",
             GATE,
             "p95 is nearest-rank latency over raw samples in each phase, NOT the confidence interval of a median.",
             "empty-100 is a separately reported unsigned-forms workload; its hinted parse cost is not excluded.",
             "Protocol adapters differ and are separately hashed: baseline readiness=Opened; candidate readiness=matching final discovery. Save awaits readiness, without fixed sleeps.",
             "analysis_ready_ms is headless gate readiness from Open, not measured GUI editability. post_render_analysis_wait_ms reports deferred work after the first render.",
             "fully_interactive_ms=max(first_render_ms, analysis_ready_ms); annotation scan completion is separate, and no GUI interaction is measured.", "",
             f"| Workload | Phase | n per variant | Metric | {labels[0]} median ms | {labels[1]} median ms | {labels[0]} p95 ms | {labels[1]} p95 ms | Paired delta ms | Bootstrap 95% median-delta interval ms |",
             "|---|---|---:|---|---:|---:|---:|---:|---:|---:|"]
    for name in sorted({r["workload"] for r in records}):
        for phase in ("first-in-process", "steady"):
            rows = [r for r in records if r["workload"] == name and (r["iteration"] == 0) == (phase == "first-in-process")]
            for metric in METRICS:
                paired = []
                values = {s: [] for s in labels}
                for pair in sorted({r["pair"] for r in rows}):
                    med = {}
                    for side in values:
                        group = [r[metric] for r in rows if r["pair"] == pair and r["side"] == side]
                        med[side] = statistics.median(group)
                        values[side].append(med[side])
                    paired.append(med[labels[1]] - med[labels[0]])
                bootstrap = sorted(statistics.median(rng.choices(paired, k=len(paired))) for _ in range(5000))
                tail = {}
                for side in values:
                    samples = sorted(r[metric] for r in rows if r["side"] == side)
                    tail[side] = samples[(len(samples) * 95 + 99) // 100 - 1]
                n = sum(r["side"] == labels[0] for r in rows)
                lines.append(f"| {name} | {phase} | {n} | {metric} | {statistics.median(values[labels[0]]):.4f} | {statistics.median(values[labels[1]]):.4f} | {tail[labels[0]]:.4f} | {tail[labels[1]]:.4f} | {statistics.median(paired):+.4f} | [{bootstrap[125]:+.4f}, {bootstrap[4874]:+.4f}] |")
    (root / f"summary{suffix}.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="phase", required=True)
    sub.add_parser("prepare")
    b = sub.add_parser("build")
    b.add_argument("--root", type=Path, required=True)
    b.add_argument("--cargo-approved", action="store_true")
    b.add_argument("--seed-cache", action="store_true")
    b.add_argument("--build-timeout", type=int, default=900)
    b.add_argument("--side", choices=("baseline", "candidate"), help="build one side under a separate outer tool timeout")
    p = sub.add_parser("probe")
    p.add_argument("--root", type=Path, required=True)
    q = sub.add_parser("qualify")
    q.add_argument("--root", type=Path, required=True)
    r = sub.add_parser("run")
    r.add_argument("--root", type=Path, required=True)
    r.add_argument("--pairs", type=int, default=12, choices=range(3, 31))
    r.add_argument("--iterations", type=int, default=4, choices=range(2, 11))
    r.add_argument("--aa", action="store_true", help="baseline executable on both sides, separate labelled control")
    r.add_argument("--workload", choices=("plain-1", "plain-24", "plain-100", "empty-100", "text-100"))
    r.add_argument("--quiet-window-confirmed", action="store_true")
    args = parser.parse_args()
    {"prepare": prepare, "build": build, "probe": probe, "qualify": qualify, "run": run}[args.phase](args)


if __name__ == "__main__":
    main()
