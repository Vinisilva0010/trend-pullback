//! Odds for the daily report.
//!
//! Every number here comes from the asset's own history: the chance that a
//! price level is touched, measured only on past days in the same trend state
//! as today, and what happened after past candles with the same shape. A
//! forward result is only called an edge when it has enough cases and a large
//! t-statistic, so the report can say "no edge" instead of guessing.

use chrono::Datelike;

use crate::indicators::as_f64;
use crate::liquidity::{continuous_runs, mean_and_t};
use crate::market::{Candle, CandleFields};

/// Share of the candle's range a wick needs to count as large.
const BIG_WICK: f64 = 0.5;
/// Share of the candle's range the body needs to count as strong.
const STRONG_BODY: f64 = 0.6;
/// Minimum absolute t-statistic for a forward result to count as an edge.
pub const EDGE_T: f64 = 2.5;
/// Minimum number of past cases for a forward result to count as an edge.
pub const EDGE_MIN_CASES: usize = 50;

/// Shape of one candle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Upper wick is at least half of the range: the rise was rejected.
    UpperWick,
    /// Lower wick is at least half of the range: the drop was rejected.
    LowerWick,
    /// Body is at least 60% of the range and the candle closed up.
    StrongUp,
    /// Body is at least 60% of the range and the candle closed down.
    StrongDown,
    /// None of the above.
    Normal,
}

impl Shape {
    /// Classifies a candle by the size of its wicks and body.
    #[must_use]
    pub fn of(c: &Candle) -> Self {
        let range = c.high() - c.low();
        if range <= 0.0 {
            return Self::Normal;
        }
        let (upper, lower) = wick_shares(c);
        let body = (c.close() - c.open()).abs() / range;
        if upper >= BIG_WICK {
            Self::UpperWick
        } else if lower >= BIG_WICK {
            Self::LowerWick
        } else if body >= STRONG_BODY && c.close() > c.open() {
            Self::StrongUp
        } else if body >= STRONG_BODY && c.close() < c.open() {
            Self::StrongDown
        } else {
            Self::Normal
        }
    }

    /// Portuguese description used in the Telegram report.
    #[must_use]
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::UpperWick => "pavio grande em cima (alta rejeitada)",
            Self::LowerWick => "pavio grande embaixo (queda rejeitada)",
            Self::StrongUp => "corpo forte de alta",
            Self::StrongDown => "corpo forte de baixa",
            Self::Normal => "candle sem pavio grande nem corpo forte",
        }
    }
}

/// Upper and lower wick as shares of the candle's range (0 when the range is 0).
#[must_use]
pub fn wick_shares(c: &Candle) -> (f64, f64) {
    let range = c.high() - c.low();
    if range <= 0.0 {
        return (0.0, 0.0);
    }
    (
        (c.high() - c.open().max(c.close())) / range,
        (c.open().min(c.close()) - c.low()) / range,
    )
}

/// What happened `horizon` candles after a set of past cases, next to what
/// happened after any day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forward {
    /// Number of past cases, taken without overlapping windows.
    pub cases: usize,
    /// Share of cases where the close `horizon` candles later was higher.
    pub up_rate: f64,
    /// Same share over every day of the history, for comparison.
    pub base_up_rate: f64,
    /// Mean change of the close, in ATRs of the case day.
    pub mean_atr: f64,
    /// Mean change after any day, in ATRs.
    pub base_mean_atr: f64,
    /// t-statistic of the cases' mean change minus the any-day mean.
    pub t: f64,
}

impl Forward {
    /// True when the result has enough cases and differs enough from any day.
    #[must_use]
    pub fn is_edge(&self) -> bool {
        self.cases >= EDGE_MIN_CASES && self.t.abs() >= EDGE_T
    }
}

