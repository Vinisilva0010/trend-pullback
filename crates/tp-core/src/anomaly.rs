//! Abnormal hourly candles and what followed them.
//!
//! An hour is abnormal when its range (high minus low) or its volume is at
//! least three times the average of the previous 30 days of hours. For the
//! history, every abnormal-range hour in one direction is followed for 24
//! hours: does the price first move one more candle range in the same
//! direction, or first give the whole candle back?

use crate::indicators::as_f64;
use crate::intraday::{Bar, HOUR_MS};

/// Hours in the baseline window (30 days).
pub const BASE_HOURS: usize = 720;
/// Ratio to the baseline average that makes an hour abnormal.
pub const ANOMALY_RATIO: f64 = 3.0;
/// Hours followed after an abnormal candle.
pub const FOLLOW_HOURS: usize = 24;

/// Size of one hour compared with the previous 30 days.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spike {
    /// Range divided by the average range of the baseline hours.
    pub range_ratio: f64,
    /// Volume divided by the average volume of the baseline hours (0 when unknown).
    pub volume_ratio: f64,
    /// True when the hour closed above its open.
    pub up: bool,
}

impl Spike {
    /// True when the range or the volume is at least [`ANOMALY_RATIO`] times normal.
    #[must_use]
    pub fn is_abnormal(&self) -> bool {
        self.range_ratio >= ANOMALY_RATIO || self.volume_ratio >= ANOMALY_RATIO
    }
}

/// Spike of bar `i` against the [`BASE_HOURS`] bars before it. Needs a full,
/// gap-free baseline.
#[must_use]
pub fn spike_at(bars: &[Bar], i: usize) -> Option<Spike> {
    if i < BASE_HOURS || i >= bars.len() {
        return None;
    }
    let base = &bars[i - BASE_HOURS..i];
    let continuous = bars[i - BASE_HOURS..=i]
        .windows(2)
        .all(|w| w[1].open_time_ms - w[0].open_time_ms == HOUR_MS);
    if !continuous {
        return None;
    }
    let n = as_f64(BASE_HOURS);
    let mean_range = base.iter().map(|b| b.high - b.low).sum::<f64>() / n;
    if mean_range <= 0.0 {
        return None;
    }
    let mean_volume = base.iter().map(|b| b.volume).sum::<f64>() / n;
    let bar = bars[i];
    Some(Spike {
        range_ratio: (bar.high - bar.low) / mean_range,
        volume_ratio: if mean_volume > 0.0 {
            bar.volume / mean_volume
        } else {
            0.0
        },
        up: bar.close > bar.open,
    })
}

/// What followed past abnormal-range candles in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Follow {
    /// Price moved one more candle range in the candle's direction first.
    pub continued: usize,
    /// Price gave the whole candle range back first (or both in one hour).
    pub reversed: usize,
    /// Neither within [`FOLLOW_HOURS`].
    pub neither: usize,
}

impl Follow {
    #[must_use]
    pub fn total(&self) -> usize {
        self.continued + self.reversed + self.neither
    }

    /// z-score of continued against reversed under a 50/50 null.
    #[must_use]
    pub fn z(&self) -> f64 {
        let n = self.continued + self.reversed;
        if n == 0 {
            return 0.0;
        }
        (as_f64(self.continued) - as_f64(self.reversed)) / as_f64(n).sqrt()
    }
}

