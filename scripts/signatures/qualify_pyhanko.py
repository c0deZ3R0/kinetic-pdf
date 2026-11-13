#!/usr/bin/env python3
# Optional isolated pyHanko qualification; never an application dependency.
#
# PATH: scripts/signatures/qualify_pyhanko.py
# CONSUMED BY: developers and docs/signature-backend-qualification.md.
# Creates synthetic identities/documents only, in a private OS temporary directory.
# Installs hash-locked wheels into a temporary uv venv; prints no key/PDF contents.
# All signing/validation is offline after setup; local test TSA is not a real TSA.

import argparse
import hashlib
import io
import json
import logging
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

VERSION = "0.38.1"
WHEEL_SHA256 = "723fdf1a6e27be72d0d7fbf65deccf03aeaa6fafbdddc5c698b6b2eba06c543d"
SOURCE_SHA256 = "27348b2bb5c6756402102a930b3404a1ccce4d1102f81e56399df268dd90f70d"
SOURCE_URL = "https://files.pythonhosted.org/packages/89/d6/dd2189158321cd5a4326a4d82121cce8d0a1a8037deb5cdabab2e8f32d4a/pyhanko-0.38.1.tar.gz"
CONFIG = Path(__file__).resolve().parent / "pyhanko-qualification"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def command(args, *, timeout=180, env=None):
    result = subprocess.run([str(a) for a in args], capture_output=True, timeout=timeout, env=env)
    require(result.returncode == 0, "Command failed: " + Path(str(args[0])).name)
    return result.stdout


def secure_directory(path):
    if os.name == "nt":
        import csv
        row = next(csv.reader(io.StringIO(command(["whoami", "/user", "/fo", "csv", "/nh"]).decode(errors="replace"))))
        sid = row[-1].strip()
        require(sid.startswith("S-1-"), "Could not identify temp-directory owner")
        command(["icacls", path, "/inheritance:r", "/grant:r", f"*{sid}:(OI)(CI)F"])
        return "Windows ACL: inheritance removed, current SID full control"
    path.chmod(0o700)
    return "POSIX directory mode 0700"


def private_write(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)


def signed_content(data, ranges):
    """Reconstruct our synthetic two-segment ByteRange; not a general PDF parser."""
    require(len(ranges) == 4 and all(type(n) is int for n in ranges), "Invalid synthetic ByteRange types")
    start, length, second, tail = ranges
    require(start == 0 and 0 < length < second and tail > 0 and second + tail <= len(data),
            "Invalid synthetic ByteRange bounds")
    gap = data[length:second]
    require(gap.startswith(b"<") and gap.endswith(b">"), "Synthetic Contents gap not hex-delimited")
    return data[:length] + data[second:second + tail]


def provenance():
    with urllib.request.urlopen(f"https://pypi.org/pypi/pyHanko/{VERSION}/json", timeout=30) as response:
        metadata = json.load(response)
    require(metadata["info"]["version"] == VERSION, "Unexpected PyPI version")
    require(metadata["info"]["license_expression"] == "MIT", "Unexpected license metadata")
    wheel = next(u for u in metadata["urls"] if u["filename"].endswith(".whl"))
    source = next(u for u in metadata["urls"] if u["filename"].endswith(".tar.gz"))
    require(wheel["digests"]["sha256"] == WHEEL_SHA256, "Wheel provenance changed")
    require(source["url"] == SOURCE_URL and source["digests"]["sha256"] == SOURCE_SHA256, "Source provenance changed")
    with urllib.request.urlopen(SOURCE_URL, timeout=30) as response:
        archive = response.read(16 * 1024 * 1024)
    require(hashlib.sha256(archive).hexdigest() == SOURCE_SHA256, "Source download hash mismatch")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as source_archive:
        # Read known members only; never extract untrusted archive paths.
        license_text = source_archive.extractfile(f"pyhanko-{VERSION}/LICENSE").read().decode()
        require("MIT License" in license_text, "Source license mismatch")
        notices = [m.name for m in source_archive.getmembers() if m.name.endswith("LICENSE.PyPDF2")]
        require(notices, "Missing inherited PyPDF2 license notice")
    return {
        "pypi": f"https://pypi.org/pypi/pyHanko/{VERSION}/json", "version": VERSION,
        "wheel_url": wheel["url"], "wheel_sha256": WHEEL_SHA256, "wheel_bytes": wheel["size"],
        "source_url": SOURCE_URL, "source_sha256": SOURCE_SHA256, "source_bytes": len(archive),
        "license": "MIT; inherited PyPDF2 BSD-3-Clause-style notice present", "uploaded": wheel["upload_time_iso_8601"],
        "development_status": [c for c in metadata["info"]["classifiers"] if c.startswith("Development Status")],
    }


