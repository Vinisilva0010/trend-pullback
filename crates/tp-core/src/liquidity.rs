//! Liquidity-level study (Phase 3, hypotheses 1 and 2).
//!
//! Hypothesis 1 (magnet): price reaches liquidity levels (prior 20- and
//! 60-day highs and lows, round numbers) more often than any level at the
//! same distance in ATR, in the same direction, on the same asset.
//! Hypothesis 2 (after the sweep): forward returns after a close beyond a
//! 20-day extreme (break) or a wick beyond it with a close back inside
//! (sweep and reject), relative to the asset's average forward return.
//! Positive values mean continuation in the direction of the event.

use std::collections::BTreeMap;

use crate::indicators::as_f64;
use crate::market::Candle;

/// Levels farther than this many ATR are ignored.
pub const MAX_ATR_DISTANCE: f64 = 8.0;
const SHORT: usize = 20;
const LONG: usize = 60;

/// Kind of liquidity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LevelKind {
    High20,
    High60,
    RoundAbove,
    Low20,
    Low60,
    RoundBelow,
}

impl LevelKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::High20 => "high 20d",
            Self::High60 => "high 60d",
            Self::RoundAbove => "round above",
            Self::Low20 => "low 20d",
            Self::Low60 => "low 60d",
            Self::RoundBelow => "round below",
        }
    }
}

/// Kind of event around the 20-day extremes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventKind {
    BreakHigh,
    SweepHigh,
    BreakLow,
    SweepLow,
}

impl EventKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BreakHigh => "break high",
            Self::SweepHigh => "sweep high",
            Self::BreakLow => "break low",
            Self::SweepLow => "sweep low",
        }
    }
}

/// Touch counts against the distance-matched expectation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TouchStats {
    pub count: usize,
    pub hits: usize,
    pub expected: f64,
    pub variance: f64,
}

impl TouchStats {
    fn add(&mut self, hit: bool, p: f64) {
        self.count += 1;
        self.hits += usize::from(hit);
        self.expected += p;
        self.variance += p * (1.0 - p);
    }

    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            as_f64(self.hits) / as_f64(self.count)
        }
    }

    #[must_use]
    pub fn expected_rate(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.expected / as_f64(self.count)
        }
    }

    /// Standard score of hits against the expectation.
    #[must_use]
    pub fn z(&self) -> f64 {
        if self.variance > 0.0 {
            (as_f64(self.hits) - self.expected) / self.variance.sqrt()
        } else {
            0.0
        }
    }
}

/// Accumulated results over symbols for one horizon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiquidityResult {
    pub touches: BTreeMap<LevelKind, TouchStats>,
    pub events: BTreeMap<EventKind, Vec<f64>>,
}

/// Round-number step: half of the largest power of ten not above `price`.
#[must_use]
pub fn round_step(price: f64) -> f64 {
    if !(price.is_finite() && price > 0.0) {
        return 0.0;
    }
    let mut step = 1.0_f64;
    while step * 10.0 <= price {
        step *= 10.0;
    }
    while step > price {
        step /= 10.0;
    }
    step / 2.0
}

/// Next round number strictly above `price`.
#[must_use]
pub fn round_above(price: f64) -> f64 {
    let step = round_step(price);
    ((price / step).floor() + 1.0) * step
}

/// Next round number strictly below `price`.
#[must_use]
pub fn round_below(price: f64) -> f64 {
    let step = round_step(price);
    ((price / step).ceil() - 1.0) * step
}

/// Mean and t-statistic of a sample.
#[must_use]
pub fn mean_and_t(values: &[f64]) -> (f64, f64) {
    if values.len() < 2 {
        return (values.first().copied().unwrap_or(0.0), 0.0);
    }
    let n = as_f64(values.len());
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let t = if variance > 0.0 {
        mean / (variance.sqrt() / n.sqrt())
    } else {
        0.0
    };
    (mean, t)
}

fn continuous_runs(candles: &[Candle]) -> (Vec<usize>, Vec<usize>) {
    let mut start = Vec::with_capacity(candles.len());
    let mut current = 0;
    for (i, candle) in candles.iter().enumerate() {
        if i > 0 && (candle.date() - candles[i - 1].date()).num_days() != 1 {
            current = i;
        }
        start.push(current);
    }
    let mut end = vec![0; candles.len()];
    let mut current = candles.len().saturating_sub(1);
    for (i, slot) in end.iter_mut().enumerate().rev() {
        if i + 1 < candles.len() && (candles[i + 1].date() - candles[i].date()).num_days() != 1 {
            current = i;
        }
        *slot = current;
    }
    (start, end)
}

fn window_high(window: &[Candle]) -> f64 {
    window.iter().map(Candle::high).fold(f64::MIN, f64::max)
}

fn window_low(window: &[Candle]) -> f64 {
    window.iter().map(Candle::low).fold(f64::MAX, f64::min)
}

fn share_at_least(sorted: &[f64], d: f64) -> f64 {
    as_f64(sorted.len() - sorted.partition_point(|x| *x < d)) / as_f64(sorted.len())
}

