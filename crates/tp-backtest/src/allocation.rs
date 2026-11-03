//! Single-asset target-weight simulation. A weight decided at the close of a
//! day is traded at the next day's open, paying slippage and fees. Weight
//! changes caused only by volatility are traded when they exceed the
//! rebalance threshold; entries and full exits always trade.

use chrono::NaiveDate;
use tp_core::market::Candle;

use crate::{EquityPoint, as_f64, yearly_returns};

/// Costs and rebalancing settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AllocationConfig {
    pub initial_capital: f64,
    pub fee_rate: f64,
    pub slippage: f64,
    /// Relative weight change needed to rebalance (0.2 = 20%).
    pub rebalance_threshold: f64,
}

impl AllocationConfig {
    #[must_use]
    pub fn crypto() -> Self {
        Self {
            initial_capital: 10_000.0,
            fee_rate: 0.001,
            slippage: 0.001,
            rebalance_threshold: 0.2,
        }
    }
}

/// Daily equity and activity of one run.
#[derive(Debug, Clone)]
pub struct AllocationResult {
    pub equity: Vec<EquityPoint>,
    pub trades: usize,
    pub average_exposure: f64,
}

/// Performance numbers of one run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AllocationStats {
    pub cagr: f64,
    pub sharpe: f64,
    pub max_drawdown: f64,
    pub total_return: f64,
    pub non_losing_year_fraction: f64,
}

/// Simulates holding `weights[i]` (decided at the close of day `i`) from the
/// next open, between `start` and `end` inclusive.
#[must_use]
pub fn run_allocation(
    candles: &[Candle],
    weights: &[f64],
    config: &AllocationConfig,
    start: NaiveDate,
    end: NaiveDate,
) -> AllocationResult {
    let mut cash = config.initial_capital;
    let mut units = 0.0_f64;
    let mut pending: Option<f64> = None;
    let mut equity = Vec::new();
    let mut trades = 0;
    let mut exposure_sum = 0.0;
    for (candle, &weight) in candles.iter().zip(weights) {
        let date = candle.date();
        if date < start || date > end {
            continue;
        }
        if let Some(target) = pending {
            let open = candle.open();
            let value = cash + units * open;
            let current = if value > 0.0 {
                units * open / value
            } else {
                0.0
            };
            let (flat_target, flat_now) = (target <= 0.0, units <= 0.0);
            let drift =
                current > 0.0 && (target - current).abs() / current > config.rebalance_threshold;
            if flat_target != flat_now || (!flat_target && drift) {
                let delta = target * value / open - units;
                if delta > 0.0 {
                    let price = open * (1.0 + config.slippage);
                    let buy = delta.min(cash / (price * (1.0 + config.fee_rate)));
                    let cost = buy * price;
                    cash -= cost + cost * config.fee_rate;
                    units += buy;
                } else if delta < 0.0 {
                    let sell = (-delta).min(units);
                    let proceeds = sell * open * (1.0 - config.slippage);
                    cash += proceeds - proceeds * config.fee_rate;
                    units -= sell;
                }
                trades += 1;
            }
        }
        let held = units * candle.close();
        let total = cash + held;
        exposure_sum += if total > 0.0 { held / total } else { 0.0 };
        equity.push(EquityPoint {
            date,
            equity: total,
            cash,
            positions: usize::from(units > 0.0),
        });
        pending = Some(weight);
    }
    let average_exposure = if equity.is_empty() {
        0.0
    } else {
        exposure_sum / as_f64(equity.len())
    };
    AllocationResult {
        equity,
        trades,
        average_exposure,
    }
}

