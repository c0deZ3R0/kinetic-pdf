//! Append-only first-signature PDF writer and byte-range/CMS integrity inspection.
//!
//! PATH: src/signing/pdf.rs
//! CONSUMED BY: isolated signing service only; no PDFium/UI/native keys on UI thread.
//! Approval or P1 certification binds all finalized visual blocks before hashing:
//! earlier blocks are ordinary stamps; only the last is the single signature widget.
//! Unsigned incremental history is
//! inspected then normalized to a fresh working copy; signed/ambiguous history
//! is refused. Prefix preservation refers to that disclosed prepared copy.
//! CMS/range integrity is independent of current-time, cache-only Windows trust.
//! Pre-publication verification never evaluates trust; ordinary verify does.
//! Trust failure/uncertainty never vetoes signed-copy publication; no historical
//! signing-time trust, PAdES/QES/LTV or full permission-validation claim is made.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use pdf_content::lopdf::{dictionary, Dictionary, Document, IncrementalDocument, LoadOptions, Object, ObjectId, StringFormat};
use pdf_content::lopdf::xref::XrefEntry;
use super::{cms, identity::Identity, IdentityInfo, IdentityTrust, SignOptions, SignatureCheck, SignatureMode, TrustIssueCode, TrustReport, VerificationReport, MAX_PDF_BYTES};

const RESERVE: usize = 32 * 1024;
const MAX_NODES: usize = 100_000;
const MAX_SIGNATURES: usize = 32;
const PLACEHOLDER: i64 = 9_999_999_999;
pub(crate) const NORMALIZED_NOTE: &str = "Unsigned revision history was normalized to a fresh working copy before signing; earlier signature history is not preserved or validated.";

fn load(bytes: &[u8]) -> Result<Document, String> {
    if bytes.is_empty() || bytes.len() > MAX_PDF_BYTES { return Err("PDF exceeds the signing size limit or is empty.".into()); }
    let doc = Document::load_mem_with_options(bytes, LoadOptions {
        strict: true, max_decompressed_size: Some(16 * 1024 * 1024), ..Default::default()
    }).map_err(|_| "Malformed or unsupported PDF structure.".to_owned())?;
    if doc.is_encrypted() || doc.trailer.has(b"Encrypt") || doc.encryption_state.is_some() { return Err("Encrypted PDFs are not supported for signing/verification.".into()); }
    if doc.objects.len() > MAX_NODES || doc.xref_start == 0 { return Err("Unsupported PDF object count or recovered cross-reference table.".into()); }
    Ok(doc)
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> Result<&'a Object, String> {
    doc.dereference(object).map(|(_, o)| o).map_err(|_| "Unresolved PDF object reference.".into())
}
fn name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Result<Option<&'a [u8]>, String> {
    dict.get(key).ok().map(|v| resolve(doc, v)?.as_name().map_err(|_| "Malformed PDF name.".into())).transpose()
}

