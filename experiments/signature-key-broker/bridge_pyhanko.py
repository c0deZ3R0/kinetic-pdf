#!/usr/bin/env python3
# Optional Rust-CNG/Python-ORACLE bridge; not a Rust PDF signing engine.
#
# PATH: experiments/signature-key-broker/bridge_pyhanko.py
# CONSUMED BY: developers and docs/signature-native-broker.md only.
# One unnamed ephemeral Rust key; synthetic Python CA; private OS-temp fixtures.
# Bounded stdio IPC; no user stores/identities, Windows-key export or TCP transport.

import argparse
import base64
import hashlib
import importlib.util
import io
import json
import logging
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
ORACLE = ROOT / "scripts/signatures/qualify_pyhanko.py"
MAX_REPLY = 4096
IPC_TIMEOUT = 10
ALGORITHM = "rsa_pkcs1_sha256"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def oracle_helpers():
    spec = importlib.util.spec_from_file_location("readonly_qualification_helpers", ORACLE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def response_body(frame, operation, expected_kind):
    require(len(frame) <= MAX_REPLY and frame.endswith(b"\n"), "Invalid bounded broker frame")
    def unique_object(pairs):
        value = dict(pairs)
        require(len(value) == len(pairs), "Duplicate broker JSON field")
        return value
    try:
        response = json.loads(frame, object_pairs_hook=unique_object)
    except (ValueError, UnicodeError):
        raise RuntimeError("Malformed broker JSON") from None
    require(type(response) is dict and set(response) == {"version", "operation_id", "body"}, "Unexpected broker envelope")
    require(type(response["version"]) is int and response["version"] == 1
            and response["operation_id"] == operation, "Broker response version/operation mismatch")
    body = response["body"]
    expected_fields = {"kind", "algorithm", "key_id", "test_only",
                       "spki_der_b64" if expected_kind == "public_key" else "signature_b64"}
    require(type(body) is dict and set(body) == expected_fields and body["kind"] == expected_kind,
            "Broker returned an error or unexpected payload")
    require(body["algorithm"] == ALGORITHM and body["test_only"] is True
            and type(body["key_id"]) is str and re.fullmatch(r"[0-9a-f]{64}", body["key_id"]), "Invalid broker key/algorithm")
    return body


def bounded_wait(channel, timeout=IPC_TIMEOUT):
    try:
        return channel.get(timeout=timeout)
    except queue.Empty:
        raise RuntimeError("Broker IPC timeout") from None


class Broker:
    def __init__(self, executable):
        self.process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, bufsize=0, close_fds=True)
        self.frames = queue.Queue(maxsize=1)
        self.reader_error = threading.Event()
        self.requests = 0
        self.sign_requests = 0
        self.key_id = None
        self.exit_code = None
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._read, daemon=True)
        try:
            self.reader.start()
        except Exception:
            self.process.kill()
            self.process.wait(timeout=5)
            self.process.stdin.close(); self.process.stdout.close()
            raise RuntimeError("Broker reader initialization failed") from None

    def _read(self):
        try:
            while True:
                frame = self.process.stdout.readline(MAX_REPLY + 1)
                if not frame:
                    return
                if len(frame) > MAX_REPLY or not frame.endswith(b"\n"):
                    self.reader_error.set()
                    return
                self.frames.put_nowait(frame)
        except (OSError, ValueError, queue.Full):
            self.reader_error.set()

    def request(self, body, expected_kind):
        with self.lock:
            require(self.requests < 128 and self.process.poll() is None, "Broker session unavailable")
            self.requests += 1
            operation = f"bridge-{self.requests:04d}"
            frame = json.dumps({"version": 1, "operation_id": operation, "body": body}, separators=(",", ":")).encode() + b"\n"
            require(len(frame) <= 16 * 1024, "Broker request exceeds bound")
            written = queue.Queue(maxsize=1)
            def send():
                try:
                    # Our three request frames are tiny, but a daemon writer and
                    # timeout also bound waiting if a faulty child stops reading.
                    view = memoryview(frame)
                    while view:
                        count = self.process.stdin.write(view)
                        require(count is not None and count > 0, "Broker write failed")
                        view = view[count:]
                    self.process.stdin.flush()
                    written.put_nowait(True)
                except Exception:
                    written.put_nowait(False)
            writer = threading.Thread(target=send, daemon=True)
            writer.start()
            require(bounded_wait(written), "Broker write failed")
            reply = bounded_wait(self.frames)
            require(not self.reader_error.is_set(), "Broker emitted invalid/unsolicited data")
            writer.join(timeout=1)
            return response_body(reply, operation, expected_kind)

    def public_spki(self):
        body = self.request({"kind": "public_key"}, "public_key")
        try:
            spki = base64.b64decode(body["spki_der_b64"], validate=True)
        except (ValueError, TypeError):
            raise RuntimeError("Invalid public SPKI encoding") from None
        require(0 < len(spki) <= 512 and hashlib.sha256(spki).hexdigest() == body["key_id"], "SPKI/key ID mismatch")
        self.key_id = body["key_id"]
        return spki

    def sign_digest(self, digest):
        require(self.key_id is not None and len(digest) == 32, "Prehash/key binding missing")
        body = self.request({"kind": "sign", "expected_key_id": self.key_id, "algorithm": ALGORITHM,
                             "input": {"kind": "sha256_prehashed", "digest_b64": base64.b64encode(digest).decode()}}, "signature")
        require(body["key_id"] == self.key_id, "Signing key changed")
        try:
            signature = base64.b64decode(body["signature_b64"], validate=True)
        except (ValueError, TypeError):
            raise RuntimeError("Invalid RSA signature encoding") from None
        require(len(signature) == 256, "Unexpected RSA signature size")
        self.sign_requests += 1
        return signature

    def close(self):
        # Only this Popen child is ever closed/killed; no process-name/global kill.
        try:
            if self.process.stdin:
                self.process.stdin.close()  # EOF permits normal RAII key cleanup.
            self.exit_code = self.process.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            if self.process.poll() is None:
                self.process.kill()
            self.exit_code = self.process.wait(timeout=5)
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=5)
            for stream in (self.process.stdin, self.process.stdout):
                if stream:
                    stream.close()
            self.reader.join(timeout=1)


