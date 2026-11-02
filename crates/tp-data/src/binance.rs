//! Binance public data archive, spot daily klines.

pub mod archive;
pub mod checksum;
pub mod download;
pub mod kline;
pub mod listing;

use tp_core::market::Candle;

/// Errors while decoding one monthly kline file.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("file name {0:?} does not end in .zip")]
    FileName(String),
    #[error(transparent)]
    Checksum(#[from] checksum::VerifyError),
    #[error(transparent)]
    Archive(#[from] archive::ExtractError),
    #[error(transparent)]
    Kline(#[from] kline::ParseError),
}

/// Verifies, extracts and parses one monthly daily-kline zip file.
///
/// The checksum is verified before the archive is opened, and the archive must
/// hold exactly one CSV named after the zip file.
///
/// # Errors
///
/// Returns [`DecodeError`] when the name is not a zip name, the checksum does
/// not match, the archive is invalid, or any row fails validation.
pub fn decode_monthly_file(
    zip_name: &str,
    zip_bytes: &[u8],
    checksum_text: &str,
) -> Result<Vec<Candle>, DecodeError> {
    let stem = zip_name
        .strip_suffix(".zip")
        .ok_or_else(|| DecodeError::FileName(zip_name.to_owned()))?;
    checksum::verify(zip_name, zip_bytes, checksum_text)?;
    let csv = archive::extract_single_csv(zip_bytes, &format!("{stem}.csv"))?;
    Ok(kline::parse_daily_klines(&csv)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const ROW: &str = "1733011200000,96407.99000000,97836.00000000,95693.88000000,97185.18000000,16938.60452000,1733097599999,1641327626.60622060,3342200,8114.89569000,786489040.19363780,0";

    fn make_zip(name: &str, content: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(name, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn decodes_valid_file() {
        let zip = make_zip("BTCUSDT-1d-2024-12.csv", ROW);
        let checksum = format!("{}  BTCUSDT-1d-2024-12.zip", checksum::sha256_hex(&zip));
        let candles = decode_monthly_file("BTCUSDT-1d-2024-12.zip", &zip, &checksum).unwrap();
        assert_eq!(candles.len(), 1);
    }

    #[test]
    fn rejects_tampered_file() {
        let zip = make_zip("BTCUSDT-1d-2024-12.csv", ROW);
        let checksum = format!("{}  BTCUSDT-1d-2024-12.zip", checksum::sha256_hex(b"other"));
        let result = decode_monthly_file("BTCUSDT-1d-2024-12.zip", &zip, &checksum);
        assert!(matches!(result, Err(DecodeError::Checksum(_))));
    }

    #[test]
    fn rejects_non_zip_name() {
        let result = decode_monthly_file("BTCUSDT-1d-2024-12.csv", b"", "");
        assert!(matches!(result, Err(DecodeError::FileName(_))));
    }
}