fn candidates<'a>(doc: &'a Document) -> Result<Vec<(Option<ObjectId>, &'a Dictionary)>, String> {
    struct Walk<'a> { doc: &'a Document, seen: HashSet<ObjectId>, signatures: HashSet<ObjectId>, out: Vec<(Option<ObjectId>, &'a Dictionary)>, nodes: usize }
    impl<'a> Walk<'a> {
        fn add(&mut self, value: &'a Object) -> Result<(), String> {
            let (id, value) = self.doc.dereference(value).map_err(|_| "Unresolved signature value.")?;
            let dict = value.as_dict().map_err(|_| "Populated signature field has an unsupported non-dictionary value.")?;
            if id.map_or_else(|| !self.out.iter().any(|(_, old)| std::ptr::eq(*old, dict)), |id| self.signatures.insert(id)) {
                self.out.push((id, dict));
            }
            if self.out.len() > MAX_SIGNATURES { return Err("Too many PDF signatures for bounded verification.".into()); }
            Ok(())
        }
        fn field(&mut self, value: &'a Object, inherited_kind: Option<&'a [u8]>, inherited_value: Option<&'a Object>, depth: usize) -> Result<(), String> {
            self.nodes += 1;
            if depth > 64 || self.nodes > MAX_NODES { return Err("PDF field traversal limit exceeded.".into()); }
            let (id, value) = self.doc.dereference(value).map_err(|_| "Unresolved form field.")?;
            if id.is_some_and(|id| !self.seen.insert(id)) { return Err("Repeated/cyclic form field.".into()); }
            let field = value.as_dict().map_err(|_| "Malformed form field.")?;
            let kind = name(self.doc, field, b"FT")?.or(inherited_kind);
            let value = match field.get(b"V").ok() {
                Some(v) => { if matches!(resolve(self.doc, v)?, Object::Null) { None } else { Some(v) } }
                None => inherited_value,
            };
            if value.is_some() && !matches!(kind, Some(b"Sig" | b"Tx" | b"Btn" | b"Ch")) { return Err("Populated field has unknown/missing type; signing state is ambiguous.".into()); }
            if kind == Some(b"Sig".as_slice()) {
                if let Some(raw) = value { self.add(raw)?; }
            }
            if let Ok(kids) = field.get(b"Kids") {
                for kid in resolve(self.doc, kids)?.as_array().map_err(|_| "Malformed field children.")? {
                    self.field(kid, kind, value, depth + 1)?;
                }
            }
            Ok(())
        }
    }
    let mut walk = Walk { doc, seen: HashSet::new(), signatures: HashSet::new(), out: vec![], nodes: 0 };
    if let Ok(form) = doc.catalog().map_err(|_| "Missing PDF catalog.")?.get(b"AcroForm") {
        let form = resolve(doc, form)?.as_dict().map_err(|_| "Malformed AcroForm.")?;
        if form.has(b"XFA") { return Err("XFA PDFs are unsupported.".into()); }
        for field in resolve(doc, form.get(b"Fields").map_err(|_| "Missing form field tree.")?)?.as_array().map_err(|_| "Malformed form field tree.")? {
            walk.field(field, None, None, 0)?;
        }
    }
    for (&id, object) in &doc.objects {
        if let Ok(dict) = object.as_dict() {
            if dict.has(b"ByteRange") || matches!(name(doc, dict, b"Type")?, Some(b"Sig" | b"DocTimeStamp")) {
                if walk.signatures.insert(id) { walk.out.push((Some(id), dict)); }
            }
        }
    }
    if walk.out.len() > MAX_SIGNATURES { return Err("Too many signatures.".into()); }
    Ok(walk.out)
}

fn unsigned_graph_guard(doc: &Document) -> Result<(), String> {
    if doc.catalog().map_err(|_| "Missing catalog.")?.has(b"Perms") {
        return Err("PDF has signatures, certification, or ambiguous signature history; signing is refused.".into());
    }
    if !candidates(doc)?.is_empty() { return Err("PDF already has populated signatures; co-signing is not supported.".into()); }
    fn inspect(doc: &Document, value: &Object, depth: usize, budget: &mut usize) -> Result<(), String> {
        if depth > 64 || *budget == 0 { return Err("PDF signature inspection limit exceeded.".into()); }
        *budget -= 1;
        let dict = match value { Object::Dictionary(d) => Some(d), Object::Stream(s) => Some(&s.dict), Object::Array(a) => {
            for o in a { inspect(doc, o, depth + 1, budget)?; } None
        }, _ => None };
        if let Some(d) = dict {
            if d.has(b"ByteRange") || d.has(b"Perms") || matches!(name(doc, d, b"Type")?, Some(b"Sig" | b"DocTimeStamp"))
                || matches!(name(doc, d, b"TransformMethod")?, Some(b"DocMDP" | b"FieldMDP"))
            { return Err("Existing signature/certification objects are unsupported.".into()); }
            if name(doc, d, b"FT")? == Some(b"Sig".as_slice()) {
                if let Ok(v) = d.get(b"V") { if !matches!(resolve(doc, v)?, Object::Null) { return Err("Populated signature field is protected.".into()); } }
            }
            for (_, v) in d.iter() { inspect(doc, v, depth + 1, budget)?; }
        }
        Ok(())
    }
    let mut budget = MAX_NODES;
    for value in doc.objects.values() { inspect(doc, value, 0, &mut budget)?; }
    Ok(())
}

fn prepare_unsigned(bytes: &[u8]) -> Result<(Vec<u8>, bool), String> {
    if bytes.len() > MAX_PDF_BYTES || !bytes.starts_with(b"%PDF-") { return Err("Unsupported PDF header/size.".into()); }
    // Raw history/signature cues are deliberately conservative, including
    // escaped names and false positives in data. Never silently strip signatures.
    if raw_name(bytes, b"ByteRange") || raw_name(bytes, b"Perms") {
        return Err("Signature/certification history is present or ambiguous; signing a normalized copy is refused.".into());
    }
    if raw_name(bytes, b"XRefStm") { return Err("Hybrid cross-reference history cannot be established unsigned by this MVP.".into()); }
    let ends: Vec<_> = bytes.windows(5).enumerate().filter_map(|(i, w)| (w == b"%%EOF").then_some(i + 5)).take(33).collect();
    if ends.is_empty() || ends.len() > 32 || ends.iter().try_fold(0usize, |n, end| n.checked_add(*end)).unwrap_or(usize::MAX) > MAX_PDF_BYTES * 4 {
        return Err("Unsigned PDF revision history exceeds the bounded inspection limit.".into());
    }
    if ends.len() == 1 && raw_name(bytes, b"Prev") { return Err("Ambiguous earlier PDF revision; first-signature status cannot be established.".into()); }
    for &end in &ends {
        let revision = load(&bytes[..end]).map_err(|e| format!("Cannot establish unsigned revision history: {e}"))?;
        unsigned_graph_guard(&revision)?;
    }
    let mut doc = load(bytes)?;
    unsigned_graph_guard(&doc)?;
    if ends.len() == 1 { return Ok((bytes.to_vec(), false)); }
    // Only after every bounded revision/current graph passed the independent
    // unsigned guard. This preserves current reachable annotations/layers/pages,
    // but intentionally does NOT preserve old revision bytes or crypto claims.
    doc.prune_objects();
    doc.trailer.remove(b"Prev");
    doc.trailer.remove(b"XRefStm");
    let mut prepared = Vec::new();
    doc.save_to(&mut prepared).map_err(|e| e.to_string())?;
    let fresh = load(&prepared)?;
    unsigned_graph_guard(&fresh)?;
    if prepared.windows(5).filter(|w| *w == b"%%EOF").count() != 1 || raw_name(&prepared, b"Prev") {
        return Err("Could not establish a fresh unsigned signing revision.".into());
    }
    Ok((prepared, true))
}

fn raw_name(bytes: &[u8], wanted: &[u8]) -> bool {
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'/' {
            let mut p = i;
            if let Ok(t) = token(bytes, &mut p) { if matches!(t.kind, Kind::Name(ref n) if n == wanted) { return true; } }
        }
    }
    false
}

