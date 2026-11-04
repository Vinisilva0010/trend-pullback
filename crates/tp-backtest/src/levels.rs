//! Horizontal range levels (Phase 6): ranges drawn the way a chartist draws
//! them, from repeated touches of a top and a bottom, and two events at their
//! edges: a close outside (breakout, trade the continuation) or a wick outside
//! with a close back inside (failed breakout, trade toward the other edge).
//!
//! Entries are market orders at the next bar's open (taker fee plus
//! slippage); targets are resting limit orders that fill only when traded
//! through; stops pay taker plus slippage; a stop and a target in the same
//! bar count as the stop.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate};
use tp_core::intraday::{Bar, HOUR_MS};

use crate::range::{RangeCosts, RangeResult};
use crate::{EquityPoint, as_f64};

/// Event traded at the range edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeEvent {
    /// Close outside the range: trade the continuation.
    Breakout,
    /// Wick outside, close back inside: trade toward the other edge.
    Failure,
}

/// Rules of one level variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelParams {
    pub bar_hours: i64,
    /// Bars that define the range (also the time stop).
    pub lookback: usize,
    pub event: EdgeEvent,
    /// Maximum range height in average true ranges.
    pub max_height_atr: f64,
    /// Distance to an edge that counts as a touch, in average true ranges.
    pub touch_atr: f64,
    /// Minimum bars between two touches.
    pub touch_gap: usize,
    /// Fraction of equity risked to the stop.
    pub risk: f64,
}