/// Close-to-close change `horizon` candles after every day where `mask` is
/// true, in ATRs of that day, compared with the change after any day.
///
/// Cases are taken without overlapping windows (after a case, the next
/// `horizon - 1` days are skipped), so one long move is not counted many
/// times and the t-statistic is not inflated. The t-statistic measures the
/// difference from the any-day mean, so a market that simply rose over the
/// years does not make every pattern look bullish. Days whose window crosses
/// a gap in the data are skipped.
#[must_use]
pub fn forward_stats(
    candles: &[Candle],
    atr: &[Option<f64>],
    mask: &[bool],
    horizon: usize,
) -> Option<Forward> {
    if horizon == 0 || candles.len() != atr.len() || candles.len() != mask.len() {
        return None;
    }
    let (_, end) = continuous_runs(candles);
    let change = |t: usize| -> Option<f64> {
        if t + horizon > end[t] {
            return None;
        }
        let a = atr[t].filter(|a| *a > 0.0)?;
        Some((candles[t + horizon].close() - candles[t].close()) / a)
    };
    let base: Vec<f64> = (0..candles.len()).filter_map(change).collect();
    let mut changes = Vec::new();
    let mut next_free = 0usize;
    for (t, &wanted) in mask.iter().enumerate() {
        if !wanted || t < next_free {
            continue;
        }
        if let Some(c) = change(t) {
            changes.push(c);
            next_free = t + horizon;
        }
    }
    if changes.is_empty() || base.is_empty() {
        return None;
    }
    let rate = |v: &[f64]| as_f64(v.iter().filter(|c| **c > 0.0).count()) / as_f64(v.len());
    let base_mean_atr = base.iter().sum::<f64>() / as_f64(base.len());
    let excess: Vec<f64> = changes.iter().map(|c| c - base_mean_atr).collect();
    let (_, t) = mean_and_t(&excess);
    Some(Forward {
        cases: changes.len(),
        up_rate: rate(&changes),
        base_up_rate: rate(&base),
        mean_atr: changes.iter().sum::<f64>() / as_f64(changes.len()),
        base_mean_atr,
        t,
    })
}

/// Chance that the price moves at least `distance_atr` ATRs up (or down)
/// within `horizon` candles, counted only on past days where `mask` is true.
/// Returns the chance and the number of cases.
#[must_use]
pub fn touch_odds(
    candles: &[Candle],
    atr: &[Option<f64>],
    mask: &[bool],
    horizon: usize,
    distance_atr: f64,
    up: bool,
) -> Option<(f64, usize)> {
    if horizon == 0 || candles.len() != atr.len() || candles.len() != mask.len() {
        return None;
    }
    let (_, end) = continuous_runs(candles);
    let mut hits = 0usize;
    let mut total = 0usize;
    for (t, candle) in candles.iter().enumerate() {
        if !mask[t] || t + horizon > end[t] {
            continue;
        }
        let Some(a) = atr[t].filter(|a| *a > 0.0) else {
            continue;
        };
        let ahead = &candles[t + 1..=t + horizon];
        let moved = if up {
            ahead.iter().map(Candle::high).fold(f64::MIN, f64::max) - candle.close()
        } else {
            candle.close() - ahead.iter().map(Candle::low).fold(f64::MAX, f64::min)
        };
        total += 1;
        hits += usize::from(moved / a >= distance_atr);
    }
    (total > 0).then(|| (as_f64(hits) / as_f64(total), total))
}