fn existing_names(doc: &Document) -> Result<HashSet<String>, String> {
    fn visit(doc: &Document, value: &Object, out: &mut HashSet<String>, depth: usize, budget: &mut usize) -> Result<(), String> {
        if depth > 64 || *budget == 0 { return Err("Field/annotation-name inspection limit exceeded.".into()); }
        *budget -= 1;
        match value {
            Object::Dictionary(dict) => {
                if let Ok(icon) = dict.get(b"Name").and_then(Object::as_name) { out.insert(String::from_utf8_lossy(icon).into_owned()); }
                for key in [b"T".as_slice(), b"NM".as_slice()] {
                    if let Ok(name) = dict.get(key) {
                        if let Ok(bytes) = resolve(doc, name)?.as_str() {
                            let text = if bytes.starts_with(&[0xfe, 0xff]) && bytes.len() % 2 == 0 {
                                String::from_utf16(&bytes[2..].chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect::<Vec<_>>()).map_err(|_| "Malformed UTF-16 field/annotation name.")?
                            } else { String::from_utf8_lossy(bytes).into_owned() };
                            out.insert(text);
                        }
                    }
                }
                for (_, v) in dict.iter() { visit(doc, v, out, depth + 1, budget)?; }
            }
            Object::Array(values) => for v in values { visit(doc, v, out, depth + 1, budget)?; },
            Object::Stream(s) => visit(doc, &Object::Dictionary(s.dict.clone()), out, depth + 1, budget)?,
            _ => {}
        }
        Ok(())
    }
    let mut names = HashSet::new(); let mut budget = MAX_NODES;
    for value in doc.objects.values() { visit(doc, value, &mut names, 0, &mut budget)?; }
    Ok(names)
}

fn appearance_specs(options: &SignOptions) -> Result<Vec<&super::SignatureAppearance>, String> {
    if options.appearance.is_some() && !options.appearances.is_empty() { return Err("Specify either appearance or appearances, not both.".into()); }
    if options.appearances.len() > 256 { return Err("At most 256 visible signature blocks are supported.".into()); }
    Ok(options.appearances.iter().chain(options.appearance.iter()).collect())
}

