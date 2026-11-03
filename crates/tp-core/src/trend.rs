//! Long-or-flat trend following with volatility targeting.
//!
//! Follows Zarattini, Pagani and Barbon (2025), "Catching Crypto Trends":
//! nine Donchian channels on closing prices. Each enters when the close breaks
//! above the highest close of the previous `lookback` days and exits when the
//! close falls below a trailing stop that only moves up: the higher of the
//! previous stop and the channel midpoint. The nine are averaged and scaled to
//! a target annualized volatility. Inputs must be gap-free daily closes.

use crate::indicators::{as_f64, sma};

/// Channel lookbacks used by the paper.
pub const DONCHIAN_LOOKBACKS: [usize; 9] = [5, 10, 20, 30, 60, 90, 150, 250, 360];
const DAYS_PER_YEAR: f64 = 365.0;

/// Volatility-targeting settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolTarget {
    /// Target annualized volatility (0.25 = 25%).
    pub target: f64,
    /// Days of returns used to estimate volatility.
    pub window: usize,
    /// Cap on the scale of one signal.
    pub max_per_signal: f64,
    /// Cap on the total weight (1.0 = fully invested, no leverage).
    pub max_total: f64,
}

impl VolTarget {
    /// The paper's sizing, capped at 100% for spot trading.
    #[must_use]
    pub fn paper_spot() -> Self {
        Self {
            target: 0.25,
            window: 90,
            max_per_signal: 2.0,
            max_total: 1.0,
        }
    }
}

/// In-position flags of one Donchian channel on closes.
#[must_use]
pub fn donchian_positions(closes: &[f64], lookback: usize) -> Vec<bool> {
    let mut out = vec![false; closes.len()];
    if lookback == 0 {
        return out;
    }
    let mut in_position = false;
    let mut stop = 0.0_f64;
    for (i, &close) in closes.iter().enumerate() {
        let Some(start) = i.checked_sub(lookback) else {
            continue;
        };
        let window = &closes[start..i];
        let high = window.iter().copied().fold(f64::MIN, f64::max);
        let low = window.iter().copied().fold(f64::MAX, f64::min);
        let mid = f64::midpoint(high, low);
        if in_position {
            stop = stop.max(mid);
            if close < stop {
                in_position = false;
            }
        } else if close > high {
            in_position = true;
            stop = mid;
        }
        out[i] = in_position;
    }
    out
}

/// Annualized standard deviation of the last `window` daily log returns.
#[must_use]
pub fn realized_volatility(closes: &[f64], window: usize) -> Vec<Option<f64>> {
    let returns: Vec<f64> = closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    (0..closes.len())
        .map(|i| {
            if window < 2 || i < window {
                return None;
            }
            let slice = &returns[i - window..i];
            let mean = slice.iter().sum::<f64>() / as_f64(window);
            let variance =
                slice.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / as_f64(window - 1);
            Some(variance.sqrt() * DAYS_PER_YEAR.sqrt())
        })
        .collect()
}

fn scale(volatility: Option<f64>, v: &VolTarget) -> f64 {
    volatility
        .filter(|x| *x > 0.0)
        .map_or(0.0, |x| (v.target / x).min(v.max_per_signal))
}

/// Target weight per day of the nine-channel ensemble.
#[must_use]
pub fn donchian_ensemble_weights(closes: &[f64], v: &VolTarget) -> Vec<f64> {
    let volatility = realized_volatility(closes, v.window);
    let positions: Vec<Vec<bool>> = DONCHIAN_LOOKBACKS
        .iter()
        .map(|&lookback| donchian_positions(closes, lookback))
        .collect();
    let count = as_f64(DONCHIAN_LOOKBACKS.len());
    (0..closes.len())
        .map(|i| {
            let active = positions.iter().filter(|p| p[i]).count();
            (as_f64(active) / count * scale(volatility[i], v)).min(v.max_total)
        })
        .collect()
}

/// Target weight per day: volatility-scaled when the close is above its
/// simple moving average, zero otherwise.
#[must_use]
pub fn sma_trend_weights(closes: &[f64], period: usize, v: &VolTarget) -> Vec<f64> {
    let volatility = realized_volatility(closes, v.window);
    let average = sma(closes, period);
    closes
        .iter()
        .enumerate()
        .map(|(i, &close)| {
            if average[i].is_some_and(|a| close > a) {
                scale(volatility[i], v).min(v.max_total)
            } else {
                0.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn donchian_by_hand() {
        let closes = [1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0];
        assert_eq!(
            donchian_positions(&closes, 3),
            vec![false, false, false, true, true, true, false, false]
        );
    }

    #[test]
    fn volatility_of_steady_growth_is_zero_and_weight_is_zero() {
        let closes: Vec<f64> = (0..200).map(|i| 1.01_f64.powi(i)).collect();
        let vol = realized_volatility(&closes, 90);
        assert!(vol[89].is_none());
        assert!(vol[150].unwrap() < 1e-9);
        let weights = donchian_ensemble_weights(&closes, &VolTarget::paper_spot());
        assert!(weights.iter().all(|w| *w == 0.0));
    }

    #[test]
    fn weights_stay_between_zero_and_cap() {
        let closes: Vec<f64> = (0..600)
            .map(|i| 100.0 * (1.0 + 0.3 * (f64::from(i) / 17.0).sin()) * 1.002_f64.powi(i))
            .collect();
        let v = VolTarget::paper_spot();
        for w in donchian_ensemble_weights(&closes, &v)
            .into_iter()
            .chain(sma_trend_weights(&closes, 50, &v))
        {
            assert!((0.0..=1.0).contains(&w));
        }
    }

    #[test]
    fn sma_weight_is_zero_below_the_average() {
        let closes: Vec<f64> = (0..200).map(|i| 300.0 - f64::from(i)).collect();
        let weights = sma_trend_weights(&closes, 50, &VolTarget::paper_spot());
        assert!(weights.iter().all(|w| *w == 0.0));
    }
}
