//! SHA-256 artifact verification shared by the publisher and the client.

use std::io::Read;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

const STREAM_BUFFER_BYTES: usize = 1024 * 1024;

/// Lowercase hex SHA-256 of an in-memory artifact.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Lowercase hex SHA-256 of a stream, computed without loading it whole.
pub fn sha256_hex_reader(mut reader: impl Read) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; STREAM_BUFFER_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("reading artifact for hashing")?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// 64 hex digits, either case.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Verify a stream against a manifest hash (case-insensitive).
pub fn verify_sha256_reader(reader: impl Read, expected_hex: &str) -> Result<()> {
    ensure!(
        is_sha256_hex(expected_hex),
        "expected SHA-256 {expected_hex:?} is not 64 hex digits"
    );
    let actual = sha256_hex_reader(reader)?;
    ensure!(
        actual.eq_ignore_ascii_case(expected_hex),
        "SHA-256 mismatch: computed {actual}, expected {}",
        expected_hex.to_ascii_lowercase()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn slice_and_stream_digests_agree_with_the_known_vector() {
        assert_eq!(sha256_hex(b"abc"), ABC);
        assert_eq!(sha256_hex_reader(&b"abc"[..]).unwrap(), ABC);
        let large = vec![7u8; STREAM_BUFFER_BYTES * 2 + 13];
        assert_eq!(sha256_hex_reader(&large[..]).unwrap(), sha256_hex(&large));
    }

    #[test]
    fn verification_is_case_insensitive_and_rejects_mismatches() {
        verify_sha256_reader(&b"abc"[..], ABC).unwrap();
        verify_sha256_reader(&b"abc"[..], &ABC.to_ascii_uppercase()).unwrap();
        assert!(verify_sha256_reader(&b"abd"[..], ABC).is_err());
        assert!(verify_sha256_reader(&b"abc"[..], "nope").is_err());
    }
}
