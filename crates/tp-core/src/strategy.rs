//! Rules of the trend-pullback method: which pairs have a valid setup on a
//! given day, and where the entry order, stop and first target are.
//!
//! Every value is computed from data up to and including the signal day. The
//! entry order derived from a setup is for the following day only; setups are
//! re-evaluated every day, so an order lives as long as its setup does.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::indicators::{
    BullishFvg, FvgTracker, as_f64, atr, by_segment, close_values, ema, highest, lowest,
    rate_of_change, sma,
};
use crate::market::Candle;

/// Where the entry limit order is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryMode {
    /// At the exponential moving average.
    Ema,
    /// At the top of the highest unfilled bullish fair value gap inside the
    /// band around the moving average; no order without such a gap.
    Fvg,
}

/// Parameters of the method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrategyParams {
    pub sma_fast: usize,
    pub sma_slow: usize,
    pub ema_entry: usize,
    pub atr_period: usize,
    pub momentum_days: usize,
    /// Fraction of the universe, ranked by momentum, allowed to trade.
    pub momentum_top_fraction: f64,
    pub high_lookback: usize,
    pub high_ago_min: usize,
    pub high_ago_max: usize,
    pub min_pullback_atr: f64,
    pub stop_atr: f64,
    pub stop_low_lookback: usize,
    pub target_r: f64,
    pub entry: EntryMode,
    pub fvg_below_atr: f64,
    pub fvg_above_atr: f64,
}

impl StrategyParams {
    /// Phase 1 defaults for crypto. Markets trade every day, so three months
    /// of momentum is 90 daily bars.
    #[must_use]
    pub fn crypto() -> Self {
        Self {
            sma_fast: 50,
            sma_slow: 200,
            ema_entry: 20,
            atr_period: 14,
            momentum_days: 90,
            momentum_top_fraction: 1.0 / 3.0,
            high_lookback: 10,
            high_ago_min: 2,
            high_ago_max: 7,
            min_pullback_atr: 1.0,
            stop_atr: 1.5,
            stop_low_lookback: 10,
            target_r: 2.0,
            entry: EntryMode::Ema,
            fvg_below_atr: 1.0,
            fvg_above_atr: 0.5,
        }
    }
}

/// Candles of one symbol with every indicator the rules need, aligned by index.
#[derive(Debug, Clone)]
pub struct Frame {
    pub symbol: String,
    pub candles: Vec<Candle>,
    pub sma_fast: Vec<Option<f64>>,
    pub sma_slow: Vec<Option<f64>>,
    pub ema: Vec<Option<f64>>,
    pub atr: Vec<Option<f64>>,
    pub momentum: Vec<Option<f64>>,
    pub highest_high: Vec<Option<(f64, usize)>>,
    pub lowest_low: Vec<Option<(f64, usize)>>,
    pub fvg_entry: Vec<Option<f64>>,
}

impl Frame {
    /// Computes every indicator, restarting after each gap in the data.
    #[must_use]
    pub fn new(symbol: &str, candles: Vec<Candle>, p: &StrategyParams) -> Self {
        let sma_fast = by_segment(&candles, &|s| sma(&close_values(s), p.sma_fast));
        let sma_slow = by_segment(&candles, &|s| sma(&close_values(s), p.sma_slow));
        let ema_values = by_segment(&candles, &|s| ema(&close_values(s), p.ema_entry));
        let atr_values = by_segment(&candles, &|s| atr(s, p.atr_period));
        let momentum = by_segment(&candles, &|s| {
            rate_of_change(&close_values(s), p.momentum_days)
        });
        let highest_high = by_segment(&candles, &|s| {
            let highs: Vec<f64> = s.iter().map(Candle::high).collect();
            highest(&highs, p.high_lookback)
        });
        let lowest_low = by_segment(&candles, &|s| {
            let lows: Vec<f64> = s.iter().map(Candle::low).collect();
            lowest(&lows, p.stop_low_lookback)
        });
        let fvg_entry = by_segment(&candles, &|s| fvg_entries(s, p));
        Self {
            symbol: symbol.to_owned(),
            candles,
            sma_fast,
            sma_slow,
            ema: ema_values,
            atr: atr_values,
            momentum,
            highest_high,
            lowest_low,
            fvg_entry,
        }
    }