/// Complete Monday-to-Sunday weeks built from consecutive daily candles.
/// Each weekly candle is dated on its Sunday. Weeks with a missing day are
/// skipped, and so is the current week until its Sunday closes.
#[must_use]
pub fn weekly(candles: &[Candle]) -> Vec<Candle> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < candles.len() {
        if candles[i].date().weekday().num_days_from_monday() != 0 || i + 7 > candles.len() {
            i += 1;
            continue;
        }
        let week = &candles[i..i + 7];
        let consecutive = week
            .windows(2)
            .all(|w| (w[1].date() - w[0].date()).num_days() == 1);
        if !consecutive {
            i += 1;
            continue;
        }
        let fields = CandleFields {
            date: week[6].date(),
            open: week[0].open(),
            high: week.iter().map(Candle::high).fold(f64::MIN, f64::max),
            low: week.iter().map(Candle::low).fold(f64::MAX, f64::min),
            close: week[6].close(),
            volume: week.iter().map(Candle::volume).sum(),
            quote_volume: week.iter().map(Candle::quote_volume).sum(),
        };
        if let Ok(candle) = Candle::try_new(fields) {
            out.push(candle);
        }
        i += 7;
    }
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::{Days, NaiveDate};

    fn bar(day: u64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        // 2024-01-01 is a Monday.
        Candle::try_new(CandleFields {
            date: NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .checked_add_days(Days::new(day))
                .unwrap(),
            open,
            high,
            low,
            close,
            volume: 1.0,
            quote_volume: 1.0,
        })
        .unwrap()
    }

    #[test]
    fn shapes_follow_wick_and_body_shares() {
        assert_eq!(Shape::of(&bar(0, 10.0, 20.0, 9.0, 11.0)), Shape::UpperWick);
        assert_eq!(Shape::of(&bar(0, 19.0, 20.0, 9.0, 18.0)), Shape::LowerWick);
        assert_eq!(Shape::of(&bar(0, 10.0, 20.0, 9.5, 19.5)), Shape::StrongUp);
        assert_eq!(Shape::of(&bar(0, 19.5, 20.0, 9.5, 10.0)), Shape::StrongDown);
        assert_eq!(Shape::of(&bar(0, 12.0, 17.0, 10.0, 15.0)), Shape::Normal);
        assert_eq!(Shape::of(&bar(0, 10.0, 10.0, 10.0, 10.0)), Shape::Normal);
    }

    #[test]
    fn wick_shares_add_up_with_body() {
        let c = bar(0, 12.0, 20.0, 10.0, 15.0);
        let (upper, lower) = wick_shares(&c);
        assert_eq!(upper, 0.5);
        assert_eq!(lower, 0.2);
    }

    #[test]
    fn forward_stats_measure_change_in_atr() {
        let candles: Vec<Candle> = (0..5)
            .map(|i| {
                let p = 10.0 + f64::from(u8::try_from(i).unwrap());
                bar(i, p, p + 0.5, p - 0.5, p)
            })
            .collect();
        let atr = vec![Some(1.0); 5];
        let mask = vec![true, false, false, false, false];
        let f = forward_stats(&candles, &atr, &mask, 2).unwrap();
        assert_eq!(f.cases, 1);
        assert_eq!(f.up_rate, 1.0);
        assert_eq!(f.base_up_rate, 1.0);
        assert_eq!(f.mean_atr, 2.0);
        assert_eq!(f.base_mean_atr, 2.0);
        assert!(!f.is_edge());
    }

    #[test]
    fn forward_stats_do_not_overlap_cases() {
        let candles: Vec<Candle> = (0..10).map(|i| bar(i, 10.0, 11.0, 9.0, 10.0)).collect();
        let atr = vec![Some(1.0); 10];
        let f = forward_stats(&candles, &atr, &[true; 10], 3).unwrap();
        // Days 0, 3 and 6 are cases; 9 has no window left.
        assert_eq!(f.cases, 3);
    }

    #[test]
    fn forward_stats_skip_windows_past_the_end_or_a_gap() {
        let candles = vec![
            bar(0, 10.0, 11.0, 9.0, 10.0),
            bar(1, 10.0, 11.0, 9.0, 10.0),
            bar(5, 10.0, 13.0, 9.0, 12.0),
        ];
        let atr = vec![Some(1.0); 3];
        assert!(
            forward_stats(&candles, &atr, &[true, true, true], 1).is_some_and(|f| f.cases == 1)
        );
        assert!(forward_stats(&candles, &atr, &[false, true, true], 1).is_none());
    }

    #[test]
    fn touch_odds_count_only_masked_days() {
        let candles = vec![
            bar(0, 10.0, 10.5, 9.5, 10.0),
            bar(1, 10.0, 12.5, 9.5, 12.0),
            bar(2, 12.0, 12.5, 11.5, 12.0),
            bar(3, 12.0, 12.5, 11.5, 12.0),
        ];
        let atr = vec![Some(1.0); 4];
        let all = [true, true, true, false];
        let (p, n) = touch_odds(&candles, &atr, &all, 1, 2.0, true).unwrap();
        assert_eq!(n, 3);
        assert!((p - 1.0 / 3.0).abs() < 1e-12);
        let only_first = [true, false, false, false];
        assert_eq!(
            touch_odds(&candles, &atr, &only_first, 1, 2.0, true),
            Some((1.0, 1))
        );
        assert_eq!(
            touch_odds(&candles, &atr, &only_first, 1, 1.0, false),
            Some((0.0, 1))
        );
    }

    #[test]
    fn weekly_builds_complete_monday_to_sunday_weeks() {
        // Starts on a Wednesday (day 2), so the first week begins on day 7.
        let candles: Vec<Candle> = (2..21)
            .map(|i| {
                let p = 10.0 + f64::from(u8::try_from(i).unwrap());
                bar(i, p, p + 1.0, p - 1.0, p + 0.5)
            })
            .collect();
        let weeks = weekly(&candles);
        assert_eq!(weeks.len(), 2);
        assert_eq!(
            weeks[0].date(),
            NaiveDate::from_ymd_opt(2024, 1, 14).unwrap()
        );
        assert_eq!(weeks[0].open(), 17.0);
        assert_eq!(weeks[0].high(), 24.0);
        assert_eq!(weeks[0].low(), 16.0);
        assert_eq!(weeks[0].close(), 23.5);
        assert_eq!(weeks[0].volume(), 7.0);
        assert_eq!(
            weeks[1].date(),
            NaiveDate::from_ymd_opt(2024, 1, 21).unwrap()
        );
    }

    #[test]
    fn weekly_skips_weeks_with_a_missing_day() {
        let candles: Vec<Candle> = (0..14)
            .filter(|i| *i != 3)
            .map(|i| bar(i, 10.0, 11.0, 9.0, 10.0))
            .collect();
        let weeks = weekly(&candles);
        assert_eq!(weeks.len(), 1);
        assert_eq!(
            weeks[0].date(),
            NaiveDate::from_ymd_opt(2024, 1, 14).unwrap()
        );
    }
}