pub fn sign(bytes: &[u8], identity: &Identity, options: &SignOptions) -> Result<(Vec<u8>, VerificationReport), String> {
    if let Some(expected) = &options.expected_identity_fingerprint {
        if !expected.eq_ignore_ascii_case(&identity.info().sha256_fingerprint) {
            return Err("Identity file changed; reopen it to confirm".into());
        }
    }
    if options.reason.len() > 1024 { return Err("Signing reason exceeds 1024 UTF-8 bytes.".into()); }
    let specs = appearance_specs(options)?;
    let (prepared, normalized) = prepare_unsigned(bytes)?;
    let bytes = prepared.as_slice();
    let doc = load(bytes)?;
    let root = doc.trailer.get(b"Root").map_err(|_| "Missing catalog reference.")?.as_reference().map_err(|_| "Direct catalog is unsupported.")?;
    let visible = specs.into_iter().map(|spec| super::appearance::placement(&doc, spec)).collect::<Result<Vec<_>, _>>()?;
    let page = match visible.last() { Some(placed) => placed.page, None => *doc.get_pages().values().next().ok_or("PDF has no pages.")? };
    let signed_at = chrono::Utc::now();
    let mut update = IncrementalDocument::create_from(bytes.to_vec(), doc);
    let signature = update.new_document.new_object_id();
    let mut sig = dictionary! { "Type" => "Sig", "Filter" => "Adobe.PPKLite", "SubFilter" => "adbe.pkcs7.detached",
        "ByteRange" => vec![Object::Integer(PLACEHOLDER); 4],
        "Contents" => Object::String(vec![0; RESERVE], StringFormat::Hexadecimal),
        "M" => Object::string_literal(signed_at.format("D:%Y%m%d%H%M%SZ").to_string()),
        "Reason" => super::appearance::pdf_text(&options.reason),
    };
    if normalized { sig.set("KineticNormalizedUnsignedHistory", true); }
    if options.mode == SignatureMode::CertifyNoChanges {
        sig.set("Reference", vec![Object::Dictionary(dictionary! { "Type" => "SigRef", "TransformMethod" => "DocMDP",
            "Data" => root, "DigestMethod" => "SHA256",
            "TransformParams" => dictionary! { "Type" => "TransformParams", "P" => 1, "V" => "1.2" },
        })]);
    }
    update.new_document.objects.insert(signature, Object::Dictionary(sig));
    let previous = update.get_prev_documents();
    let mut form = match previous.catalog().map_err(|_| "Missing catalog.")?.get(b"AcroForm").ok() {
        Some(v) => resolve(previous, v)?.as_dict().map_err(|_| "Malformed AcroForm.")?.clone(), None => Dictionary::new(),
    };
    let mut fields = match form.get(b"Fields").ok() { Some(v) => resolve(previous, v)?.as_array().map_err(|_| "Malformed Fields.")?.clone(), None => vec![] };
    let mut names = existing_names(previous)?;
    let field_name = (0..10_000).map(|n| format!("KineticSignature-{}-{n}", signature.0)).find(|n| !names.contains(n)).ok_or("Cannot allocate unique signature field.")?;
    let mut widget = dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig",
        "T" => Object::string_literal(field_name), "V" => signature, "P" => page, "F" => 132,
        "Rect" => vec![Object::Integer(0); 4],
    };
    let date = signed_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let info = identity.info();
    let mut resources = super::appearance::Resources::default();
    let mut additions = BTreeMap::<ObjectId, Vec<Object>>::new();
    for (index, placed) in visible.iter().enumerate() {
        let ap = super::appearance::append(&mut update, &mut resources, placed, &info, &date, &options.reason, options.mode)?;
        let rect = placed.rect.iter().map(|v| Object::Real(*v as f32)).collect::<Vec<_>>();
        let native_name = (0..10_000).map(|n| format!("KineticSignatureBlock-{}-{index}-{n}", signature.0))
            .find(|n| !names.contains(n)).ok_or("Cannot allocate unique signature block annotation name.")?;
        names.insert(native_name.clone());
        if index + 1 == visible.len() {
            widget.set("Rect", rect);
            widget.set("AP", dictionary! { "N" => ap });
            widget.set("NM", Object::string_literal(native_name));
            // Form /Matrix counters page rotation; no /MK/R double rotation.
        } else {
            // These are finalized visual blocks covered by the one signature,
            // never extra signature fields, /V values or approval revisions.
            let stamp = update.new_document.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp",
                "P" => placed.page, "F" => 132, "Rect" => rect, "AP" => dictionary! { "N" => ap },
                "NM" => Object::string_literal(native_name.clone()), "Name" => Object::Name(native_name.into_bytes()),
                "Contents" => super::appearance::pdf_text("Visual block covered by this document's single digital signature; not an additional signature field."),
            });
            additions.entry(placed.page).or_default().push(Object::Reference(stamp));
        }
    }
    let widget = update.new_document.add_object(widget);
    additions.entry(page).or_default().push(Object::Reference(widget));
    fields.push(Object::Reference(widget));
    form.set("Fields", fields);
    let flags = form.get(b"SigFlags").ok().map(|v| resolve(update.get_prev_documents(), v)?.as_i64().map_err(|_| "Malformed SigFlags.".to_owned())).transpose()?.unwrap_or(0);
    form.set("SigFlags", flags | 3);
    form.set("NeedAppearances", false);
    let form_id = update.new_document.add_object(form);
    update.opt_clone_object_to_new_document(root).map_err(|e| e.to_string())?;
    let catalog = update.new_document.get_dictionary_mut(root).map_err(|e| e.to_string())?;
    catalog.set("AcroForm", form_id);
    if options.mode == SignatureMode::CertifyNoChanges { catalog.set("Perms", dictionary! { "DocMDP" => signature }); }
    // Merge each original Annots array once. Re-cloning a page from the previous
    // revision for every block would discard earlier blocks on that same page.
    for (page, added) in additions {
        let mut annotations = match update.get_prev_documents().get_dictionary(page).map_err(|e| e.to_string())?.get(b"Annots").ok() {
            Some(v) => resolve(update.get_prev_documents(), v)?.as_array().map_err(|_| "Malformed page annotations.")?.clone(), None => vec![],
        };
        annotations.extend(added);
        update.opt_clone_object_to_new_document(page).map_err(|e| e.to_string())?;
        update.new_document.get_dictionary_mut(page).map_err(|e| e.to_string())?.set("Annots", annotations);
    }
    let mut output = Vec::new();
    update.save_to(&mut output).map_err(|e| e.to_string())?;
    if !output.starts_with(bytes) || output.len() > MAX_PDF_BYTES { return Err("Signed candidate exceeds limit or lost its prepared unsigned byte prefix.".into()); }
    let written = load(&output)?;
    let slots = signature_slots(&output, &written, signature)?;
    if slots.contents.start < bytes.len() || slots.range_tokens.iter().any(|r| r.len() != 10) { return Err("Invalid scoped signature reservation.".into()); }
    let ranges = [0, slots.contents.start, slots.contents.end, output.len() - slots.contents.end];
    for (slot, value) in slots.range_tokens.iter().zip(ranges) {
        let value = format!("{value:010}");
        if value.len() != slot.len() { return Err("ByteRange reservation overflow.".into()); }
        output[slot.clone()].copy_from_slice(value.as_bytes());
    }
    let data = range_data(&output, ranges)?;
    let cms = cms::sign_detached(identity, &data)?;
    if cms.len() > RESERVE { return Err("CMS exceeds the fixed signature reservation; no file was published.".into()); }
    cms::verify_detached(&cms, &data)?;
    let hex = encode_hex(&cms);
    let start = slots.contents.start + 1;
    output[start..start + hex.len()].copy_from_slice(hex.as_bytes());
    let final_doc = load(&output)?;
    if final_doc.get_pages().is_empty() || !output.starts_with(bytes) { return Err("Final signed PDF failed structural/prefix validation.".into()); }
    let report = verify_publication(&output)?;
    if report.signatures.len() != 1 || !report.signatures[0].integrity_valid || report.signatures[0].later_changes
        || (options.mode == SignatureMode::CertifyNoChanges && report.signatures[0].certification != Some(1))
    { return Err("Final signed PDF did not pass independent range/CMS re-verification.".into()); }
    Ok((output, report))
}

