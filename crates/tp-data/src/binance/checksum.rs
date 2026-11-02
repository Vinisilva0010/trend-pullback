//! SHA-256 verification against Binance CHECKSUM files.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Errors while verifying a file against its CHECKSUM content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("CHECKSUM content is malformed")]
    Malformed,
    #[error("CHECKSUM refers to {found:?}, expected {expected:?}")]
    FileName { expected: String, found: String },
    #[error("SHA-256 mismatch for {file}: expected {expected}, computed {actual}")]
    Mismatch {
        file: String,
        expected: String,
        actual: String,
    },
}

/// Verifies `bytes` against CHECKSUM content in the `sha256sum` format
/// (`<64 hex chars>  <file name>`).
///
/// # Errors
///
/// Returns [`VerifyError`] when the content is malformed, names another file,
/// or the computed hash differs from the expected one.
pub fn verify(file_name: &str, bytes: &[u8], checksum_text: &str) -> Result<(), VerifyError> {
    let mut parts = checksum_text.split_whitespace();
    let (Some(hash), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(VerifyError::Malformed);
    };
    let expected = hash.to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(VerifyError::Malformed);
    }
    let name = name.strip_prefix('*').unwrap_or(name);
    if name != file_name {
        return Err(VerifyError::FileName {
            expected: file_name.to_owned(),
            found: name.to_owned(),
        });
    }
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(VerifyError::Mismatch {
            file: file_name.to_owned(),
            expected,
            actual,
        });
    }
    Ok(())
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest.as_slice() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn computes_known_hash() {
        assert_eq!(sha256_hex(b"abc"), ABC_HASH);
    }

    #[test]
    fn accepts_matching_checksum() {
        assert_eq!(
            verify("f.zip", b"abc", &format!("{ABC_HASH}  f.zip\n")),
            Ok(())
        );
    }

    #[test]
    fn accepts_uppercase_hash_and_binary_marker() {
        let text = format!("{} *f.zip", ABC_HASH.to_ascii_uppercase());
        assert_eq!(verify("f.zip", b"abc", &text), Ok(()));
    }

    #[test]
    fn rejects_mismatch() {
        let result = verify("f.zip", b"abd", &format!("{ABC_HASH}  f.zip"));
        assert!(matches!(result, Err(VerifyError::Mismatch { .. })));
    }

    #[test]
    fn rejects_other_file_name() {
        let result = verify("f.zip", b"abc", &format!("{ABC_HASH}  g.zip"));
        assert!(matches!(result, Err(VerifyError::FileName { .. })));
    }

    #[test]
    fn rejects_malformed_content() {
        let short = format!("{}  f.zip", &ABC_HASH[..63]);
        let non_hex = format!("{}g  f.zip", &ABC_HASH[..63]);
        let extra = format!("{ABC_HASH}  f.zip extra");
        for text in [
            "",
            ABC_HASH,
            short.as_str(),
            non_hex.as_str(),
            extra.as_str(),
        ] {
            assert_eq!(verify("f.zip", b"abc", text), Err(VerifyError::Malformed));
        }
    }
}