def launch(broker):
    helpers = oracle_helpers()
    require(os.name == "nt" and sys.version_info[:2] == (3, 12), "This bridge run requires Windows/Python 3.12")
    broker = broker.resolve(strict=True)
    require(broker.name == "signature-key-broker-experiment.exe"
            and broker.is_relative_to(Path(tempfile.gettempdir()).resolve()), "Use the experiment executable from its OS-temp target")
    require(shutil.which("uv") and shutil.which("openssl"), "uv and OpenSSL required")
    origin = helpers.provenance()
    binary_hash = hashlib.sha256(broker.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix="kinetic-native-bridge-") as directory:
        work = Path(directory)
        permissions = helpers.secure_directory(work)
        copied = work / broker.name
        shutil.copyfile(broker, copied)
        require(hashlib.sha256(copied.read_bytes()).hexdigest() == binary_hash, "Copied binary hash mismatch")
        environment = work / "venv"
        helpers.command(["uv", "venv", "--no-config", "--python", sys.executable, environment])
        python = environment / "Scripts/python.exe"
        helpers.command(["uv", "pip", "install", "--no-config", "--python", python,
                         "--index-url", "https://pypi.org/simple", "--only-binary", ":all:", "--require-hashes",
                         "--requirements", helpers.CONFIG / "requirements.txt"])
        completed = subprocess.run([str(python), "-B", str(Path(__file__).resolve()), "--worker", str(work),
                                    "--expected-sha256", binary_hash], capture_output=True, timeout=180)
        require(completed.returncode == 0, completed.stdout.decode(errors="replace")[:700] or "Bridge worker failed")
        report = json.loads(completed.stdout)
        report.update(binary_sha256=binary_hash, provenance=origin, temp_permissions=permissions,
                      experiment_lock_sha256=hashlib.sha256((Path(__file__).parent / "Cargo.lock").read_bytes()).hexdigest())
    report["temp_cleanup"] = "completed"
    print(json.dumps(report, indent=2, sort_keys=True))


