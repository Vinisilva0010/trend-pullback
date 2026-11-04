//! Range module (Phase 6): fade Bollinger band touches with limit orders on
//! hourly or 4-hour USDT perpetual bars, only while the previous UTC day was
//! classified as a range day.
//!
//! Fills are conservative: a resting limit order fills at its price only when
//! the bar trades through it; a stop and a target in the same bar count as
//! the stop; stops and forced exits pay the taker fee plus slippage.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate};
use tp_core::intraday::{Bar, HOUR_MS};
use tp_core::regime::Regime;

use crate::{EquityPoint, as_f64};

/// Rules of one range variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeParams {
    pub bar_hours: i64,
    /// Bars in the mean and standard deviation (also the time stop).
    pub window: usize,
    /// Band distance in standard deviations.
    pub band: f64,
    /// Stop distance beyond the band, in standard deviations.
    pub stop_extra: f64,
    /// Fraction of equity risked to the stop.
    pub risk: f64,
    pub max_hold: usize,
}

/// Trading costs per side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeCosts {
    pub maker: f64,
    pub taker: f64,
    pub slippage: f64,
}

impl RangeCosts {
    /// Binance USDT-M futures regular tier.
    #[must_use]
    pub fn binance_futures() -> Self {
        Self {
            maker: 0.0002,
            taker: 0.0005,
            slippage: 0.0005,
        }
    }
}

/// Outcome of a range run.
#[derive(Debug, Clone, Default)]
pub struct RangeResult {
    /// Equity at the last bar of each day, marked to market.
    pub equity: Vec<EquityPoint>,
    /// Return of each trade relative to equity before it.
    pub trade_returns: Vec<f64>,
    pub bars_in_market: usize,
    pub bars_total: usize,
    pub funding_paid: f64,
}

