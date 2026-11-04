//! Recent daily klines from the public Binance market-data API, used to
//! extend the archive (monthly archive files appear only after a month ends).

use chrono::{DateTime, NaiveDate, NaiveTime};
use serde_json::Value;
use tp_core::intraday::Bar;
use tp_core::market::{Candle, CandleFields};

use super::download::{DownloadError, Http};
use super::listing::is_valid_symbol;

/// Public market-data endpoint (no account or key).
pub const API_URL: &str = "https://data-api.binance.vision/api/v3/klines";

/// Errors while fetching or parsing recent klines.
#[derive(Debug, thiserror::Error)]
pub enum RestError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("klines response is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("klines row {row}: {reason}")]
    Row { row: usize, reason: String },
    #[error("invalid symbol {0:?}")]
    Symbol(String),
}

/// Parses a klines JSON response (millisecond times), keeping only candles
/// that closed before `now_ms`.
///
/// # Errors
///
/// Returns [`RestError`] on malformed JSON, a malformed row, an invalid
/// candle, a candle not opening at 00:00 UTC, or dates out of order.
pub fn parse_klines_json(text: &str, now_ms: i64) -> Result<Vec<Candle>, RestError> {
    let rows: Vec<Vec<Value>> = serde_json::from_str(text)?;
    let mut out: Vec<Candle> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let bad = |reason: &str| RestError::Row {
            row: index,
            reason: reason.to_owned(),
        };
        let (Some(open_time), Some(close_time)) = (
            row.first().and_then(Value::as_i64),
            row.get(6).and_then(Value::as_i64),
        ) else {
            return Err(bad("missing open or close time"));
        };
        if close_time >= now_ms {
            continue;
        }
        let number = |i: usize, name: &str| -> Result<f64, RestError> {
            row.get(i)
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<f64>().ok())
                .ok_or_else(|| bad(&format!("invalid {name}")))
        };
        let opened = DateTime::from_timestamp_millis(open_time)
            .ok_or_else(|| bad("open time out of range"))?;
        if opened.time() != NaiveTime::MIN {
            return Err(bad("daily candle does not open at 00:00 UTC"));
        }
        let candle = Candle::try_new(CandleFields {
            date: opened.date_naive(),
            open: number(1, "open")?,
            high: number(2, "high")?,
            low: number(3, "low")?,
            close: number(4, "close")?,
            volume: number(5, "volume")?,
            quote_volume: number(7, "quote volume")?,
        })
        .map_err(|e| bad(&e.to_string()))?;
        if out.last().is_some_and(|c| c.date() >= candle.date()) {
            return Err(bad("dates are not increasing"));
        }
        out.push(candle);
    }
    Ok(out)
}

/// Fetches closed daily candles of `symbol` from `since` (inclusive).
///
/// # Errors
///
/// Returns [`RestError`] for an invalid symbol, a failed request, or an
/// invalid response.
pub async fn fetch_daily_since(
    symbol: &str,
    since: NaiveDate,
    now_ms: i64,
) -> Result<Vec<Candle>, RestError> {
    if !is_valid_symbol(symbol) {
        return Err(RestError::Symbol(symbol.to_owned()));
    }
    let start_ms = since
        .and_hms_opt(0, 0, 0)
        .map_or(0, |d| d.and_utc().timestamp_millis());
    let url = format!("{API_URL}?symbol={symbol}&interval=1d&startTime={start_ms}&limit=1000");
    let http = Http::new(5)?;
    let body = http.get_bytes(&url).await.map_err(DownloadError::from)?;
    let text = String::from_utf8(body).map_err(|_| DownloadError::Utf8)?;
    parse_klines_json(&text, now_ms)
}

