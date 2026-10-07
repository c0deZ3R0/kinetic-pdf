//! Immutable document revisions shared by readers. A generation identifies
//! an open document; a revision identifies the exact bytes read or committed.

use std::sync::Arc;

#[derive(Clone)]
pub struct Snapshot {
    bytes: Arc<Vec<u8>>,
    file: u64,
}

impl Snapshot {
    pub fn new(bytes: Vec<u8>) -> Self {
        let file = crate::cache::fingerprint(&bytes);
        Self { bytes: Arc::new(bytes), file }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn file(&self) -> u64 {
        self.file
    }

    /// Render processes stream from an immutable file rather than independently
    /// opening the user's mutable path. Only needed when helpers are running.
    pub(crate) fn backing_file(&self) -> std::io::Result<Arc<tempfile::NamedTempFile>> {
        use std::io::Write;
        let mut file = tempfile::Builder::new().prefix("kinetic-pdf-revision-").suffix(".pdf").tempfile()?;
        file.write_all(self.bytes())?;
        Ok(Arc::new(file))
    }
}
