//! Conservative replacement guards for PDF exports, not a signature validator.
//!
//! PATH: src/output_safety.rs
//! CONSUMED BY: overlay and worker distinct-destination exports. No PDFium or UI
//! work; ordinary same-source saves must retain their existing external-change checks.
//! Exact byte/identity rechecks narrow races but are not filesystem compare-and-swap.
//! Path/parent swaps or writes after the final check remain possible; crash durability
//! and complete hostile-PDF validation are not guaranteed.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use pdf_content::lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};
use same_file::Handle;

const MAX_DESTINATION_BYTES: usize = 256 * 1024 * 1024;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 64;

struct ExistingDestination {
    bytes: Vec<u8>,
    identity: Handle,
}

struct ProtectedSource {
    path: PathBuf,
    location: PathBuf,
    identity: Option<Handle>,
}

/// A one-output transaction's inspected destination and protected source identities.
/// Keep this guard alive through commit; never reuse it for a subsequent revision.
/// Existing PDF destinations with ambiguous signatures/history are refused, not repaired.
pub struct DestinationGuard {
    target: PathBuf,
    parent: PathBuf,
    sources: Vec<ProtectedSource>,
    expected: Option<ExistingDestination>,
}

/// Inspect only distinct export destinations, on a background/worker thread.
/// Returns the exact expected bytes (via expected_bytes), or captures absence.
/// Missing sources are allowed for snapshot recovery (NotFound only). Captured and
/// current path locations/available identities remain protected, including reappearance.
/// This conservatively refuses recognizable incremental/hybrid history. Raw hints
/// and current-object inspection cannot prove the absence of malformed/escaped
/// historical signatures. Existing files over 256 MiB are refused; use a new path.
pub fn inspect_destination(target: &Path, protected_sources: &[&Path]) -> Result<DestinationGuard, String> {
    let target = std::path::absolute(target).map_err(|e| format!("could not resolve output path: {e}"))?;
    let parent = target.parent().ok_or("Output needs a parent directory.")?
        .canonicalize().map_err(|e| format!("could not resolve output directory: {e}"))?;
    let mut sources = Vec::new();
    for source in protected_sources {
        sources.push(ProtectedSource {
            path: std::path::absolute(source).map_err(|e| e.to_string())?,
            location: resolved_location(source, 0)?,
            identity: source_identity(source)?,
        });
    }
    let expected = read_destination(&target)?;
    let guard = DestinationGuard { target, parent, sources, expected };
    guard.check_aliases(guard.expected.as_ref())?;
    if let Some(existing) = &guard.expected {
        inspect_pdf(&existing.bytes)?;
    }
    // Do not reread a potentially large destination after parsing: commit's final
    // exact-byte/identity recheck catches changes during this initial inspection.
    Ok(guard)
}

impl DestinationGuard {
    /// None means the destination did not exist; Some holds its exact original bytes.
    pub fn expected_bytes(&self) -> Option<&[u8]> {
        self.expected.as_ref().map(|e| e.bytes.as_slice())
    }

    fn check_aliases(&self, current: Option<&ExistingDestination>) -> Result<(), String> {
        let target_location = resolved_location(&self.target, 0)?;
        for source in &self.sources {
            // Captured identities protect moved/deleted sources; any newly
            // appearing source must also be checked, not treated as still absent.
            let now = source_identity(&source.path)?;
            let location = resolved_location(&source.path, 0)?;
            if same_location(&self.target, &source.path)?
                || same_location(&target_location, &source.location)?
                || same_location(&target_location, &location)?
                || current.is_some_and(|e| source.identity.as_ref().is_some_and(|id| e.identity == *id)
                    || now.as_ref().is_some_and(|id| e.identity == *id))
            {
                return Err("Choose a different output; an export cannot replace either protected input or its filesystem aliases.".into());
            }
        }
        Ok(())
    }

    /// Recheck exact bytes, file identity, directory and source aliases. Must be
    /// called immediately before rename if the caller uses its own commit routine.
    /// For captured absence, callers must additionally use a no-clobber commit.
    pub fn recheck(&self) -> Result<(), String> {
        let parent = self.target.parent().ok_or("Output needs a parent directory.")?
            .canonicalize().map_err(|e| format!("could not recheck output directory: {e}"))?;
        if parent != self.parent { return Err("The output directory changed during export.".into()); }
        let current = read_destination(&self.target)?;
        self.check_aliases(current.as_ref())?;
        match (&self.expected, current) {
            (None, None) => Ok(()),
            (Some(expected), Some(current)) if expected.identity == current.identity && expected.bytes == current.bytes => Ok(()),
            _ => Err("The output destination changed during export. Choose a new destination or retry.".into()),
        }
    }