    /// Index of the candle on `date`, if there is one.
    #[must_use]
    pub fn index_of(&self, date: NaiveDate) -> Option<usize> {
        self.candles.binary_search_by_key(&date, Candle::date).ok()
    }
}

fn fvg_entries(segment: &[Candle], p: &StrategyParams) -> Vec<Option<f64>> {
    let ema_values = ema(&close_values(segment), p.ema_entry);
    let atr_values = atr(segment, p.atr_period);
    let mut tracker = FvgTracker::default();
    segment
        .iter()
        .zip(ema_values.iter().zip(&atr_values))
        .map(|(candle, (ema_value, atr_value))| {
            tracker.push(*candle);
            (*ema_value)
                .zip(*atr_value)
                .and_then(|(e, a)| pick_fvg(tracker.open(), e, a, p.fvg_below_atr, p.fvg_above_atr))
        })
        .collect()
}

/// Top of the highest unfilled gap whose top lies within
/// `[ema - below_atr * atr, ema + above_atr * atr]`.
#[must_use]
pub fn pick_fvg(
    open: &[BullishFvg],
    ema: f64,
    atr: f64,
    below_atr: f64,
    above_atr: f64,
) -> Option<f64> {
    let band = (ema - below_atr * atr)..=(ema + above_atr * atr);
    open.iter()
        .map(|gap| gap.top)
        .filter(|top| band.contains(top))
        .max_by(f64::total_cmp)
}

/// A pair with a valid setup on `date`, with the order for the next day.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub symbol: String,
    /// Signal day; the order is for the following day.
    pub date: NaiveDate,
    pub close: f64,
    /// Limit price of the buy order.
    pub limit: f64,
    pub stop: f64,
    /// Price where the first half is sold.
    pub target: f64,
    pub atr: f64,
    pub momentum: f64,
}

/// True when the benchmark closed above its slow moving average on `date`.
#[must_use]
pub fn market_ok(benchmark: &Frame, date: NaiveDate) -> bool {
    benchmark.index_of(date).is_some_and(|i| {
        benchmark.sma_slow[i].is_some_and(|slow| benchmark.candles[i].close() > slow)
    })
}

/// How many of `n` ranked members fit in the top `fraction` (rounded up).
#[must_use]
pub fn top_count(n: usize, fraction: f64) -> usize {
    (0..=n)
        .find(|&k| as_f64(k) >= as_f64(n) * fraction)
        .unwrap_or(n)
}

