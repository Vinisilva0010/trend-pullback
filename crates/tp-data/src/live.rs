//! Live daily signal support: extending the archived history with recent
//! candles, the per-asset state of the last signaled weight, and a minimal
//! `.env` reader.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::NaiveDate;
use tp_core::market::Candle;

/// Errors of the live signal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveError {
    #[error(
        "recent data starts on {found} but the history ends on {last}: {missing} day(s) missing"
    )]
    Gap {
        last: NaiveDate,
        found: NaiveDate,
        missing: i64,
    },
    #[error("state file, line {line}: {reason}")]
    State { line: usize, reason: String },
}

/// Weight last signaled for each asset, with its signal day.
pub type State = BTreeMap<String, (NaiveDate, f64)>;

/// Appends the candles of `recent` that come after the last day of
/// `history`, and returns how many were added.
///
/// # Errors
///
/// Returns [`LiveError::Gap`] when a new candle is not the day after the
/// previous one.
pub fn extend_series(history: &mut Vec<Candle>, recent: &[Candle]) -> Result<usize, LiveError> {
    let mut added = 0;
    for candle in recent {
        if let Some(last) = history.last() {
            if candle.date() <= last.date() {
                continue;
            }
            let step = (candle.date() - last.date()).num_days();
            if step != 1 {
                return Err(LiveError::Gap {
                    last: last.date(),
                    found: candle.date(),
                    missing: step - 1,
                });
            }
        }
        history.push(*candle);
        added += 1;
    }
    Ok(added)
}

/// Parses the state file (`asset,date,weight` with a header line).
///
/// # Errors
///
/// Returns [`LiveError::State`] on a malformed line or a weight outside 0..=1.
pub fn parse_state(text: &str) -> Result<State, LiveError> {
    let mut state = State::new();
    for (index, raw) in text.lines().enumerate() {
        let row = raw.trim();
        if row.is_empty() || (index == 0 && row.starts_with("asset,")) {
            continue;
        }
        let bad = |reason: String| LiveError::State {
            line: index + 1,
            reason,
        };
        let fields: Vec<&str> = row.split(',').collect();
        let &[asset, date, weight] = fields.as_slice() else {
            return Err(bad(format!("expected 3 fields, found {}", fields.len())));
        };
        let date = date
            .parse::<NaiveDate>()
            .map_err(|_| bad(format!("invalid date {date:?}")))?;
        let weight = weight
            .parse::<f64>()
            .ok()
            .filter(|w| (0.0..=3.0).contains(w))
            .ok_or_else(|| bad(format!("invalid weight {weight:?}")))?;
        state.insert(asset.to_owned(), (date, weight));
    }
    Ok(state)
}

/// State as CSV.
#[must_use]
pub fn state_csv(state: &State) -> String {
    let mut out = String::from("asset,date,weight\n");
    for (asset, (date, weight)) in state {
        let _ = writeln!(out, "{asset},{date},{weight}");
    }
    out
}

/// Reads `KEY=VALUE` lines; blank lines and `#` comments are ignored and
/// surrounding double quotes are removed from values.
#[must_use]
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|raw| {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((
                key.trim().to_owned(),
                value.trim().trim_matches('"').to_owned(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_core::market::CandleFields;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn candle(d: u32) -> Candle {
        Candle::try_new(CandleFields {
            date: day(d),
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
            quote_volume: 1.0,
        })
        .unwrap()
    }

    #[test]
    fn extends_after_last_day_and_skips_overlap() {
        let mut history = vec![candle(1), candle(2)];
        let added = extend_series(&mut history, &[candle(2), candle(3), candle(4)]).unwrap();
        assert_eq!(added, 2);
        assert_eq!(history.len(), 4);
    }

    #[test]
    fn refuses_a_gap() {
        let mut history = vec![candle(1)];
        assert_eq!(
            extend_series(&mut history, &[candle(3)]),
            Err(LiveError::Gap {
                last: day(1),
                found: day(3),
                missing: 1
            })
        );
    }

    #[test]
    fn state_round_trip_and_validation() {
        let mut state = State::new();
        state.insert("BTCUSDT".to_owned(), (day(2), 0.352));
        state.insert("ETHUSDT".to_owned(), (day(2), 0.0));
        assert_eq!(parse_state(&state_csv(&state)).unwrap(), state);
        assert!(parse_state("asset,date,weight\nBTCUSDT,2026-11-02,3.5\n").is_err());
        assert!(parse_state("asset,date,weight\nBTCUSDT,2026-13-02,0.5\n").is_err());
        assert!(parse_state("asset,date,weight\nBTCUSDT,2026-11-02\n").is_err());
    }

    #[test]
    fn reads_env_lines() {
        let env = parse_env("# comment\n\nTELEGRAM_BOT_TOKEN=\"123:abc\"\nTELEGRAM_CHAT_ID = 42\n");
        assert_eq!(
            env.get("TELEGRAM_BOT_TOKEN").map(String::as_str),
            Some("123:abc")
        );
        assert_eq!(env.get("TELEGRAM_CHAT_ID").map(String::as_str), Some("42"));
        assert_eq!(env.len(), 2);
    }
}
