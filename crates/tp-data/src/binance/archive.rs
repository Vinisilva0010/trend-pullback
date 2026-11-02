//! Extraction of the single CSV inside a Binance zip file.

use std::io::{Cursor, Read};

/// Upper bound for one extracted CSV. A monthly daily-kline file is a few KB;
/// the limit guards against corrupted or malicious archives.
pub const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

/// Errors while extracting the CSV from a zip archive.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("invalid zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("expected exactly one file in the archive, found {0}")]
    EntryCount(usize),
    #[error("archive contains {found:?}, expected {expected:?}")]
    EntryName { expected: String, found: String },
    #[error("archive entry exceeds the {limit} byte limit")]
    TooLarge { limit: u64 },
    #[error("failed to read archive entry: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive entry is not valid UTF-8")]
    Utf8,
}

/// Extracts the only entry of `zip_bytes`, which must be named `expected_name`.
///
/// # Errors
///
/// Returns [`ExtractError`] when the archive is invalid, does not hold exactly
/// one entry with the expected name, is larger than [`MAX_ENTRY_BYTES`], or is
/// not UTF-8 text.
pub fn extract_single_csv(zip_bytes: &[u8], expected_name: &str) -> Result<String, ExtractError> {
    extract_with_limit(zip_bytes, expected_name, MAX_ENTRY_BYTES)
}

fn extract_with_limit(
    zip_bytes: &[u8],
    expected_name: &str,
    limit: u64,
) -> Result<String, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))?;
    if archive.len() != 1 {
        return Err(ExtractError::EntryCount(archive.len()));
    }
    let entry = archive.by_index(0)?;
    let entry_name = entry.name()?.into_owned();
    if entry_name != expected_name {
        return Err(ExtractError::EntryName {
            expected: expected_name.to_owned(),
            found: entry_name,
        });
    }
    let mut buffer = Vec::new();
    entry
        .take(limit.saturating_add(1))
        .read_to_end(&mut buffer)?;
    let size = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
    if size > limit {
        return Err(ExtractError::TooLarge { limit });
    }
    String::from_utf8(buffer).map_err(|_| ExtractError::Utf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_single_entry() {
        let zip = make_zip(&[("a.csv", b"1,2,3".as_slice())]);
        assert_eq!(extract_single_csv(&zip, "a.csv").unwrap(), "1,2,3");
    }

    #[test]
    fn rejects_empty_archive() {
        let zip = make_zip(&[]);
        assert!(matches!(
            extract_single_csv(&zip, "a.csv"),
            Err(ExtractError::EntryCount(0))
        ));
    }

    #[test]
    fn rejects_two_entries() {
        let zip = make_zip(&[("a.csv", b"1".as_slice()), ("b.csv", b"2".as_slice())]);
        assert!(matches!(
            extract_single_csv(&zip, "a.csv"),
            Err(ExtractError::EntryCount(2))
        ));
    }

    #[test]
    fn rejects_wrong_name() {
        let zip = make_zip(&[("b.csv", b"1".as_slice())]);
        assert!(matches!(
            extract_single_csv(&zip, "a.csv"),
            Err(ExtractError::EntryName { .. })
        ));
    }

    #[test]
    fn rejects_entry_over_limit() {
        let zip = make_zip(&[("a.csv", b"12345678901".as_slice())]);
        assert!(matches!(
            extract_with_limit(&zip, "a.csv", 10),
            Err(ExtractError::TooLarge { limit: 10 })
        ));
        assert!(extract_with_limit(&zip, "a.csv", 11).is_ok());
    }

    #[test]
    fn rejects_invalid_utf8() {
        let zip = make_zip(&[("a.csv", [0xff_u8, 0xfe].as_slice())]);
        assert!(matches!(
            extract_single_csv(&zip, "a.csv"),
            Err(ExtractError::Utf8)
        ));
    }

    #[test]
    fn rejects_garbage_bytes() {
        assert!(matches!(
            extract_single_csv(b"not a zip", "a.csv"),
            Err(ExtractError::Zip(_))
        ));
    }
}
