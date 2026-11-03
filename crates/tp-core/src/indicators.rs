//! Technical indicators on daily candles.
//!
//! Every indicator returns one value per input element, `None` until enough
//! history exists. Series with missing days are split with [`segments`] so
//! that no indicator spans a gap; [`by_segment`] restarts an indicator after
//! every gap.

use chrono::NaiveDate;

use crate::market::Candle;

pub(crate) fn as_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Closing prices of `candles`.
#[must_use]
pub fn close_values(candles: &[Candle]) -> Vec<f64> {
    candles.iter().map(Candle::close).collect()
}

/// Splits candles into runs of consecutive calendar days.
#[must_use]
pub fn segments(candles: &[Candle]) -> Vec<&[Candle]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, pair) in candles.windows(2).enumerate() {
        if let [a, b] = pair
            && (b.date() - a.date()).num_days() != 1
        {
            out.push(&candles[start..=i]);
            start = i + 1;
        }
    }
    if start < candles.len() {
        out.push(&candles[start..]);
    }
    out
}

/// Applies `indicator` to each gap-free segment and joins the results, so
/// every indicator restarts after a gap.
#[must_use]
pub fn by_segment<T>(
    candles: &[Candle],
    indicator: &dyn Fn(&[Candle]) -> Vec<Option<T>>,
) -> Vec<Option<T>> {
    let mut out = Vec::with_capacity(candles.len());
    for segment in segments(candles) {
        out.extend(indicator(segment));
    }
    out
}

/// Simple moving average over `period` elements.
#[must_use]
pub fn sma(values: &[f64], period: usize) -> Vec<Option<f64>> {
    if period == 0 {
        return vec![None; values.len()];
    }
    (0..values.len())
        .map(|i| {
            (i + 1 >= period)
                .then(|| values[i + 1 - period..=i].iter().sum::<f64>() / as_f64(period))
        })
        .collect()
}

/// Exponential moving average, seeded with the simple average of the first
/// `period` elements and smoothed with `2 / (period + 1)`.
#[must_use]
pub fn ema(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if period == 0 || values.len() < period {
        return out;
    }
    let alpha = 2.0 / (as_f64(period) + 1.0);
    let mut current = values[..period].iter().sum::<f64>() / as_f64(period);
    out[period - 1] = Some(current);
    for (slot, value) in out.iter_mut().zip(values).skip(period) {
        current += alpha * (value - current);
        *slot = Some(current);
    }
    out
}

/// Average true range with Wilder smoothing. The first true range of the
/// input has no previous close and uses high minus low.
#[must_use]
pub fn atr(candles: &[Candle], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; candles.len()];
    if period == 0 || candles.len() < period {
        return out;
    }
    let true_ranges: Vec<f64> = candles
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let range = c.high() - c.low();
            i.checked_sub(1)
                .and_then(|p| candles.get(p))
                .map_or(range, |prev| {
                    range
                        .max((c.high() - prev.close()).abs())
                        .max((c.low() - prev.close()).abs())
                })
        })
        .collect();
    let n = as_f64(period);
    let mut current = true_ranges[..period].iter().sum::<f64>() / n;
    out[period - 1] = Some(current);
    for (slot, value) in out.iter_mut().zip(&true_ranges).skip(period) {
        current = (current * (n - 1.0) + value) / n;
        *slot = Some(current);
    }
    out
}

/// Return over `period` elements: `value / value_period_ago - 1`.
#[must_use]
pub fn rate_of_change(values: &[f64], period: usize) -> Vec<Option<f64>> {
    (0..values.len())
        .map(|i| {
            i.checked_sub(period)
                .filter(|_| period > 0)
                .map(|j| values[i] / values[j] - 1.0)
        })
        .collect()
}

/// Highest value over the last `period` elements including the current one,
/// with how many elements ago it occurred (0 = current). Ties resolve to the
/// most recent occurrence.
#[must_use]
pub fn highest(values: &[f64], period: usize) -> Vec<Option<(f64, usize)>> {
    extreme(values, period, |candidate, best| candidate >= best)
}

