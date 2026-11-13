//! Experimental portable test-key contract, not a PDF signing/validation backend.
//!
//! PATH: experiments/signature-key-broker/src/lib.rs
//! CONSUMED BY: this experiment's stdio binary and gated verification tests.
//! Only the Windows adapter exists; no identity/store access or private-key export.

pub mod protocol;
mod spki;
#[cfg(windows)]
pub mod windows;

/// Public material for one ephemeral synthetic key; no certificate/trust assertion.
#[derive(Clone)]
pub struct PublicKey {
    pub spki_der: Vec<u8>,
    /// Lowercase hex SHA-256 of the exact DER SPKI, for request/key binding.
    pub key_id: String,
}

/// Controlled errors contain only static operation names and public native codes.
#[derive(Debug)]
pub enum CryptoError {
    Native { operation: &'static str, status: i32 },
    InvalidPublicBlob,
    UnexpectedNativeOutput,
}

/// Portable primitive: the argument is already the SHA-256 digest (exactly once).
/// Message hashing, operation IDs, algorithm/key checks and replay policy belong
/// to the versioned protocol layer. Real user-key consent/authentication is absent.
pub trait TestKeyBroker {
    fn public_key(&self) -> &PublicKey;
    fn sign_sha256_prehashed(&self, digest: &[u8; 32]) -> Result<Vec<u8>, CryptoError>;
}