/// Every setup on `date` among `universe` (the members of that month).
#[must_use]
pub fn setups_on(
    date: NaiveDate,
    benchmark: &Frame,
    frames: &BTreeMap<String, Frame>,
    universe: &[String],
    p: &StrategyParams,
) -> Vec<Setup> {
    if !market_ok(benchmark, date) {
        return Vec::new();
    }
    let mut ranked: Vec<(f64, &str)> = universe
        .iter()
        .filter_map(|symbol| {
            let frame = frames.get(symbol)?;
            let i = frame.index_of(date)?;
            Some((frame.momentum[i]?, symbol.as_str()))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    ranked.truncate(top_count(ranked.len(), p.momentum_top_fraction));
    ranked
        .iter()
        .filter_map(|&(momentum, symbol)| setup_for(frames.get(symbol)?, date, momentum, p))
        .collect()
}

fn setup_for(frame: &Frame, date: NaiveDate, momentum: f64, p: &StrategyParams) -> Option<Setup> {
    let i = frame.index_of(date)?;
    let close = frame.candles[i].close();
    let (fast, slow) = (frame.sma_fast[i]?, frame.sma_slow[i]?);
    let (atr_value, ema_value) = (frame.atr[i]?, frame.ema[i]?);
    if close <= slow || fast <= slow {
        return None;
    }
    let (high, ago) = frame.highest_high[i]?;
    if !(p.high_ago_min..=p.high_ago_max).contains(&ago)
        || high - close < p.min_pullback_atr * atr_value
    {
        return None;
    }
    let limit = match p.entry {
        EntryMode::Ema => ema_value,
        EntryMode::Fvg => frame.fvg_entry[i]?,
    };
    let (recent_low, _) = frame.lowest_low[i]?;
    let stop = recent_low.min(limit - p.stop_atr * atr_value);
    if stop <= 0.0 || stop >= limit {
        return None;
    }
    Some(Setup {
        symbol: frame.symbol.clone(),
        date,
        close,
        limit,
        stop,
        target: limit + p.target_r * (limit - stop),
        atr: atr_value,
        momentum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::CandleFields;
    use chrono::Days;

    fn date(offset: usize) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .checked_add_days(Days::new(u64::try_from(offset).unwrap()))
            .unwrap()
    }

    fn bar(offset: usize, high: f64, low: f64, close: f64) -> Candle {
        Candle::try_new(CandleFields {
            date: date(offset),
            open: close,
            high,
            low,
            close,
            volume: 1.0,
            quote_volume: 1.0,
        })
        .unwrap()
    }

    fn flat_series(closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| bar(i, c, c, c))
            .collect()
    }

    fn small_params() -> StrategyParams {
        StrategyParams {
            sma_fast: 5,
            sma_slow: 10,
            ema_entry: 3,
            atr_period: 3,
            momentum_days: 3,
            momentum_top_fraction: 1.0,
            high_lookback: 5,
            high_ago_min: 2,
            high_ago_max: 4,
            min_pullback_atr: 1.0,
            stop_atr: 1.5,
            stop_low_lookback: 5,
            target_r: 2.0,
            entry: EntryMode::Ema,
            fvg_below_atr: 1.0,
            fvg_above_atr: 0.5,
        }
    }

    /// Rises from 10 to 20, then pulls back to 18 and 17.8.
    fn pullback_closes() -> Vec<f64> {
        let mut closes: Vec<f64> = (10..=20).map(f64::from).collect();
        closes.extend([18.0, 17.8]);
        closes
    }

    fn one_symbol(p: &StrategyParams) -> (Frame, BTreeMap<String, Frame>) {
        let candles = flat_series(&pullback_closes());
        let benchmark = Frame::new("BENCH", candles.clone(), p);
        let mut frames = BTreeMap::new();
        frames.insert("AAAUSDT".to_owned(), Frame::new("AAAUSDT", candles, p));
        (benchmark, frames)
    }

    #[test]
    fn finds_setup_on_handmade_pullback() {
        let p = small_params();
        let (benchmark, frames) = one_symbol(&p);
        let universe = vec!["AAAUSDT".to_owned()];
        let setups = setups_on(date(12), &benchmark, &frames, &universe, &p);
        assert_eq!(setups.len(), 1);
        let s = &setups[0];
        let frame = &frames["AAAUSDT"];
        assert!((s.limit - 18.15).abs() < 1e-9, "EMA3 by hand is 18.15");
        assert!((s.stop - 17.8f64.min(s.limit - 1.5 * frame.atr[12].unwrap())).abs() < 1e-12);
        assert!((s.target - (s.limit + 2.0 * (s.limit - s.stop))).abs() < 1e-12);
        assert!(s.stop < s.limit && s.limit < s.target);
        assert!(setups_on(date(11), &benchmark, &frames, &universe, &p).is_empty());
    }

    #[test]
    fn no_setup_outside_universe_or_in_weak_market() {
        let p = small_params();
        let (benchmark, frames) = one_symbol(&p);
        assert!(setups_on(date(12), &benchmark, &frames, &[], &p).is_empty());
        let falling: Vec<f64> = (0..13).map(|i| 40.0 - f64::from(i)).collect();
        let weak = Frame::new("BENCH", flat_series(&falling), &p);
        assert!(!market_ok(&weak, date(12)));
        let universe = vec!["AAAUSDT".to_owned()];
        assert!(setups_on(date(12), &weak, &frames, &universe, &p).is_empty());
    }

    #[test]
    fn no_setup_when_high_is_outside_the_window() {
        let mut p = small_params();
        p.high_ago_max = 1;
        let (benchmark, frames) = one_symbol(&p);
        let universe = vec!["AAAUSDT".to_owned()];
        assert!(setups_on(date(12), &benchmark, &frames, &universe, &p).is_empty());
    }

    #[test]
    fn top_count_rounds_up() {
        assert_eq!(top_count(3, 1.0 / 3.0), 1);
        assert_eq!(top_count(20, 1.0 / 3.0), 7);
        assert_eq!(top_count(0, 1.0 / 3.0), 0);
        assert_eq!(top_count(5, 1.0), 5);
    }

    #[test]
    fn picks_highest_gap_inside_band() {
        let gap = |bottom: f64, top: f64| BullishFvg {
            formed: date(0),
            bottom,
            top,
        };
        let open = [gap(90.0, 95.0), gap(97.0, 99.0), gap(104.0, 106.0)];
        assert_eq!(pick_fvg(&open, 100.0, 4.0, 1.0, 0.5), Some(99.0));
        assert_eq!(pick_fvg(&open[..1], 100.0, 4.0, 1.0, 0.5), None);
    }

    fn random_walk(seed: u64, days: usize) -> Vec<Candle> {
        let mut state = seed;
        let mut price = 100.0;
        (0..days)
            .map(|i| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let unit = f64::from(u32::try_from(state >> 33).unwrap()) / 2_147_483_648.0;
                price *= 1.0 + 0.002 + (unit - 0.5) * 0.06;
                bar(i, price * 1.01, price * 0.99, price)
            })
            .collect()
    }

    fn frames_for(
        series: &[Vec<Candle>],
        cut: usize,
        p: &StrategyParams,
    ) -> (Frame, BTreeMap<String, Frame>) {
        let mut frames = BTreeMap::new();
        for (k, candles) in series.iter().enumerate() {
            let name = format!("S{k}USDT");
            let visible = candles[..cut.min(candles.len())].to_vec();
            frames.insert(name.clone(), Frame::new(&name, visible, p));
        }
        let benchmark = frames["S0USDT"].clone();
        (benchmark, frames)
    }

    #[test]
    fn no_look_ahead() {
        let p = StrategyParams {
            sma_fast: 10,
            sma_slow: 30,
            ema_entry: 5,
            atr_period: 5,
            momentum_days: 10,
            momentum_top_fraction: 2.0 / 3.0,
            high_lookback: 10,
            min_pullback_atr: 0.5,
            ..small_params()
        };
        let p = StrategyParams {
            high_ago_max: 7,
            stop_low_lookback: 10,
            ..p
        };
        let series: Vec<Vec<Candle>> = (1..=3).map(|seed| random_walk(seed, 300)).collect();
        let universe: Vec<String> = (0..3).map(|k| format!("S{k}USDT")).collect();
        let (full_bench, full_frames) = frames_for(&series, 300, &p);
        let mut total = 0;
        for cut in [120, 200, 260] {
            let (cut_bench, cut_frames) = frames_for(&series, cut, &p);
            for day in 0..cut {
                let full = setups_on(date(day), &full_bench, &full_frames, &universe, &p);
                let partial = setups_on(date(day), &cut_bench, &cut_frames, &universe, &p);
                assert_eq!(
                    full, partial,
                    "day {day} changed when data after day {cut} was removed"
                );
                total += full.len();
            }
        }
        assert!(
            total > 0,
            "the random series must produce some setups for this test to mean anything"
        );
    }
}