def launcher():
    require(sys.version_info[:2] == (3, 12), "Run with uv-managed Python 3.12; lock resolved for that interpreter")
    require(shutil.which("uv") and shutil.which("openssl"), "uv and OpenSSL must already be available")
    origin = provenance()
    started = time.perf_counter()
    with tempfile.TemporaryDirectory(prefix="kinetic-pyhanko-qualification-") as directory:
        work = Path(directory)
        permissions = secure_directory(work)
        environment = work / "venv"
        command(["uv", "venv", "--no-config", "--python", sys.executable, environment])
        python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        command(["uv", "pip", "install", "--no-config", "--python", python,
                 "--index-url", "https://pypi.org/simple", "--only-binary", ":all:",
                 "--require-hashes", "--requirements", CONFIG / "requirements.txt"])
        setup_seconds = time.perf_counter() - started
        result = subprocess.run([str(python), str(Path(__file__).resolve()), "--worker", str(work)],
                                capture_output=True, timeout=180)
        if result.returncode:
            # Worker prints only a controlled error type/message, never raw PDF/key dumps.
            raise RuntimeError(result.stdout.decode(errors="replace")[:1000] or "Qualification worker failed")
        report = json.loads(result.stdout)
        report["temporary_venv_bytes"] = sum(p.stat().st_size for p in environment.rglob("*") if p.is_file())
        report["setup_seconds"] = round(setup_seconds, 3)
        report["temp_permissions"] = permissions
    # Report success only after cleanup, including the venv and all private fixtures.
    report.update(provenance=origin, temp_cleanup="completed", python=sys.version.split()[0])
    print(json.dumps(report, indent=2, sort_keys=True))


