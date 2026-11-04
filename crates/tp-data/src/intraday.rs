//! Hourly futures bars saved by `scripts/futures_1h.py`.

use tp_core::intraday::Bar;

/// A malformed line in an hourly file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("hourly file, line {line}: {reason}")]
pub struct BarError {
    pub line: usize,
    pub reason: String,
}

/// Parses `open_time_ms,open,high,low,close,volume` lines.
///
/// # Errors
///
/// Returns [`BarError`] on a malformed line, inconsistent prices, or times
/// that do not increase.
pub fn parse_bars_csv(text: &str) -> Result<Vec<Bar>, BarError> {
    let mut out: Vec<Bar> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let row = raw.trim();
        if row.is_empty() || (index == 0 && row.starts_with("open_time_ms")) {
            continue;
        }
        let bad = |reason: String| BarError {
            line: index + 1,
            reason,
        };
        let fields: Vec<&str> = row.split(',').collect();
        let &[time, open, high, low, close, volume] = fields.as_slice() else {
            return Err(bad(format!("expected 6 fields, found {}", fields.len())));
        };
        let time: i64 = time
            .parse()
            .map_err(|_| bad(format!("invalid time {time:?}")))?;
        let number = |s: &str, name: &str| {
            s.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| bad(format!("invalid {name} {s:?}")))
        };
        let bar = Bar {
            open_time_ms: time,
            open: number(open, "open")?,
            high: number(high, "high")?,
            low: number(low, "low")?,
            close: number(close, "close")?,
            volume: number(volume, "volume")?,
        };
        if !(bar.low > 0.0
            && bar.low <= bar.open.min(bar.close)
            && bar.high >= bar.open.max(bar.close))
        {
            return Err(bad("inconsistent prices".to_owned()));
        }
        if out.last().is_some_and(|p| p.open_time_ms >= time) {
            return Err(bad("times are not increasing".to_owned()));
        }
        out.push(bar);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates() {
        let ok = "open_time_ms,open,high,low,close,volume\n1704067200000,100,101,99,100.5,10\n1704070800000,100.5,102,100,101,12\n";
        assert_eq!(parse_bars_csv(ok).unwrap().len(), 2);
        assert!(
            parse_bars_csv(
                "open_time_ms,open,high,low,close,volume\n1704067200000,100,99,98,100,1\n"
            )
            .is_err()
        );
        assert!(parse_bars_csv("open_time_ms,open,high,low,close,volume\n1704070800000,100,101,99,100,1\n1704067200000,100,101,99,100,1\n").is_err());
        assert!(
            parse_bars_csv("open_time_ms,open,high,low,close,volume\n1704067200000,100,101\n")
                .is_err()
        );
    }
}