/// Parses an hourly klines JSON response, keeping bars that closed before `now_ms`.
///
/// # Errors
///
/// Returns [`RestError`] on malformed JSON or rows, inconsistent prices, or
/// times that do not increase.
pub fn parse_hourly_json(text: &str, now_ms: i64) -> Result<Vec<Bar>, RestError> {
    let rows: Vec<Vec<Value>> = serde_json::from_str(text)?;
    let mut out: Vec<Bar> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let bad = |reason: &str| RestError::Row {
            row: index,
            reason: reason.to_owned(),
        };
        let (Some(open_time), Some(close_time)) = (
            row.first().and_then(Value::as_i64),
            row.get(6).and_then(Value::as_i64),
        ) else {
            return Err(bad("missing open or close time"));
        };
        if close_time >= now_ms {
            continue;
        }
        let number = |i: usize, name: &str| -> Result<f64, RestError> {
            row.get(i)
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|x| x.is_finite())
                .ok_or_else(|| bad(&format!("invalid {name}")))
        };
        let bar = Bar {
            open_time_ms: open_time,
            open: number(1, "open")?,
            high: number(2, "high")?,
            low: number(3, "low")?,
            close: number(4, "close")?,
            volume: number(5, "volume")?,
        };
        if !(bar.low > 0.0
            && bar.low <= bar.open.min(bar.close)
            && bar.high >= bar.open.max(bar.close))
        {
            return Err(bad("inconsistent prices"));
        }
        if out.last().is_some_and(|p| p.open_time_ms >= open_time) {
            return Err(bad("times are not increasing"));
        }
        out.push(bar);
    }
    Ok(out)
}

/// Fetches the closed hourly bars of `symbol` from the last `hours` hours.
///
/// # Errors
///
/// Returns [`RestError`] for an invalid symbol, a failed request, or an
/// invalid response.
pub async fn fetch_recent_hourly(
    symbol: &str,
    hours: i64,
    now_ms: i64,
) -> Result<Vec<Bar>, RestError> {
    if !is_valid_symbol(symbol) {
        return Err(RestError::Symbol(symbol.to_owned()));
    }
    let start_ms = now_ms - hours * 3_600_000;
    let url = format!("{API_URL}?symbol={symbol}&interval=1h&startTime={start_ms}&limit=1000");
    let http = Http::new(5)?;
    let body = http.get_bytes(&url).await.map_err(DownloadError::from)?;
    let text = String::from_utf8(body).map_err(|_| DownloadError::Utf8)?;
    parse_hourly_json(&text, now_ms)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const DAY1: &str = r#"[1761955200000,"110000.0","111000.0","109000.0","110500.0","100.0",1762041599999,"11000000.0",1000,"50.0","5500000.0","0"]"#;
    const DAY2: &str = r#"[1762041600000,"110500.0","112000.0","110000.0","111500.0","120.0",1762127999999,"13000000.0",1200,"60.0","6500000.0","0"]"#;

    #[test]
    fn parses_hourly_bars() {
        let h1 = r#"[1761955200000,"100.0","101.0","99.0","100.5","1.0",1761958799999,"100.0",1,"0","0","0"]"#;
        let h2 = r#"[1761958800000,"100.5","102.0","100.0","101.5","1.0",1761962399999,"100.0",1,"0","0","0"]"#;
        let both = format!("[{h1},{h2}]");
        let bars = parse_hourly_json(&both, 1_761_962_400_000).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[1].close, 101.5);
        assert_eq!(
            parse_hourly_json(&both, 1_761_960_000_000).unwrap().len(),
            1
        );
        assert!(parse_hourly_json(&format!("[{h2},{h1}]"), i64::MAX).is_err());
    }

    #[test]
    fn keeps_only_closed_candles() {
        let text = format!("[{DAY1},{DAY2}]");
        let all = parse_klines_json(&text, 1_762_128_000_000).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].date(), NaiveDate::from_ymd_opt(2025, 11, 1).unwrap());
        assert_eq!(all[1].close(), 111_500.0);
        let open_day_excluded = parse_klines_json(&text, 1_762_100_000_000).unwrap();
        assert_eq!(open_day_excluded.len(), 1);
    }

    #[test]
    fn rejects_bad_rows() {
        assert!(parse_klines_json("not json", 0).is_err());
        let bad_price = DAY1.replacen("\"111000.0\"", "\"abc\"", 1);
        assert!(parse_klines_json(&format!("[{bad_price}]"), i64::MAX).is_err());
        let inverted = DAY1.replacen("\"109000.0\"", "\"115000.0\"", 1);
        assert!(parse_klines_json(&format!("[{inverted}]"), i64::MAX).is_err());
        assert!(parse_klines_json(&format!("[{DAY2},{DAY1}]"), i64::MAX).is_err());
        let not_midnight = DAY1.replacen("1761955200000", "1761955200001", 1);
        assert!(parse_klines_json(&format!("[{not_midnight}]"), i64::MAX).is_err());
    }
}
