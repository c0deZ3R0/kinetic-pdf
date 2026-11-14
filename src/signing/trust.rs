//! Current-time, cache-only Windows certificate checks, separate from CMS integrity.
//!
//! PATH: src/signing/trust.rs
//! CONSUMED BY: signing PDF verification helper and portable Signatures UI reports.
//! Embedded certificates supply issuers, never anchors. No root installation,
//! user-key access, online fetching or historical signing-time trust is provided.

use serde::{Deserialize, Serialize};
use super::IdentityTrust;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustCheck { NotChecked, Passed, Failed, Indeterminate }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStatus { NotChecked, Good, Revoked, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTimeBasis { CurrentSystemTime }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustAnchorSource { WindowsCurrentUser }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationMode { CacheOnly }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningTimeStatus { NotEstablished }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustIssueCode {
    IncompleteChain, UntrustedRoot, ExplicitDistrust, InvalidChain,
    Expired, NotYetValid, InvalidUsage, UnsupportedUsage,
    RevocationUnavailable, Revoked, WeakAlgorithm, UnsupportedAlgorithm,
    InvalidPolicy, EvaluationUnavailable, WindowsApiError, BoundsExceeded,
    UnsupportedPlatform, UnknownNativeError, IssuesTruncated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustIssue {
    pub code: TrustIssueCode,
    /// Zero-based position in the primary chain: signer first. None is global.
    pub certificate_index: Option<u32>,
    pub native_error: Option<u32>,
}

/// Certificate assessment only: callers must independently retain CMS integrity,
/// signed-revision coverage and document-policy results. Revocation excludes the
/// trust anchor; Good requires affirmative evidence for every other certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustReport {
    pub checked_at_utc: String,
    pub time_basis: TrustTimeBasis,
    pub anchor_source: TrustAnchorSource,
    pub revocation_mode: RevocationMode,
    pub chain: TrustCheck,
    pub validity: TrustCheck,
    pub usage: TrustCheck,
    pub revocation: RevocationStatus,
    pub algorithm_policy: TrustCheck,
    pub signing_time: SigningTimeStatus,
    pub issues: Vec<TrustIssue>,
}

impl TrustReport {
    /// A failed attempt is distinct from no attempt (SignatureCheck::trust_report
    /// is None). This never changes the independently verified CMS result.
    pub fn unavailable(code: TrustIssueCode) -> Self {
        Self {
            checked_at_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            time_basis: TrustTimeBasis::CurrentSystemTime, anchor_source: TrustAnchorSource::WindowsCurrentUser,
            revocation_mode: RevocationMode::CacheOnly, chain: TrustCheck::Indeterminate,
            validity: TrustCheck::Indeterminate, usage: TrustCheck::Indeterminate,
            revocation: RevocationStatus::Unknown, algorithm_policy: TrustCheck::Indeterminate,
            signing_time: SigningTimeStatus::NotEstablished,
            issues: vec![TrustIssue { code, certificate_index: None, native_error: None }],
        }
    }

    /// This never asserts trusted signing time or whole-document validity.
    pub fn identity_trust(&self) -> IdentityTrust {
        let checks = [self.chain, self.validity, self.usage, self.algorithm_policy];
        if checks.contains(&TrustCheck::Failed) || self.revocation == RevocationStatus::Revoked {
            IdentityTrust::Untrusted
        } else if checks.iter().all(|check| *check == TrustCheck::Passed) && self.revocation == RevocationStatus::Good && self.issues.is_empty() {
            IdentityTrust::Trusted
        } else {
            IdentityTrust::Indeterminate
        }
    }
}

const MAX_ISSUES: usize = 8;

fn issue(report: &mut TrustReport, code: TrustIssueCode, certificate_index: Option<u32>, native_error: Option<u32>) {
    let entry = TrustIssue { code, certificate_index, native_error };
    if report.issues.contains(&entry) { return; }
    if report.issues.len() < MAX_ISSUES { report.issues.push(entry); }
    else { report.issues[MAX_ISSUES - 1] = TrustIssue { code: TrustIssueCode::IssuesTruncated, certificate_index: None, native_error: None }; }
}

/// Helper-only: no production caller roots, historical dates or network option.
/// Operational failure is an indeterminate report, never failed CMS mathematics.
pub(crate) fn evaluate(material: &super::cms::VerifiedSignerMaterial) -> TrustReport {
    let mut report = TrustReport::unavailable(TrustIssueCode::EvaluationUnavailable);
    report.issues.clear();
    #[cfg(windows)] {
        if let Err((code, native_error)) = platform::evaluate(material, &mut report) {
            report.chain = TrustCheck::Indeterminate;
            issue(&mut report, code, None, native_error);
        }
    }
    #[cfg(not(windows))] {
        let _ = material;
        issue(&mut report, TrustIssueCode::UnsupportedPlatform, None, None);
    }
    report
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::mem::size_of;
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use sha2::{Digest, Sha256};
    use crate::signing::cms::{VerifiedSignerMaterial, MAX_CERTIFICATES, MAX_CERTIFICATE_BYTES, MAX_MATERIAL_BYTES};
    use crate::signing::native::{c, Api, Cert, Store};
    use windows_sys::Win32::Foundation::{FILETIME, GetLastError};

    type Failure = (TrustIssueCode, Option<u32>);
    type Outcome<T> = Result<T, Failure>;
    const CRYPT_E_NOT_FOUND: u32 = 0x80092004;
    // Adobe's OIDs and Certificates / Security OIDs defines Security.5 as
    // authenticdocumentstrust EKU, under 1.2.840.113583.1.1:
    // https://www.adobe.com/devnet-docs/acrobatetk/tools/DigSigDC/oids.html#security-oids
    const ADOBE_DOCUMENT_SIGNING_OID: &[u8] = b"1.2.840.113583.1.1.5\0";
    // RFC 9336 section 3.1: id-kp-documentSigning ::= { id-kp 36 }.
    const RFC9336_DOCUMENT_SIGNING_OID: &[u8] = b"1.3.6.1.5.5.7.3.36\0";
    const MAX_CHAIN_ELEMENTS: usize = 64;
    const MAX_SIMPLE_CHAINS: usize = 8;
    const CHAIN_FLAGS: u32 = c::CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL | c::CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY
        | c::CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT | c::CERT_CHAIN_DISABLE_AIA
        | c::CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE | c::CERT_CHAIN_DISABLE_MY_PEER_TRUST
        | c::CERT_CHAIN_REVOCATION_ACCUMULATIVE_TIMEOUT;

    fn windows_ok(result: i32) -> Outcome<()> {
        if result != 0 { Ok(()) } else { Err((TrustIssueCode::WindowsApiError, Some(unsafe { GetLastError() }))) }
    }

    struct Engine { raw: c::HCERTCHAINENGINE, api: Arc<Api>, _stores: Vec<Store> }
    impl Drop for Engine {
        fn drop(&mut self) { if !self.raw.is_null() { unsafe { (self.api.CertFreeCertificateChainEngine)(self.raw); } } }
    }
    struct Chain { raw: *mut c::CERT_CHAIN_CONTEXT, _engine: Arc<Engine> }
    impl Drop for Chain {
        fn drop(&mut self) { if !self.raw.is_null() { unsafe { (self._engine.api.CertFreeCertificateChain)(self.raw); } } }
    }

    fn memory_store(api: &Arc<Api>) -> Outcome<Store> {
        let raw = unsafe { (api.CertOpenStore)(c::CERT_STORE_PROV_MEMORY, 0, 0, c::CERT_STORE_CREATE_NEW_FLAG, null()) };
        if raw.is_null() { return Err((TrustIssueCode::WindowsApiError, Some(unsafe { GetLastError() }))); }
        Ok(Store { raw, api: api.clone() })
    }

    fn certificate(api: &Arc<Api>, der: &[u8]) -> Outcome<Cert> {
        if der.is_empty() || der.len() > MAX_CERTIFICATE_BYTES { return Err((TrustIssueCode::BoundsExceeded, None)); }
        let raw = unsafe { (api.CertCreateCertificateContext)(c::X509_ASN_ENCODING, der.as_ptr(), der.len() as u32) };
        if raw.is_null() { return Err((TrustIssueCode::WindowsApiError, Some(unsafe { GetLastError() }))); }
        Ok(Cert { raw, api: api.clone() })
    }

    fn engine(api: &Arc<Api>, config: &c::CERT_CHAIN_ENGINE_CONFIG, stores: Vec<Store>) -> Outcome<Arc<Engine>> {
        let mut owner = Engine { raw: null_mut(), api: api.clone(), _stores: stores };
        windows_ok(unsafe { (api.CertCreateCertificateChainEngine)(config, &mut owner.raw) })?;
        if owner.raw.is_null() { return Err((TrustIssueCode::EvaluationUnavailable, None)); }
        Ok(Arc::new(owner))
    }

    fn config() -> c::CERT_CHAIN_ENGINE_CONFIG {
        c::CERT_CHAIN_ENGINE_CONFIG {
            cbSize: size_of::<c::CERT_CHAIN_ENGINE_CONFIG>() as u32,
            dwFlags: c::CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL | c::CERT_CHAIN_DISABLE_AIA,
            dwUrlRetrievalTimeout: 1000, MaximumCachedCertificates: 128,
            ..Default::default()
        }
    }

    pub(super) fn evaluate(material: &VerifiedSignerMaterial, report: &mut TrustReport) -> Outcome<()> {
        let api = Api::load().map_err(|_| (TrustIssueCode::EvaluationUnavailable, None))?;
        // Default scope is the current-user chain engine, including applicable
        // machine roots. No explicit root/TrustedPeople store is opened or edited.
        let engine = engine(&api, &config(), Vec::new())?;
        evaluate_on(&engine, material, report)
    }

    fn evaluate_on(engine: &Arc<Engine>, material: &VerifiedSignerMaterial, report: &mut TrustReport) -> Outcome<()> {
        let api = &engine.api;
        if material.supporting_certificates_der.len() >= MAX_CERTIFICATES { return Err((TrustIssueCode::BoundsExceeded, None)); }
        let mut total = material.signer_der.len();
        for der in &material.supporting_certificates_der {
            if der.is_empty() || der.len() > MAX_CERTIFICATE_BYTES { return Err((TrustIssueCode::BoundsExceeded, None)); }
            total = total.checked_add(der.len()).ok_or((TrustIssueCode::BoundsExceeded, None))?;
        }
        if total > MAX_MATERIAL_BYTES { return Err((TrustIssueCode::BoundsExceeded, None)); }
        let fingerprint: String = Sha256::digest(&material.signer_der).iter().map(|b| format!("{b:02x}")).collect();
        if !fingerprint.eq_ignore_ascii_case(&material.info.sha256_fingerprint) { return Err((TrustIssueCode::InvalidChain, None)); }
        let signer = certificate(api, &material.signer_der)?;
        report.algorithm_policy = combine(message_algorithm(material), algorithm(api, signer.raw, false));
        if report.algorithm_policy != TrustCheck::Passed {
            issue(report, if report.algorithm_policy == TrustCheck::Failed { TrustIssueCode::WeakAlgorithm } else { TrustIssueCode::UnsupportedAlgorithm }, Some(0), None);
        }
        let additional = memory_store(api)?;
        for der in &material.supporting_certificates_der {
            let cert = certificate(api, der)?;
            // A fresh cert-only memory store cannot promote embedded roots or CTLs.
            windows_ok(unsafe { (api.CertAddCertificateLinkToStore)(additional.raw, cert.raw, c::CERT_STORE_ADD_ALWAYS, null_mut()) })?;
        }
        let now = chrono::DateTime::parse_from_rfc3339(&report.checked_at_utc).map_err(|_| (TrustIssueCode::EvaluationUnavailable, None))?;
        let ticks = (now.timestamp() + 11_644_473_600) as u64 * 10_000_000;
        let time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        // The SDK defines this Microsoft document-signing purpose. Do not use
        // Authenticode/TLS policy as a substitute for PDF signing usage.
        let mut purposes = [RFC9336_DOCUMENT_SIGNING_OID.as_ptr().cast_mut(),
            c::szOID_KP_DOCUMENT_SIGNING.cast_mut(), ADOBE_DOCUMENT_SIGNING_OID.as_ptr().cast_mut()];
        // OS_1 is the SDK's ANSI form of the current built-in strong policy OID;
        // do not cast the SDK's UTF-16 OS_CURRENT constant to pszOID.
        let mut strong = c::CERT_STRONG_SIGN_PARA { cbSize: size_of::<c::CERT_STRONG_SIGN_PARA>() as u32,
            dwInfoChoice: c::CERT_STRONG_SIGN_OID_INFO_CHOICE,
            Anonymous: c::CERT_STRONG_SIGN_PARA_0 { pszOID: c::szOID_CERT_STRONG_SIGN_OS_1.cast_mut() } };
        let parameters = c::CERT_CHAIN_PARA {
            cbSize: size_of::<c::CERT_CHAIN_PARA>() as u32,
            RequestedUsage: c::CERT_USAGE_MATCH { dwType: c::USAGE_MATCH_TYPE_OR,
                Usage: c::CTL_USAGE { cUsageIdentifier: purposes.len() as u32, rgpszUsageIdentifier: purposes.as_mut_ptr() } },
            dwUrlRetrievalTimeout: 1000, pStrongSignPara: &mut strong, ..Default::default()
        };
        let mut chain = Chain { raw: null_mut(), _engine: engine.clone() };
        windows_ok(unsafe { (api.CertGetCertificateChain)(engine.raw, signer.raw, &time, additional.raw, &parameters,
            CHAIN_FLAGS, null(), &mut chain.raw) })?;
        if chain.raw.is_null() { return Err((TrustIssueCode::EvaluationUnavailable, None)); }
        inspect(&chain, material, &time, report)?;
        let policy = c::CERT_CHAIN_POLICY_PARA { cbSize: size_of::<c::CERT_CHAIN_POLICY_PARA>() as u32, ..Default::default() };
        let mut status = c::CERT_CHAIN_POLICY_STATUS { cbSize: size_of::<c::CERT_CHAIN_POLICY_STATUS>() as u32, ..Default::default() };
        windows_ok(unsafe { (api.CertVerifyCertificateChainPolicy)(c::CERT_CHAIN_POLICY_BASE, chain.raw, &policy, &mut status) })?;
        // TRUE says the API ran, not that policy passed. Preserve detailed native
        // chain findings; unexplained policy failure must never turn green.
        if status.dwError != 0 && unsafe { (*chain.raw).TrustStatus.dwErrorStatus } == 0 {
            report.chain = TrustCheck::Indeterminate;
            issue(report, TrustIssueCode::InvalidPolicy, None, Some(status.dwError));
        }
        let leaf_policy = c::CERT_CHAIN_POLICY_PARA { cbSize: size_of::<c::CERT_CHAIN_POLICY_PARA>() as u32,
            dwFlags: c::BASIC_CONSTRAINTS_CERT_CHAIN_POLICY_END_ENTITY_FLAG, ..Default::default() };
        let mut leaf_status = c::CERT_CHAIN_POLICY_STATUS { cbSize: size_of::<c::CERT_CHAIN_POLICY_STATUS>() as u32, ..Default::default() };
        windows_ok(unsafe { (api.CertVerifyCertificateChainPolicy)(c::CERT_CHAIN_POLICY_BASIC_CONSTRAINTS, chain.raw, &leaf_policy, &mut leaf_status) })?;
        if leaf_status.dwError != 0 {
            report.usage = TrustCheck::Failed;
            issue(report, TrustIssueCode::InvalidUsage, None, Some(leaf_status.dwError));
        }
        Ok(())
    }

    fn read_oid(pointer: *const u8) -> Option<String> {
        if pointer.is_null() { return None; }
        let mut bytes = Vec::new();
        for index in 0..128 {
            let byte = unsafe { *pointer.add(index) };
            if byte == 0 { return if bytes.is_empty() { None } else { String::from_utf8(bytes).ok() }; }
            if !byte.is_ascii_digit() && byte != b'.' { return None; }
            bytes.push(byte);
        }
        None
    }

    fn combine(current: TrustCheck, next: TrustCheck) -> TrustCheck {
        if current == TrustCheck::Failed || next == TrustCheck::Failed { TrustCheck::Failed }
        else if current != TrustCheck::Passed || next != TrustCheck::Passed { TrustCheck::Indeterminate }
        else { TrustCheck::Passed }
    }

    fn usage(api: &Api, cert: *const c::CERT_CONTEXT, leaf: bool) -> TrustCheck {
        let cert = unsafe { &*cert };
        if cert.pCertInfo.is_null() { return TrustCheck::Indeterminate; }
        let info = unsafe { &*cert.pCertInfo };
        if info.cExtension > 128 { return TrustCheck::Indeterminate; }
        if leaf && !unsafe { (api.CertFindExtension)(c::szOID_KEY_USAGE, info.cExtension, info.rgExtension) }.is_null() {
            let mut bits = [0u8; 2];
            if unsafe { (api.CertGetIntendedKeyUsage)(c::X509_ASN_ENCODING, cert.pCertInfo, bits.as_mut_ptr(), bits.len() as u32) } == 0 { return TrustCheck::Indeterminate; }
            if bits[0] & (c::CERT_DIGITAL_SIGNATURE_KEY_USAGE | c::CERT_NON_REPUDIATION_KEY_USAGE) as u8 == 0 { return TrustCheck::Failed; }
        }
        let mut size = 0;
        if unsafe { (api.CertGetEnhancedKeyUsage)(cert, 0, null_mut(), &mut size) } == 0 {
            return if unsafe { GetLastError() } == CRYPT_E_NOT_FOUND { TrustCheck::Passed } else { TrustCheck::Indeterminate };
        }
        if size < size_of::<c::CTL_USAGE>() as u32 || size > 64 * 1024 { return TrustCheck::Indeterminate; }
        let capacity = size;
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let pointer = storage.as_mut_ptr().cast::<c::CTL_USAGE>();
        if unsafe { (api.CertGetEnhancedKeyUsage)(cert, 0, pointer, &mut size) } == 0 { return TrustCheck::Indeterminate; }
        let last = unsafe { GetLastError() };
        if size < size_of::<c::CTL_USAGE>() as u32 || size > capacity { return TrustCheck::Indeterminate; }
        let eku = unsafe { &*pointer };
        if eku.cUsageIdentifier == 0 { return if last == CRYPT_E_NOT_FOUND { TrustCheck::Passed } else { TrustCheck::Failed }; }
        if eku.cUsageIdentifier > 128 || eku.rgpszUsageIdentifier.is_null() { return TrustCheck::Indeterminate; }
        let mut unsupported = false;
        for oid in unsafe { std::slice::from_raw_parts(eku.rgpszUsageIdentifier, eku.cUsageIdentifier as usize) } {
            match read_oid(*oid).as_deref() {
                Some("1.3.6.1.5.5.7.3.36" | "1.3.6.1.4.1.311.10.3.12" | "1.2.840.113583.1.1.5" | "2.5.29.37.0") => return TrustCheck::Passed,
                // Recognized non-document purposes explicitly prohibit this use;
                // other purposes are unsupported, never silently accepted.
                Some("1.3.6.1.5.5.7.3.1" | "1.3.6.1.5.5.7.3.2" | "1.3.6.1.5.5.7.3.3" | "1.3.6.1.5.5.7.3.4" | "1.3.6.1.5.5.7.3.8" | "1.3.6.1.5.5.7.3.9") => {}
                _ => unsupported = true,
            }
        }
        if unsupported { TrustCheck::Indeterminate } else { TrustCheck::Failed }
    }

    fn algorithm(api: &Api, cert: *const c::CERT_CONTEXT, root: bool) -> TrustCheck {
        let cert = unsafe { &*cert };
        if cert.pCertInfo.is_null() { return TrustCheck::Indeterminate; }
        let info = unsafe { &*cert.pCertInfo };
        let key = &info.SubjectPublicKeyInfo;
        let bits = unsafe { (api.CertGetPublicKeyLength)(c::X509_ASN_ENCODING, key) };
        let key_status = match read_oid(key.Algorithm.pszObjId).as_deref() {
            Some("1.2.840.113549.1.1.1") if (2048..=8192).contains(&bits) => TrustCheck::Passed,
            Some("1.2.840.113549.1.1.1") if bits != 0 && bits < 2048 => TrustCheck::Failed,
            _ => TrustCheck::Indeterminate,
        };
        if root { return key_status; } // Anchor self-signature is not the source of trust.
        let signature_status = match read_oid(info.SignatureAlgorithm.pszObjId).as_deref() {
            Some("1.2.840.113549.1.1.11" | "1.2.840.113549.1.1.12" | "1.2.840.113549.1.1.13") => TrustCheck::Passed,
            Some("1.2.840.113549.1.1.2" | "1.2.840.113549.1.1.3" | "1.2.840.113549.1.1.4" | "1.2.840.113549.1.1.5") => TrustCheck::Failed,
            _ => TrustCheck::Indeterminate,
        };
        combine(key_status, signature_status)
    }

    fn message_algorithm(material: &VerifiedSignerMaterial) -> TrustCheck {
        let digest = match material.digest_algorithm_oid.as_str() {
            "2.16.840.1.101.3.4.2.1" | "2.16.840.1.101.3.4.2.2" | "2.16.840.1.101.3.4.2.3" => TrustCheck::Passed,
            "1.2.840.113549.2.2" | "1.2.840.113549.2.4" | "1.2.840.113549.2.5" | "1.3.14.3.2.26" => TrustCheck::Failed,
            _ => TrustCheck::Indeterminate,
        };
        let signature = match material.signature_algorithm_oid.as_str() {
            "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.11" | "1.2.840.113549.1.1.12" | "1.2.840.113549.1.1.13" => TrustCheck::Passed,
            "1.2.840.113549.1.1.2" | "1.2.840.113549.1.1.3" | "1.2.840.113549.1.1.4" | "1.2.840.113549.1.1.5" => TrustCheck::Failed,
            _ => TrustCheck::Indeterminate,
        };
        combine(digest, signature)
    }

    fn inspect(chain: &Chain, material: &VerifiedSignerMaterial, time: &FILETIME, report: &mut TrustReport) -> Outcome<()> {
        let api = &chain._engine.api;
        let context = unsafe { &*chain.raw };
        if context.cChain == 0 || context.cChain as usize > MAX_SIMPLE_CHAINS || context.rgpChain.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
        report.chain = TrustCheck::Passed; report.validity = TrustCheck::Passed; report.usage = TrustCheck::Passed;
        report.algorithm_policy = message_algorithm(material);
        let mut errors = context.TrustStatus.dwErrorStatus;
        let mut checked = 0;
        let mut revoked = false;
        let mut revocation_unknown = false;
        for (chain_index, simple) in unsafe { std::slice::from_raw_parts(context.rgpChain, context.cChain as usize) }.iter().enumerate() {
            if simple.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
            let simple = unsafe { &**simple };
            if simple.cElement == 0 || simple.cElement as usize > MAX_CHAIN_ELEMENTS || simple.rgpElement.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
            errors |= simple.TrustStatus.dwErrorStatus;
            for (index, element) in unsafe { std::slice::from_raw_parts(simple.rgpElement, simple.cElement as usize) }.iter().enumerate() {
                if element.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
                let element = unsafe { &**element };
                errors |= element.TrustStatus.dwErrorStatus;
                if element.pCertContext.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
                if chain_index != 0 { continue; }
                let position = Some(index as u32);
                let cert = unsafe { &*element.pCertContext };
                if cert.pCertInfo.is_null() { return Err((TrustIssueCode::BoundsExceeded, None)); }
                let validity = unsafe { (api.CertVerifyTimeValidity)(time, cert.pCertInfo) };
                if validity != 0 {
                    report.validity = TrustCheck::Failed;
                    issue(report, if validity < 0 { TrustIssueCode::NotYetValid } else { TrustIssueCode::Expired }, position, None);
                }
                let purpose = usage(api, element.pCertContext, index == 0);
                report.usage = combine(report.usage, purpose);
                if purpose != TrustCheck::Passed { issue(report, if purpose == TrustCheck::Failed { TrustIssueCode::InvalidUsage } else { TrustIssueCode::UnsupportedUsage }, position, None); }
                let root = index + 1 == simple.cElement as usize;
                let crypto = algorithm(api, element.pCertContext, root);
                report.algorithm_policy = combine(report.algorithm_policy, crypto);
                if crypto != TrustCheck::Passed { issue(report, if crypto == TrustCheck::Failed { TrustIssueCode::WeakAlgorithm } else { TrustIssueCode::UnsupportedAlgorithm }, position, None); }
                if !root {
                    checked += 1;
                    if element.pRevocationInfo.is_null() { revocation_unknown = true; }
                    else {
                        let result = unsafe { (*element.pRevocationInfo).dwRevocationResult };
                        if result != 0 { revocation_unknown = true; }
                    }
                    revoked |= element.TrustStatus.dwErrorStatus & c::CERT_TRUST_IS_REVOKED != 0;
                    revocation_unknown |= element.TrustStatus.dwErrorStatus & (c::CERT_TRUST_REVOCATION_STATUS_UNKNOWN | c::CERT_TRUST_IS_OFFLINE_REVOCATION) != 0;
                }
            }
        }
        let revoked_bits = c::CERT_TRUST_IS_REVOKED;
        revoked |= errors & revoked_bits != 0;
        revocation_unknown |= errors & (c::CERT_TRUST_REVOCATION_STATUS_UNKNOWN | c::CERT_TRUST_IS_OFFLINE_REVOCATION | c::CERT_TRUST_IS_PARTIAL_CHAIN) != 0;
        report.revocation = if revoked { RevocationStatus::Revoked } else if checked != 0 && !revocation_unknown { RevocationStatus::Good } else { RevocationStatus::Unknown };
        if report.revocation != RevocationStatus::Good { issue(report, if revoked { TrustIssueCode::Revoked } else { TrustIssueCode::RevocationUnavailable }, None, None); }
        if errors & c::CERT_TRUST_IS_NOT_TIME_VALID != 0 { report.validity = TrustCheck::Failed; }
        if errors & c::CERT_TRUST_IS_NOT_TIME_NESTED != 0 {
            report.validity = combine(report.validity, TrustCheck::Indeterminate);
            issue(report, TrustIssueCode::InvalidPolicy, None, Some(c::CERT_TRUST_IS_NOT_TIME_NESTED));
        }
        if errors & c::CERT_TRUST_IS_NOT_VALID_FOR_USAGE != 0 && report.usage != TrustCheck::Indeterminate {
            report.usage = TrustCheck::Failed; issue(report, TrustIssueCode::InvalidUsage, None, None);
        }
        if errors & c::CERT_TRUST_HAS_WEAK_SIGNATURE != 0 { report.algorithm_policy = TrustCheck::Failed; }
        if report.algorithm_policy != TrustCheck::Passed { issue(report, if report.algorithm_policy == TrustCheck::Failed { TrustIssueCode::WeakAlgorithm } else { TrustIssueCode::UnsupportedAlgorithm }, None, None); }
        let invalid = c::CERT_TRUST_IS_NOT_SIGNATURE_VALID | c::CERT_TRUST_IS_CYCLIC | c::CERT_TRUST_INVALID_EXTENSION
            | c::CERT_TRUST_INVALID_POLICY_CONSTRAINTS | c::CERT_TRUST_INVALID_BASIC_CONSTRAINTS | c::CERT_TRUST_INVALID_NAME_CONSTRAINTS
            | c::CERT_TRUST_HAS_NOT_SUPPORTED_NAME_CONSTRAINT | c::CERT_TRUST_HAS_NOT_DEFINED_NAME_CONSTRAINT
            | c::CERT_TRUST_HAS_NOT_PERMITTED_NAME_CONSTRAINT | c::CERT_TRUST_HAS_EXCLUDED_NAME_CONSTRAINT
            | c::CERT_TRUST_HAS_NOT_SUPPORTED_CRITICAL_EXT | c::CERT_TRUST_NO_ISSUANCE_CHAIN_POLICY;
        if errors & invalid != 0 { report.chain = TrustCheck::Failed; issue(report, TrustIssueCode::InvalidChain, None, Some(errors & invalid)); }
        if errors & c::CERT_TRUST_IS_PARTIAL_CHAIN != 0 { report.chain = combine(report.chain, TrustCheck::Indeterminate); issue(report, TrustIssueCode::IncompleteChain, None, None); }
        if errors & c::CERT_TRUST_IS_UNTRUSTED_ROOT != 0 {
            if errors & c::CERT_TRUST_IS_PARTIAL_CHAIN == 0 { report.chain = TrustCheck::Failed; }
            issue(report, TrustIssueCode::UntrustedRoot, None, None);
        }
        if errors & c::CERT_TRUST_IS_EXPLICIT_DISTRUST != 0 { report.chain = TrustCheck::Failed; issue(report, TrustIssueCode::ExplicitDistrust, None, None); }
        let known = invalid | c::CERT_TRUST_IS_NOT_TIME_VALID | c::CERT_TRUST_IS_NOT_TIME_NESTED | c::CERT_TRUST_IS_NOT_VALID_FOR_USAGE | c::CERT_TRUST_HAS_WEAK_SIGNATURE
            | c::CERT_TRUST_IS_PARTIAL_CHAIN | c::CERT_TRUST_IS_UNTRUSTED_ROOT | c::CERT_TRUST_IS_EXPLICIT_DISTRUST
            | c::CERT_TRUST_REVOCATION_STATUS_UNKNOWN | c::CERT_TRUST_IS_OFFLINE_REVOCATION | revoked_bits;
        if errors & !known != 0 { report.chain = combine(report.chain, TrustCheck::Indeterminate); issue(report, TrustIssueCode::UnknownNativeError, None, Some(errors & !known)); }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::signing::native::{self, Key};
        use chrono::{Datelike, Timelike, Utc};
        use windows_sys::Win32::Foundation::SYSTEMTIME;

        struct Fixture { api: Arc<Api>, root: Cert, key: Key, leaf: Vec<u8> }

        fn system_time(time: chrono::DateTime<Utc>) -> SYSTEMTIME {
            SYSTEMTIME { wYear: time.year() as u16, wMonth: time.month() as u16, wDay: time.day() as u16,
                wHour: time.hour() as u16, wMinute: time.minute() as u16, wSecond: time.second() as u16, ..Default::default() }
        }
        fn file_time(time: chrono::DateTime<Utc>) -> FILETIME {
            let ticks = (time.timestamp() + 11_644_473_600) as u64 * 10_000_000;
            FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 }
        }
        fn key(api: &Arc<Api>, bits: u32) -> Key {
            let mut provider = Key { raw: 0, api: api.clone() };
            native::status(unsafe { (api.NCryptOpenStorageProvider)(&mut provider.raw, c::MS_KEY_STORAGE_PROVIDER, 0) }, "Open synthetic KSP").unwrap();
            let mut key = Key { raw: 0, api: api.clone() };
            // NULL names create new, ephemeral test keys, never user containers.
            native::status(unsafe { (api.NCryptCreatePersistedKey)(provider.raw, &mut key.raw, c::BCRYPT_RSA_ALGORITHM, null(), 0, 0) }, "Create synthetic key").unwrap();
            for (name, value) in [(c::NCRYPT_LENGTH_PROPERTY, bits), (c::NCRYPT_KEY_USAGE_PROPERTY, c::NCRYPT_ALLOW_SIGNING_FLAG)] {
                native::status(unsafe { (api.NCryptSetProperty)(key.raw, name, value.to_le_bytes().as_ptr(), 4, 0) }, "Configure synthetic key").unwrap();
            }
            native::status(unsafe { (api.NCryptFinalizeKey)(key.raw, c::NCRYPT_SILENT_FLAG) }, "Generate synthetic key").unwrap();
            key
        }
        fn der(cert: &Cert) -> Vec<u8> {
            let context = unsafe { &*cert.raw };
            unsafe { std::slice::from_raw_parts(context.pbCertEncoded, context.cbCertEncoded as usize) }.to_vec()
        }
        fn self_signed(api: &Arc<Api>, key: &Key, ca: bool) -> Cert {
            static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let text: Vec<u16> = format!("CN=Kinetic synthetic trust {id} {}", Utc::now().timestamp_nanos_opt().unwrap()).encode_utf16().chain(Some(0)).collect();
            // Use one immutable text for both calls: length changes are not safe.
            let mut size = 0;
            native::ok(unsafe { (api.CertStrToNameW)(c::X509_ASN_ENCODING, text.as_ptr(), c::CERT_X500_NAME_STR, null(), null_mut(), &mut size, null_mut()) }, "Size exact synthetic name").unwrap();
            let mut name = vec![0u8; size as usize];
            native::ok(unsafe { (api.CertStrToNameW)(c::X509_ASN_ENCODING, text.as_ptr(), c::CERT_X500_NAME_STR, null(), name.as_mut_ptr(), &mut size, null_mut()) }, "Encode synthetic name").unwrap();
            let blob = c::CRYPT_INTEGER_BLOB { cbData: size, pbData: name.as_mut_ptr() };
            let mut basic = if ca { vec![0x30, 3, 1, 1, 0xff] } else { vec![0x30, 0] };
            let mut usage = if ca { vec![3, 2, 1, 6] } else { vec![3, 2, 7, 0x80] };
            let mut extensions = [
                c::CERT_EXTENSION { pszObjId: c::szOID_BASIC_CONSTRAINTS2.cast_mut(), fCritical: 1, Value: c::CRYPT_INTEGER_BLOB { cbData: basic.len() as u32, pbData: basic.as_mut_ptr() } },
                c::CERT_EXTENSION { pszObjId: c::szOID_KEY_USAGE.cast_mut(), fCritical: 1, Value: c::CRYPT_INTEGER_BLOB { cbData: usage.len() as u32, pbData: usage.as_mut_ptr() } },
            ];
            let extensions = c::CERT_EXTENSIONS { cExtension: 2, rgExtension: extensions.as_mut_ptr() };
            let algorithm = c::CRYPT_ALGORITHM_IDENTIFIER { pszObjId: c::szOID_RSA_SHA256RSA.cast_mut(), ..Default::default() };
            let start = system_time(Utc::now() - chrono::Duration::days(30));
            let end = system_time(Utc::now() + chrono::Duration::days(365));
            let raw = unsafe { (api.CertCreateSelfSignCertificate)(key.raw, &blob, c::CERT_CREATE_SELFSIGN_NO_KEY_INFO, null(), &algorithm, &start, &end, &extensions) };
            assert!(!raw.is_null(), "Synthetic certificate generation failed");
            Cert { raw, api: api.clone() }
        }
        fn encode(api: &Api, key: &Key, kind: *const u8, input: *const std::ffi::c_void) -> Vec<u8> {
            let algorithm = c::CRYPT_ALGORITHM_IDENTIFIER { pszObjId: c::szOID_RSA_SHA256RSA.cast_mut(), ..Default::default() };
            let mut size = 0;
            native::ok(unsafe { (api.CryptSignAndEncodeCertificate)(key.raw, c::CERT_NCRYPT_KEY_SPEC, c::X509_ASN_ENCODING, kind, input, &algorithm, null(), null_mut(), &mut size) }, "Size synthetic signed object").unwrap();
            assert!(size != 0 && size as usize <= MAX_CERTIFICATE_BYTES);
            let mut bytes = vec![0u8; size as usize];
            native::ok(unsafe { (api.CryptSignAndEncodeCertificate)(key.raw, c::CERT_NCRYPT_KEY_SPEC, c::X509_ASN_ENCODING, kind, input, &algorithm, null(), bytes.as_mut_ptr(), &mut size) }, "Sign synthetic object").unwrap();
            bytes.truncate(size as usize); bytes
        }
        impl Fixture {
            fn new(bits: u32) -> Self {
                let api = Api::load().unwrap();
                let root_key = key(&api, 2048);
                let root = self_signed(&api, &root_key, true);
                let leaf_key = key(&api, bits);
                let template = self_signed(&api, &leaf_key, false);
                let mut info = unsafe { *(*template.raw).pCertInfo };
                info.Issuer = unsafe { (*(*root.raw).pCertInfo).Subject };
                info.NotBefore = file_time(Utc::now() - chrono::Duration::days(1));
                info.NotAfter = file_time(Utc::now() + chrono::Duration::days(7));
                let leaf = encode(&api, &root_key, c::X509_CERT_TO_BE_SIGNED, (&info as *const c::CERT_INFO).cast());
                Self { api, root, key: root_key, leaf }
            }
            fn material(&self, leaf: Vec<u8>) -> VerifiedSignerMaterial {
                let cert = certificate(&self.api, &leaf).unwrap();
                VerifiedSignerMaterial { info: crate::signing::identity::describe_certificate(&self.api, cert.raw).unwrap(),
                    signer_der: leaf, supporting_certificates_der: vec![der(&self.root)],
                    digest_algorithm_oid: "2.16.840.1.101.3.4.2.1".into(), signature_algorithm_oid: "1.2.840.113549.1.1.1".into() }
            }
            fn crl(&self, stale: bool, revoked: bool) -> Vec<u8> {
                let leaf = certificate(&self.api, &self.leaf).unwrap();
                let mut entry = c::CRL_ENTRY { SerialNumber: unsafe { (*(*leaf.raw).pCertInfo).SerialNumber },
                    RevocationDate: file_time(Utc::now() - chrono::Duration::hours(2)), ..Default::default() };
                let crl = c::CRL_INFO { SignatureAlgorithm: c::CRYPT_ALGORITHM_IDENTIFIER { pszObjId: c::szOID_RSA_SHA256RSA.cast_mut(), ..Default::default() },
                    Issuer: unsafe { (*(*self.root.raw).pCertInfo).Subject },
                    ThisUpdate: file_time(Utc::now() - chrono::Duration::days(if stale { 3 } else { 1 })),
                    NextUpdate: file_time(Utc::now() + chrono::Duration::days(if stale { -1 } else { 1 })),
                    cCRLEntry: u32::from(revoked), rgCRLEntry: if revoked { &mut entry } else { null_mut() }, ..Default::default() };
                encode(&self.api, &self.key, c::X509_CERT_CRL_TO_BE_SIGNED, (&crl as *const c::CRL_INFO).cast())
            }
            fn evaluate(&self, material: &VerifiedSignerMaterial, trusted_root: bool, crl: Option<&[u8]>) -> TrustReport {
                let roots = memory_store(&self.api).unwrap();
                if trusted_root { windows_ok(unsafe { (self.api.CertAddCertificateLinkToStore)(roots.raw, self.root.raw, c::CERT_STORE_ADD_ALWAYS, null_mut()) }).unwrap(); }
                let other = memory_store(&self.api).unwrap();
                if let Some(crl) = crl { windows_ok(unsafe { (self.api.CertAddEncodedCRLToStore)(other.raw, c::X509_ASN_ENCODING, crl.as_ptr(), crl.len() as u32, c::CERT_STORE_ADD_ALWAYS, null_mut()) }).unwrap(); }
                let ctl = memory_store(&self.api).unwrap();
                let mut additional = [other.raw];
                // Windows rejects mixing exclusive anchors with restricted
                // system-store handles. Exclusive roots/peers are both memory
                // stores; unique synthetic issuers cannot match ambient certs.
                let config = c::CERT_CHAIN_ENGINE_CONFIG { hExclusiveRoot: roots.raw, hExclusiveTrustedPeople: ctl.raw,
                    cAdditionalStore: 1, rghAdditionalStore: additional.as_mut_ptr(), ..config() };
                let engine = engine(&self.api, &config, vec![roots, other, ctl]).unwrap();
                let mut report = TrustReport::unavailable(TrustIssueCode::EvaluationUnavailable); report.issues.clear();
                if let Err((code, error)) = evaluate_on(&engine, material, &mut report) { report.chain = TrustCheck::Indeterminate; issue(&mut report, code, None, error); }
                report
            }
        }

        #[test]
        fn native_exclusive_memory_root_requires_affirmative_revocation() {
            let fixture = Fixture::new(2048);
            let material = fixture.material(fixture.leaf.clone());
            let unknown = fixture.evaluate(&material, true, None);
            assert_eq!(unknown.chain, TrustCheck::Passed, "{unknown:?}");
            assert_eq!(unknown.validity, TrustCheck::Passed);
            assert_eq!(unknown.usage, TrustCheck::Passed);
            assert_eq!(unknown.revocation, RevocationStatus::Unknown);
            assert_eq!(unknown.identity_trust(), IdentityTrust::Indeterminate);
            let crl = fixture.crl(false, false);
            let good = fixture.evaluate(&material, true, Some(&crl));
            assert_eq!(good.identity_trust(), IdentityTrust::Trusted, "{good:?}");
            let untrusted = fixture.evaluate(&material, false, Some(&crl));
            assert_ne!(untrusted.identity_trust(), IdentityTrust::Trusted, "Embedded root must not become an anchor");
        }
        #[test]
        fn native_stale_and_revoked_synthetic_crls_never_pass() {
            let fixture = Fixture::new(2048);
            let material = fixture.material(fixture.leaf.clone());
            let stale = fixture.crl(true, false);
            let report = fixture.evaluate(&material, true, Some(&stale));
            assert_eq!(report.revocation, RevocationStatus::Unknown, "{report:?}");
            assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
            let revoked = fixture.crl(false, true);
            let report = fixture.evaluate(&material, true, Some(&revoked));
            assert_eq!(report.revocation, RevocationStatus::Revoked, "{report:?}");
            assert_eq!(report.identity_trust(), IdentityTrust::Untrusted);
        }
        #[test]
        fn native_invalid_crl_signature_does_not_supply_good_revocation() {
            let fixture = Fixture::new(2048);
            let material = fixture.material(fixture.leaf.clone());
            let mut crl = fixture.crl(false, false); *crl.last_mut().unwrap() ^= 1;
            let report = fixture.evaluate(&material, true, Some(&crl));
            assert_eq!(report.revocation, RevocationStatus::Unknown, "{report:?}");
            assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
        }
        #[test]
        fn native_non_ca_intermediate_cannot_issue_a_trusted_leaf() {
            let fixture = Fixture::new(2048);
            let middle_key = key(&fixture.api, 2048);
            let middle_template = self_signed(&fixture.api, &middle_key, false);
            let mut middle_info = unsafe { *(*middle_template.raw).pCertInfo };
            middle_info.Issuer = unsafe { (*(*fixture.root.raw).pCertInfo).Subject };
            middle_info.NotBefore = file_time(Utc::now() - chrono::Duration::days(7));
            middle_info.NotAfter = file_time(Utc::now() + chrono::Duration::days(15));
            let middle_der = encode(&fixture.api, &fixture.key, c::X509_CERT_TO_BE_SIGNED, (&middle_info as *const c::CERT_INFO).cast());
            let middle = certificate(&fixture.api, &middle_der).unwrap();
            let leaf_key = key(&fixture.api, 2048);
            let leaf_template = self_signed(&fixture.api, &leaf_key, false);
            let mut leaf_info = unsafe { *(*leaf_template.raw).pCertInfo };
            leaf_info.Issuer = unsafe { (*(*middle.raw).pCertInfo).Subject };
            leaf_info.NotBefore = file_time(Utc::now() - chrono::Duration::days(1));
            leaf_info.NotAfter = file_time(Utc::now() + chrono::Duration::days(7));
            let leaf = encode(&fixture.api, &middle_key, c::X509_CERT_TO_BE_SIGNED, (&leaf_info as *const c::CERT_INFO).cast());
            let mut material = fixture.material(leaf); material.supporting_certificates_der.push(middle_der);
            let report = fixture.evaluate(&material, true, None);
            assert_eq!(report.chain, TrustCheck::Failed, "{report:?}");
            assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
        }
        #[test]
        fn native_bad_issuer_signature_and_weak_key_are_not_cms_math_results() {
            let fixture = Fixture::new(2048);
            let mut bad = fixture.leaf.clone(); *bad.last_mut().unwrap() ^= 1;
            let report = fixture.evaluate(&fixture.material(bad), true, None);
            assert_eq!(report.chain, TrustCheck::Failed, "{report:?}");
            let weak = Fixture::new(1024);
            let report = weak.evaluate(&weak.material(weak.leaf.clone()), true, None);
            assert_eq!(report.algorithm_policy, TrustCheck::Failed, "{report:?}");
        }
        #[test]
        fn native_claimed_signing_dates_do_not_rescue_expired_certificates() {
            let fixture = Fixture::new(2048);
            let cert = certificate(&fixture.api, &fixture.leaf).unwrap();
            let mut info = unsafe { *(*cert.raw).pCertInfo };
            info.NotBefore = file_time(Utc::now() - chrono::Duration::days(7));
            info.NotAfter = file_time(Utc::now() - chrono::Duration::days(1));
            let expired = encode(&fixture.api, &fixture.key, c::X509_CERT_TO_BE_SIGNED, (&info as *const c::CERT_INFO).cast());
            let report = fixture.evaluate(&fixture.material(expired), true, None);
            assert_eq!(report.validity, TrustCheck::Failed, "{report:?}");
            assert_eq!(report.signing_time, SigningTimeStatus::NotEstablished);
            assert!(report.issues.iter().any(|issue| issue.code == TrustIssueCode::Expired));
        }
        #[test]
        fn native_usage_absence_prohibition_and_unknown_purpose_are_distinct() {
            let fixture = Fixture::new(2048);
            let cert = certificate(&fixture.api, &fixture.leaf).unwrap();
            let original = unsafe { *(*cert.raw).pCertInfo };
            let base = unsafe { std::slice::from_raw_parts(original.rgExtension, original.cExtension as usize) }.to_vec();
            for (encoded, expected) in [
                (None, TrustCheck::Passed),
                (Some(vec![0x30, 0x0a, 0x06, 8, 0x2b, 6, 1, 5, 5, 7, 3, 36]), TrustCheck::Passed),
                (Some(vec![0x30, 0x0c, 0x06, 0x0a, 0x2b, 6, 1, 4, 1, 0x82, 0x37, 0x0a, 3, 0x0c]), TrustCheck::Passed),
                (Some(vec![0x30, 0x0b, 0x06, 9, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x2f, 1, 1, 5]), TrustCheck::Passed),
                (Some(vec![0x30, 6, 0x06, 4, 0x55, 0x1d, 0x25, 0]), TrustCheck::Passed),
                (Some(vec![0x30, 0x0a, 0x06, 8, 0x2b, 6, 1, 5, 5, 7, 3, 1]), TrustCheck::Failed),
                (Some(vec![0x30, 0x0a, 0x06, 8, 0x2b, 6, 1, 5, 5, 7, 3, 3]), TrustCheck::Failed),
                (Some(vec![0x30, 0x0a, 0x06, 8, 0x2b, 6, 1, 5, 5, 7, 3, 4]), TrustCheck::Failed),
                (Some(vec![0x30, 4, 0x06, 2, 0x2a, 3]), TrustCheck::Indeterminate),
                (Some(vec![0x30, 0]), TrustCheck::Failed),
            ] {
                let mut extensions: Vec<_> = base.iter().copied().filter(|extension| read_oid(extension.pszObjId).as_deref() != Some("2.5.29.15")).collect();
                let mut encoded = encoded;
                if let Some(bytes) = encoded.as_mut() {
                    extensions.push(c::CERT_EXTENSION { pszObjId: c::szOID_ENHANCED_KEY_USAGE.cast_mut(), fCritical: 0,
                        Value: c::CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_mut_ptr() } });
                }
                let mut info = original; info.cExtension = extensions.len() as u32; info.rgExtension = extensions.as_mut_ptr();
                let leaf = encode(&fixture.api, &fixture.key, c::X509_CERT_TO_BE_SIGNED, (&info as *const c::CERT_INFO).cast());
                let report = fixture.evaluate(&fixture.material(leaf), true, None);
                assert_eq!(report.usage, expected, "{report:?}");
            }
        }
        #[test]
        fn native_future_validity_and_missing_issuer_remain_distinct() {
            let fixture = Fixture::new(2048);
            let cert = certificate(&fixture.api, &fixture.leaf).unwrap();
            let original = unsafe { *(*cert.raw).pCertInfo };
            let mut future = original;
            future.NotBefore = file_time(Utc::now() + chrono::Duration::days(1));
            let leaf = encode(&fixture.api, &fixture.key, c::X509_CERT_TO_BE_SIGNED, (&future as *const c::CERT_INFO).cast());
            let report = fixture.evaluate(&fixture.material(leaf), true, None);
            assert_eq!(report.validity, TrustCheck::Failed, "{report:?}");
            assert!(report.issues.iter().any(|issue| issue.code == TrustIssueCode::NotYetValid));
            let unknown_issuer = self_signed(&fixture.api, &fixture.key, true);
            let mut incomplete = original; incomplete.Issuer = unsafe { (*(*unknown_issuer.raw).pCertInfo).Subject };
            let leaf = encode(&fixture.api, &fixture.key, c::X509_CERT_TO_BE_SIGNED, (&incomplete as *const c::CERT_INFO).cast());
            let report = fixture.evaluate(&fixture.material(leaf), true, None);
            assert_eq!(report.chain, TrustCheck::Indeterminate, "{report:?}");
            assert_eq!(report.revocation, RevocationStatus::Unknown);
            assert!(report.issues.iter().any(|issue| issue.code == TrustIssueCode::IncompleteChain));
        }
        #[test]
        fn native_bounds_unknown_digest_and_cache_only_flags_fail_closed() {
            let fixture = Fixture::new(2048);
            let mut material = fixture.material(fixture.leaf.clone());
            material.digest_algorithm_oid = "1.2.3.4".into();
            let report = fixture.evaluate(&material, true, None);
            assert_eq!(report.algorithm_policy, TrustCheck::Indeterminate);
            material.signer_der = vec![0; MAX_CERTIFICATE_BYTES + 1];
            let report = fixture.evaluate(&material, true, None);
            assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
            let required = c::CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL | c::CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY
                | c::CERT_CHAIN_DISABLE_AIA | c::CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE | c::CERT_CHAIN_DISABLE_MY_PEER_TRUST;
            assert_eq!(CHAIN_FLAGS & required, required);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn passed() -> TrustReport {
        let mut report = TrustReport::unavailable(TrustIssueCode::EvaluationUnavailable);
        report.issues.clear(); report.chain = TrustCheck::Passed; report.validity = TrustCheck::Passed;
        report.usage = TrustCheck::Passed; report.algorithm_policy = TrustCheck::Passed; report.revocation = RevocationStatus::Good;
        report
    }
    #[test]
    fn every_axis_and_unknown_issue_prevents_an_unqualified_green_result() {
        let report = passed(); assert_eq!(report.identity_trust(), IdentityTrust::Trusted);
        for status in [TrustCheck::NotChecked, TrustCheck::Failed, TrustCheck::Indeterminate] {
            for axis in 0..4 {
                let mut report = passed();
                match axis { 0 => report.chain = status, 1 => report.validity = status, 2 => report.usage = status, _ => report.algorithm_policy = status }
                assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
            }
        }
        for status in [RevocationStatus::NotChecked, RevocationStatus::Unknown, RevocationStatus::Revoked] {
            let mut report = passed(); report.revocation = status; assert_ne!(report.identity_trust(), IdentityTrust::Trusted);
        }
        let mut report = passed(); issue(&mut report, TrustIssueCode::UnknownNativeError, None, Some(0xffffffff));
        assert_eq!(report.identity_trust(), IdentityTrust::Indeterminate);
    }
    #[test]
    fn reports_are_portable_bounded_and_never_claim_trusted_signing_time() {
        let mut report = passed();
        for index in 0..100 { issue(&mut report, TrustIssueCode::UnknownNativeError, Some(index), None); }
        assert_eq!(report.issues.len(), MAX_ISSUES);
        assert_eq!(report.issues.last().unwrap().code, TrustIssueCode::IssuesTruncated);
        let bytes = serde_json::to_vec(&report).unwrap();
        let decoded: TrustReport = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, report);
        assert_eq!(decoded.time_basis, TrustTimeBasis::CurrentSystemTime);
        assert_eq!(decoded.signing_time, SigningTimeStatus::NotEstablished);
        assert_eq!(decoded.identity_trust(), IdentityTrust::Indeterminate);
    }
}
