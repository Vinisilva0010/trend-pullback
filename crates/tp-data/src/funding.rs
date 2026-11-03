//! Binance USDT-M funding history saved by `scripts/funding_history.py`,
//! summed per UTC day. Positive rates mean longs pay shorts.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate};

/// A malformed line in a funding file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("funding file, line {line}: {reason}")]
pub struct FundingError {
    pub line: usize,
    pub reason: String,
}

/// Parses `funding_time_ms,rate` lines into the sum of rates per UTC day.
///
/// # Errors
///
/// Returns [`FundingError`] on a malformed line or an implausible rate.
pub fn parse_funding_csv(text: &str) -> Result<BTreeMap<NaiveDate, f64>, FundingError> {
    let mut daily = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let row = raw.trim();
        if row.is_empty() || (index == 0 && row.starts_with("funding_time_ms")) {
            continue;
        }
        let bad = |reason: String| FundingError {
            line: index + 1,
            reason,
        };
        let (time, rate) = row
            .split_once(',')
            .ok_or_else(|| bad("expected 2 fields".to_owned()))?;
        let ms: i64 = time
            .parse()
            .map_err(|_| bad(format!("invalid time {time:?}")))?;
        let rate: f64 = rate
            .parse()
            .ok()
            .filter(|r: &f64| r.is_finite() && r.abs() < 0.05)
            .ok_or_else(|| bad(format!("invalid rate {rate:?}")))?;
        let date = DateTime::from_timestamp_millis(ms)
            .ok_or_else(|| bad("time out of range".to_owned()))?
            .date_naive();
        *daily.entry(date).or_insert(0.0) += rate;
    }
    Ok(daily)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_rates_per_day() {
        let text = "funding_time_ms,rate\n1704067200000,0.0001\n1704096000000,0.0002\n1704153600000,-0.0001\n";
        let daily = parse_funding_csv(text).unwrap();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        assert!((daily[&d1] - 0.0003).abs() < 1e-12);
        assert!((daily[&d2] + 0.0001).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse_funding_csv("funding_time_ms,rate\nabc,0.0001\n").is_err());
        assert!(parse_funding_csv("funding_time_ms,rate\n1704067200000,0.5\n").is_err());
        assert!(parse_funding_csv("funding_time_ms,rate\n1704067200000\n").is_err());
    }
}
