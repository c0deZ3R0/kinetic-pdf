//! On-demand digital identity and PDF signing jobs; no startup crypto initialization.
//!
//! PATH: src/signing/mod.rs
//! CONSUMED BY: app Signatures panel and the isolated signing-helper entry point.
//! Passwords use owned memory/private stdin; opt-in persistence uses Windows
//! credential storage, never arguments or preferences. Visible appearances are signed.
//! Self-created IDs do not establish independently trusted identity. CMS integrity,
//! current-time cache-only certificate trust and post-signing changes are separate.
//! Publication uses a distinct integrity-only task; ordinary verification assesses trust.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub mod client;
pub mod credentials;
pub(crate) mod appearance;
pub(crate) mod cms;
pub(crate) mod identity;
pub(crate) mod native;
pub(crate) mod pdf;
pub(crate) mod trust;
pub use trust::{TrustReport, TrustCheck, RevocationStatus, TrustTimeBasis, TrustAnchorSource, RevocationMode, SigningTimeStatus, TrustIssue, TrustIssueCode};
pub mod service;

pub const HELPER_FLAG: &str = "--signature-service";
pub const PROTOCOL_VERSION: u16 = 3;
pub const MAX_PDF_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_PFX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Deliberately no Debug/Display. Own buffers are cleared on drop; no secure-erasure
/// claim is made for GUI, serializer, OS or native-library internal copies.
#[derive(Default, Serialize, Deserialize)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self { Self(value) }
    pub fn expose(&self) -> &str { &self.0 }
    pub fn edit(&mut self) -> &mut String { &mut self.0 }
    pub fn clear(&mut self) { self.0.zeroize(); }
}
impl Drop for Secret { fn drop(&mut self) { self.0.zeroize(); } }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIdentitySpec {
    pub name: String,
    pub organization: String,
    pub email: String,
    pub valid_years: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityInfo {
    pub subject: String,
    pub issuer: String,
    pub sha256_fingerprint: String,
    pub not_before: String,
    pub not_after: String,
    pub algorithm: String,
    pub key_bits: u32,
    pub self_signed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureMode {
    #[default]
    Approval,
    CertifyNoChanges,
}

/// Visible widget placement in unrotated PDF user space, never screen pixels.
/// `rotation` is the saved page's clockwise rotation in degrees, not quarter turns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureAppearance {
    pub page: usize,
    pub rect: [f32; 4],
    pub rotation: u16,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignOptions {
    pub mode: SignatureMode,
    pub reason: String,
    /// UI binds the operation to the certificate the user inspected/selected.
    /// None is reserved for callers explicitly choosing a PFX without prior inspection.
    #[serde(default)]
    pub expected_identity_fingerprint: Option<String>,
    /// None preserves invisible signing for callers not requesting an appearance.
    #[serde(default)]
    pub appearance: Option<SignatureAppearance>,
    /// Multi-page visual blocks covered by one document signature. The last
    /// block is the signature widget; earlier blocks are signed stamp appearances.
    /// Mutually exclusive with `appearance`; at most 256 blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<SignatureAppearance>,
}

/// Trust is deliberately not inferred from successful CMS verification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityTrust {
    #[default]
    NotChecked,
    Trusted,
    Untrusted,
    Indeterminate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureCheck {
    pub signer: Option<IdentityInfo>,
    pub integrity_valid: bool,
    pub identity_trust: IdentityTrust,
    /// None means certificate trust was not evaluated, not that it passed.
    #[serde(default)]
    pub trust_report: Option<TrustReport>,
    pub signed_revision_end: u64,
    pub file_bytes: u64,
    pub later_changes: bool,
    pub certification: Option<u8>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationReport { pub signatures: Vec<SignatureCheck> }

/// Private helper transport: callers use client jobs, not this wire format.
/// No Debug/Clone derives: variants contain passwords.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperRequest { pub version: u16, pub id: u64, pub task: HelperTask }

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum HelperTask {
    CreatePfx { spec: CreateIdentitySpec, password: Secret, output: PathBuf },
    InspectPfx { pfx: PathBuf, password: Secret },
    SignPdf { input: PathBuf, input_sha256: String, output: PathBuf, pfx: PathBuf, password: Secret, options: SignOptions },
    /// Explicit/reopened-document verification always assesses certificate trust.
    VerifyPdf { input: PathBuf, input_sha256: String },
    /// Pre-publication CMS/range verification only; never performs chain work.
    VerifyPublicationPdf { input: PathBuf, input_sha256: String },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperResponse { pub version: u16, pub id: u64, pub result: Result<HelperSuccess, String> }

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum HelperSuccess {
    IdentityCreated { info: IdentityInfo },
    IdentityInspected { info: IdentityInfo },
    PdfSigned { report: VerificationReport },
    PdfVerified { report: VerificationReport },
    PdfPublicationVerified { report: VerificationReport },
}

#[derive(Clone, Debug)]
pub enum JobOutcome {
    IdentityCreated { path: PathBuf, info: IdentityInfo },
    IdentityInspected { path: PathBuf, info: IdentityInfo },
    PdfSigned { path: PathBuf, report: VerificationReport },
    PdfVerified { report: VerificationReport },
}