/// Lowest value over the last `period` elements including the current one,
/// with how many elements ago it occurred. Ties resolve to the most recent.
#[must_use]
pub fn lowest(values: &[f64], period: usize) -> Vec<Option<(f64, usize)>> {
    extreme(values, period, |candidate, best| candidate <= best)
}

fn extreme(values: &[f64], period: usize, wins: fn(f64, f64) -> bool) -> Vec<Option<(f64, usize)>> {
    (0..values.len())
        .map(|i| {
            if period == 0 || i + 1 < period {
                return None;
            }
            let start = i + 1 - period;
            let mut best = (values[start], i - start);
            for (offset, &value) in values[start..=i].iter().enumerate().skip(1) {
                if wins(value, best.0) {
                    best = (value, i - start - offset);
                }
            }
            Some(best)
        })
        .collect()
}

/// A bullish fair value gap: a candle's low is above the high of the candle
/// two days earlier, leaving a price range that did not trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BullishFvg {
    /// Date of the candle that completed the gap.
    pub formed: NaiveDate,
    /// High of the candle two days before.
    pub bottom: f64,
    /// Low of the candle that completed the gap.
    pub top: f64,
}

/// Tracks bullish fair value gaps that are still unfilled. A gap counts as
/// filled once a later candle closes below its bottom. Use a new tracker
/// after every gap in the data.
#[derive(Debug, Clone, Default)]
pub struct FvgTracker {
    two_back: Option<Candle>,
    one_back: Option<Candle>,
    open: Vec<BullishFvg>,
}

impl FvgTracker {
    /// Feeds the next daily candle.
    pub fn push(&mut self, candle: Candle) {
        self.open.retain(|gap| candle.close() >= gap.bottom);
        if let Some(two_back) = self.two_back
            && candle.low() > two_back.high()
        {
            self.open.push(BullishFvg {
                formed: candle.date(),
                bottom: two_back.high(),
                top: candle.low(),
            });
        }
        self.two_back = self.one_back;
        self.one_back = Some(candle);
    }

    /// Unfilled gaps, oldest first.
    #[must_use]
    pub fn open(&self) -> &[BullishFvg] {
        &self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::CandleFields;
    use proptest::prelude::*;

    fn close_to(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, n).unwrap()
    }

    fn bar(date: NaiveDate, high: f64, low: f64, close: f64) -> Candle {
        Candle::try_new(CandleFields {
            date,
            open: close,
            high,
            low,
            close,
            volume: 1.0,
            quote_volume: 1.0,
        })
        .unwrap()
    }

    fn flat(date: NaiveDate, price: f64) -> Candle {
        bar(date, price, price, price)
    }

    #[test]
    fn sma_by_hand() {
        let out = sma(&[2.0, 4.0, 6.0, 8.0, 12.0], 3);
        assert_eq!(out[..2], [None, None]);
        assert!(close_to(out[2].unwrap(), 4.0));
        assert!(close_to(out[3].unwrap(), 6.0));
        assert!(close_to(out[4].unwrap(), 26.0 / 3.0));
        assert_eq!(sma(&[1.0], 0), vec![None]);
    }

    #[test]
    fn ema_by_hand() {
        let out = ema(&[2.0, 4.0, 6.0, 8.0, 12.0], 3);
        assert_eq!(out[..2], [None, None]);
        assert!(close_to(out[2].unwrap(), 4.0));
        assert!(close_to(out[3].unwrap(), 6.0));
        assert!(close_to(out[4].unwrap(), 9.0));
        assert_eq!(ema(&[1.0, 2.0], 3), vec![None, None]);
    }

    #[test]
    fn atr_by_hand() {
        let candles = [
            bar(day(1), 10.0, 8.0, 9.0),
            bar(day(2), 11.0, 9.0, 10.0),
            bar(day(3), 13.0, 10.0, 12.0),
            bar(day(4), 12.0, 7.0, 8.0),
        ];
        let out = atr(&candles, 3);
        assert_eq!(out[..2], [None, None]);
        assert!(close_to(out[2].unwrap(), 7.0 / 3.0));
        assert!(close_to(out[3].unwrap(), 29.0 / 9.0));
    }