/// Follows every past hour whose range was at least [`ANOMALY_RATIO`] times
/// normal and that closed in direction `up`. From its close, the target is
/// one candle range further in the same direction and the stop is one range
/// back. Cases do not overlap, and windows with missing hours are skipped.
#[must_use]
pub fn follow_after_spikes(bars: &[Bar], up: bool) -> Follow {
    let mut f = Follow {
        continued: 0,
        reversed: 0,
        neither: 0,
    };
    // Prefix sums give the baseline average in constant time per hour.
    let mut prefix = Vec::with_capacity(bars.len() + 1);
    prefix.push(0.0);
    for b in bars {
        prefix.push(prefix[prefix.len() - 1] + (b.high - b.low));
    }
    // Index of the first bar of the gap-free run each bar belongs to.
    let mut run_start = vec![0usize; bars.len()];
    for i in 1..bars.len() {
        run_start[i] = if bars[i].open_time_ms - bars[i - 1].open_time_ms == HOUR_MS {
            run_start[i - 1]
        } else {
            i
        };
    }
    let mut i = BASE_HOURS;
    while i + FOLLOW_HOURS < bars.len() {
        let bar = bars[i];
        let range = bar.high - bar.low;
        let mean = (prefix[i] - prefix[i - BASE_HOURS]) / as_f64(BASE_HOURS);
        let same_way = if up {
            bar.close > bar.open
        } else {
            bar.close < bar.open
        };
        let gap_free = run_start[i + FOLLOW_HOURS] <= i - BASE_HOURS;
        if !(gap_free && mean > 0.0 && same_way && range / mean >= ANOMALY_RATIO) {
            i += 1;
            continue;
        }
        let (target, stop) = if up {
            (bar.close + range, bar.close - range)
        } else {
            (bar.close - range, bar.close + range)
        };
        let mut outcome = None;
        for next in &bars[i + 1..=i + FOLLOW_HOURS] {
            let hit_stop = if up {
                next.low <= stop
            } else {
                next.high >= stop
            };
            let hit_target = if up {
                next.high >= target
            } else {
                next.low <= target
            };
            if hit_stop {
                outcome = Some(false);
                break;
            }
            if hit_target {
                outcome = Some(true);
                break;
            }
        }
        match outcome {
            Some(true) => f.continued += 1,
            Some(false) => f.reversed += 1,
            None => f.neither += 1,
        }
        i += FOLLOW_HOURS + 1;
    }
    f
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hour(i: usize, open: f64, high: f64, low: f64, close: f64, volume: f64) -> Bar {
        Bar {
            open_time_ms: i64::try_from(i).unwrap() * HOUR_MS,
            open,
            high,
            low,
            close,
            volume,
        }
    }

    fn calm(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| hour(i, 100.0, 100.5, 99.5, 100.0, 10.0))
            .collect()
    }

    #[test]
    fn spike_compares_with_previous_thirty_days() {
        let mut bars = calm(BASE_HOURS);
        bars.push(hour(BASE_HOURS, 100.0, 103.0, 100.0, 102.5, 50.0));
        let s = spike_at(&bars, BASE_HOURS).unwrap();
        assert_eq!(s.range_ratio, 3.0);
        assert_eq!(s.volume_ratio, 5.0);
        assert!(s.up && s.is_abnormal());
        assert!(spike_at(&bars, BASE_HOURS - 1).is_none());
    }

    #[test]
    fn spike_needs_a_gap_free_baseline() {
        let mut bars = calm(BASE_HOURS);
        bars[10].open_time_ms += HOUR_MS / 2;
        bars.push(hour(BASE_HOURS, 100.0, 103.0, 100.0, 102.5, 50.0));
        assert!(spike_at(&bars, BASE_HOURS).is_none());
    }

    #[test]
    fn follow_counts_continuation_and_reversal() {
        let mut bars = calm(BASE_HOURS);
        // Up spike of range 3 closing at 102.5: target 105.5, stop 99.5.
        bars.push(hour(BASE_HOURS, 100.0, 103.0, 100.0, 102.5, 50.0));
        bars.push(hour(BASE_HOURS + 1, 102.5, 106.0, 102.0, 105.8, 10.0));
        for k in 2..=FOLLOW_HOURS {
            bars.push(hour(BASE_HOURS + k, 105.8, 106.0, 105.5, 105.8, 10.0));
        }
        let f = follow_after_spikes(&bars, true);
        assert_eq!((f.continued, f.reversed, f.neither), (1, 0, 0));
        assert_eq!(follow_after_spikes(&bars, false).total(), 0);
        // Same spike, then a drop back below 99.5.
        bars[BASE_HOURS + 1] = hour(BASE_HOURS + 1, 102.5, 102.6, 99.0, 99.2, 10.0);
        let f = follow_after_spikes(&bars, true);
        assert_eq!((f.continued, f.reversed, f.neither), (0, 1, 0));
        assert!(f.z() < 0.0);
    }
}