    /// Write a sibling temporary file, retain the caller's external-change check,
    /// recheck this guard, then publish. Absent destinations never clobber a racing
    /// creator. Existing replacement is atomic rename, not a compare-and-swap:
    /// another process can still change paths/bytes after the final check.
    pub fn commit(&self, bytes: &[u8], before_commit: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut tmp = tempfile::NamedTempFile::new_in(&self.parent)
            .map_err(|e| format!("could not create output temporary file: {e}"))?;
        tmp.write_all(bytes).map_err(|e| format!("could not write output temporary file: {e}"))?;
        tmp.flush().map_err(|e| format!("could not flush output temporary file: {e}"))?;
        before_commit()?;
        self.recheck()?;
        if self.expected.is_none() {
            tmp.persist_noclobber(&self.target).map_err(|e| format!("could not create output without replacing another file: {e}"))?;
        } else {
            // Match the worker's Windows-compatible std::fs::rename replacement;
            // TempPath removes the sibling on errors without exposing partial output.
            let path = tmp.into_temp_path();
            std::fs::rename(&path, &self.target).map_err(|e| format!("could not atomically replace output: {e}"))?;
        }
        Ok(())
    }
}

fn source_identity(path: &Path) -> Result<Option<Handle>, String> {
    match Handle::from_path(path) {
        Ok(handle) => Ok(Some(handle)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not identify protected input: {e}")),
    }
}

// Canonicalize what exists and retain the missing suffix. Resolve dangling
// symlinks too: recreating their target must not overwrite a protected location.
fn resolved_location(path: &Path, depth: usize) -> Result<PathBuf, String> {
    if depth > MAX_DEPTH { return Err("Could not resolve protected/output path within the safety limit.".into()); }
    match path.canonicalize() {
        Ok(path) => return Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not resolve protected/output path: {e}")),
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let link = std::fs::read_link(path).map_err(|e| format!("could not resolve input symlink: {e}"))?;
            let link = if link.is_absolute() { link } else { path.parent().ok_or("Input symlink needs a parent.")?.join(link) };
            return resolved_location(&link, depth + 1);
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not inspect protected/output path: {e}")),
    }
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let parent = absolute.parent().ok_or("Could not resolve missing path parent.")?;
    let filename = absolute.file_name().ok_or("Could not resolve missing path filename.")?;
    Ok(resolved_location(parent, depth + 1)?.join(filename))
}

#[cfg(not(windows))]
fn same_location(a: &Path, b: &Path) -> Result<bool, String> { Ok(a == b) }

#[cfg(windows)]
fn same_location(a: &Path, b: &Path) -> Result<bool, String> {
    use std::os::windows::ffi::OsStrExt;
    // Missing names have no file identity. Compare their resolved locations with
    // Windows ordinal case rules (also conservatively protects case-sensitive dirs).
    #[link(name = "kernel32")]
    extern "system" {
        fn CompareStringOrdinal(a: *const u16, a_len: i32, b: *const u16, b_len: i32, ignore_case: i32) -> i32;
    }
    let a: Vec<u16> = a.as_os_str().encode_wide().collect();
    let b: Vec<u16> = b.as_os_str().encode_wide().collect();
    let a_len = i32::try_from(a.len()).map_err(|_| "Output path is too long.")?;
    let b_len = i32::try_from(b.len()).map_err(|_| "Input path is too long.")?;
    // SAFETY: buffers contain exactly the explicit UTF-16 lengths and remain
    // alive for this synchronous call. Return 0 is an error, not inequality.
    match unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) } {
        0 => Err("Could not compare protected/output path locations.".into()),
        2 => Ok(true),
        _ => Ok(false),
    }
}

