//! One continuous daily history per symbol, built from verified monthly
//! archives, plus the processed CSV format and a data-quality report.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use tp_core::market::{Candle, CandleFields};

use crate::binance::listing::is_valid_symbol;
use crate::binance::{DecodeError, decode_monthly_file};

/// Header of the processed daily CSV files.
pub const HEADER: &str = "date,open,high,low,close,volume,quote_volume";

/// Days without a candle between two consecutive candles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Last date before the gap.
    pub after: NaiveDate,
    /// First date after the gap.
    pub before: NaiveDate,
}

impl Gap {
    /// Number of calendar days with no candle.
    #[must_use]
    pub fn missing_days(self) -> i64 {
        (self.before - self.after).num_days() - 1
    }
}

/// Daily candles of one symbol in strictly increasing date order.
#[derive(Debug, Clone)]
pub struct SymbolSeries {
    pub symbol: String,
    pub candles: Vec<Candle>,
    pub gaps: Vec<Gap>,
}

/// Summary of one built series, used for the quality report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesStats {
    pub symbol: String,
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub days: usize,
    pub gaps: usize,
    pub missing_days: i64,
    pub largest_gap_days: i64,
}

impl SymbolSeries {
    /// Summary statistics, or `None` for an empty series.
    #[must_use]
    pub fn stats(&self) -> Option<SeriesStats> {
        let (first, last) = (self.candles.first()?, self.candles.last()?);
        Some(SeriesStats {
            symbol: self.symbol.clone(),
            first: first.date(),
            last: last.date(),
            days: self.candles.len(),
            gaps: self.gaps.len(),
            missing_days: self.gaps.iter().copied().map(Gap::missing_days).sum(),
            largest_gap_days: self
                .gaps
                .iter()
                .copied()
                .map(Gap::missing_days)
                .max()
                .unwrap_or(0),
        })
    }
}

/// Result of building every series.
#[derive(Debug, Clone, Default)]
pub struct BuildSummary {
    pub built: Vec<SeriesStats>,
    /// Symbols that could not be built, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Errors while building or reading series.
#[derive(Debug, thiserror::Error)]
pub enum SeriesError {
    #[error("file system error at {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{file}: {source}")]
    Decode { file: String, source: DecodeError },
    #[error("{file}: date {date} conflicts with data from an earlier file")]
    Overlap { file: String, date: NaiveDate },
    #[error("{0}: no monthly files found")]
    Empty(String),
    #[error("{path}, line {line}: {reason}")]
    Processed {
        path: String,
        line: usize,
        reason: String,
    },
}

pub(crate) fn io_err(path: &Path, source: std::io::Error) -> SeriesError {
    SeriesError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Folder that holds processed daily CSV files, one per symbol.
#[must_use]
pub fn processed_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join("processed")
        .join("binance")
        .join("spot")
        .join("1d")
}

/// Loads, verifies and joins every monthly archive of `symbol` in `symbol_dir`.
///
/// # Errors
///
/// Returns [`SeriesError`] when the folder cannot be read, an archive fails
/// verification, two files disagree about the same date, or no archive exists.
pub fn load_symbol_series(symbol_dir: &Path, symbol: &str) -> Result<SymbolSeries, SeriesError> {
    let prefix = format!("{symbol}-1d-");
    let mut zip_names = Vec::new();
    for entry in fs::read_dir(symbol_dir).map_err(|e| io_err(symbol_dir, e))? {
        let entry = entry.map_err(|e| io_err(symbol_dir, e))?;
        let file_name = entry.file_name();
        if let Some(name) = file_name.to_str()
            && name.starts_with(&prefix)
            && Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            zip_names.push(name.to_owned());
        }
    }
    zip_names.sort_unstable();
    let mut candles = Vec::new();
    for name in &zip_names {
        let zip_path = symbol_dir.join(name);
        let checksum_path = symbol_dir.join(format!("{name}.CHECKSUM"));
        let zip = fs::read(&zip_path).map_err(|e| io_err(&zip_path, e))?;
        let checksum = fs::read_to_string(&checksum_path).map_err(|e| io_err(&checksum_path, e))?;
        let month =
            decode_monthly_file(name, &zip, &checksum).map_err(|source| SeriesError::Decode {
                file: name.clone(),
                source,
            })?;
        append_month(&mut candles, month, name)?;
    }
    if candles.is_empty() {
        return Err(SeriesError::Empty(symbol.to_owned()));
    }
    let gaps = find_gaps(&candles);
    Ok(SymbolSeries {
        symbol: symbol.to_owned(),
        candles,
        gaps,
    })
}

