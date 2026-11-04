//! Horizontal levels and open gaps on daily candles, seen only with the data
//! available on a given day.
//!
//! A level is a cluster of turning points (swing highs and swing lows) from
//! the past year, so an old top that later held as a floor counts too. A gap
//! is a three-candle jump where the third candle's low stays above the first
//! candle's high, left untouched since.

use crate::market::Candle;

/// One horizontal level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// Average price of the turning points in the cluster.
    pub price: f64,
    /// Number of turning points in the cluster.
    pub touches: usize,
}

/// One open gap below the price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    /// High of the candle before the jump.
    pub bottom: f64,
    /// Low of the candle after the jump; the first price met coming down.
    pub top: f64,
    /// Index of the middle candle of the jump.
    pub formed: usize,
}

/// Horizontal levels from swing points in the `lookback` candles ending at
/// `upto` (inclusive). A swing point needs `wing` candles on each side, all
/// within the window, so the most recent `wing` candles never form one.
/// Points closer than `tolerance` (in price) to the previous point of the
/// sorted chain join the same cluster.
#[must_use]
pub fn horizontal_levels(
    candles: &[Candle],
    upto: usize,
    lookback: usize,
    wing: usize,
    tolerance: f64,
) -> Vec<Level> {
    if upto >= candles.len() || wing == 0 {
        return Vec::new();
    }
    let first = (upto + 1).saturating_sub(lookback);
    let mut points = Vec::new();
    for i in first + wing..=upto.saturating_sub(wing) {
        if i < wing || i + wing > upto {
            continue;
        }
        let around = &candles[i - wing..=i + wing];
        let high = candles[i].high();
        let low = candles[i].low();
        if around.iter().all(|c| c.high() <= high) {
            points.push(high);
        }
        if around.iter().all(|c| c.low() >= low) {
            points.push(low);
        }
    }
    points.sort_by(f64::total_cmp);
    let mut levels = Vec::new();
    let mut cluster: Vec<f64> = Vec::new();
    for p in points {
        if cluster.last().is_some_and(|last| p - last > tolerance) {
            levels.push(summarize(&cluster));
            cluster.clear();
        }
        cluster.push(p);
    }
    if !cluster.is_empty() {
        levels.push(summarize(&cluster));
    }
    levels
}

fn summarize(cluster: &[f64]) -> Level {
    let n = f64::from(u32::try_from(cluster.len()).unwrap_or(u32::MAX));
    Level {
        price: cluster.iter().sum::<f64>() / n,
        touches: cluster.len(),
    }
}

/// The highest level below `price` with at least `min_touches` turning points.
#[must_use]
pub fn support_below(levels: &[Level], price: f64, min_touches: usize) -> Option<Level> {
    levels
        .iter()
        .filter(|l| l.price < price && l.touches >= min_touches)
        .max_by(|a, b| a.price.total_cmp(&b.price))
        .copied()
}

