//! Defines the [PdfSignatures] struct, a collection of all the `PdfSignature` objects in a
//! `PdfDocument`. Checked discovery preserves native errors and full counts.
//!
//! PATH: vendor/pdfium-render/src/pdf/document/signatures.rs
//! CONSUMED BY: PdfDocument and Kinetic PDF worker signature protection.

use crate::bindgen::FPDF_DOCUMENT;
use crate::error::{PdfiumError, PdfiumInternalError};
use crate::pdf::document::signature::PdfSignature;
use crate::pdfium::PdfiumLibraryBindingsAccessor;
use std::marker::PhantomData;
use std::ops::{Range, RangeInclusive};
use std::os::raw::c_int;

fn checked_count(count: c_int) -> Result<usize, PdfiumError> {
    usize::try_from(count).map_err(|_| PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::Unknown))
}

#[cfg(test)]
mod count_tests {
    use super::checked_count;

    #[test]
    fn native_errors_and_large_counts_are_not_narrowed() {
        assert!(checked_count(-1).is_err());
        assert_eq!(checked_count(0).unwrap(), 0);
        assert_eq!(checked_count(65536).unwrap(), 65536);
    }
}

/// The zero-based index of a single [PdfSignature] inside its containing [PdfSignatures] collection.
pub type PdfSignatureIndex = u16;

/// The collection of [PdfSignature] objects inside a `PdfDocument`.
pub struct PdfSignatures<'a> {
    document_handle: FPDF_DOCUMENT,
    lifetime: PhantomData<&'a FPDF_DOCUMENT>,
}

impl<'a> PdfSignatures<'a> {
    #[inline]
    pub(crate) fn from_pdfium(document_handle: FPDF_DOCUMENT) -> Self {
        PdfSignatures {
            document_handle,
            lifetime: PhantomData,
        }
    }

    /// Returns the full native count, or an error if discovery failed.
    /// Unlike len(), this does not narrow the count to a u16 index.
    pub fn count(&self) -> Result<usize, PdfiumError> {
        checked_count(unsafe { self.bindings().FPDF_GetSignatureCount(self.document_handle) })
    }

    /// Returns the number of signatures in this [PdfSignatures] collection.
    pub fn len(&self) -> PdfSignatureIndex {
        (unsafe { self.bindings().FPDF_GetSignatureCount(self.document_handle) })
            as PdfSignatureIndex
    }

    /// Returns `true` if this [PdfSignatures] collection is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a Range from `0..(number of signatures)` for this [PdfSignatures] collection.
    #[inline]
    pub fn as_range(&self) -> Range<PdfSignatureIndex> {
        0..self.len()
    }

    /// Returns an inclusive Range from `0..=(number of signatures - 1)`
    /// for this [PdfSignatures] collection.
    #[inline]
    pub fn as_range_inclusive(&self) -> RangeInclusive<PdfSignatureIndex> {
        if self.is_empty() {
            0..=0
        } else {
            0..=(self.len() - 1)
        }
    }

    /// Returns a single [PdfSignature] from this [PdfSignatures] collection.
    pub fn get(&self, index: PdfSignatureIndex) -> Result<PdfSignature<'a>, PdfiumError> {
        if index >= self.len() {
            return Err(PdfiumError::SignatureIndexOutOfBounds);
        }

        let handle = unsafe {
            self.bindings()
                .FPDF_GetSignatureObject(self.document_handle, index as c_int)
        };

        if handle.is_null() {
            Err(PdfiumError::PdfiumLibraryInternalError(
                PdfiumInternalError::Unknown,
            ))
        } else {
            Ok(PdfSignature::from_pdfium(handle))
        }
    }

    /// Returns an iterator over all the signatures in this [PdfSignatures] collection.
    #[inline]
    pub fn iter(&self) -> PdfSignaturesIterator<'_> {
        PdfSignaturesIterator::new(self)
    }
}

impl<'a> PdfiumLibraryBindingsAccessor<'a> for PdfSignatures<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Send for PdfSignatures<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Sync for PdfSignatures<'a> {}

/// An iterator over all the [PdfSignature] objects in a [PdfSignatures] collection.
pub struct PdfSignaturesIterator<'a> {
    signatures: &'a PdfSignatures<'a>,
    next_index: PdfSignatureIndex,
}

impl<'a> PdfSignaturesIterator<'a> {
    #[inline]
    pub(crate) fn new(signatures: &'a PdfSignatures<'a>) -> Self {
        PdfSignaturesIterator {
            signatures,
            next_index: 0,
        }
    }
}

impl<'a> Iterator for PdfSignaturesIterator<'a> {
    type Item = PdfSignature<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.signatures.get(self.next_index);

        self.next_index += 1;

        next.ok()
    }
}