fn append_month(
    candles: &mut Vec<Candle>,
    month: Vec<Candle>,
    file: &str,
) -> Result<(), SeriesError> {
    for candle in month {
        if let Some(last) = candles.last() {
            let conflict =
                candle.date() < last.date() || (candle.date() == last.date() && candle != *last);
            if conflict {
                return Err(SeriesError::Overlap {
                    file: file.to_owned(),
                    date: candle.date(),
                });
            }
            if candle.date() == last.date() {
                continue;
            }
        }
        candles.push(candle);
    }
    Ok(())
}

/// Every stretch of missing days between consecutive candles.
#[must_use]
pub fn find_gaps(candles: &[Candle]) -> Vec<Gap> {
    candles
        .windows(2)
        .filter_map(|pair| {
            let [a, b] = pair else {
                return None;
            };
            ((b.date() - a.date()).num_days() > 1).then_some(Gap {
                after: a.date(),
                before: b.date(),
            })
        })
        .collect()
}

/// Serializes candles to the processed CSV format.
#[must_use]
pub fn to_csv(candles: &[Candle]) -> String {
    let mut out = String::with_capacity(candles.len() * 96 + HEADER.len() + 1);
    out.push_str(HEADER);
    out.push('\n');
    for c in candles {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{}",
            c.date(),
            c.open(),
            c.high(),
            c.low(),
            c.close(),
            c.volume(),
            c.quote_volume()
        );
    }
    out
}

/// Parses the processed CSV format, validating every row.
///
/// # Errors
///
/// Returns [`SeriesError::Processed`] on a wrong header, a malformed row, an
/// invalid candle, or dates that are not strictly increasing.
pub fn from_csv(label: &str, text: &str) -> Result<Vec<Candle>, SeriesError> {
    let bad = |line: usize, reason: String| SeriesError::Processed {
        path: label.to_owned(),
        line,
        reason,
    };
    let mut lines = text.lines().enumerate();
    let header_ok = lines
        .next()
        .is_some_and(|(_, header)| header.trim() == HEADER);
    if !header_ok {
        return Err(bad(1, "missing or wrong header".to_owned()));
    }
    let mut candles: Vec<Candle> = Vec::new();
    for (index, raw) in lines {
        let line = index + 1;
        let row = raw.trim();
        if row.is_empty() {
            continue;
        }
        let fields: Vec<&str> = row.split(',').collect();
        let &[date, open, high, low, close, volume, quote_volume] = fields.as_slice() else {
            return Err(bad(
                line,
                format!("expected 7 fields, found {}", fields.len()),
            ));
        };
        let date = date
            .parse::<NaiveDate>()
            .map_err(|_| bad(line, format!("invalid date {date:?}")))?;
        let num = |name: &str, value: &str| {
            value
                .parse::<f64>()
                .map_err(|_| bad(line, format!("invalid {name} {value:?}")))
        };
        let candle = Candle::try_new(CandleFields {
            date,
            open: num("open", open)?,
            high: num("high", high)?,
            low: num("low", low)?,
            close: num("close", close)?,
            volume: num("volume", volume)?,
            quote_volume: num("quote volume", quote_volume)?,
        })
        .map_err(|error| bad(line, error.to_string()))?;
        if candles
            .last()
            .is_some_and(|last| last.date() >= candle.date())
        {
            return Err(bad(
                line,
                format!("date {} is not after the previous row", candle.date()),
            ));
        }
        candles.push(candle);
    }
    Ok(candles)
}

/// Writes one series to `out_dir/SYMBOL.csv`.
///
/// # Errors
///
/// Returns [`SeriesError::Io`] when the folder or file cannot be written.
pub fn write_series(out_dir: &Path, series: &SymbolSeries) -> Result<PathBuf, SeriesError> {
    fs::create_dir_all(out_dir).map_err(|e| io_err(out_dir, e))?;
    let path = out_dir.join(format!("{}.csv", series.symbol));
    write_atomic(&path, to_csv(&series.candles).as_bytes())?;
    Ok(path)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SeriesError> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".part");
    let temp = PathBuf::from(temp);
    fs::write(&temp, bytes).map_err(|e| io_err(&temp, e))?;
    fs::rename(&temp, path).map_err(|e| io_err(path, e))
}

