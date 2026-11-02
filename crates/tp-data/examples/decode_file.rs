//! Decodes one Binance monthly daily-kline zip plus its CHECKSUM file from disk.
//!
//! Usage: `cargo run -p tp-data --example decode_file -- /path/to/BTCUSDT-1d-2025-01.zip`

use std::{env, error::Error, fs, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let path_arg = env::args()
        .nth(1)
        .ok_or("usage: decode_file <path-to-zip>")?;
    let path = Path::new(&path_arg);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid file name")?;
    let zip_bytes = fs::read(path)?;
    let checksum_text = fs::read_to_string(format!("{path_arg}.CHECKSUM"))?;
    let candles = tp_data::binance::decode_monthly_file(file_name, &zip_bytes, &checksum_text)?;
    let (Some(first), Some(last)) = (candles.first(), candles.last()) else {
        return Err("no candles".into());
    };
    println!(
        "{file_name}: {} candles, {} to {}, first close {:.2}, last close {:.2}",
        candles.len(),
        first.date(),
        last.date(),
        first.close(),
        last.close()
    );
    Ok(())
}
