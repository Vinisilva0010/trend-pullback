//! Market regimes from daily closes.
//!
//! Up: at least 5 of the 9 Donchian channels long. Down: at least 5 of the 9
//! mirrored channels short (closes breaking below prior lows). Range: neither,
//! and Kaufman's efficiency ratio over 30 days below 0.3, meaning price moved
//! back and forth more than it travelled. Transition: everything else.
//! Days before the longest channel has history are unclassified.

use crate::indicators::as_f64;
use crate::trend::{DONCHIAN_LOOKBACKS, donchian_positions};

/// Channels that must agree for a trend regime.
pub const TREND_VOTES: usize = 5;
/// Window of the efficiency ratio.
pub const ER_WINDOW: usize = 30;
/// Efficiency ratio below which a non-trending market is a range.
pub const RANGE_ER: f64 = 0.3;

/// Market regime of one day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Regime {
    Up,
    Down,
    Range,
    Transition,
}

impl Regime {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Range => "range",
            Self::Transition => "transition",
        }
    }
}

/// In-position flags of one short Donchian channel: the long channel applied
/// to negated prices, so it enters below the prior lowest close and exits
/// above a trailing stop that only moves down.
#[must_use]
pub fn donchian_short_positions(closes: &[f64], lookback: usize) -> Vec<bool> {
    let negated: Vec<f64> = closes.iter().map(|c| -c).collect();
    donchian_positions(&negated, lookback)
}

/// Kaufman's efficiency ratio: net move over the window divided by the sum of
/// absolute daily moves (1 = straight line, near 0 = back and forth).
#[must_use]
pub fn efficiency_ratio(closes: &[f64], window: usize) -> Vec<Option<f64>> {
    (0..closes.len())
        .map(|i| {
            if window == 0 || i < window {
                return None;
            }
            let path: f64 = closes[i - window..=i]
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .sum();
            if path > 0.0 {
                Some((closes[i] - closes[i - window]).abs() / path)
            } else {
                Some(0.0)
            }
        })
        .collect()
}

/// Regime of every day (None before the longest channel has history).
#[must_use]
pub fn classify(closes: &[f64]) -> Vec<Option<Regime>> {
    let longs: Vec<Vec<bool>> = DONCHIAN_LOOKBACKS
        .iter()
        .map(|&l| donchian_positions(closes, l))
        .collect();
    let shorts: Vec<Vec<bool>> = DONCHIAN_LOOKBACKS
        .iter()
        .map(|&l| donchian_short_positions(closes, l))
        .collect();
    let er = efficiency_ratio(closes, ER_WINDOW);
    let warmup = DONCHIAN_LOOKBACKS.iter().copied().max().unwrap_or(0);
    (0..closes.len())
        .map(|i| {
            if i < warmup {
                return None;
            }
            let up = longs.iter().filter(|p| p[i]).count();
            let down = shorts.iter().filter(|p| p[i]).count();
            Some(if up >= TREND_VOTES {
                Regime::Up
            } else if down >= TREND_VOTES {
                Regime::Down
            } else if er[i].is_some_and(|e| e < RANGE_ER) {
                Regime::Range
            } else {
                Regime::Transition
            })
        })
        .collect()
}

/// Average length in days of uninterrupted runs of `regime`.
#[must_use]
pub fn average_run(regimes: &[Option<Regime>], regime: Regime) -> f64 {
    let mut runs = Vec::new();
    let mut current = 0usize;
    for r in regimes {
        if *r == Some(regime) {
            current += 1;
        } else if current > 0 {
            runs.push(current);
            current = 0;
        }
    }
    if current > 0 {
        runs.push(current);
    }
    if runs.is_empty() {
        0.0
    } else {
        as_f64(runs.iter().sum()) / as_f64(runs.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_channel_mirrors_the_long_one() {
        let closes = [5.0, 4.0, 3.0, 2.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(
            donchian_short_positions(&closes, 3),
            vec![false, false, false, true, true, true, false, false]
        );
    }

    #[test]
    fn efficiency_ratio_by_hand() {
        let line: Vec<f64> = (0..40).map(f64::from).collect();
        assert!((efficiency_ratio(&line, 30)[35].unwrap() - 1.0).abs() < 1e-12);
        let zigzag: Vec<f64> = (0..40)
            .map(|i| if i % 2 == 0 { 10.0 } else { 11.0 })
            .collect();
        assert!(efficiency_ratio(&zigzag, 30)[34].unwrap().abs() < 1e-12);
        assert!(efficiency_ratio(&line, 30)[29].is_none());
    }

    #[test]
    fn classifies_trends_and_ranges() {
        let rising: Vec<f64> = (0..500).map(|i| 100.0 * 1.01_f64.powi(i)).collect();
        assert_eq!(classify(&rising)[450], Some(Regime::Up));
        let falling: Vec<f64> = (0..500).map(|i| 100.0 * 0.99_f64.powi(i)).collect();
        assert_eq!(classify(&falling)[450], Some(Regime::Down));
        let flat: Vec<f64> = (0..500)
            .map(|i| 100.0 + if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        assert_eq!(classify(&flat)[450], Some(Regime::Range));
        assert_eq!(classify(&flat)[100], None);
    }

    #[test]
    fn run_lengths() {
        let r = [
            Some(Regime::Up),
            Some(Regime::Up),
            Some(Regime::Range),
            Some(Regime::Up),
            None,
        ];
        assert!((average_run(&r, Regime::Up) - 1.5).abs() < 1e-12);
        assert!((average_run(&r, Regime::Range) - 1.0).abs() < 1e-12);
    }
}