/// Quality report as CSV, one row per built series.
#[must_use]
pub fn quality_report_csv(stats: &[SeriesStats]) -> String {
    let mut out =
        String::from("symbol,first_date,last_date,days,gaps,missing_days,largest_gap_days\n");
    for s in stats {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{}",
            s.symbol, s.first, s.last, s.days, s.gaps, s.missing_days, s.largest_gap_days
        );
    }
    out
}

/// Builds every symbol folder under `kline_dir`, writes processed CSVs to
/// `out_dir` and the quality report to `report_path`.
///
/// # Errors
///
/// Returns [`SeriesError`] only when the archive folder cannot be listed or
/// the report cannot be written. Symbols that fail are collected in
/// [`BuildSummary::failed`].
pub fn build_all(
    kline_dir: &Path,
    out_dir: &Path,
    report_path: &Path,
) -> Result<BuildSummary, SeriesError> {
    let mut symbols = Vec::new();
    for entry in fs::read_dir(kline_dir).map_err(|e| io_err(kline_dir, e))? {
        let entry = entry.map_err(|e| io_err(kline_dir, e))?;
        let file_name = entry.file_name();
        if let Some(name) = file_name.to_str() {
            symbols.push(name.to_owned());
        }
    }
    symbols.sort_unstable();
    let mut summary = BuildSummary::default();
    for symbol in symbols {
        if !is_valid_symbol(&symbol) {
            summary
                .failed
                .push((symbol, "not a valid symbol name".to_owned()));
            continue;
        }
        let result = load_symbol_series(&kline_dir.join(&symbol), &symbol)
            .and_then(|series| write_series(out_dir, &series).map(|_| series));
        match result {
            Ok(series) => {
                if let Some(stats) = series.stats() {
                    summary.built.push(stats);
                }
            }
            Err(error) => summary.failed.push((symbol, error.to_string())),
        }
    }
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    write_atomic(report_path, quality_report_csv(&summary.built).as_bytes())?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binance::checksum::sha256_hex;
    use std::io::{Cursor, Write};

    const DEC_01: &str = "1733011200000,96407.99000000,97836.00000000,95693.88000000,97185.18000000,16938.60452000,1733097599999,1641327626.60622060,3342200,8114.89569000,786489040.19363780,0";
    const DEC_02: &str = "1733097600000,97185.17000000,98130.00000000,94395.00000000,95840.62000000,37958.66981000,1733183999999,3646639621.73758780,7560103,18821.93327000,1808458555.89058410,0";
    const JAN_01: &str = "1735689600000000,93576.00000000,95151.15000000,92888.00000000,94591.79000000,10373.32613000,1735775999999999,975444194.13799830,1516556,5347.73648000,502914035.64059070,0";

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn candle(date: NaiveDate, close: f64) -> Candle {
        Candle::try_new(CandleFields {
            date,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
            quote_volume: close,
        })
        .unwrap()
    }

    fn make_zip(name: &str, content: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(name, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tp-history-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_gaps() {
        let candles = vec![
            candle(d(2024, 1, 1), 1.0),
            candle(d(2024, 1, 2), 1.0),
            candle(d(2024, 1, 5), 1.0),
        ];
        let gaps = find_gaps(&candles);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].missing_days(), 2);
        assert!(find_gaps(&candles[..2]).is_empty());
    }

    #[test]
    fn appends_identical_overlap_and_rejects_conflicts() {
        let mut candles = vec![candle(d(2024, 1, 1), 1.0)];
        append_month(
            &mut candles,
            vec![candle(d(2024, 1, 1), 1.0), candle(d(2024, 1, 2), 2.0)],
            "f",
        )
        .unwrap();
        assert_eq!(candles.len(), 2);
        assert!(matches!(
            append_month(&mut candles, vec![candle(d(2024, 1, 2), 3.0)], "g"),
            Err(SeriesError::Overlap { .. })
        ));
        assert!(matches!(
            append_month(&mut candles, vec![candle(d(2023, 12, 31), 1.0)], "h"),
            Err(SeriesError::Overlap { .. })
        ));
    }

    #[test]
    fn csv_round_trip_is_exact() {
        let original = vec![
            Candle::try_new(CandleFields {
                date: d(2025, 1, 1),
                open: 93_576.0,
                high: 95_151.15,
                low: 92_888.0,
                close: 94_591.79,
                volume: 10_373.326_13,
                quote_volume: 975_444_194.14,
            })
            .unwrap(),
            Candle::try_new(CandleFields {
                date: d(2025, 1, 2),
                open: 1e-8,
                high: 3e-8,
                low: 1e-8,
                close: 2e-8,
                volume: 0.0,
                quote_volume: 0.0,
            })
            .unwrap(),
        ];
        let text = to_csv(&original);
        assert_eq!(from_csv("t", &text).unwrap(), original);
    }

    #[test]
    fn rejects_bad_processed_csv() {
        let header = format!("{HEADER}\n");
        assert!(from_csv("t", "wrong\n").is_err());
        assert!(from_csv("t", &format!("{header}2025-13-01,1,1,1,1,0,0\n")).is_err());
        assert!(from_csv("t", &format!("{header}2025-01-01,1,1,1\n")).is_err());
        assert!(
            from_csv(
                "t",
                &format!("{header}2025-01-02,1,1,1,1,0,0\n2025-01-01,1,1,1,1,0,0\n")
            )
            .is_err()
        );
        assert!(from_csv("t", &format!("{header}2025-01-01,1,2,1,1,0,0\n")).is_ok());
        assert!(from_csv("t", &format!("{header}2025-01-01,1,0.5,1,1,0,0\n")).is_err());
        assert!(from_csv("t", &header).unwrap().is_empty());
    }

    #[test]
    fn builds_series_across_months_with_report() {
        let root = fresh_dir("build");
        let kline = root.join("klines");
        let symbol_dir = kline.join("BTCUSDT");
        fs::create_dir_all(&symbol_dir).unwrap();
        for (stem, content) in [
            ("BTCUSDT-1d-2024-12", format!("{DEC_01}\n{DEC_02}")),
            ("BTCUSDT-1d-2025-01", JAN_01.to_owned()),
        ] {
            let zip = make_zip(&format!("{stem}.csv"), &content);
            fs::write(symbol_dir.join(format!("{stem}.zip")), &zip).unwrap();
            fs::write(
                symbol_dir.join(format!("{stem}.zip.CHECKSUM")),
                format!("{}  {stem}.zip\n", sha256_hex(&zip)),
            )
            .unwrap();
        }
        let out = root.join("processed");
        let report = root.join("report.csv");
        let summary = build_all(&kline, &out, &report).unwrap();
        assert!(summary.failed.is_empty());
        let stats = &summary.built[0];
        assert_eq!(stats.days, 3);
        assert_eq!(stats.gaps, 1);
        assert_eq!(stats.missing_days, 29);
        let written = fs::read_to_string(out.join("BTCUSDT.csv")).unwrap();
        assert_eq!(from_csv("BTCUSDT.csv", &written).unwrap().len(), 3);
        assert!(
            fs::read_to_string(&report)
                .unwrap()
                .contains("BTCUSDT,2024-12-01,2025-01-01,3,1,29,29")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_tampered_archive_as_failed_symbol() {
        let root = fresh_dir("tampered");
        let kline = root.join("klines");
        let symbol_dir = kline.join("ETHUSDT");
        fs::create_dir_all(&symbol_dir).unwrap();
        let zip = make_zip("ETHUSDT-1d-2024-12.csv", DEC_01);
        fs::write(symbol_dir.join("ETHUSDT-1d-2024-12.zip"), &zip).unwrap();
        fs::write(
            symbol_dir.join("ETHUSDT-1d-2024-12.zip.CHECKSUM"),
            format!("{}  ETHUSDT-1d-2024-12.zip\n", sha256_hex(b"other")),
        )
        .unwrap();
        let summary = build_all(&kline, &root.join("out"), &root.join("r.csv")).unwrap();
        assert!(summary.built.is_empty());
        assert_eq!(summary.failed.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
