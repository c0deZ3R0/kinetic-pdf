//! Version-1 bounded stdio protocol for an ephemeral test-key broker.
//!
//! PATH: experiments/signature-key-broker/src/protocol.rs
//! CONSUMED BY: experiment binary, portable-engine callers and contract tests.
//! No input command paths, handles, provider/key names, network or message logging.

use std::collections::HashSet;
use std::io::{self, BufRead, Write};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::TestKeyBroker;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 16 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 4096;
pub const MAX_REQUESTS: usize = 128;
const MAX_BASE64_MESSAGE: usize = ((MAX_MESSAGE_BYTES + 2) / 3) * 4;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm { RsaPkcs1Sha256 }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub operation_id: String,
    pub body: Command,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    PublicKey {},
    Sign { expected_key_id: String, algorithm: Algorithm, input: SignInput },
}

// Deliberately no Debug derive: message/digest bytes must not be dumped to logs.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignInput {
    Message { bytes_b64: String },
    Sha256Prehashed { digest_b64: String },
}

impl SignInput {
    pub fn sha256_digest(&self) -> Result<[u8; 32], ErrorCode> {
        match self {
            Self::Message { bytes_b64 } => {
                if bytes_b64.len() > MAX_BASE64_MESSAGE { return Err(ErrorCode::InputTooLarge); }
                let bytes = STANDARD.decode(bytes_b64).map_err(|_| ErrorCode::InvalidInput)?;
                if bytes.len() > MAX_MESSAGE_BYTES { return Err(ErrorCode::InputTooLarge); }
                Ok(Sha256::digest(bytes).into())
            }
            Self::Sha256Prehashed { digest_b64 } => {
                if digest_b64.len() != 44 { return Err(ErrorCode::InvalidInput); }
                let digest = STANDARD.decode(digest_b64).map_err(|_| ErrorCode::InvalidInput)?;
                digest.try_into().map_err(|_| ErrorCode::InvalidInput)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    MalformedRequest,
    InvalidVersion,
    InvalidOperationId,
    DuplicateOperation,
    SessionLimit,
    InvalidInput,
    InputTooLarge,
    WrongKey,
    BackendFailure,
    FrameTooLarge,
}

#[derive(Serialize)]
pub struct Response {
    pub version: u16,
    pub operation_id: Option<String>,
    pub body: ResponseBody,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseBody {
    PublicKey { algorithm: Algorithm, key_id: String, spki_der_b64: String, test_only: bool },
    Signature { algorithm: Algorithm, key_id: String, signature_b64: String, test_only: bool },
    Error { code: ErrorCode },
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[derive(Default)]
pub struct Session { seen: HashSet<String> }

impl Session {
    pub fn admit(&mut self, request: &Request) -> Result<(), ErrorCode> {
        if request.version != VERSION { return Err(ErrorCode::InvalidVersion); }
        if !valid_id(&request.operation_id) { return Err(ErrorCode::InvalidOperationId); }
        if self.seen.contains(&request.operation_id) { return Err(ErrorCode::DuplicateOperation); }
        if self.seen.len() >= MAX_REQUESTS { return Err(ErrorCode::SessionLimit); }
        self.seen.insert(request.operation_id.clone());
        Ok(())
    }

    pub fn handle(&mut self, broker: &impl TestKeyBroker, request: Request) -> Response {
        // Never echo invalid/arbitrarily long request IDs or request contents.
        let id = valid_id(&request.operation_id).then(|| request.operation_id.clone());
        let result = (|| {
            self.admit(&request)?;
            let public = broker.public_key();
            match request.body {
                Command::PublicKey {} => Ok(ResponseBody::PublicKey {
                    algorithm: Algorithm::RsaPkcs1Sha256, key_id: public.key_id.clone(),
                    spki_der_b64: STANDARD.encode(&public.spki_der), test_only: true,
                }),
                Command::Sign { expected_key_id, algorithm, input } => {
                    if expected_key_id != public.key_id { return Err(ErrorCode::WrongKey); }
                    let digest = input.sha256_digest()?;
                    let signature = broker.sign_sha256_prehashed(&digest).map_err(|_| ErrorCode::BackendFailure)?;
                    Ok(ResponseBody::Signature { algorithm, key_id: public.key_id.clone(),
                        signature_b64: STANDARD.encode(signature), test_only: true })
                }
            }
        })();
        Response { version: VERSION, operation_id: id,
            body: result.unwrap_or_else(|code| ResponseBody::Error { code }) }
    }
}

/// Read without unbounded read_line allocation; an oversized frame ends the session.
/// A final non-newline frame at EOF is supported. Input cannot choose filesystem paths.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() { return Ok((!frame.is_empty()).then_some(frame)); }
        let count = buffer.iter().position(|b| *b == b'\n').map_or(buffer.len(), |n| n + 1);
        if frame.len() + count > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        frame.extend_from_slice(&buffer[..count]);
        let complete = buffer[count - 1] == b'\n';
        reader.consume(count);
        if complete { return Ok(Some(frame)); }
    }
}

fn write_response(writer: &mut impl Write, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, response).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// One test key/session; closes after 128 frames. No arbitrary error detail logging.
pub fn serve(broker: &impl TestKeyBroker, mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
    let mut session = Session::default();
    for _ in 0..MAX_REQUESTS {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                write_response(&mut writer, &Response { version: VERSION, operation_id: None,
                    body: ResponseBody::Error { code: ErrorCode::FrameTooLarge } })?;
                break;
            }
            Err(e) => return Err(e),
        };
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(request) => session.handle(broker, request),
            Err(_) => Response { version: VERSION, operation_id: None,
                body: ResponseBody::Error { code: ErrorCode::MalformedRequest } },
        };
        write_response(&mut writer, &response)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn messages_hash_once_and_prehashes_are_not_hashed_again() {
        let expected: [u8; 32] = Sha256::digest(b"synthetic broker message").into();
        let message = SignInput::Message { bytes_b64: STANDARD.encode(b"synthetic broker message") };
        let prehash = SignInput::Sha256Prehashed { digest_b64: STANDARD.encode(expected) };
        assert_eq!(message.sha256_digest().unwrap(), expected);
        assert_eq!(prehash.sha256_digest().unwrap(), expected);
        assert!(SignInput::Sha256Prehashed { digest_b64: STANDARD.encode([0u8; 31]) }.sha256_digest().is_err());
        assert!(SignInput::Sha256Prehashed { digest_b64: STANDARD.encode([0u8; 33]) }.sha256_digest().is_err());
        assert!(SignInput::Message { bytes_b64: "!".into() }.sha256_digest().is_err());
        assert!(SignInput::Message { bytes_b64: STANDARD.encode(vec![0; MAX_MESSAGE_BYTES + 1]) }.sha256_digest().is_err());
    }

    #[test]
    fn strict_shape_ids_version_and_replay_checks() {
        let request = r#"{"version":1,"operation_id":"op-1","body":{"kind":"public_key"}}"#;
        let mut request: Request = serde_json::from_str(request).unwrap();
        let mut session = Session::default();
        assert_eq!(session.admit(&request), Ok(()));
        assert_eq!(session.admit(&request), Err(ErrorCode::DuplicateOperation));
        request.version = 2;
        assert_eq!(session.admit(&request), Err(ErrorCode::InvalidVersion));
        request.version = 1; request.operation_id = "x".repeat(65);
        assert_eq!(session.admit(&request), Err(ErrorCode::InvalidOperationId));
        assert!(serde_json::from_str::<Request>(r#"{"version":1,"operation_id":"op","body":{"kind":"public_key","path":"forbidden"}}"#).is_err());
        assert!(serde_json::from_str::<SignInput>(r#"{"kind":"sha256_prehashed","digest_b64":"","bytes_b64":""}"#).is_err());
        assert!(serde_json::from_str::<Algorithm>(r#""rsa_pss_sha256""#).is_err());
        assert!(serde_json::from_str::<Algorithm>(r#""rsa_pkcs1_sha1""#).is_err());
        let mut session = Session::default();
        for number in 0..MAX_REQUESTS {
            let request = Request { version: VERSION, operation_id: format!("op-{number}"), body: Command::PublicKey {} };
            assert_eq!(session.admit(&request), Ok(()));
        }
        let request = Request { version: VERSION, operation_id: "one-too-many".into(), body: Command::PublicKey {} };
        assert_eq!(session.admit(&request), Err(ErrorCode::SessionLimit));
    }

    #[test]
    fn framing_is_bounded_and_final_eof_frame_is_retained() {
        let mut reader = Cursor::new(b"one\ntwo");
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"one\n");
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"two");
        assert!(read_frame(&mut reader).unwrap().is_none());
        assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1])).is_err());
    }
}