/// Adds one symbol to `out` for the given horizon. `atr` and `eligible`
/// align with `candles`; only eligible days with 60 days of history and
/// `horizon` days ahead in the same continuous run are used.
pub fn study_symbol(
    candles: &[Candle],
    atr: &[Option<f64>],
    eligible: &[bool],
    horizon: usize,
    out: &mut LiquidityResult,
) {
    if horizon == 0 || candles.len() != atr.len() || candles.len() != eligible.len() {
        return;
    }
    let (start, end) = continuous_runs(candles);
    let valid: Vec<(usize, f64)> = (0..candles.len())
        .filter(|&t| eligible[t] && t >= start[t] + LONG && t + horizon <= end[t])
        .filter_map(|t| atr[t].filter(|a| *a > 0.0).map(|a| (t, a)))
        .collect();
    if valid.is_empty() {
        return;
    }
    let forward = |t: usize, a: f64| {
        let ahead = &candles[t + 1..=t + horizon];
        let close = candles[t].close();
        (
            window_high(ahead),
            window_low(ahead),
            (candles[t + horizon].close() - close) / a,
        )
    };
    let mut ups = Vec::with_capacity(valid.len());
    let mut downs = Vec::with_capacity(valid.len());
    let mut sum_forward = 0.0;
    for &(t, a) in &valid {
        let (up, down, r) = forward(t, a);
        ups.push((up - candles[t].close()) / a);
        downs.push((candles[t].close() - down) / a);
        sum_forward += r;
    }
    ups.sort_by(f64::total_cmp);
    downs.sort_by(f64::total_cmp);
    let mean_forward = sum_forward / as_f64(valid.len());
    for (k, &(t, a)) in valid.iter().enumerate() {
        let (up, down, r) = forward(t, a);
        let today = &candles[t];
        let close = today.close();
        let (high20, low20) = (
            window_high(&candles[t - SHORT..t]),
            window_low(&candles[t - SHORT..t]),
        );
        let excess = r - mean_forward;
        if close > high20 {
            out.events
                .entry(EventKind::BreakHigh)
                .or_default()
                .push(excess);
        } else if today.high() > high20 {
            out.events
                .entry(EventKind::SweepHigh)
                .or_default()
                .push(excess);
        }
        if close < low20 {
            out.events
                .entry(EventKind::BreakLow)
                .or_default()
                .push(-excess);
        } else if today.low() < low20 {
            out.events
                .entry(EventKind::SweepLow)
                .or_default()
                .push(-excess);
        }
        if k % horizon != 0 {
            continue;
        }
        let above = [
            (LevelKind::High20, high20),
            (LevelKind::High60, window_high(&candles[t - LONG..t])),
            (LevelKind::RoundAbove, round_above(close)),
        ];
        for (kind, level) in above {
            let d = (level - close) / a;
            if d > 0.0 && d <= MAX_ATR_DISTANCE {
                out.touches
                    .entry(kind)
                    .or_default()
                    .add(up >= level, share_at_least(&ups, d));
            }
        }
        let below = [
            (LevelKind::Low20, low20),
            (LevelKind::Low60, window_low(&candles[t - LONG..t])),
            (LevelKind::RoundBelow, round_below(close)),
        ];
        for (kind, level) in below {
            let d = (close - level) / a;
            if level > 0.0 && d > 0.0 && d <= MAX_ATR_DISTANCE {
                out.touches
                    .entry(kind)
                    .or_default()
                    .add(down <= level, share_at_least(&downs, d));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::market::CandleFields;
    use chrono::{Days, NaiveDate};

    fn bar(i: u64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle::try_new(CandleFields {
            date: NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .checked_add_days(Days::new(i))
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
    fn round_numbers() {
        assert_eq!(round_step(76_816.0), 5_000.0);
        assert_eq!(round_step(2_253.29), 500.0);
        assert!((round_step(0.53) - 0.05).abs() < 1e-12);
        assert_eq!(round_above(76_816.0), 80_000.0);
        assert_eq!(round_below(76_816.0), 75_000.0);
        assert_eq!(round_above(75_000.0), 80_000.0);
        assert_eq!(round_below(75_000.0), 70_000.0);
    }

    #[test]
    fn touch_stats_arithmetic() {
        let mut s = TouchStats::default();
        s.add(true, 0.5);
        s.add(false, 0.5);
        s.add(true, 0.5);
        s.add(true, 0.5);
        assert_eq!(s.hit_rate(), 0.75);
        assert_eq!(s.expected_rate(), 0.5);
        assert!((s.z() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn detects_break_and_sweep() {
        let mut candles: Vec<Candle> = (0..70).map(|i| bar(i, 100.0, 101.0, 99.0, 100.0)).collect();
        candles.push(bar(70, 100.0, 106.0, 99.5, 105.0));
        candles.extend((71..80).map(|i| bar(i, 100.0, 101.0, 99.0, 100.0)));
        candles.push(bar(80, 100.0, 107.0, 99.5, 100.5));
        candles.extend((81..90).map(|i| bar(i, 100.0, 101.0, 99.0, 100.0)));
        let atr = vec![Some(2.0); candles.len()];
        let eligible = vec![true; candles.len()];
        let mut out = LiquidityResult::default();
        study_symbol(&candles, &atr, &eligible, 1, &mut out);
        assert_eq!(out.events.get(&EventKind::BreakHigh).map(Vec::len), Some(1));
        assert_eq!(out.events.get(&EventKind::SweepHigh).map(Vec::len), Some(1));
        assert!(!out.events.contains_key(&EventKind::BreakLow));
    }

    #[test]
    fn mean_and_t_by_hand() {
        let (mean, t) = mean_and_t(&[1.0, 2.0, 3.0]);
        assert_eq!(mean, 2.0);
        assert!((t - 2.0 / (1.0 / 3.0_f64.sqrt())).abs() < 1e-12);
    }
}
