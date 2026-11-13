//! Small RSA-public-blob to DER SPKI encoder, not a cryptographic implementation.
//!
//! PATH: experiments/signature-key-broker/src/spki.rs
//! CONSUMED BY: Windows test adapter and gated public-encoding regression tests.
//! Accepts only fixed RSA-2048 PUBLIC blobs; rejects private components/trailing bytes.

use crate::CryptoError;

pub(crate) fn from_rsa_public_blob(blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < 24 { return Err(CryptoError::InvalidPublicBlob); }
    let word = |offset| u32::from_le_bytes(blob[offset..offset + 4].try_into().unwrap());
    let exponent_bytes = word(8) as usize;
    if word(0) != 0x3141_5352 || word(4) != 2048 || !(1..=8).contains(&exponent_bytes)
        || word(12) != 256 || word(16) != 0 || word(20) != 0 || blob.len() != 24 + exponent_bytes + 256
    { return Err(CryptoError::InvalidPublicBlob); }
    let exponent = &blob[24..24 + exponent_bytes];
    let modulus = &blob[24 + exponent_bytes..];
    let e = exponent.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    if e < 3 || e % 2 == 0 || modulus[0] & 0x80 == 0 || modulus[255] & 1 == 0 {
        return Err(CryptoError::InvalidPublicBlob);
    }
    Ok(rsa_spki(modulus, exponent))
}

fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut output = vec![tag];
    if value.len() < 128 {
        output.push(value.len() as u8);
    } else {
        let bytes = value.len().to_be_bytes();
        let start = bytes.iter().position(|b| *b != 0).unwrap();
        output.push(0x80 | (bytes.len() - start) as u8);
        output.extend_from_slice(&bytes[start..]);
    }
    output.extend_from_slice(value);
    output
}

fn positive_integer(value: &[u8]) -> Vec<u8> {
    let start = value.iter().position(|b| *b != 0).unwrap_or(value.len() - 1);
    let value = &value[start..];
    let mut unsigned = Vec::new();
    if value[0] & 0x80 != 0 { unsigned.push(0); }
    unsigned.extend_from_slice(value);
    tlv(0x02, &unsigned)
}

fn rsa_spki(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    let mut key = positive_integer(modulus);
    key.extend_from_slice(&positive_integer(exponent));
    let rsa_key = tlv(0x30, &key);
    let mut bits = vec![0]; // BIT STRING has zero unused bits.
    bits.extend_from_slice(&rsa_key);
    // AlgorithmIdentifier: rsaEncryption (1.2.840.113549.1.1.1), NULL parameters.
    let mut spki = vec![0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
    spki.extend_from_slice(&tlv(0x03, &bits));
    tlv(0x30, &spki)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_positive_integer_sign_and_sequence_lengths() {
        assert_eq!(positive_integer(&[0, 0x80]), [2, 2, 0, 0x80]);
        assert_eq!(positive_integer(&[0, 3]), [2, 1, 3]);
        let spki = rsa_spki(&[0x80], &[3]);
        assert_eq!(&spki[..2], &[0x30, 0x1b]);
        assert_eq!(&spki[17..], &[3, 10, 0, 0x30, 7, 2, 2, 0, 0x80, 2, 1, 3]);
    }

    #[test]
    fn refuses_private_blob_magic_invalid_lengths_and_trailing_components() {
        let mut blob = Vec::new();
        for word in [0x3141_5352u32, 2048, 3, 256, 0, 0] { blob.extend_from_slice(&word.to_le_bytes()); }
        blob.extend_from_slice(&[1, 0, 1]);
        blob.extend_from_slice(&[0xff; 256]);
        assert!(from_rsa_public_blob(&blob).is_ok());
        blob[..4].copy_from_slice(&0x3341_5352u32.to_le_bytes()); // RSA3/full-private.
        assert!(from_rsa_public_blob(&blob).is_err());
        blob[..4].copy_from_slice(&0x3141_5352u32.to_le_bytes());
        blob.push(0);
        assert!(from_rsa_public_blob(&blob).is_err());
        assert!(from_rsa_public_blob(&[]).is_err());
    }
}