def worker(work, expected_sha256):
    helpers = oracle_helpers()
    logging.disable(logging.CRITICAL)
    from datetime import datetime, timedelta, timezone
    from importlib import metadata
    from asn1crypto import algos, cms, x509 as asn1_x509
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import padding, rsa, utils
    from cryptography.x509.oid import NameOID
    from pyhanko.pdf_utils import generic
    from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
    from pyhanko.pdf_utils.reader import PdfFileReader
    from pyhanko.pdf_utils.writer import PdfFileWriter
    from pyhanko.sign import fields, signers
    from pyhanko.sign.validation import validate_pdf_signature
    from pyhanko_certvalidator import ValidationContext
    from pyhanko_certvalidator.registry import SimpleCertificateStore

    require(metadata.version("pyhanko") == "0.38.1", "Unexpected oracle version")
    require(work.resolve().is_relative_to(Path(tempfile.gettempdir()).resolve())
            and work.name.startswith("kinetic-native-bridge-"), "Worker requires launcher-created OS temp")
    executable = work / "signature-key-broker-experiment.exe"
    require(hashlib.sha256(executable.read_bytes()).hexdigest() == expected_sha256, "Broker executable changed")
    broker = Broker(executable)
    try:
        spki = broker.public_spki()
        public = serialization.load_der_public_key(spki)
        require(isinstance(public, rsa.RSAPublicKey) and public.key_size == 2048, "Expected test RSA-2048 SPKI")
        now = datetime.now(timezone.utc)
        root_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        helpers.private_write(work / "synthetic-root-key.pem", root_key.private_bytes(serialization.Encoding.PEM,
                              serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        root_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Synthetic native bridge root")])
        leaf_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Synthetic native bridge leaf")])
        def certificate(subject, key, ca):
            return (x509.CertificateBuilder().subject_name(subject).issuer_name(root_name).public_key(key)
                    .serial_number(x509.random_serial_number()).not_valid_before(now - timedelta(days=1))
                    .not_valid_after(now + timedelta(days=7))
                    .add_extension(x509.BasicConstraints(ca=ca, path_length=0 if ca else None), True)
                    .add_extension(x509.KeyUsage(digital_signature=not ca, content_commitment=not ca,
                        key_encipherment=False, data_encipherment=False, key_agreement=False,
                        key_cert_sign=ca, crl_sign=ca, encipher_only=None, decipher_only=None), True)
                    .add_extension(x509.SubjectKeyIdentifier.from_public_key(key), False)
                    .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(root_key.public_key()), False)
                    .sign(root_key, hashes.SHA256()))
        root = certificate(root_name, root_key.public_key(), True)
        leaf = certificate(leaf_name, public, False)
        require(leaf.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo) == spki,
                "Leaf certificate does not bind broker SPKI")
        root_asn = asn1_x509.Certificate.load(root.public_bytes(serialization.Encoding.DER))
        leaf_asn = asn1_x509.Certificate.load(leaf.public_bytes(serialization.Encoding.DER))
        registry = SimpleCertificateStore(); registry.register(root_asn)

        class CngOracleSigner(signers.Signer):
            def __init__(self):
                super().__init__(signing_cert=leaf_asn, cert_registry=registry,
                    signature_mechanism=algos.SignedDigestAlgorithm({"algorithm": "rsassa_pkcs1v15"}))
                self.dry_runs = 0
                self.actual_signs = 0
                self.verified = 0

            async def async_sign_raw(self, data, digest_algorithm, dry_run=False):
                require(digest_algorithm == "sha256", "Unsupported callback digest")
                if dry_run:
                    before = broker.requests
                    self.dry_runs += 1
                    placeholder = bytes(256)
                    require(broker.requests == before, "Reservation consumed broker operation")
                    return placeholder
                require(0 < len(data) <= 4096 and data[0] == 0x31 and cms.CMSAttributes.load(data).dump() == data,
                        "Callback data is not bounded DER CMS signed attributes")
                digest = hashlib.sha256(data).digest()
                signature = broker.sign_digest(digest)
                self.actual_signs += 1
                # Verify before returning to pyHanko: it cannot embed an unchecked
                # CNG result, mismatched SPKI, or accidentally double-hashed input.
                public.verify(signature, digest, padding.PKCS1v15(), utils.Prehashed(hashes.SHA256()))
                self.verified += 1
                return signature

        signer = CngOracleSigner()
        def base_pdf():
            writer = PdfFileWriter()
            content = writer.add_object(generic.StreamObject(stream_data=b"q 0 0 0 RG 10 10 50 50 re S Q\n"))
            writer.insert_page(generic.DictionaryObject({generic.pdf_name("/Type"): generic.pdf_name("/Page"),
                generic.pdf_name("/MediaBox"): generic.ArrayObject([generic.NumberObject(n) for n in (0, 0, 200, 200)]),
                generic.pdf_name("/Resources"): generic.DictionaryObject(), generic.pdf_name("/Contents"): content}))
            fields.append_signature_field(writer, fields.SigFieldSpec(sig_field_name="Sig1"))
            output = io.BytesIO(); writer.write(output)
            return output.getvalue()

        def context(trusted):
            return ValidationContext(trust_roots=[root_asn] if trusted else [], allow_fetching=False, revocation_mode="soft-fail")

        openssl_checks = {"positive": 0, "wrong_content_rejected": 0, "wrong_signature_rejected": 0}
        file_counter = 0
        def openssl_verify(der, content):
            nonlocal file_counter
            file_counter += 1
            paths = [work / f"oracle-{file_counter}.{suffix}" for suffix in ("der", "data", "verified")]
            helpers.private_write(paths[0], der); helpers.private_write(paths[1], content)
            result = subprocess.run(["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", str(paths[0]),
                "-content", str(paths[1]), "-noverify", "-out", str(paths[2])], capture_output=True, timeout=15,
                env=dict(os.environ, OPENSSL_CONF=os.devnull))
            verified = paths[2].read_bytes() if paths[2].exists() else b""
            return result.returncode == 0 and verified == content

        cases = {}
        for label, certify in (("approval_B_B", False), ("fresh_base_certification_P1_B_B", True)):
            base = base_pdf()  # Certification NEVER appends to the signed approval.
            spec = signers.PdfSignatureMetadata(field_name="Sig1", md_algorithm="sha256",
                subfilter=fields.SigSeedSubFilter.PADES, certify=certify, docmdp_permissions=fields.MDPPerm.NO_CHANGES)
            before = broker.sign_requests
            pdf = signers.sign_pdf(IncrementalPdfFileWriter(io.BytesIO(base)), spec, signer=signer).getvalue()
            require(broker.sign_requests == before + 1 and pdf.startswith(base), "Unexpected signing count/prefix rewrite")
            helpers.private_write(work / (label + ".pdf"), pdf)
            reader = PdfFileReader(io.BytesIO(pdf), strict=True)
            require(len(reader.embedded_signatures) == 1, "Expected one signature on fresh base")
            sig = reader.embedded_signatures[0]
            require(str(sig.sig_object["/SubFilter"]) == "/ETSI.CAdES.detached", "Wrong baseline subfilter")
            attributes = {a["type"].native for a in sig.signer_info["signed_attrs"]}
            require({"content_type", "message_digest", "signing_certificate_v2"} <= attributes
                    and "signing_time" not in attributes and not sig.signer_info["unsigned_attrs"].native,
                    "Unexpected baseline attributes/timestamp")
            ranges = [int(n) for n in sig.byte_range]
            content = helpers.signed_content(pdf, ranges)
            require(ranges[2] + ranges[3] == len(pdf), "ByteRange does not cover exact final revision")
            gap = pdf[ranges[1]:ranges[2]]
            require(re.fullmatch(rb"<[0-9a-fA-F]+>", gap) and (len(gap) - 2) % 2 == 0, "Unexpected Contents gap")
            slot = bytes.fromhex(gap[1:-1].decode("ascii"))
            container = cms.ContentInfo.load(slot)
            der = container.dump()
            require(slot.startswith(der) and not any(slot[len(der):])
                    and der == cms.ContentInfo.load(sig.pkcs7_content).dump(), "Contents gap is not exactly CMS plus zero padding")
            if certify:
                certification = reader.root["/Perms"]["/DocMDP"]
                require(certification == sig.sig_object
                        and int(certification["/Reference"][0].get_object()["/TransformParams"]["/P"]) == 1, "Wrong actual certification P1")
            else:
                require("/Perms" not in reader.root, "Approval unexpectedly certifies the PDF")
            trusted = validate_pdf_signature(sig, signer_validation_context=context(True))
            untrusted = validate_pdf_signature(sig, signer_validation_context=context(False))
            require(trusted.intact and trusted.valid and trusted.trusted and trusted.bottom_line
                    and untrusted.intact and untrusted.valid and not untrusted.trusted and not untrusted.bottom_line
                    and trusted.timestamp_validity is None, "Fixture integrity/trust separation failed")
            require(openssl_verify(der, content), "Independent CMS verification failed")
            openssl_checks["positive"] += 1
            changed_content = bytes([content[0] ^ 1]) + content[1:]
            require(not openssl_verify(der, changed_content), "Wrong content accepted")
            openssl_checks["wrong_content_rejected"] += 1
            info = container["content"]["signer_infos"][0]
            raw = info["signature"].native
            info["signature"] = bytes([raw[0] ^ 1]) + raw[1:]
            require(not openssl_verify(container.dump(), content), "Wrong CMS signature accepted")
            openssl_checks["wrong_signature_rejected"] += 1
            cases[label] = {"pdf_bytes": len(pdf), "prefix_preserved": True, "exact_revision_and_contents_gap": True,
                            "fixture_root_trusted": True, "empty_roots_trusted": False, "timestamp": False,
                            "actual_docmdp_permission": 1 if certify else None}
        require(signer.actual_signs == signer.verified == broker.sign_requests == 2
                and broker.requests == 3 and signer.dry_runs >= 2, "Unexpected private-operation/reservation counts")
        report = {"assembly_engine": "PYTHON TEST ORACLE, NOT RUST PDF ENGINE", "cases": cases,
                  "dry_run_callbacks": signer.dry_runs, "actual_signs_verified_before_embedding": signer.verified,
                  "broker_requests": broker.requests, "native_private_key_exported": False, "openssl_checks": openssl_checks,
                  "versions": {d.metadata["Name"]: d.version for d in metadata.distributions()},
                  "openssl": helpers.command(["openssl", "version"]).decode().strip(), "python": sys.version.split()[0],
                  "ipc": "bounded stdio only; 10s send/read timeout; no TCP signing transport",
                  "trust_policy": "synthetic root only; revocation soft-fail; no real identity/timestamp/LTV qualification",
                  "external_profile_validation": False, "production_ipc_qualified": False,
                  # Static unperformed qualification flag, not a timeout test result.
                  "forced_timeout_cleanup_qualified": False,
                  "cleanup_scope": "normal successful run only; forced launcher-timeout descendant cleanup not qualified"}
    finally:
        broker.close()
    require(broker.exit_code == 0, "Broker did not exit normally after stdin EOF")
    report["broker_exit_code"] = broker.exit_code
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Optional CNG broker to pyHanko TEST ORACLE bridge")
    parser.add_argument("--broker", type=Path, help="Experiment executable in its OS-temp Cargo target")
    parser.add_argument("--worker", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--expected-sha256", help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if args.worker:
            worker(args.worker, args.expected_sha256)
        else:
            require(args.broker is not None, "Specify the already-built experimental --broker executable")
            launch(args.broker)
    except Exception as error:
        reason = str(error)[:700] if isinstance(error, RuntimeError) else "Unexpected exception; data details suppressed"
        print(json.dumps({"bridge_failed": type(error).__name__, "reason": reason}))
        sys.exit(1)
