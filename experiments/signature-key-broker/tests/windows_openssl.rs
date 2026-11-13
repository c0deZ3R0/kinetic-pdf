//! Gated CNG/OpenSSL feasibility check using one ephemeral TEST key only.
//!
//! PATH: experiments/signature-key-broker/tests/windows_openssl.rs
//! CONSUMED BY: standalone Cargo test after coordinator CPU-gate release.
//! Public SPKI/signature/synthetic-message files live only in OS temp; no key export.

#![cfg(windows)]

use std::process::Command;

use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use signature_key_broker_experiment::protocol::{self, Algorithm, ErrorCode, Request, ResponseBody, Session, SignInput};
use signature_key_broker_experiment::{windows::EphemeralTestKey, TestKeyBroker};

#[test]
fn ephemeral_cng_message_and_prehash_signatures_verify_independently() {
    // This creates/finalizes an unnamed RSA key: DO NOT RUN during perf quiet time.
    let key = EphemeralTestKey::new().expect("unnamed ephemeral key/policy must work; do not use a persisted fallback");
    let public = key.public_key();
    assert_eq!(public.key_id.len(), 64);
    let message = b"synthetic native broker qualification message";
    let message_input = SignInput::Message { bytes_b64: STANDARD.encode(message) };
    let digest: [u8; 32] = Sha256::digest(message).into();
    let digest_input = SignInput::Sha256Prehashed { digest_b64: STANDARD.encode(digest) };
    let mut session = Session::default();
    let mut signatures = Vec::new();
    for (operation_id, input) in [("message-1", message_input), ("prehash-1", digest_input)] {
        let request = Request { version: protocol::VERSION, operation_id: operation_id.into(),
            body: protocol::Command::Sign { expected_key_id: public.key_id.clone(),
                algorithm: Algorithm::RsaPkcs1Sha256, input } };
        let response = session.handle(&key, request);
        assert_eq!(response.operation_id.as_deref(), Some(operation_id));
        match response.body {
            ResponseBody::Signature { signature_b64, key_id, test_only, .. } => {
                assert!(test_only);
                assert_eq!(key_id, public.key_id);
                signatures.push(STANDARD.decode(signature_b64).unwrap());
            }
            _ => panic!("Expected a test signature, never a private-key object"),
        }
    }
    assert_eq!(signatures[0], signatures[1], "prehash must NOT be hashed again");
    assert_eq!(signatures[0].len(), 256);

    let directory = tempfile::tempdir().unwrap();
    let public_path = directory.path().join("public.der");
    let signature_path = directory.path().join("signature.bin");
    let message_path = directory.path().join("message.bin");
    std::fs::write(&public_path, &public.spki_der).unwrap();
    std::fs::write(&signature_path, &signatures[0]).unwrap();
    std::fs::write(&message_path, message).unwrap();
    let verify = || {
        // Fixed developer tool only. No command path or arguments come from the
        // wire protocol, and only PUBLIC material is passed to this oracle.
        Command::new("openssl").env("OPENSSL_CONF", "NUL").env_remove("OPENSSL_MODULES").env_remove("OPENSSL_ENGINES")
            .args(["dgst", "-sha256", "-verify"])
            .arg(&public_path).args(["-keyform", "DER", "-signature"])
            .arg(&signature_path).arg(&message_path).output().expect("OpenSSL required for independent verification")
            .status.success()
    };
    assert!(verify(), "OpenSSL must accept CNG signature and independently decode public SPKI");
    std::fs::write(&message_path, b"changed synthetic message").unwrap();
    assert!(!verify(), "Changed message must fail verification");
    std::fs::write(&message_path, message).unwrap();
    let mut changed_signature = signatures[0].clone();
    changed_signature[0] ^= 1;
    std::fs::write(&signature_path, &changed_signature).unwrap();
    assert!(!verify(), "Changed signature must fail verification");
    // A genuine CNG signature over the WRONG 32-byte digest must not verify as
    // the original message. This is not just a corrupted-signature negative.
    let mut wrong_digest = digest;
    wrong_digest[0] ^= 1;
    let request = Request { version: protocol::VERSION, operation_id: "wrong-hash".into(),
        body: protocol::Command::Sign { expected_key_id: public.key_id.clone(), algorithm: Algorithm::RsaPkcs1Sha256,
            input: SignInput::Sha256Prehashed { digest_b64: STANDARD.encode(wrong_digest) } } };
    let ResponseBody::Signature { signature_b64, .. } = session.handle(&key, request).body else {
        panic!("Expected a signature over the supplied test digest");
    };
    std::fs::write(&signature_path, STANDARD.decode(signature_b64).unwrap()).unwrap();
    assert!(!verify(), "Wrong prehash must not verify for original message");
    let digest_path = directory.path().join("wrong-digest.bin");
    std::fs::write(&digest_path, wrong_digest).unwrap();
    let exact_digest_verification = Command::new("openssl").env("OPENSSL_CONF", "NUL")
        .env_remove("OPENSSL_MODULES").env_remove("OPENSSL_ENGINES")
        .args(["pkeyutl", "-verify", "-pubin", "-inkey"]).arg(&public_path)
        .args(["-keyform", "DER", "-in"]).arg(&digest_path)
        .arg("-sigfile").arg(&signature_path)
        .args(["-pkeyopt", "rsa_padding_mode:pkcs1", "-pkeyopt", "digest:sha256"])
        .output().expect("OpenSSL prehash oracle required");
    assert!(exact_digest_verification.status.success(), "Wrong-hash signature must verify for precisely that supplied digest");

    let request = || Request { version: protocol::VERSION, operation_id: "message-1".into(),
        body: protocol::Command::PublicKey {} };
    assert!(matches!(session.handle(&key, request()).body, ResponseBody::Error { code: ErrorCode::DuplicateOperation }));
    let wrong_key = Request { version: protocol::VERSION, operation_id: "wrong-key".into(),
        body: protocol::Command::Sign { expected_key_id: "0".repeat(64), algorithm: Algorithm::RsaPkcs1Sha256,
            input: SignInput::Message { bytes_b64: STANDARD.encode(message) } } };
    assert!(matches!(session.handle(&key, wrong_key).body, ResponseBody::Error { code: ErrorCode::WrongKey }));

    let mut wire = concat!(
        "{\"version\":1,\"operation_id\":\"wire-1\",\"body\":{\"kind\":\"public_key\"}}\n",
        "{\"version\":1,\"operation_id\":\"wire-1\",\"body\":{\"kind\":\"public_key\"}}\n",
        "{\"version\":1,\"operation_id\":\"wire-2\",\"body\":{\"kind\":\"sign\",\"expected_key_id\":\"x\",\"algorithm\":\"rsa_pss_sha256\",\"input\":{\"kind\":\"message\",\"bytes_b64\":\"\"}}}\n",
    ).as_bytes().to_vec();
    wire.extend(vec![b'x'; protocol::MAX_FRAME_BYTES + 1]);
    let mut output = Vec::new();
    protocol::serve(&key, std::io::Cursor::new(wire), &mut output).unwrap();
    let replies: Vec<serde_json::Value> = output.split(|b| *b == b'\n').filter(|v| !v.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap()).collect();
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["body"]["key_id"], public.key_id);
    assert_eq!(replies[1]["body"]["code"], "duplicate_operation");
    assert_eq!(replies[2]["body"]["code"], "malformed_request", "unknown algorithm never reaches CNG");
    assert_eq!(replies[3]["body"]["code"], "frame_too_large");
    // No persisted key deletion: dropping key invokes NCryptFreeObject only.
}