/// The highest open gap below `price` formed in the `lookback` candles ending
/// at `upto`. A gap is open while no later candle (through `upto`) has a low
/// at or below its top.
#[must_use]
pub fn open_gap_below(candles: &[Candle], upto: usize, lookback: usize, price: f64) -> Option<Gap> {
    if upto >= candles.len() || upto < 2 {
        return None;
    }
    let first = (upto + 1).saturating_sub(lookback).max(1);
    // Lowest low from each index through `upto`.
    let mut later_low = vec![f64::MAX; upto + 2];
    for i in (first..=upto).rev() {
        later_low[i] = later_low[i + 1].min(candles[i].low());
    }
    let mut best: Option<Gap> = None;
    for i in first..upto {
        let bottom = candles[i - 1].high();
        let top = candles[i + 1].low();
        if top <= bottom || top >= price {
            continue;
        }
        let untouched = i + 2 > upto || later_low[i + 2] > top;
        if untouched && best.is_none_or(|b| top > b.top) {
            best = Some(Gap {
                bottom,
                top,
                formed: i,
            });
        }
    }
    best
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::market::CandleFields;
    use chrono::{Days, NaiveDate};

    fn bar(day: u64, high: f64, low: f64) -> Candle {
        let mid = f64::midpoint(high, low);
        Candle::try_new(CandleFields {
            date: NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .checked_add_days(Days::new(day))
                .unwrap(),
            open: mid,
            high,
            low,
            close: mid,
            volume: 1.0,
            quote_volume: 1.0,
        })
        .unwrap()
    }

    fn from_lows(lows: &[f64]) -> Vec<Candle> {
        (0..lows.len())
            .map(|i| bar(u64::try_from(i).unwrap(), lows[i] + 1.0, lows[i]))
            .collect()
    }

    #[test]
    fn levels_cluster_swing_lows_and_highs() {
        // Two swing lows near 10 and one swing high near 15.
        let lows = [
            12.0, 11.0, 10.0, 11.0, 12.0, 13.0, 14.0, 13.0, 12.0, 11.0, 10.2, 11.0, 12.0,
        ];
        let candles = from_lows(&lows);
        let levels = horizontal_levels(&candles, candles.len() - 1, 100, 2, 0.5);
        let floor = levels.iter().find(|l| l.price < 11.0).unwrap();
        assert_eq!(floor.touches, 2);
        assert!((floor.price - 10.1).abs() < 1e-9);
        assert!(
            levels
                .iter()
                .any(|l| (l.price - 15.0).abs() < 1e-9 && l.touches == 1)
        );
    }

    #[test]
    fn levels_ignore_points_without_both_wings() {
        let lows = [12.0, 11.0, 10.0, 11.0, 12.0, 13.0, 9.0];
        let candles = from_lows(&lows);
        let levels = horizontal_levels(&candles, candles.len() - 1, 100, 2, 0.5);
        // The last candle (9.0) has no right wing, so it is not a swing low.
        assert!(levels.iter().all(|l| l.price > 9.5));
    }

    #[test]
    fn support_below_picks_highest_level_with_enough_touches() {
        let levels = [
            Level {
                price: 8.0,
                touches: 3,
            },
            Level {
                price: 9.0,
                touches: 1,
            },
            Level {
                price: 9.5,
                touches: 2,
            },
            Level {
                price: 12.0,
                touches: 5,
            },
        ];
        assert_eq!(support_below(&levels, 10.0, 2).unwrap().price, 9.5);
        assert_eq!(support_below(&levels, 9.2, 2).unwrap().price, 8.0);
        assert!(support_below(&levels, 7.0, 1).is_none());
    }

    #[test]
    fn open_gap_found_until_price_trades_into_it() {
        // Jump between day 1 (high 11) and day 3 (low 13); later lows stay above 13.
        let candles = vec![
            bar(0, 10.5, 9.5),
            bar(1, 11.0, 10.0),
            bar(2, 14.0, 11.5),
            bar(3, 15.0, 13.0),
            bar(4, 16.0, 14.0),
            bar(5, 16.5, 14.5),
        ];
        let gap = open_gap_below(&candles, 5, 100, 15.0).unwrap();
        assert_eq!((gap.bottom, gap.top, gap.formed), (11.0, 13.0, 2));
        let mut filled = candles.clone();
        filled.push(bar(6, 15.0, 12.5));
        // The upper gap is now filled; the smaller one below (10.5 to 11.5) is still open.
        let lower = open_gap_below(&filled, 6, 100, 15.0).unwrap();
        assert_eq!((lower.bottom, lower.top, lower.formed), (10.5, 11.5, 1));
        filled.push(bar(7, 13.0, 11.0));
        assert!(open_gap_below(&filled, 7, 100, 15.0).is_none());
        // Only gaps whose top is below the price count.
        assert_eq!(open_gap_below(&candles, 5, 100, 12.0).unwrap().top, 11.5);
    }
}