/// CAGR, Sharpe (daily returns, 365 days a year), drawdown and yearly record.
#[must_use]
pub fn allocation_stats(equity: &[EquityPoint], initial_capital: f64) -> AllocationStats {
    let mut previous = initial_capital;
    let mut returns = Vec::with_capacity(equity.len());
    let mut peak = initial_capital;
    let mut max_drawdown: f64 = 0.0;
    for point in equity {
        returns.push(point.equity / previous - 1.0);
        previous = point.equity;
        peak = peak.max(point.equity);
        max_drawdown = max_drawdown.max((peak - point.equity) / peak);
    }
    let n = as_f64(returns.len());
    let mean = if returns.is_empty() {
        0.0
    } else {
        returns.iter().sum::<f64>() / n
    };
    let variance = if returns.len() < 2 {
        0.0
    } else {
        returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0)
    };
    let sharpe = if variance > 0.0 {
        mean / variance.sqrt() * 365.0_f64.sqrt()
    } else {
        0.0
    };
    let final_equity = equity.last().map_or(initial_capital, |p| p.equity);
    let years = n / 365.0;
    let cagr = if years > 0.0 {
        (final_equity / initial_capital).powf(1.0 / years) - 1.0
    } else {
        0.0
    };
    let yearly = yearly_returns(equity, initial_capital);
    let non_losing = yearly.iter().filter(|(_, r)| *r >= 0.0).count();
    let non_losing_year_fraction = if yearly.is_empty() {
        0.0
    } else {
        as_f64(non_losing) / as_f64(yearly.len())
    };
    AllocationStats {
        cagr,
        sharpe,
        max_drawdown,
        total_return: final_equity / initial_capital - 1.0,
        non_losing_year_fraction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Days;
    use tp_core::market::CandleFields;

    fn day(offset: u64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .checked_add_days(Days::new(offset))
            .unwrap()
    }

    fn bars(pairs: &[(f64, f64)]) -> Vec<Candle> {
        pairs
            .iter()
            .zip(0..)
            .map(|(&(open, close), i)| {
                Candle::try_new(CandleFields {
                    date: day(i),
                    open,
                    high: open.max(close),
                    low: open.min(close),
                    close,
                    volume: 1.0,
                    quote_volume: 1.0,
                })
                .unwrap()
            })
            .collect()
    }

    fn no_costs() -> AllocationConfig {
        AllocationConfig {
            fee_rate: 0.0,
            slippage: 0.0,
            ..AllocationConfig::crypto()
        }
    }

    #[test]
    fn flat_weights_keep_cash() {
        let candles = bars(&[(100.0, 100.0), (100.0, 110.0), (110.0, 90.0)]);
        let r = run_allocation(&candles, &[0.0; 3], &no_costs(), day(0), day(2));
        assert_eq!(r.trades, 0);
        assert!(r.equity.iter().all(|p| (p.equity - 10_000.0).abs() < 1e-9));
    }

    #[test]
    fn full_weight_tracks_price_from_next_open() {
        let candles = bars(&[(100.0, 100.0), (100.0, 110.0), (110.0, 121.0)]);
        let r = run_allocation(&candles, &[1.0; 3], &no_costs(), day(0), day(2));
        assert_eq!(r.trades, 1);
        assert!((r.equity[0].equity - 10_000.0).abs() < 1e-9);
        assert!((r.equity[1].equity - 11_000.0).abs() < 1e-9);
        assert!((r.equity[2].equity - 12_100.0).abs() < 1e-9);
    }

    #[test]
    fn costs_are_paid_on_entry_and_exit() {
        let candles = bars(&[(100.0, 100.0), (100.0, 105.0), (110.0, 110.0)]);
        let r = run_allocation(
            &candles,
            &[1.0, 0.0, 0.0],
            &AllocationConfig::crypto(),
            day(0),
            day(2),
        );
        let units = 10_000.0 / (100.0 * 1.001 * 1.001);
        let expected = units * 110.0 * 0.999 * 0.999;
        assert_eq!(r.trades, 2);
        assert!((r.equity[2].equity - expected).abs() < 1e-6);
        assert_eq!(r.equity[2].positions, 0);
    }

    #[test]
    fn small_weight_changes_do_not_trade() {
        let candles = bars(&[(100.0, 100.0), (100.0, 100.0), (100.0, 100.0)]);
        let r = run_allocation(&candles, &[1.0, 0.9, 0.9], &no_costs(), day(0), day(2));
        assert_eq!(r.trades, 1);
    }

    #[test]
    fn stats_of_steady_growth() {
        let points: Vec<EquityPoint> = (1..=365)
            .map(|i| EquityPoint {
                date: day(u64::try_from(i).unwrap()),
                equity: 10_000.0 * 1.001_f64.powi(i),
                cash: 0.0,
                positions: 1,
            })
            .collect();
        let s = allocation_stats(&points, 10_000.0);
        assert!(s.max_drawdown.abs() < 1e-12);
        assert!((s.cagr - (1.001_f64.powi(365) - 1.0)).abs() < 1e-9);
        assert!((s.non_losing_year_fraction - 1.0).abs() < 1e-12);
    }
}