impl RangeResult {
    /// Sum of winning trade returns over the sum of losing ones.
    #[must_use]
    pub fn profit_factor(&self) -> f64 {
        let wins: f64 = self.trade_returns.iter().filter(|r| **r > 0.0).sum();
        let losses: f64 = -self.trade_returns.iter().filter(|r| **r < 0.0).sum::<f64>();
        if losses > 0.0 {
            wins / losses
        } else if wins > 0.0 {
            f64::INFINITY
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Open {
    long: bool,
    qty: f64,
    entry: f64,
    stop: f64,
    opened: usize,
    equity_before: f64,
}

fn mean_sd(window: &[Bar]) -> (f64, f64) {
    let n = as_f64(window.len());
    let mean = window.iter().map(|b| b.close).sum::<f64>() / n;
    let variance = window.iter().map(|b| (b.close - mean).powi(2)).sum::<f64>() / n;
    (mean, variance.sqrt())
}

fn close_trade(cash: &mut f64, p: &Open, price: f64, fee_rate: f64, out: &mut Vec<f64>) {
    let direction = if p.long { 1.0 } else { -1.0 };
    *cash += (price - p.entry) * p.qty * direction - price * p.qty * fee_rate;
    out.push(*cash / p.equity_before - 1.0);
}

/// Runs one range variant on `bars` between `start` and `end` (UTC dates).
#[must_use]
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // one bar-by-bar loop; splitting it would hide the order of events
pub fn run_range(
    bars: &[Bar],
    regimes: &BTreeMap<NaiveDate, Regime>,
    funding: &BTreeMap<NaiveDate, f64>,
    params: &RangeParams,
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
    for i in params.window..bars.len() {
        let bar = bars[i];
        let Some(date) = date_of(bar.open_time_ms) else {
            continue;
        };
        if date < start || date > end {
            continue;
        }
        result.bars_total += 1;
        let window = &bars[i - params.window..i];
        let contiguous = window
            .windows(2)
            .all(|w| w[1].open_time_ms - w[0].open_time_ms == bar_ms)
            && window
                .last()
                .is_some_and(|w| bar.open_time_ms - w.open_time_ms == bar_ms);
        let (mean, sd) = mean_sd(window);
        let in_range = date.pred_opt().and_then(|d| regimes.get(&d)) == Some(&Regime::Range);
        let mut traded = false;
        if let Some(p) = open {
            let held = i - p.opened;
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
            let exit = if !in_range || held >= params.max_hold {
                Some((slip(bar.open), costs.taker))
            } else if p.long && bar.low <= p.stop {
                Some((slip(bar.open.min(p.stop)), costs.taker))
            } else if !p.long && bar.high >= p.stop {
                Some((slip(bar.open.max(p.stop)), costs.taker))
            } else if (p.long && bar.high > mean) || (!p.long && bar.low < mean) {
                Some((mean, costs.maker))
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
        if open.is_none() && !traded && in_range && contiguous && sd > 0.0 {
            let lower = mean - params.band * sd;
            let upper = mean + params.band * sd;
            let long_hit = bar.low < lower;
            let short_hit = bar.high > upper;
            if long_hit != short_hit {
                let long = long_hit;
                let entry = if long { lower } else { upper };
                let stop = if long {
                    entry - params.stop_extra * sd
                } else {
                    entry + params.stop_extra * sd
                };
                let qty = (params.risk * cash / (entry - stop).abs()).min(cash / entry);
                if qty > 0.0 && entry > 0.0 {
                    let p = Open {
                        long,
                        qty,
                        entry,
                        stop,
                        opened: i,
                        equity_before: cash,
                    };
                    cash -= entry * qty * costs.maker;
                    if (long && bar.low <= stop) || (!long && bar.high >= stop) {
                        let price = if long {
                            stop * (1.0 - costs.slippage)
                        } else {
                            stop * (1.0 + costs.slippage)
                        };
                        close_trade(&mut cash, &p, price, costs.taker, &mut result.trade_returns);
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

    fn flat_then(extra: &[Bar]) -> Vec<Bar> {
        let mut bars: Vec<Bar> = (0..4)
            .map(|i| {
                let c = if i % 2 == 0 { 99.0 } else { 101.0 };
                bar(i, c, c, c, c)
            })
            .collect();
        bars.extend_from_slice(extra);
        bars
    }

    fn all_range() -> BTreeMap<NaiveDate, Regime> {
        (1..=3)
            .map(|d| (NaiveDate::from_ymd_opt(2024, 1, d).unwrap(), Regime::Range))
            .collect()
    }

    fn params() -> RangeParams {
        RangeParams {
            bar_hours: 1,
            window: 4,
            band: 2.0,
            stop_extra: 1.5,
            risk: 0.01,
            max_hold: 4,
        }
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, d).unwrap()
    }

    #[test]
    fn long_from_lower_band_to_the_mean_by_hand() {
        let bars = flat_then(&[
            bar(4, 100.0, 100.0, 97.5, 99.0),
            bar(5, 99.0, 100.5, 98.5, 100.0),
        ]);
        let r = run_range(
            &bars,
            &all_range(),
            &BTreeMap::new(),
            &params(),
            &RangeCosts::binance_futures(),
            day(1),
            day(3),
            10_000.0,
        );
        assert_eq!(r.trade_returns.len(), 1);
        let qty = 100.0 / 1.5;
        let expected = (2.0 * qty - 98.0 * qty * 0.0002 - 100.0 * qty * 0.0002) / 10_000.0;
        assert!((r.trade_returns[0] - expected).abs() < 1e-9);
    }

    #[test]
    fn stop_in_the_entry_bar_counts_as_a_loss() {
        let bars = flat_then(&[bar(4, 100.0, 100.0, 96.0, 97.0)]);
        let r = run_range(
            &bars,
            &all_range(),
            &BTreeMap::new(),
            &params(),
            &RangeCosts::binance_futures(),
            day(1),
            day(3),
            10_000.0,
        );
        assert_eq!(r.trade_returns.len(), 1);
        let qty = 100.0 / 1.5;
        let exit = 96.5 * (1.0 - 0.0005);
        let expected = ((exit - 98.0) * qty - 98.0 * qty * 0.0002 - exit * qty * 0.0005) / 10_000.0;
        assert!((r.trade_returns[0] - expected).abs() < 1e-9);
    }

    #[test]
    fn no_trades_outside_range_days() {
        let bars = flat_then(&[
            bar(4, 100.0, 100.0, 97.5, 99.0),
            bar(5, 99.0, 100.5, 98.5, 100.0),
        ]);
        let regimes: BTreeMap<NaiveDate, Regime> = (1..=3).map(|d| (day(d), Regime::Up)).collect();
        let r = run_range(
            &bars,
            &regimes,
            &BTreeMap::new(),
            &params(),
            &RangeCosts::binance_futures(),
            day(1),
            day(3),
            10_000.0,
        );
        assert!(r.trade_returns.is_empty());
    }
}