// Public-material extraction can fail after CMS mathematics succeeds. Retry the
// integrity-only path, never treating a trust-input/Windows-query failure as bad
// CMS. No PDF name, /M value or unverified certificate participates in assessment.
fn assess_signer_material(material: Result<cms::VerifiedSignerMaterial, String>,
    integrity_only: impl FnOnce() -> Result<IdentityInfo, String>,
) -> Result<(IdentityInfo, TrustReport), String> {
    match material {
        Ok(material) => {
            #[cfg(test)] TRUST_EVALUATIONS.with(|count| count.set(count.get() + 1));
            let assessment = super::trust::evaluate(&material);
            Ok((material.info, assessment))
        }
        Err(_) => {
            let signer = integrity_only()?;
            Ok((signer, TrustReport::unavailable(TrustIssueCode::EvaluationUnavailable)))
        }
    }
}

#[cfg(test)]
thread_local! { pub(crate) static TRUST_EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

pub fn verify(bytes: &[u8]) -> Result<VerificationReport, String> {
    verify_document(bytes, true)
}
/// Internal publication receipt only. There is deliberately no trust toggle on
/// the ordinary verification entry point or its helper task.
pub(crate) fn verify_publication(bytes: &[u8]) -> Result<VerificationReport, String> {
    verify_document(bytes, false)
}
fn verify_document(bytes: &[u8], assess_trust: bool) -> Result<VerificationReport, String> {
    let doc = load(bytes)?;
    let candidates = candidates(&doc)?;
    if candidates.is_empty() && doc.catalog().map_err(|_| "Missing catalog.")?.has(b"Perms") { return Err("Certification permissions exist without a supported signature field.".into()); }
    let mut report = VerificationReport::default();
    for (id, dict) in candidates {
        let mut check = SignatureCheck { signer: None, integrity_valid: false, identity_trust: IdentityTrust::NotChecked, trust_report: None,
            signed_revision_end: 0, file_bytes: bytes.len() as u64, later_changes: true, certification: declared_certification(&doc, dict), message: String::new() };
        let result = (|| {
            if name(&doc, dict, b"SubFilter")? != Some(b"adbe.pkcs7.detached".as_slice()) { return Err("Unsupported PDF signature SubFilter.".into()); }
            let slots = signature_slots(bytes, &doc, id.ok_or("Direct signature dictionaries are unsupported.")?)?;
            let numbers = resolve(&doc, dict.get(b"ByteRange").map_err(|_| "Missing ByteRange.")?)?.as_array().map_err(|_| "Malformed ByteRange.")?;
            if numbers.len() != 4 { return Err("ByteRange must have exactly four integers.".into()); }
            let mut ranges = [0usize; 4];
            for (out, n) in ranges.iter_mut().zip(numbers) { *out = usize::try_from(n.as_i64().map_err(|_| "Invalid ByteRange integer.")?).map_err(|_| "Negative ByteRange value.")?; }
            let data = range_data(bytes, ranges)?;
            if ranges[1] != slots.contents.start || ranges[2] != slots.contents.end { return Err("ByteRange hole is not exactly this signature's Contents token.".into()); }
            check.signed_revision_end = (ranges[2] + ranges[3]) as u64;
            check.later_changes = check.signed_revision_end != bytes.len() as u64;
            let revision = &bytes[..check.signed_revision_end as usize];
            let trimmed = revision.iter().rposition(|b| !space(*b)).map(|p| p + 1).unwrap_or(0);
            if !revision[..trimmed].ends_with(b"%%EOF") { return Err("Signed byte range does not end at a readable PDF revision EOF.".into()); }
            let signed_doc = load(revision)?;
            let signed_slots = signature_slots(revision, &signed_doc, id.ok_or("Missing signature reference.")?)?;
            if signed_slots.contents != slots.contents { return Err("Current signature dictionary does not match its signed revision.".into()); }
            let content = dict.get(b"Contents").map_err(|_| "Missing signature Contents.")?.as_str().map_err(|_| "Invalid Contents value.")?;
            let actual = decode_hex(&bytes[slots.contents.start + 1..slots.contents.end - 1])?;
            if content != actual.as_slice() { return Err("Parsed signature Contents differs from its literal byte hole.".into()); }
            let cms = der_contents(&actual)?;
            if assess_trust {
                let (signer, assessment) = assess_signer_material(cms::verify_detached_with_material(cms, &data), || cms::verify_detached(cms, &data))?;
                check.signer = Some(signer);
                check.identity_trust = assessment.identity_trust();
                check.trust_report = Some(assessment);
            } else {
                check.signer = Some(cms::verify_detached(cms, &data)?);
            }
            check.integrity_valid = true;
            Ok::<(), String>(())
        })();
        check.message = match result {
            Ok(()) => format!("CMS integrity verified for the signed revision. {} Signing-time trust is not established. {}{}",
                match check.identity_trust {
                    IdentityTrust::Trusted => "Current-time Windows certificate checks passed (cache-only revocation).",
                    IdentityTrust::Untrusted => "Current-time Windows certificate checks failed; identity is not trusted.",
                    IdentityTrust::Indeterminate => "Current-time Windows certificate trust is indeterminate (cache-only revocation).",
                    IdentityTrust::NotChecked => "Certificate trust was not checked.",
                },
                if check.later_changes { "Later bytes/changes exist; current-document validity is not established. " } else { "" },
                if check.certification.is_some() { "DocMDP policy is declared, not fully permission-validated." } else { "" }),
            Err(error) => format!("Integrity not established: {error}"),
        };
        if dict.get(b"KineticNormalizedUnsignedHistory").ok().and_then(|v| v.as_bool().ok()) == Some(true) {
            check.message.push(' '); check.message.push_str(NORMALIZED_NOTE);
        }
        if check.message.len() > 2048 { check.message = "Integrity verification failed (detail exceeds limit).".into(); }
        report.signatures.push(check);
    }
    Ok(report)
}

fn declared_certification(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let refs = resolve(doc, dict.get(b"Reference").ok()?).ok()?.as_array().ok()?;
    for value in refs {
        let reference = resolve(doc, value).ok()?.as_dict().ok()?;
        if name(doc, reference, b"TransformMethod").ok()? == Some(b"DocMDP".as_slice()) {
            let params = resolve(doc, reference.get(b"TransformParams").ok()?).ok()?.as_dict().ok()?;
            let p = params.get(b"P").ok()?.as_i64().ok()?;
            return u8::try_from(p).ok().filter(|p| (1..=3).contains(p));
        }
    }
    None
}

fn range_data(bytes: &[u8], r: [usize; 4]) -> Result<Vec<u8>, String> {
    let end = r[2].checked_add(r[3]).ok_or("ByteRange overflow.")?;
    if r[0] != 0 || r[1] >= r[2] || end > bytes.len() || r[2] > bytes.len() { return Err("ByteRange is overlapping, out of bounds or does not start at zero.".into()); }
    let mut data = Vec::with_capacity(r[1] + r[3]);
    data.extend_from_slice(&bytes[..r[1]]);
    data.extend_from_slice(&bytes[r[2]..end]);
    Ok(data)
}
fn encode_hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02X}")).collect() }
fn decode_hex(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let bytes: Vec<_> = bytes.iter().copied().filter(|b| !space(*b)).collect();
    if bytes.len() % 2 != 0 || bytes.len() > RESERVE * 2 { return Err("Unsupported signature hex size.".into()); }
    bytes.chunks_exact(2).map(|p| Ok((digit(p[0]).ok_or("Invalid hex digit.")? << 4) | digit(p[1]).ok_or("Invalid hex digit.")?)).collect()
}
fn der_contents(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 2 || bytes[0] != 0x30 { return Err("Signature Contents is not a DER CMS sequence.".into()); }
    let (header, len) = if bytes[1] < 128 { (2, bytes[1] as usize) } else {
        let n = (bytes[1] & 0x7f) as usize;
        if n == 0 || n > 4 || bytes.len() < 2 + n { return Err("Unsupported DER length.".into()); }
        let mut len = 0usize;
        for b in &bytes[2..2 + n] { len = len.checked_mul(256).and_then(|n| n.checked_add(*b as usize)).ok_or("DER length overflow.")?; }
        (2 + n, len)
    };
    let end = header.checked_add(len).ok_or("DER size overflow.")?;
    if end > bytes.len() || bytes[end..].iter().any(|b| *b != 0) { return Err("Invalid CMS length or nonzero trailing Contents padding.".into()); }
    Ok(&bytes[..end])
}