def worker(work):
    started = time.perf_counter()
    logging.disable(logging.CRITICAL)
    from datetime import datetime, timedelta, timezone
    from importlib import metadata
    import socket
    from asn1crypto import cms, crl, keys, tsp, x509 as asn1_x509
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID
    from pyhanko.pdf_utils import generic
    from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
    from pyhanko.pdf_utils.reader import PdfFileReader
    from pyhanko.pdf_utils.writer import PdfFileWriter
    from pyhanko.sign import fields, signers
    from pyhanko.sign.timestamps import DummyTimeStamper
    from pyhanko.sign.validation import validate_pdf_signature, validate_pdf_timestamp
    from pyhanko_certvalidator import ValidationContext
    from pyhanko_certvalidator.registry import SimpleCertificateStore

    require(metadata.version("pyhanko") == VERSION, "Wrong installed backend version")
    require(work.resolve().is_relative_to(Path(tempfile.gettempdir()).resolve())
            and work.name.startswith("kinetic-pyhanko-qualification-"), "Worker requires launcher-created OS temp directory")
    # Windows asyncio needs loopback sockets for its event-loop wakeup channel.
    # Permit literal loopback only; disallow general connection helpers/egress.
    import ipaddress
    original_connect, original_connect_ex = socket.socket.connect, socket.socket.connect_ex
    def no_network(*args, **kwargs):
        raise RuntimeError("Non-loopback connection attempted in offline qualification worker")
    def local_connect(sock, address, *, connect_ex=False):
        try:
            loopback = isinstance(address, tuple) and ipaddress.ip_address(address[0]).is_loopback
        except ValueError:
            loopback = False
        require(loopback, "Non-loopback connection attempted in offline qualification worker")
        return (original_connect_ex if connect_ex else original_connect)(sock, address)
    socket.socket.connect = local_connect
    socket.socket.connect_ex = lambda sock, address: local_connect(sock, address, connect_ex=True)
    socket.create_connection = no_network
    now = datetime.now(timezone.utc).replace(microsecond=0)

    def identity(label, issuer_cert=None, issuer_key=None, *, ca=False, tsa=False):
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Synthetic qualification " + label)])
        builder = (x509.CertificateBuilder().subject_name(subject)
                   .issuer_name(issuer_cert.subject if issuer_cert else subject)
                   .public_key(key.public_key()).serial_number(x509.random_serial_number())
                   .not_valid_before(now - timedelta(days=1)).not_valid_after(now + timedelta(days=7))
                   .add_extension(x509.BasicConstraints(ca=ca, path_length=0 if ca else None), critical=True)
                   .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
                   .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key((issuer_key or key).public_key()), critical=False)
                   .add_extension(x509.KeyUsage(digital_signature=not ca, content_commitment=not ca and not tsa,
                       key_encipherment=False, data_encipherment=False, key_agreement=False,
                       key_cert_sign=ca, crl_sign=ca, encipher_only=None, decipher_only=None), critical=True))
        if tsa:
            builder = builder.add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.TIME_STAMPING]), critical=True)
        cert = builder.sign(issuer_key or key, hashes.SHA256())
        private_write(work / (label + "-key.pem"), key.private_bytes(serialization.Encoding.PEM,
                      serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        return key, cert

    root_key, root = identity("root", ca=True)
    key, cert = identity("signer", root, root_key)
    tsa_key, tsa_cert = identity("tsa", root, root_key, tsa=True)
    root_asn = asn1_x509.Certificate.load(root.public_bytes(serialization.Encoding.DER))
    cert_asn = asn1_x509.Certificate.load(cert.public_bytes(serialization.Encoding.DER))
    tsa_asn = asn1_x509.Certificate.load(tsa_cert.public_bytes(serialization.Encoding.DER))
    def key_asn(key):
        return keys.PrivateKeyInfo.load(key.private_bytes(serialization.Encoding.DER,
                    serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    revocations = (x509.CertificateRevocationListBuilder().issuer_name(root.subject)
                   .last_update(now - timedelta(hours=1)).next_update(now + timedelta(days=1))
                   .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(root_key.public_key()), False)
                   .sign(root_key, hashes.SHA256()))
    crl_asn = crl.CertificateList.load(revocations.public_bytes(serialization.Encoding.DER))
    registry = SimpleCertificateStore()
    registry.register(root_asn)
    signer = signers.SimpleSigner(cert_asn, key_asn(key), registry)
    timestamper = DummyTimeStamper(tsa_asn, key_asn(tsa_key), certs_to_embed=registry)
    def context(trust=True):
        return ValidationContext(trust_roots=[root_asn] if trust else [], other_certs=[cert_asn, tsa_asn],
                                 crls=[crl_asn], allow_fetching=False, revocation_mode="require")

    def base_pdf():
        writer = PdfFileWriter()
        stream = writer.add_object(generic.StreamObject(stream_data=b"q 0 0 0 RG 20 20 100 80 re S Q\n"))
        writer.insert_page(generic.DictionaryObject({
            generic.pdf_name("/Type"): generic.pdf_name("/Page"),
            generic.pdf_name("/MediaBox"): generic.ArrayObject([generic.NumberObject(n) for n in (0, 0, 300, 300)]),
            generic.pdf_name("/Resources"): generic.DictionaryObject(),
            generic.pdf_name("/Contents"): stream,
        }))
        for name in ("Sig1", "Sig2"):
            fields.append_signature_field(writer, fields.SigFieldSpec(sig_field_name=name))
        output = io.BytesIO(); writer.write(output)
        return output.getvalue()

    def sign(data, *, name="Sig1", permission=None, timestamp=False, lt=False, lta=False):
        spec = signers.PdfSignatureMetadata(field_name=name, md_algorithm="sha256", subfilter=fields.SigSeedSubFilter.PADES,
            certify=permission is not None, docmdp_permissions=fields.MDPPerm(permission or 2),
            embed_validation_info=lt or lta, use_pades_lta=lta, validation_context=context() if (lt or lta) else None)
        return signers.sign_pdf(IncrementalPdfFileWriter(io.BytesIO(data)), spec, signer=signer,
            timestamper=timestamper if (timestamp or lt or lta) else None).getvalue()

    openssl_checks = {"cms_positive": 0, "timestamp_cms_positive": 0, "content_negative": 0,
                      "signature_negative": 0, "timestamp_imprints": 0}
    check_number = 0
    def openssl_cms(der, content=None):
        nonlocal check_number
        check_number += 1
        paths = [work / f"cms-{check_number}.{suffix}" for suffix in ("der", "data", "verified")]
        private_write(paths[0], der)
        args = ["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", str(paths[0]), "-noverify", "-out", str(paths[2])]
        if content is not None:
            private_write(paths[1], content)
            args += ["-content", str(paths[1])]
        env = dict(os.environ, OPENSSL_CONF=os.devnull)
        result = subprocess.run(args, capture_output=True, timeout=20, env=env)
        output = paths[2].read_bytes() if paths[2].exists() else b""
        for path in paths:
            path.unlink(missing_ok=True)
        return result.returncode == 0, output

    def check_cms(data):
        reader = PdfFileReader(io.BytesIO(data), strict=True)
        for sig in reader.embedded_signatures:
            ranges = [int(n) for n in sig.byte_range]
            content = signed_content(data, ranges)
            container = cms.ContentInfo.load(sig.pkcs7_content)
            der = container.dump()  # ASN.1 strips unused PDF Contents zero padding.
            is_timestamp = str(sig.sig_object.get("/Type")) == "/DocTimeStamp"
            ok, verified = openssl_cms(der, None if is_timestamp else content)
            require(ok and (is_timestamp or verified == content), "Independent CMS integrity failed")
            openssl_checks["cms_positive"] += 1
            if is_timestamp:
                info = tsp.TSTInfo.load(container["content"]["encap_content_info"]["content"].parsed.dump())
                imprint = info["message_imprint"]
                require(hashlib.new(imprint["hash_algorithm"]["algorithm"].native, content).digest()
                        == imprint["hashed_message"].native, "Document timestamp imprint mismatch")
                openssl_checks["timestamp_imprints"] += 1
            else:
                changed = bytes([content[0] ^ 1]) + content[1:]
                require(not openssl_cms(der, changed)[0], "Tampered signed bytes accepted")
                openssl_checks["content_negative"] += 1
                attrs = container["content"]["signer_infos"][0]["unsigned_attrs"]
                for attr in attrs:
                    if attr["type"].native == "signature_time_stamp_token":
                        token = attr["values"][0]
                        require(openssl_cms(token.dump())[0], "Timestamp CMS integrity failed")
                        openssl_checks["timestamp_cms_positive"] += 1
                        imprint = token["content"]["encap_content_info"]["content"].parsed["message_imprint"]
                        raw_signature = container["content"]["signer_infos"][0]["signature"].native
                        require(hashlib.new(imprint["hash_algorithm"]["algorithm"].native, raw_signature).digest()
                                == imprint["hashed_message"].native, "Signature timestamp imprint mismatch")
                        openssl_checks["timestamp_imprints"] += 1
            info = container["content"]["signer_infos"][0]
            raw = info["signature"].native
            info["signature"] = bytes([raw[0] ^ 1]) + raw[1:]
            require(not openssl_cms(container.dump(), None if is_timestamp else content)[0], "Tampered CMS signature accepted")
            openssl_checks["signature_negative"] += 1

    def status(data, index=0, trust=True):
        sig = PdfFileReader(io.BytesIO(data), strict=True).embedded_signatures[index]
        result = validate_pdf_signature(sig, signer_validation_context=context(trust), ts_validation_context=context(trust))
        return {"intact": result.intact, "valid": result.valid, "trusted_test_root": result.trusted,
                "docmdp_ok": result.docmdp_ok, "bottom_line": result.bottom_line,
                "modification_level": result.modification_level.name,
                "timestamp_trusted": bool(result.timestamp_validity and result.timestamp_validity.trusted)}

    report = {"versions": {d.metadata["Name"]: d.version for d in metadata.distributions()},
              "openssl": command(["openssl", "version"]).decode().strip(), "cases": {}}
    base = base_pdf()
    approval = sign(base)
    approval_sig = PdfFileReader(io.BytesIO(approval), strict=True).embedded_signatures[0]
    signed_attributes = {a["type"].native for a in approval_sig.signer_info["signed_attrs"]}
    require(str(approval_sig.sig_object["/SubFilter"]) == "/ETSI.CAdES.detached" and approval_sig.md_algorithm == "sha256"
            and {"content_type", "message_digest", "signing_certificate_v2"} <= signed_attributes
            and "signing_time" not in signed_attributes, "Unexpected PAdES baseline construction")
    require(approval.startswith(base), "Initial approval rewrote its original prefix")
    check_cms(approval)
    approved = status(approval)
    require(approved["intact"] and approved["valid"] and approved["trusted_test_root"], "Fixture approval validation failed")
    untrusted = status(approval, trust=False)
    require(untrusted["intact"] and untrusted["valid"] and not untrusted["trusted_test_root"], "Empty trust store was not kept separate")
    report["cases"]["approval_B_B"] = dict(approved, prefix_preserved=True, bytes=len(approval), untrusted_root_result=untrusted)
    certs = {}
    for permission in (1, 2, 3):
        signed = certs[permission] = sign(base, permission=permission)
        require(signed.startswith(base), "Certification rewrote original prefix")
        reader = PdfFileReader(io.BytesIO(signed), strict=True)
        require("/DocMDP" in reader.root["/Perms"], "Certification catalog permission missing")
        certification = reader.root["/Perms"]["/DocMDP"]
        require(certification == reader.embedded_signatures[0].sig_object, "Catalog points to a different certification")
        transform = certification["/Reference"][0].get_object()
        actual_permission = int(transform["/TransformParams"]["/P"])
        require(actual_permission == permission, "Wrong DocMDP permission")
        result = status(signed)
        require(result["bottom_line"], "Certification fixture validation failed")
        check_cms(signed)
        report["cases"][f"certification_P{permission}"] = dict(result, prefix_preserved=True, bytes=len(signed),
            actual_docmdp_permission=actual_permission)
        if permission == 1:
            try:
                sign(signed, name="Sig2")
            except Exception as error:
                require(type(error).__name__ == "SigningError" and str(error) == "Author signature forbids all changes",
                        "Unexpected P=1 co-signing failure")
                report["cases"]["additional_P1"] = {"rejected": True, "error_type": type(error).__name__, "reason": "DocMDP_NO_CHANGES"}
            else:
                raise RuntimeError("P=1 incorrectly permitted another approval")
        else:
            second = sign(signed, name="Sig2")
            require(second.startswith(signed), "Additional approval rewrote signed prefix")
            original, latest = status(second), status(second, 1)
            require(original["bottom_line"] and latest["bottom_line"], "Permitted co-signing validation failed")
            check_cms(second)
            report["cases"][f"additional_P{permission}"] = {"prefix_preserved": True, "original": original, "latest": latest, "bytes": len(second)}

    writer = IncrementalPdfFileWriter(io.BytesIO(certs[3]))
    page_ref, _ = writer.find_page_for_modification(0)
    annotation = generic.DictionaryObject({generic.pdf_name("/Type"): generic.pdf_name("/Annot"),
        generic.pdf_name("/Subtype"): generic.pdf_name("/Text"), generic.pdf_name("/P"): page_ref,
        generic.pdf_name("/Rect"): generic.ArrayObject([generic.NumberObject(n) for n in (20, 20, 40, 40)]),
        generic.pdf_name("/Contents"): generic.TextStringObject("Synthetic annotation qualification")})
    writer.register_annotation(page_ref, writer.add_object(annotation))
    output = io.BytesIO(); writer.write(output); annotated = output.getvalue()
    require(annotated.startswith(certs[3]), "Annotation update rewrote signed prefix")
    result = status(annotated)
    require(result["intact"] and result["valid"] and not result["docmdp_ok"], "Expected conservative P=3 annotation limitation changed; review backend")
    check_cms(annotated)
    report["cases"]["P3_annotation"] = dict(result, prefix_preserved=True, bytes=len(annotated), supported=False)

    for profile in ("B_T", "B_LT", "B_LTA"):
        signed = sign(base, timestamp=True, lt=profile == "B_LT", lta=profile == "B_LTA")
        require(signed.startswith(base), "Timestamp/LTV generation rewrote original prefix")
        result = status(signed)
        require(result["bottom_line"] and result["timestamp_trusted"], "Local test TSA validation failed")
        reader = PdfFileReader(io.BytesIO(signed), strict=True)
        dss = reader.root["/DSS"] if "/DSS" in reader.root else None
        if profile != "B_T":
            require(dss is not None and len(dss["/CRLs"]) > 0, "LT fixture lacks embedded revocation evidence")
        timestamps = [sig for sig in reader.embedded_signatures if str(sig.sig_object.get("/Type")) == "/DocTimeStamp"]
        if profile == "B_LTA":
            require(timestamps, "LTA fixture lacks document timestamp")
            timestamp_statuses = [validate_pdf_timestamp(sig, context()) for sig in timestamps]
            require(all(s.intact and s.valid and s.trusted for s in timestamp_statuses), "Document timestamp validation failed")
        check_cms(signed)
        report["cases"][profile + "_local_fixture"] = dict(result, bytes=len(signed), dss=dss is not None,
            document_timestamps=len(timestamps), real_TSA=False, archival_policy_qualified=False,
            embedded_only_evidence_qualified=False, validation_evidence="external fixture certificates and CRL plus generated trust anchor")
    report["baseline_construction"] = {"subfilter": "/ETSI.CAdES.detached", "sha256": True,
        "signed_attributes_checked": sorted(signed_attributes), "external_profile_validator": False}
    report["openssl_checks"] = openssl_checks
    report["offline_worker_seconds"] = round(time.perf_counter() - started, 3)
    report["unperformed"] = ["embedded-only LT/LTA evidence validation", "P2 permitted form filling",
        "P2 forbidden page-content edit validation", "validly re-signed wrong-imprint timestamp negatives"]
    report["scope"] = "synthetic local-root/CRL/TSA fixtures only; no native keys, remote TSA, hostile-PDF or long-term policy qualification"
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Optional isolated pyHanko qualification using synthetic temporary fixtures only.")
    parser.add_argument("--worker", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if args.worker:
            worker(args.worker)
        else:
            launcher()
    except Exception as error:
        reason = str(error)[:500] if isinstance(error, RuntimeError) else "Unexpected exception; details suppressed to avoid data disclosure"
        print(json.dumps({"qualification_failed": type(error).__name__, "reason": reason}))
        sys.exit(1)