fn read_destination(path: &Path) -> Result<Option<ExistingDestination>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not inspect output destination: {e}")),
    };
    // Reject symlinks (including dangling ones), directories and special files.
    // Followed-source aliases are additionally caught using same-file identities.
    if !metadata.file_type().is_file() { return Err("Output replacement requires a regular, non-symlink PDF file.".into()); }
    let mut file = File::open(path).map_err(|e| format!("could not read output destination: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() { return Err("Output is not a regular file.".into()); }
    if metadata.len() > MAX_DESTINATION_BYTES as u64 {
        return Err("Existing destination is too large for conservative replacement inspection; choose a new path.".into());
    }
    let identity = Handle::from_file(file.try_clone().map_err(|e| e.to_string())?)
        .map_err(|e| format!("could not identify output destination: {e}"))?;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_DESTINATION_BYTES as u64 + 1).read_to_end(&mut bytes)
        .map_err(|e| format!("could not snapshot output destination: {e}"))?;
    if bytes.len() > MAX_DESTINATION_BYTES { return Err("Existing destination is too large for conservative replacement inspection; choose a new path.".into()); }
    Ok(Some(ExistingDestination { bytes, identity }))
}

fn ambiguous() -> String {
    "Existing destination has detected or ambiguous PDF signatures/structure; choose a new output path.".into()
}

fn resolved<'a>(doc: &'a Document, value: &'a Object) -> Result<&'a Object, String> {
    doc.dereference(value).map(|(_, value)| value).map_err(|_| ambiguous())
}

fn name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Result<Option<&'a [u8]>, String> {
    match dict.get(key).ok() {
        None => Ok(None),
        Some(value) => resolved(doc, value)?.as_name().map(Some).map_err(|_| ambiguous()),
    }
}

fn populated(doc: &Document, dict: &Dictionary) -> Result<bool, String> {
    match dict.get(b"V").ok() {
        None => Ok(false),
        Some(value) => Ok(!matches!(resolved(doc, value)?, Object::Null)),
    }
}

fn inspect_pdf(bytes: &[u8]) -> Result<(), String> {
    // Conservative history hints, not a lexical PDF parser. False positives in
    // streams/comments refuse replacement. lopdf consumes /Prev while loading,
    // so do not infer a single revision from its merged trailer alone.
    if !bytes.starts_with(b"%PDF-")
        || bytes.windows(5).filter(|s| *s == b"%%EOF").count() != 1
        || [b"/Prev".as_slice(), b"/XRefStm".as_slice()].iter().any(|token| bytes.windows(token.len()).any(|s| s == *token))
    { return Err(ambiguous()); }
    let doc = Document::load_mem_with_options(bytes, LoadOptions {
        strict: true, max_decompressed_size: Some(16 * 1024 * 1024), ..Default::default()
    }).map_err(|_| ambiguous())?;
    if doc.is_encrypted() || doc.encryption_state.is_some() || doc.trailer.has(b"Encrypt") || doc.objects.len() > MAX_NODES { return Err(ambiguous()); }
    let catalog = doc.catalog().map_err(|_| ambiguous())?;
    if name(&doc, catalog, b"Type")? != Some(b"Catalog".as_slice()) || catalog.has(b"Perms") { return Err(ambiguous()); }
    let mut budget = MAX_NODES;
    for object in doc.objects.values() { inspect_object(&doc, object, 0, &mut budget)?; }
    if let Ok(form) = catalog.get(b"AcroForm") {
        let form = resolved(&doc, form)?;
        if matches!(form, Object::Null) { return Ok(()); }
        let form = form.as_dict().map_err(|_| ambiguous())?;
        let fields = resolved(&doc, form.get(b"Fields").map_err(|_| ambiguous())?)?.as_array().map_err(|_| ambiguous())?;
        let mut seen = HashSet::new();
        for field in fields { inspect_field(&doc, field, None, false, 0, &mut seen, &mut budget)?; }
    }
    Ok(())
}

fn consume(depth: usize, budget: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH || *budget == 0 { return Err(ambiguous()); }
    *budget -= 1;
    Ok(())
}

fn inspect_object(doc: &Document, object: &Object, depth: usize, budget: &mut usize) -> Result<(), String> {
    consume(depth, budget)?;
    let dict = match object {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        Object::Array(array) => {
            for value in array { inspect_object(doc, value, depth + 1, budget)?; }
            None
        }
        _ => None,
    };
    if let Some(dict) = dict {
        if dict.has(b"ByteRange") || dict.has(b"Perms")
            || matches!(name(doc, dict, b"Type")?, Some(b"Sig" | b"DocTimeStamp"))
            || matches!(name(doc, dict, b"TransformMethod")?, Some(b"DocMDP" | b"FieldMDP"))
            || (name(doc, dict, b"FT")? == Some(b"Sig".as_slice()) && populated(doc, dict)?)
        { return Err(ambiguous()); }
        for (_, value) in dict.iter() { inspect_object(doc, value, depth + 1, budget)?; }
    }
    Ok(())
}