    #[test]
    fn rate_of_change_by_hand() {
        let out = rate_of_change(&[10.0, 11.0, 12.0, 15.0], 2);
        assert_eq!(out[..2], [None, None]);
        assert!(close_to(out[2].unwrap(), 0.2));
        assert!(close_to(out[3].unwrap(), 15.0 / 11.0 - 1.0));
    }

    #[test]
    fn highest_and_lowest_report_bars_ago() {
        let values = [1.0, 3.0, 2.0, 5.0, 4.0];
        assert_eq!(
            highest(&values, 3),
            vec![None, None, Some((3.0, 1)), Some((5.0, 0)), Some((5.0, 1))]
        );
        assert_eq!(
            lowest(&values, 3),
            vec![None, None, Some((1.0, 2)), Some((2.0, 1)), Some((2.0, 2))]
        );
        assert_eq!(highest(&[2.0, 2.0], 2), vec![None, Some((2.0, 0))]);
    }

    #[test]
    fn segments_split_at_gaps_and_indicators_restart() {
        let candles = [flat(day(1), 1.0), flat(day(2), 3.0), flat(day(4), 5.0)];
        let parts = segments(&candles);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 2);
        assert_eq!(parts[1].len(), 1);
        let out = by_segment(&candles, &|s| sma(&close_values(s), 2));
        assert_eq!(out, vec![None, Some(2.0), None]);
        assert!(segments(&[]).is_empty());
    }

    #[test]
    fn fvg_forms_and_fills() {
        let mut tracker = FvgTracker::default();
        tracker.push(bar(day(1), 10.0, 9.0, 9.5));
        tracker.push(bar(day(2), 12.0, 10.0, 11.5));
        tracker.push(bar(day(3), 13.0, 11.0, 12.5));
        assert_eq!(
            tracker.open(),
            &[BullishFvg {
                formed: day(3),
                bottom: 10.0,
                top: 11.0
            }]
        );
        tracker.push(bar(day(4), 12.0, 10.2, 10.5));
        assert_eq!(
            tracker.open().len(),
            1,
            "close inside the gap does not fill it"
        );
        tracker.push(bar(day(5), 10.4, 9.5, 9.8));
        assert!(tracker.open().is_empty(), "close below the bottom fills it");
    }

    #[test]
    fn no_fvg_when_ranges_overlap() {
        let mut tracker = FvgTracker::default();
        tracker.push(bar(day(1), 10.0, 9.0, 9.5));
        tracker.push(bar(day(2), 11.0, 9.5, 10.5));
        tracker.push(bar(day(3), 12.0, 10.0, 11.0));
        assert!(tracker.open().is_empty());
    }

    proptest! {
        #[test]
        fn sma_matches_naive_mean(values in prop::collection::vec(1.0f64..1e6, 1..60), period in 1usize..20) {
            let out = sma(&values, period);
            for (i, item) in out.iter().enumerate() {
                if i + 1 < period {
                    prop_assert!(item.is_none());
                } else {
                    let window = &values[i + 1 - period..=i];
                    let mean = window.iter().sum::<f64>() / as_f64(period);
                    prop_assert!((item.unwrap() - mean).abs() <= 1e-9 * mean.abs().max(1.0));
                }
            }
        }

        #[test]
        fn highest_is_the_window_max(values in prop::collection::vec(-1e6f64..1e6, 1..60), period in 1usize..20) {
            for (i, item) in highest(&values, period).iter().enumerate() {
                if let Some((value, ago)) = item {
                    let window = &values[i + 1 - period..=i];
                    let max = window.iter().copied().fold(f64::MIN, f64::max);
                    prop_assert!((value - max).abs() < f64::EPSILON);
                    prop_assert!((values[i - ago] - max).abs() < f64::EPSILON);
                }
            }
        }
    }
}