#[derive(Debug)]
enum Kind { Name(Vec<u8>), Atom, Hex, Literal, Dict, EndDict, Array, EndArray }
struct Token { kind: Kind, span: Range<usize> }
fn space(b: u8) -> bool { matches!(b, 0 | 9 | 10 | 12 | 13 | 32) }
fn delimiter(b: u8) -> bool { space(b) || b"()<>[]{}/%".contains(&b) }
fn digit(b: u8) -> Option<u8> { match b { b'0'..=b'9' => Some(b - b'0'), b'a'..=b'f' => Some(b - b'a' + 10), b'A'..=b'F' => Some(b - b'A' + 10), _ => None } }
fn token(bytes: &[u8], pos: &mut usize) -> Result<Token, String> {
    while *pos < bytes.len() {
        if space(bytes[*pos]) { *pos += 1; } else if bytes[*pos] == b'%' {
            while *pos < bytes.len() && !matches!(bytes[*pos], b'\r' | b'\n') { *pos += 1; }
        } else { break; }
    }
    let start = *pos;
    let b = *bytes.get(*pos).ok_or("Unexpected PDF token EOF.")?;
    *pos += 1;
    let kind = match b {
        b'<' if bytes.get(*pos) == Some(&b'<') => { *pos += 1; Kind::Dict }
        b'>' if bytes.get(*pos) == Some(&b'>') => { *pos += 1; Kind::EndDict }
        b'[' => Kind::Array, b']' => Kind::EndArray,
        b'<' => { while *pos < bytes.len() && bytes[*pos] != b'>' { if !space(bytes[*pos]) && digit(bytes[*pos]).is_none() { return Err("Invalid hex string.".into()); } *pos += 1; }
            if *pos == bytes.len() { return Err("Unterminated hex string.".into()); } *pos += 1; Kind::Hex }
        b'(' => { let mut depth = 1; while *pos < bytes.len() && depth > 0 {
            match bytes[*pos] { b'\\' => { *pos += 1; if *pos < bytes.len() { *pos += 1; } }, b'(' => { depth += 1; *pos += 1; }, b')' => { depth -= 1; *pos += 1; }, _ => *pos += 1 }
        } if depth != 0 { return Err("Unterminated PDF literal.".into()); } Kind::Literal }
        b'/' => { let mut name = Vec::new(); while *pos < bytes.len() && !delimiter(bytes[*pos]) {
            if bytes[*pos] == b'#' {
                let a = *bytes.get(*pos + 1).ok_or("Truncated name escape.")?; let b = *bytes.get(*pos + 2).ok_or("Truncated name escape.")?;
                name.push(digit(a).ok_or("Bad name escape.")? * 16 + digit(b).ok_or("Bad name escape.")?); *pos += 3;
            } else { name.push(bytes[*pos]); *pos += 1; }
        } Kind::Name(name) }
        _ if !delimiter(b) => { while *pos < bytes.len() && !delimiter(bytes[*pos]) { *pos += 1; } Kind::Atom }
        _ => return Err("Unsupported PDF token.".into()),
    };
    Ok(Token { kind, span: start..*pos })
}
fn skip_value(bytes: &[u8], pos: &mut usize, depth: usize) -> Result<Range<usize>, String> {
    if depth > 64 { return Err("PDF value nesting limit exceeded.".into()); }
    let first = token(bytes, pos)?;
    match first.kind {
        Kind::Dict => loop { let key = token(bytes, pos)?; if matches!(key.kind, Kind::EndDict) { break; }
            if !matches!(key.kind, Kind::Name(_)) { return Err("Invalid dictionary key.".into()); } skip_value(bytes, pos, depth + 1)?; },
        Kind::Array => loop { let saved = *pos; if matches!(token(bytes, pos)?.kind, Kind::EndArray) { break; } *pos = saved; skip_value(bytes, pos, depth + 1)?; },
        Kind::Atom => { let saved = *pos; if bytes[first.span.clone()].iter().all(u8::is_ascii_digit) {
            if let Ok(second) = token(bytes, pos) { if matches!(second.kind, Kind::Atom) && bytes[second.span].iter().all(u8::is_ascii_digit) {
                if let Ok(third) = token(bytes, pos) { if &bytes[third.span] == b"R" { return Ok(first.span.start..*pos); } }
            } } *pos = saved;
        } },
        Kind::Name(_) | Kind::Hex | Kind::Literal => {},
        _ => return Err("Invalid dictionary value.".into()),
    }
    Ok(first.span.start..*pos)
}
struct Slots { contents: Range<usize>, range_tokens: Vec<Range<usize>> }
fn signature_slots(bytes: &[u8], doc: &Document, id: ObjectId) -> Result<Slots, String> {
    let mut pos = match doc.reference_table.entries.get(&id.0) {
        Some(XrefEntry::Normal { offset, generation }) if *generation == id.1 => *offset as usize,
        _ => return Err("Compressed/missing signature objects are unsupported.".into()),
    };
    for expected in [id.0.to_string(), id.1.to_string(), "obj".into()] {
        let t = token(bytes, &mut pos)?;
        if !matches!(t.kind, Kind::Atom) || bytes.get(t.span) != Some(expected.as_bytes()) { return Err("Signature xref/object mismatch.".into()); }
    }
    if !matches!(token(bytes, &mut pos)?.kind, Kind::Dict) { return Err("Signature object is not a dictionary.".into()); }
    let mut keys = HashSet::new(); let (mut contents, mut ranges) = (None, None);
    loop {
        let key = token(bytes, &mut pos)?;
        if matches!(key.kind, Kind::EndDict) { break; }
        let Kind::Name(key) = key.kind else { return Err("Malformed signature key.".into()); };
        if !keys.insert(key.clone()) { return Err("Duplicate signature dictionary key.".into()); }
        let value = skip_value(bytes, &mut pos, 0)?;
        if key == b"Contents" {
            let mut p = value.start; let t = token(bytes, &mut p)?;
            if !matches!(t.kind, Kind::Hex) || t.span != value { return Err("Only direct hexadecimal Contents is supported.".into()); }
            contents = Some(value);
        } else if key == b"ByteRange" {
            let mut p = value.start;
            if !matches!(token(bytes, &mut p)?.kind, Kind::Array) { return Err("Only direct ByteRange is supported.".into()); }
            let mut tokens = vec![];
            for _ in 0..4 { let t = token(bytes, &mut p)?; if !matches!(t.kind, Kind::Atom) { return Err("Invalid ByteRange token.".into()); } tokens.push(t.span); }
            if !matches!(token(bytes, &mut p)?.kind, Kind::EndArray) || p != value.end { return Err("ByteRange must contain four values.".into()); }
            ranges = Some(tokens);
        }
    }
    let end = token(bytes, &mut pos)?;
    if bytes.get(end.span) != Some(b"endobj".as_slice()) { return Err("Signature object terminator mismatch.".into()); }
    Ok(Slots { contents: contents.ok_or("Missing Contents reservation.")?, range_tokens: ranges.ok_or("Missing ByteRange reservation.")? })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trust_material_failure_preserves_verified_integrity_but_never_invents_trust() {
        let info = IdentityInfo { subject: "Synthetic unit signer".into(), issuer: "Synthetic unit signer".into(),
            sha256_fingerprint: "ab".repeat(32), not_before: String::new(), not_after: String::new(),
            algorithm: "RSA".into(), key_bits: 2048, self_signed: true };
        let (signer, assessment) = assess_signer_material(Err("public material bound/query failure".into()), || Ok(info.clone())).unwrap();
        assert_eq!(signer, info);
        assert_eq!(assessment.identity_trust(), IdentityTrust::Indeterminate);
        assert_eq!(assessment.issues[0].code, TrustIssueCode::EvaluationUnavailable);
        assert!(assess_signer_material(Err("invalid CMS".into()), || Err("CMS integrity failed".into())).is_err());
    }
    #[test]
    fn visible_block_contract_keeps_legacy_and_rejects_ambiguity_and_over_limit() {
        let spec = super::super::SignatureAppearance { page: 0, rect: [60.0, 80.0, 300.0, 190.0], rotation: 0 };
        let mut options = SignOptions::default();
        assert!(appearance_specs(&options).unwrap().is_empty());
        options.appearance = Some(spec.clone());
        assert_eq!(appearance_specs(&options).unwrap(), vec![&spec]);
        options.appearances.push(spec.clone());
        assert!(appearance_specs(&options).is_err());
        options.appearance = None;
        options.appearances = vec![spec.clone(); 256];
        assert_eq!(appearance_specs(&options).unwrap().len(), 256);
        options.appearances.push(spec);
        assert!(appearance_specs(&options).is_err());
    }
    #[test]
    fn byte_range_arithmetic_and_der_padding_are_fail_closed() {
        assert!(range_data(b"0123456789", [0, 4, 3, 7]).is_err());
        assert!(range_data(b"0123456789", [1, 2, 4, 6]).is_err());
        assert!(range_data(b"0123456789", [0, 2, usize::MAX, 3]).is_err());
        assert!(der_contents(&[0x30, 0x80, 0, 0]).is_err());
        assert!(der_contents(&[0x30, 1, 0, 1]).is_err());
        assert_eq!(der_contents(&[0x30, 1, 0, 0]).unwrap(), &[0x30, 1, 0]);
    }
}