fn inspect_field(doc: &Document, field: &Object, inherited: Option<Vec<u8>>, inherited_populated: bool,
    depth: usize, seen: &mut HashSet<ObjectId>, budget: &mut usize) -> Result<(), String>
{
    consume(depth, budget)?;
    let (id, field) = doc.dereference(field).map_err(|_| ambiguous())?;
    if id.is_some_and(|id| !seen.insert(id)) { return Err(ambiguous()); }
    let field = field.as_dict().map_err(|_| ambiguous())?;
    let kind = name(doc, field, b"FT")?.map(|n| n.to_vec()).or(inherited);
    // Like worker discovery, absent V inherits across field kinds; an explicit
    // (possibly indirect) null clears it. A populated value is unsafe for a Sig
    // descendant even if an ancestor supplied it under another field type.
    let value_populated = if field.has(b"V") { populated(doc, field)? } else { inherited_populated };
    if value_populated && (kind.is_none() || kind.as_deref() == Some(b"Sig".as_slice())) { return Err(ambiguous()); }
    if let Ok(kids) = field.get(b"Kids") {
        for kid in resolved(doc, kids)?.as_array().map_err(|_| ambiguous())? {
            inspect_field(doc, kid, kind.clone(), value_populated, depth + 1, seen, budget)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_content::lopdf::dictionary;

    fn pdf(field: Option<Object>, perms: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => dictionary! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 } };
        if let Some(field) = field {
            let field = doc.add_object(field);
            catalog.set("AcroForm", dictionary! { "Fields" => vec![Object::Reference(field)] });
        }
        if perms { catalog.set("Perms", dictionary! { "DocMDP" => Object::Null }); }
        let root = doc.add_object(catalog);
        doc.trailer.set("Root", root);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn destinations_accept_plain_and_empty_fields_but_reject_populated_or_ambiguous() {
        assert!(inspect_pdf(&pdf(None, false)).is_ok());
        for value in [None, Some(Object::Null)] {
            let mut field = dictionary! { "FT" => "Sig" };
            if let Some(value) = value { field.set("V", value); }
            assert!(inspect_pdf(&pdf(Some(Object::Dictionary(field)), false)).is_ok());
        }
        for field in [
            dictionary! { "FT" => "Sig", "V" => dictionary! {} },
            dictionary! { "FT" => "Sig", "V" => Object::string_literal("") },
            dictionary! { "FT" => "Sig", "V" => Object::Reference((999, 0)) },
            dictionary! { "FT" => "Sig", "Kids" => vec![Object::Dictionary(dictionary! { "V" => dictionary! {} })] },
            dictionary! { "Kids" => Object::Integer(3) },
        ] { assert!(inspect_pdf(&pdf(Some(Object::Dictionary(field)), false)).is_err()); }
        assert!(inspect_pdf(&pdf(None, true)).is_err());
        assert!(inspect_pdf(b"not a PDF").is_err());
        let mut history = pdf(None, false);
        history.extend_from_slice(b"\n%%EOF");
        assert!(inspect_pdf(&history).is_err());
    }

    #[test]
    fn signature_descendants_inherit_values_unless_explicitly_overridden_with_null() {
        let inherited = dictionary! { "FT" => "Tx", "V" => Object::string_literal("parent value"),
            "Kids" => vec![Object::Dictionary(dictionary! { "FT" => "Sig" })] };
        assert!(inspect_pdf(&pdf(Some(Object::Dictionary(inherited.clone())), false)).is_err());

        let null_override = dictionary! { "FT" => "Tx", "V" => Object::string_literal("parent value"),
            "Kids" => vec![Object::Dictionary(dictionary! { "FT" => "Sig", "V" => Object::Null })] };
        assert!(inspect_pdf(&pdf(Some(Object::Dictionary(null_override)), false)).is_ok());

        for (override_value, safe) in [(None, false), (Some(Object::Null), true), (Some(Object::Reference((999, 0))), false)] {
            let mut middle = dictionary! { "Kids" => vec![Object::Dictionary(dictionary! { "FT" => "Sig" })] };
            if let Some(value) = override_value { middle.set("V", value); }
            let parent = dictionary! { "FT" => "Tx", "V" => Object::string_literal("parent value"),
                "Kids" => vec![Object::Dictionary(middle)] };
            assert_eq!(inspect_pdf(&pdf(Some(Object::Dictionary(parent)), false)).is_ok(), safe);
        }

        // Indirect null is also an explicit override, not a populated reference.
        let mut doc = Document::load_mem(&pdf(Some(Object::Dictionary(inherited)), false)).unwrap();
        let root_field = doc.catalog().unwrap().get(b"AcroForm").unwrap().as_dict().unwrap()
            .get(b"Fields").unwrap().as_array().unwrap()[0].as_reference().unwrap();
        let null = doc.add_object(Object::Null);
        let parent = doc.get_object_mut(root_field).unwrap().as_dict_mut().unwrap();
        parent.get_mut(b"Kids").unwrap().as_array_mut().unwrap()[0].as_dict_mut().unwrap().set("V", null);
        let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
        assert!(inspect_pdf(&bytes).is_ok());
    }

    #[test]
    fn detects_orphan_signature_objects_and_broken_field_cycles() {
        let mut doc = Document::load_mem(&pdf(None, false)).unwrap();
        doc.add_object(dictionary! { "Type" => "Sig", "Contents" => Object::string_literal("") });
        let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
        assert!(inspect_pdf(&bytes).is_err());
        let mut doc = Document::load_mem(&pdf(Some(Object::Dictionary(dictionary! { "FT" => "Sig" })), false)).unwrap();
        let form = doc.catalog().unwrap().get(b"AcroForm").unwrap().as_dict().unwrap();
        let id = form.get(b"Fields").unwrap().as_array().unwrap()[0].as_reference().unwrap();
        doc.get_object_mut(id).unwrap().as_dict_mut().unwrap().set("Kids", vec![Object::Reference(id)]);
        let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
        assert!(inspect_pdf(&bytes).is_err());
    }

    #[test]
    fn exact_snapshot_rechecks_changes_and_callback_failure_without_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.pdf");
        let original = pdf(None, false);
        std::fs::write(&target, &original).unwrap();
        let guard = inspect_destination(&target, &[]).unwrap();
        assert_eq!(guard.expected_bytes(), Some(original.as_slice()));
        assert!(guard.commit(b"new", || Err("injected failure".into())).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(guard.commit(b"new", || { std::fs::write(&target, b"external").unwrap(); Ok(()) }).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"external");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn absent_target_does_not_clobber_racing_creator_and_new_output_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new.pdf");
        let guard = inspect_destination(&target, &[]).unwrap();
        assert_eq!(guard.expected_bytes(), None);
        assert!(guard.commit(b"ours", || { std::fs::write(&target, b"theirs").unwrap(); Ok(()) }).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
        std::fs::remove_file(&target).unwrap();
        let guard = inspect_destination(&target, &[]).unwrap();
        guard.commit(b"ours", || Ok(())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"ours");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn source_aliases_including_hard_links_are_rejected_and_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let alias = dir.path().join("alias.pdf");
        std::fs::write(&source, pdf(None, false)).unwrap();
        assert!(inspect_destination(&source, &[&source]).is_err());
        std::fs::hard_link(&source, &alias).unwrap();
        assert!(inspect_destination(&alias, &[&source]).is_err());
        std::fs::remove_file(&alias).unwrap();
        let guard = inspect_destination(&alias, &[&source]).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        assert!(guard.recheck().is_err());
    }

    #[test]
    fn missing_source_can_recover_to_a_distinct_output_but_not_its_own_location() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deleted.pdf");
        let target = dir.path().join("recovered.pdf");
        assert!(!source.exists());
        assert!(inspect_destination(&source, &[&source]).is_err());
        assert!(inspect_destination(&dir.path().join(".").join("deleted.pdf"), &[&source]).is_err());
        let guard = inspect_destination(&target, &[&source]).unwrap();
        guard.commit(b"recovered immutable snapshot", || Ok(())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"recovered immutable snapshot");
        assert!(!source.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        // Losing the source's directory must not prevent recovery elsewhere.
        let source = dir.path().join("deleted-directory").join("original.pdf");
        let target = dir.path().join("second-recovery.pdf");
        inspect_destination(&target, &[&source]).unwrap().commit(b"snapshot", || Ok(())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"snapshot");
    }

    #[test]
    fn source_appearing_as_a_destination_alias_during_preparation_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deleted.pdf");
        let target = dir.path().join("existing.pdf");
        let original = pdf(None, false);
        std::fs::write(&target, &original).unwrap();
        let guard = inspect_destination(&target, &[&source]).unwrap();
        assert!(guard.commit(b"must not replace alias", || {
            std::fs::hard_link(&target, &source).map_err(|e| e.to_string())?;
            Ok(())
        }).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "temporary output was removed");
    }

    #[test]
    fn existing_replacement_succeeds_with_read_identity_handles_alive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let target = dir.path().join("target.pdf");
        let original = pdf(None, false);
        std::fs::write(&source, &original).unwrap();
        std::fs::write(&target, &original).unwrap();
        let guard = inspect_destination(&target, &[&source]).unwrap();
        let replacement = pdf(Some(Object::Dictionary(dictionary! { "FT" => "Sig", "V" => Object::Null })), false);
        guard.commit(&replacement, || Ok(())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), replacement);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        // Keep the inspected destination handle live past the successful rename.
        assert_eq!(guard.expected_bytes(), Some(original.as_slice()));
    }

    #[test]
    fn replacement_by_another_file_with_identical_bytes_is_not_the_same_destination() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.pdf");
        let moved = dir.path().join("moved.pdf");
        let original = pdf(None, false);
        std::fs::write(&target, &original).unwrap();
        let guard = inspect_destination(&target, &[]).unwrap();
        std::fs::rename(&target, &moved).unwrap();
        std::fs::write(&target, &original).unwrap();
        assert!(guard.commit(b"ours", || Ok(())).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_eq!(std::fs::read(&moved).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn indirect_inherited_fields_and_bad_forms_are_conservatively_inspected() {
        let mut doc = Document::load_mem(&pdf(None, false)).unwrap();
        let null = doc.add_object(Object::Null);
        let child = doc.add_object(dictionary! { "V" => null });
        let parent = doc.add_object(dictionary! { "FT" => "Sig", "Kids" => vec![Object::Reference(child)] });
        let form = doc.add_object(dictionary! { "Fields" => vec![Object::Reference(parent)] });
        doc.catalog_mut().unwrap().set("AcroForm", form);
        let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
        assert!(inspect_pdf(&bytes).is_ok());
        doc.get_object_mut(child).unwrap().as_dict_mut().unwrap().set("V", dictionary! {});
        let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
        assert!(inspect_pdf(&bytes).is_err());
        for value in [Object::Integer(0), Object::Reference((999, 0)), Object::Dictionary(dictionary! { "Fields" => Object::Integer(0) })] {
            doc.catalog_mut().unwrap().set("AcroForm", value);
            let mut bytes = Vec::new(); doc.save_to(&mut bytes).unwrap();
            assert!(inspect_pdf(&bytes).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn case_alias_of_input_is_not_an_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        std::fs::write(&source, pdf(None, false)).unwrap();
        assert!(inspect_destination(&dir.path().join("SOURCE.PDF"), &[&source]).is_err());
        std::fs::remove_file(&source).unwrap();
        assert!(inspect_destination(&dir.path().join("SOURCE.PDF"), &[&source]).is_err(), "a missing source's case alias stays protected");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_dangling_symlinks_are_not_replacement_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let alias = dir.path().join("alias.pdf");
        std::fs::write(&source, pdf(None, false)).unwrap();
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        assert!(inspect_destination(&alias, &[&source]).is_err());
        std::fs::remove_file(&source).unwrap();
        assert!(inspect_destination(&alias, &[]).is_err());
        assert!(inspect_destination(&source, &[&alias]).is_err(), "a dangling source symlink's resolved target stays protected");
    }

    #[test]
    fn oversized_existing_destination_is_refused_without_reading_its_body() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("large.pdf");
        let file = File::create(&target).unwrap();
        file.set_len(MAX_DESTINATION_BYTES as u64 + 1).unwrap();
        assert!(inspect_destination(&target, &[]).err().unwrap().contains("too large"));
        assert_eq!(std::fs::metadata(&target).unwrap().len(), MAX_DESTINATION_BYTES as u64 + 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn signed_and_unreadable_targets_never_create_temporary_or_output_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.pdf");
        for bytes in [pdf(Some(Object::Dictionary(dictionary! { "FT" => "Sig", "V" => dictionary! {} })), false), b"broken".to_vec()] {
            std::fs::write(&target, &bytes).unwrap();
            assert!(inspect_destination(&target, &[]).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
}