impl LevelParams {
    #[must_use]
    pub fn new(bar_hours: i64, lookback: usize, event: EdgeEvent) -> Self {
        Self {
            bar_hours,
            lookback,
            event,
            max_height_atr: 8.0,
            touch_atr: 0.25,
            touch_gap: 3,
            risk: 0.01,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Setup {
    long: bool,
    stop: f64,
    target: f64,
}

#[derive(Debug, Clone, Copy)]
struct Open {
    long: bool,
    qty: f64,
    entry: f64,
    stop: f64,
    target: f64,
    opened: usize,
    equity_before: f64,
}

fn touches(window: &[Bar], hit: impl Fn(&Bar) -> bool, gap: usize) -> usize {
    let mut count = 0;
    let mut last: Option<usize> = None;
    for (i, b) in window.iter().enumerate() {
        if hit(b) && last.is_none_or(|l| i - l >= gap) {
            count += 1;
            last = Some(i);
        }
    }
    count
}

fn average_true_range(window: &[Bar]) -> f64 {
    let mut sum = 0.0;
    for (i, b) in window.iter().enumerate() {
        let range = b.high - b.low;
        sum += if i == 0 {
            range
        } else {
            let prev = window[i - 1].close;
            range.max((b.high - prev).abs()).max((b.low - prev).abs())
        };
    }
    sum / as_f64(window.len())
}

/// Setup signalled by the bar `signal` that closed right after `window`.
fn setup(window: &[Bar], signal: &Bar, p: &LevelParams) -> Option<Setup> {
    let top = window.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let bottom = window.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    let atr = average_true_range(window);
    let height = top - bottom;
    if !(atr > 0.0 && height > 0.0 && height <= p.max_height_atr * atr) {
        return None;
    }
    let tolerance = p.touch_atr * atr;
    if touches(window, |b| b.high >= top - tolerance, p.touch_gap) < 2
        || touches(window, |b| b.low <= bottom + tolerance, p.touch_gap) < 2
    {
        return None;
    }
    let mid = f64::midpoint(top, bottom);
    match p.event {
        EdgeEvent::Breakout => {
            if signal.close > top {
                Some(Setup {
                    long: true,
                    stop: mid,
                    target: top + height,
                })
            } else if signal.close < bottom {
                Some(Setup {
                    long: false,
                    stop: mid,
                    target: bottom - height,
                })
            } else {
                None
            }
        }
        EdgeEvent::Failure => {
            let inside = signal.close > bottom && signal.close < top;
            if inside && signal.low < bottom && signal.high <= top {
                Some(Setup {
                    long: true,
                    stop: signal.low - 0.1 * atr,
                    target: top,
                })
            } else if inside && signal.high > top && signal.low >= bottom {
                Some(Setup {
                    long: false,
                    stop: signal.high + 0.1 * atr,
                    target: bottom,
                })
            } else {
                None
            }
        }
    }
}

fn close_trade(cash: &mut f64, p: &Open, price: f64, fee_rate: f64, out: &mut Vec<f64>) {
    let direction = if p.long { 1.0 } else { -1.0 };
    *cash += (price - p.entry) * p.qty * direction - price * p.qty * fee_rate;
    out.push(*cash / p.equity_before - 1.0);
}

/// Runs one level variant on `bars` between `start` and `end` (UTC dates).
#[must_use]
#[allow(clippy::too_many_lines)] // one bar-by-bar loop; splitting it would hide the order of events
pub fn run_levels(
    bars: &[Bar],
    funding: &BTreeMap<NaiveDate, f64>,
    params: &LevelParams,
    costs: &RangeCosts,
    start: NaiveDate,
    end: NaiveDate,
    initial: f64,
) -> RangeResult {
    let bar_ms = params.bar_hours * HOUR_MS;
    let bars_per_day = 24.0 / as_f64(usize::try_from(params.bar_hours).unwrap_or(1).max(1));
    let date_of = |t: i64| DateTime::from_timestamp_millis(t).map(|d| d.date_naive());
    let mut result = RangeResult::default();
    let mut cash = initial;
    let mut open: Option<Open> = None;
    for i in (params.lookback + 1)..bars.len() {
        let bar = bars[i];
        let Some(date) = date_of(bar.open_time_ms) else {
            continue;
        };
        if date < start || date > end {
            continue;
        }
        result.bars_total += 1;
        let mut traded = false;
        if let Some(p) = open {
            let rate = funding.get(&date).copied().unwrap_or(0.0) / bars_per_day;
            let paid = rate * p.qty * bar.open * if p.long { 1.0 } else { -1.0 };
            cash -= paid;
            result.funding_paid += paid;
            let slip = |price: f64| {
                if p.long {
                    price * (1.0 - costs.slippage)
                } else {
                    price * (1.0 + costs.slippage)
                }
            };
            let exit = if i - p.opened >= params.lookback {
                Some((slip(bar.open), costs.taker))
            } else if p.long && bar.low <= p.stop {
                Some((slip(bar.open.min(p.stop)), costs.taker))
            } else if !p.long && bar.high >= p.stop {
                Some((slip(bar.open.max(p.stop)), costs.taker))
            } else if (p.long && bar.high > p.target) || (!p.long && bar.low < p.target) {
                Some((p.target, costs.maker))
            } else {
                None
            };
            if let Some((price, fee)) = exit {
                close_trade(&mut cash, &p, price, fee, &mut result.trade_returns);
                open = None;
                traded = true;
            } else {
                result.bars_in_market += 1;
            }
        }
        if open.is_none() && !traded {
            let s = i - 1;
            let contiguous = bars[s - params.lookback..=i]
                .windows(2)
                .all(|w| w[1].open_time_ms - w[0].open_time_ms == bar_ms);
            if contiguous
                && let Some(found) = setup(&bars[s - params.lookback..s], &bars[s], params)
            {
                let long = found.long;
                let entry = if long {
                    bar.open * (1.0 + costs.slippage)
                } else {
                    bar.open * (1.0 - costs.slippage)
                };
                let valid = if long {
                    found.stop < entry && entry < found.target
                } else {
                    found.target < entry && entry < found.stop
                };
                if valid {
                    let qty = (params.risk * cash / (entry - found.stop).abs()).min(cash / entry);
                    let p = Open {
                        long,
                        qty,
                        entry,
                        stop: found.stop,
                        target: found.target,
                        opened: i,
                        equity_before: cash,
                    };
                    cash -= entry * qty * costs.taker;
                    if (long && bar.low <= p.stop) || (!long && bar.high >= p.stop) {
                        let price = if long {
                            p.stop * (1.0 - costs.slippage)
                        } else {
                            p.stop * (1.0 + costs.slippage)
                        };
                        close_trade(&mut cash, &p, price, costs.taker, &mut result.trade_returns);
                    } else if (long && bar.high > p.target) || (!long && bar.low < p.target) {
                        close_trade(
                            &mut cash,
                            &p,
                            p.target,
                            costs.maker,
                            &mut result.trade_returns,
                        );
                    } else {
                        open = Some(p);
                        result.bars_in_market += 1;
                    }
                }
            }
        }
        let next_date = bars.get(i + 1).and_then(|b| date_of(b.open_time_ms));
        if next_date != Some(date) {
            let mtm = open.map_or(0.0, |p| {
                (bar.close - p.entry) * p.qty * if p.long { 1.0 } else { -1.0 }
            });
            result.equity.push(EquityPoint {
                date,
                equity: cash + mtm,
                cash,
                positions: usize::from(open.is_some()),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(i: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: 1_704_153_600_000 + i * HOUR_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    fn range_bars() -> Vec<Bar> {
        (0..6)
            .map(|i| {
                let c = if i % 2 == 0 { 100.5 } else { 99.5 };
                bar(i, c, 101.0, 99.0, c)
            })
            .collect()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, d).unwrap()
    }

    fn run(bars: &[Bar], event: EdgeEvent) -> RangeResult {
        run_levels(
            bars,
            &BTreeMap::new(),
            &LevelParams::new(1, 6, event),
            &RangeCosts::binance_futures(),
            day(1),
            day(3),
            10_000.0,
        )
    }

    #[test]
    fn breakout_to_measured_target_by_hand() {
        let mut bars = range_bars();
        bars.push(bar(6, 100.5, 101.6, 100.4, 101.5));
        bars.push(bar(7, 101.5, 103.5, 101.0, 103.2));
        let r = run(&bars, EdgeEvent::Breakout);
        assert_eq!(r.trade_returns.len(), 1);
        let entry = 101.5 * 1.0005;
        let qty = 100.0 / (entry - 100.0);
        let expected =
            ((103.0 - entry) * qty - entry * qty * 0.0005 - 103.0 * qty * 0.0002) / 10_000.0;
        assert!((r.trade_returns[0] - expected).abs() < 1e-9);
    }

    #[test]
    fn failed_breakdown_to_the_top_by_hand() {
        let mut bars = range_bars();
        bars.push(bar(6, 99.5, 99.8, 98.5, 99.6));
        bars.push(bar(7, 99.6, 101.5, 99.4, 101.2));
        let r = run(&bars, EdgeEvent::Failure);
        assert_eq!(r.trade_returns.len(), 1);
        let entry = 99.6 * 1.0005;
        let stop = 98.5 - 0.1 * 2.0;
        let qty = 100.0 / (entry - stop);
        let expected =
            ((101.0 - entry) * qty - entry * qty * 0.0005 - 101.0 * qty * 0.0002) / 10_000.0;
        assert!((r.trade_returns[0] - expected).abs() < 1e-9);
    }

    #[test]
    fn no_setup_without_a_range() {
        let mut bars: Vec<Bar> = (0..6)
            .map(|i| {
                let c = 100.0 + f64::from(i32::try_from(i).unwrap()) * 3.0;
                bar(i, c - 1.0, c + 0.5, c - 1.5, c)
            })
            .collect();
        bars.push(bar(6, 115.0, 120.0, 114.0, 119.0));
        bars.push(bar(7, 119.0, 125.0, 118.0, 124.0));
        assert!(run(&bars, EdgeEvent::Breakout).trade_returns.is_empty());
    }
}
