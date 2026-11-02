//! Parsing of Binance spot daily kline CSV rows.
//!
//! Row layout (12 columns, no header): open time, open, high, low, close,
//! volume, close time, quote asset volume, number of trades, taker buy base
//! volume, taker buy quote volume, ignore. Open times are milliseconds before
//! 2025-01-01 and microseconds from 2025-01-01 onwards.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use tp_core::market::{Candle, CandleFields, InvalidCandle};

/// Open times at or above this value are microseconds, below it milliseconds.
/// Millisecond epochs stay below it until the year 5138, and microsecond
/// epochs are above it for any date after 1973.
const MICROS_THRESHOLD: i64 = 100_000_000_000_000;

/// Errors while parsing a daily kline CSV.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("line {line}: expected 12 fields, found {found}")]
    FieldCount { line: usize, found: usize },
    #[error("line {line}: {field} is not a valid number: {value:?}")]
    Number {
        line: usize,
        field: &'static str,
        value: String,
    },
    #[error("line {line}: open time {value} is out of range")]
    Timestamp { line: usize, value: i64 },
    #[error("line {line}: daily candle does not open at 00:00 UTC (open time {value})")]
    NotMidnight { line: usize, value: i64 },
    #[error("line {line}: {source}")]
    Candle { line: usize, source: InvalidCandle },
    #[error("line {line}: date {date} is not after the previous candle")]
    Order { line: usize, date: NaiveDate },
    #[error("file contains no candles")]
    Empty,
}

/// Parses a daily kline CSV into validated candles in strictly increasing
/// date order. A header row on the first line is skipped.
///
/// # Errors
///
/// Returns [`ParseError`] on the first malformed row, invalid candle,
/// non-midnight open time, out-of-order or duplicate date, or when the file
/// holds no candles.
pub fn parse_daily_klines(csv: &str) -> Result<Vec<Candle>, ParseError> {
    let mut candles: Vec<Candle> = Vec::new();
    for (index, raw_line) in csv.lines().enumerate() {
        let line = index + 1;
        let text = raw_line.trim();
        if text.is_empty() {
            continue;
        }
        let fields: Vec<&str> = text.split(',').map(str::trim).collect();
        let &[
            open_time,
            open,
            high,
            low,
            close,
            volume,
            _,
            quote_volume,
            _,
            _,
            _,
            _,
        ] = fields.as_slice()
        else {
            return Err(ParseError::FieldCount {
                line,
                found: fields.len(),
            });
        };
        let Ok(open_time) = open_time.parse::<i64>() else {
            if line == 1 {
                continue;
            }
            return Err(ParseError::Number {
                line,
                field: "open time",
                value: open_time.to_owned(),
            });
        };
        let opened_at = open_time_to_datetime(open_time).ok_or(ParseError::Timestamp {
            line,
            value: open_time,
        })?;
        if opened_at.time() != NaiveTime::MIN {
            return Err(ParseError::NotMidnight {
                line,
                value: open_time,
            });
        }
        let candle = Candle::try_new(CandleFields {
            date: opened_at.date_naive(),
            open: number(line, "open", open)?,
            high: number(line, "high", high)?,
            low: number(line, "low", low)?,
            close: number(line, "close", close)?,
            volume: number(line, "volume", volume)?,
            quote_volume: number(line, "quote volume", quote_volume)?,
        })
        .map_err(|source| ParseError::Candle { line, source })?;
        if let Some(previous) = candles.last()
            && candle.date() <= previous.date()
        {
            return Err(ParseError::Order {
                line,
                date: candle.date(),
            });
        }
        candles.push(candle);
    }
    if candles.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(candles)
}

/// Converts a Binance open time (milliseconds or microseconds) to UTC.
pub(crate) fn open_time_to_datetime(value: i64) -> Option<DateTime<Utc>> {
    if value < 0 {
        return None;
    }
    if value >= MICROS_THRESHOLD {
        DateTime::from_timestamp_micros(value)
    } else {
        DateTime::from_timestamp_millis(value)
    }
}

fn number(line: usize, field: &'static str, raw: &str) -> Result<f64, ParseError> {
    raw.parse::<f64>().map_err(|_| ParseError::Number {
        line,
        field,
        value: raw.to_owned(),
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use chrono::Days;
    use proptest::prelude::*;

    const DEC_01: &str = "1733011200000,96407.99000000,97836.00000000,95693.88000000,97185.18000000,16938.60452000,1733097599999,1641327626.60622060,3342200,8114.89569000,786489040.19363780,0";
    const DEC_02: &str = "1733097600000,97185.17000000,98130.00000000,94395.00000000,95840.62000000,37958.66981000,1733183999999,3646639621.73758780,7560103,18821.93327000,1808458555.89058410,0";
    const JAN_01_MICROS: &str = "1735689600000000,93576.00000000,95151.15000000,92888.00000000,94591.79000000,10373.32613000,1735775999999999,975444194.13799830,1516556,5347.73648000,502914035.64059070,0";

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn parses_millisecond_rows() {
        let candles = parse_daily_klines(&format!("{DEC_01}\n{DEC_02}\n")).unwrap();
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].date(), date(2024, 12, 1));
        assert_eq!(candles[0].close(), 97_185.18);
        assert_eq!(candles[1].date(), date(2024, 12, 2));
    }

    #[test]
    fn parses_microsecond_rows() {
        let candles = parse_daily_klines(JAN_01_MICROS).unwrap();
        assert_eq!(candles[0].date(), date(2025, 1, 1));
        assert_eq!(candles[0].open(), 93_576.0);
        assert!((candles[0].quote_volume() - 975_444_194.137_998).abs() < 1e-3);
    }

    #[test]
    fn skips_header_on_first_line() {
        let header = "open_time,open,high,low,close,volume,close_time,quote_volume,count,taker_base,taker_quote,ignore";
        let candles = parse_daily_klines(&format!("{header}\n{DEC_01}")).unwrap();
        assert_eq!(candles.len(), 1);
    }

    #[test]
    fn rejects_non_numeric_open_time_after_first_line() {
        let bad = DEC_02.replacen("1733097600000", "abc", 1);
        let result = parse_daily_klines(&format!("{DEC_01}\n{bad}"));
        assert!(matches!(result, Err(ParseError::Number { line: 2, .. })));
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert_eq!(
            parse_daily_klines("1,2,3"),
            Err(ParseError::FieldCount { line: 1, found: 3 })
        );
    }

    #[test]
    fn rejects_non_midnight_open() {
        let bad = DEC_01.replacen("1733011200000", "1733011200001", 1);
        assert!(matches!(
            parse_daily_klines(&bad),
            Err(ParseError::NotMidnight { line: 1, .. })
        ));
    }

    #[test]
    fn rejects_invalid_candle() {
        let bad = DEC_01.replacen("97836.00000000", "90000.00000000", 1);
        assert!(matches!(
            parse_daily_klines(&bad),
            Err(ParseError::Candle { line: 1, .. })
        ));
    }

    #[test]
    fn rejects_bad_price_text() {
        let bad = DEC_01.replacen("95693.88000000", "n/a", 1);
        assert!(matches!(
            parse_daily_klines(&bad),
            Err(ParseError::Number {
                line: 1,
                field: "low",
                ..
            })
        ));
    }

    #[test]
    fn rejects_out_of_order_and_duplicates() {
        assert!(matches!(
            parse_daily_klines(&format!("{DEC_02}\n{DEC_01}")),
            Err(ParseError::Order { line: 2, .. })
        ));
        assert!(matches!(
            parse_daily_klines(&format!("{DEC_01}\n{DEC_01}")),
            Err(ParseError::Order { line: 2, .. })
        ));
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(parse_daily_klines(""), Err(ParseError::Empty));
        assert_eq!(parse_daily_klines("\n \n"), Err(ParseError::Empty));
    }

    #[test]
    fn rejects_negative_open_time() {
        let bad = DEC_01.replacen("1733011200000", "-1", 1);
        assert!(matches!(
            parse_daily_klines(&bad),
            Err(ParseError::Timestamp { line: 1, .. })
        ));
    }

    proptest! {
        #[test]
        fn millis_and_micros_give_same_date(offset in 0_u64..5_000) {
            let day = date(2017, 8, 1).checked_add_days(Days::new(offset)).unwrap();
            let millis = day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
            let from_millis = open_time_to_datetime(millis).unwrap();
            let from_micros = open_time_to_datetime(millis * 1_000).unwrap();
            prop_assert_eq!(from_millis, from_micros);
            prop_assert_eq!(from_millis.date_naive(), day);
        }
    }
}
