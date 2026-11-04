//! Day-by-day portfolio simulation of the trend-pullback method.
//!
//! Event order inside each day, for every open position: an exit scheduled the
//! day before executes at the open; an open below the stop exits at the open;
//! an open above the target sells half at the open; a stop touched during the
//! day exits at the stop; a target touched during the day sells half and moves
//! the stop to the entry. When stop and target are both touched on the same
//! day, the stop is assumed to come first. On its entry day a position can only
//! be stopped out. Close-based exits (trailing low, time limit) execute at the
//! next open. Every buy and sell pays slippage and a fee.

pub mod allocation;
pub mod levels;
pub mod paper;
pub mod range;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::{Datelike, NaiveDate};
use tp_core::strategy::{Frame, Setup, StrategyParams, setups_on};

pub(crate) fn as_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Portfolio, sizing and cost settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BacktestConfig {
    pub initial_capital: f64,
    /// Fraction of equity lost if the initial stop is hit.
    pub risk_per_trade: f64,
    /// Maximum fraction of equity in a single position.
    pub max_position_fraction: f64,
    pub max_positions: usize,
    /// Fee per side, as a fraction of the traded value.
    pub fee_rate: f64,
    /// Price penalty per side, as a fraction of the price.
    pub slippage: f64,
    /// Bars after which a position exits at the next open.
    pub max_hold_days: usize,
    /// Smallest position value worth opening.
    pub min_notional: f64,
    /// Sell half at the target and move the stop to the entry.
    pub take_partial: bool,
}

impl BacktestConfig {
    /// Phase 1 settings for crypto.
    #[must_use]
    pub fn crypto() -> Self {
        Self {
            initial_capital: 10_000.0,
            risk_per_trade: 0.02,
            max_position_fraction: 0.30,
            max_positions: 5,
            fee_rate: 0.001,
            slippage: 0.001,
            max_hold_days: 14,
            min_notional: 10.0,
            take_partial: true,
        }
    }
}

/// Why a position was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Stop,
    Breakeven,
    Trail,
    Time,
    Delisted,
    EndOfData,
}

impl ExitReason {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Breakeven => "breakeven",
            Self::Trail => "trail",
            Self::Time => "time",
            Self::Delisted => "delisted",
            Self::EndOfData => "end_of_data",
        }
    }
}

/// One completed trade.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub symbol: String,
    pub signal_date: NaiveDate,
    pub entry_date: NaiveDate,
    pub exit_date: NaiveDate,
    /// Entry price including slippage.
    pub entry_price: f64,
    pub initial_stop: f64,
    pub target: f64,
    pub quantity: f64,
    pub partial_taken: bool,
    /// Net result after fees and slippage.
    pub pnl: f64,
    /// Result in multiples of the initial risk.
    pub r_multiple: f64,
    pub reason: ExitReason,
    /// Close of the signal day.
    pub signal_close: f64,
    /// Limit price of the buy order.
    pub limit: f64,
}

/// Account value at the close of one day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EquityPoint {
    pub date: NaiveDate,
    pub equity: f64,
    pub cash: f64,
    pub positions: usize,
}

#[derive(Debug, Clone)]
struct Position {
    symbol: String,
    signal_date: NaiveDate,
    entry_date: NaiveDate,
    fill_price: f64,
    entry_price: f64,
    quantity: f64,
    initial_quantity: f64,
    stop: f64,
    initial_stop: f64,
    target: f64,
    partial_taken: bool,
    bars_held: usize,
    exit_next_open: Option<ExitReason>,
    cash_flow: f64,
    last_close: f64,
    signal_close: f64,
    limit: f64,
}

fn stop_reason(position: &Position) -> ExitReason {
    if position.partial_taken {
        ExitReason::Breakeven
    } else {
        ExitReason::Stop
    }
}

/// The simulated account.
#[derive(Debug, Clone)]
pub struct Engine {
    config: BacktestConfig,
    cash: f64,
    positions: Vec<Position>,
    orders: Vec<Setup>,
    trades: Vec<Trade>,
    equity: Vec<EquityPoint>,
    last_equity: f64,
}

impl Engine {
    #[must_use]
    pub fn new(config: BacktestConfig) -> Self {
        Self {
            config,
            cash: config.initial_capital,
            positions: Vec::new(),
            orders: Vec::new(),
            trades: Vec::new(),
            equity: Vec::new(),
            last_equity: config.initial_capital,
        }
    }

    #[must_use]
    pub fn trades(&self) -> &[Trade] {
        &self.trades
    }

    #[must_use]
    pub fn equity_curve(&self) -> &[EquityPoint] {
        &self.equity
    }

    #[must_use]
    pub fn cash(&self) -> f64 {
        self.cash
    }

    #[must_use]
    pub fn open_positions(&self) -> usize {
        self.positions.len()
    }

    #[must_use]
    pub fn holds(&self, symbol: &str) -> bool {
        self.positions.iter().any(|p| p.symbol == symbol)
    }

    /// Buy orders for the next day, in priority order. Symbols already held are dropped.
    pub fn set_orders(&mut self, orders: Vec<Setup>) {
        let kept: Vec<Setup> = orders
            .into_iter()
            .filter(|order| !self.holds(&order.symbol))
            .collect();
        self.orders = kept;
    }

    /// Processes one trading day: manages open positions, fills pending
    /// orders, and records the closing equity.
    pub fn step(&mut self, date: NaiveDate, frames: &BTreeMap<String, Frame>) {
        let mut kept = Vec::with_capacity(self.positions.len());
        for mut position in std::mem::take(&mut self.positions) {
            if !self.update_position(&mut position, date, frames, false) {
                kept.push(position);
            }
        }
        self.positions = kept;
        let existing = self.positions.len();
        self.fill_orders(date, frames);
        let new_positions = self.positions.split_off(existing);
        for mut position in new_positions {
            if !self.update_position(&mut position, date, frames, true) {
                self.positions.push(position);
            }
        }
        self.mark(date);
    }

    /// Closes every position at its last known close.
    pub fn finish(&mut self, date: NaiveDate) {
        for mut position in std::mem::take(&mut self.positions) {
            let price = position.last_close;
            self.close_all(&mut position, date, price, ExitReason::EndOfData);
        }
        self.orders.clear();
        self.mark(date);
    }

    fn fill_orders(&mut self, date: NaiveDate, frames: &BTreeMap<String, Frame>) {
        for order in std::mem::take(&mut self.orders) {
            if self.positions.len() >= self.config.max_positions {
                break;
            }
            if self.holds(&order.symbol) {
                continue;
            }
            let Some(frame) = frames.get(&order.symbol) else {
                continue;
            };
            let Some(i) = frame.index_of(date) else {
                continue;
            };
            let candle = frame.candles[i];
            if candle.low() > order.limit {
                continue;
            }
            let fill = candle.open().min(order.limit);
            if fill <= order.stop {
                continue;
            }
            self.open_position(&order, date, fill);
        }
    }

    fn open_position(&mut self, order: &Setup, date: NaiveDate, fill: f64) {
        let c = self.config;
        let entry = fill * (1.0 + c.slippage);
        let risk_per_unit = entry - order.stop;
        if risk_per_unit <= 0.0 {
            return;
        }
        let equity = self.last_equity;
        let quantity = (c.risk_per_trade * equity / risk_per_unit)
            .min(c.max_position_fraction * equity / entry)
            .min(self.cash / (entry * (1.0 + c.fee_rate)));
        let value = quantity * entry;
        if quantity <= 0.0 || value < c.min_notional {
            return;
        }
        let outlay = value + value * c.fee_rate;
        self.cash -= outlay;
        self.positions.push(Position {
            symbol: order.symbol.clone(),
            signal_date: order.date,
            entry_date: date,
            fill_price: fill,
            entry_price: entry,
            quantity,
            initial_quantity: quantity,
            stop: order.stop,
            initial_stop: order.stop,
            target: order.target,
            partial_taken: false,
            bars_held: 0,
            exit_next_open: None,
            cash_flow: -outlay,
            last_close: fill,
            signal_close: order.close,
            limit: order.limit,
        });
    }

    /// Returns true when the position was closed.
    fn update_position(
        &mut self,
        p: &mut Position,
        date: NaiveDate,
        frames: &BTreeMap<String, Frame>,
        entry_day: bool,
    ) -> bool {
        let Some(frame) = frames.get(&p.symbol) else {
            let price = p.last_close;
            self.close_all(p, date, price, ExitReason::Delisted);
            return true;
        };
        let Some(i) = frame.index_of(date) else {
            if frame.candles.last().is_none_or(|c| c.date() < date) {
                let price = p.last_close;
                self.close_all(p, date, price, ExitReason::Delisted);
                return true;
            }
            return false;
        };
        let candle = frame.candles[i];
        if !entry_day {
            if let Some(reason) = p.exit_next_open.take() {
                self.close_all(p, date, candle.open(), reason);
                return true;
            }
            if candle.open() <= p.stop {
                let reason = stop_reason(p);
                self.close_all(p, date, candle.open(), reason);
                return true;
            }
            if self.config.take_partial && !p.partial_taken && candle.open() >= p.target {
                self.take_partial(p, candle.open());
            }
        }
        if candle.low() <= p.stop {
            let (reason, price) = (stop_reason(p), p.stop);
            self.close_all(p, date, price, reason);
            return true;
        }
        if self.config.take_partial && !entry_day && !p.partial_taken && candle.high() >= p.target {
            let target = p.target;
            self.take_partial(p, target);
            if candle.low() <= p.stop {
                let price = p.stop;
                self.close_all(p, date, price, ExitReason::Breakeven);
                return true;
            }
        }
        p.last_close = candle.close();
        p.bars_held += 1;
        let trail_low = i
            .checked_sub(1)
            .and_then(|j| frame.lowest_low[j])
            .map(|(low, _)| low);
        let trailing = p.partial_taken || !self.config.take_partial;
        if trailing && trail_low.is_some_and(|low| candle.close() < low) {
            p.exit_next_open = Some(ExitReason::Trail);
        } else if p.bars_held >= self.config.max_hold_days {
            p.exit_next_open = Some(ExitReason::Time);
        }
        false
    }

    fn take_partial(&mut self, p: &mut Position, price: f64) {
        let half = p.quantity / 2.0;
        self.sell(p, half, price);
        p.partial_taken = true;
        p.stop = p.stop.max(p.fill_price);
    }

    fn sell(&mut self, p: &mut Position, quantity: f64, price: f64) {
        let value = quantity * price * (1.0 - self.config.slippage);
        let net = value - value * self.config.fee_rate;
        self.cash += net;
        p.cash_flow += net;
        p.quantity -= quantity;
    }

    fn close_all(&mut self, p: &mut Position, date: NaiveDate, price: f64, reason: ExitReason) {
        if p.quantity > 0.0 {
            let quantity = p.quantity;
            self.sell(p, quantity, price);
        }
        let initial_risk = p.initial_quantity * (p.entry_price - p.initial_stop);
        self.trades.push(Trade {
            symbol: p.symbol.clone(),
            signal_date: p.signal_date,
            entry_date: p.entry_date,
            exit_date: date,
            entry_price: p.entry_price,
            initial_stop: p.initial_stop,
            target: p.target,
            quantity: p.initial_quantity,
            partial_taken: p.partial_taken,
            pnl: p.cash_flow,
            r_multiple: p.cash_flow / initial_risk,
            reason,
            signal_close: p.signal_close,
            limit: p.limit,
        });
    }

    fn mark(&mut self, date: NaiveDate) {
        let holdings: f64 = self
            .positions
            .iter()
            .map(|p| p.quantity * p.last_close)
            .sum();
        self.last_equity = self.cash + holdings;
        self.equity.push(EquityPoint {
            date,
            equity: self.last_equity,
            cash: self.cash,
            positions: self.positions.len(),
        });
    }
}

/// Everything the simulation reads: the benchmark (also the calendar), the
/// frames of every symbol and the members of each `(year, month)`.
#[derive(Debug, Clone, Copy)]
pub struct Market<'a> {
    pub benchmark: &'a Frame,
    pub frames: &'a BTreeMap<String, Frame>,
    pub universe: &'a BTreeMap<(i32, u32), Vec<String>>,
}

/// Result of a full run.
#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub trades: Vec<Trade>,
    pub equity: Vec<EquityPoint>,
    pub final_cash: f64,
}

/// Runs the method from `start` to `end` (inclusive) on the benchmark's days.
#[must_use]
pub fn run(
    market: &Market<'_>,
    params: &StrategyParams,
    config: &BacktestConfig,
    start: NaiveDate,
    end: NaiveDate,
) -> BacktestResult {
    let mut engine = Engine::new(*config);
    let mut last = None;
    for candle in &market.benchmark.candles {
        let date = candle.date();
        if date < start || date > end {
            continue;
        }
        engine.step(date, market.frames);
        let members = market
            .universe
            .get(&(date.year(), date.month()))
            .map_or(&[][..], Vec::as_slice);
        engine.set_orders(setups_on(
            date,
            market.benchmark,
            market.frames,
            members,
            params,
        ));
        last = Some(date);
    }
    if let Some(date) = last {
        engine.finish(date);
    }
    BacktestResult {
        trades: engine.trades,
        equity: engine.equity,
        final_cash: engine.cash,
    }
}

/// Headline numbers of a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub trades: usize,
    pub win_rate: f64,
    pub average_r: f64,
    pub profit_factor: f64,
    pub max_drawdown: f64,
    pub final_equity: f64,
    pub total_return: f64,
}

/// Computes the headline numbers.
#[must_use]
pub fn summarize(result: &BacktestResult, initial_capital: f64) -> Summary {
    let n = result.trades.len();
    let wins = result.trades.iter().filter(|t| t.pnl > 0.0).count();
    let gains: f64 = result.trades.iter().map(|t| t.pnl.max(0.0)).sum();
    let losses: f64 = result.trades.iter().map(|t| (-t.pnl).max(0.0)).sum();
    let r_sum: f64 = result.trades.iter().map(|t| t.r_multiple).sum();
    let mut peak = initial_capital;
    let mut max_drawdown: f64 = 0.0;
    for point in &result.equity {
        peak = peak.max(point.equity);
        max_drawdown = max_drawdown.max((peak - point.equity) / peak);
    }
    let final_equity = result.equity.last().map_or(initial_capital, |p| p.equity);
    let per_trade = |total: f64| if n == 0 { 0.0 } else { total / as_f64(n) };
    Summary {
        trades: n,
        win_rate: per_trade(as_f64(wins)),
        average_r: per_trade(r_sum),
        profit_factor: if losses > 0.0 {
            gains / losses
        } else {
            f64::INFINITY
        },
        max_drawdown,
        final_equity,
        total_return: final_equity / initial_capital - 1.0,
    }
}

/// Trades as CSV.
#[must_use]
pub fn trades_csv(trades: &[Trade]) -> String {
    let mut out = String::from(
        "symbol,signal_date,entry_date,exit_date,entry_price,initial_stop,target,quantity,partial_taken,pnl,r_multiple,reason,signal_close,limit\n",
    );
    for t in trades {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{:.4},{:.4},{},{},{}",
            t.symbol,
            t.signal_date,
            t.entry_date,
            t.exit_date,
            t.entry_price,
            t.initial_stop,
            t.target,
            t.quantity,
            t.partial_taken,
            t.pnl,
            t.r_multiple,
            t.reason.label(),
            t.signal_close,
            t.limit
        );
    }
    out
}

/// Daily equity as CSV.
#[must_use]
pub fn equity_csv(points: &[EquityPoint]) -> String {
    let mut out = String::from("date,equity,cash,positions\n");
    for p in points {
        let _ = writeln!(
            out,
            "{},{:.4},{:.4},{}",
            p.date, p.equity, p.cash, p.positions
        );
    }
    out
}

/// Pre-registered acceptance criteria (development period, after costs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Criteria {
    pub min_trades: usize,
    pub min_average_r: f64,
    pub min_profit_factor: f64,
    pub max_drawdown: f64,
    /// Minimum fraction of calendar years without a loss.
    pub min_non_losing_years: f64,
}

impl Default for Criteria {
    fn default() -> Self {
        Self {
            min_trades: 100,
            min_average_r: 0.15,
            min_profit_factor: 1.3,
            max_drawdown: 0.35,
            min_non_losing_years: 0.6,
        }
    }
}

/// Headline numbers plus the robustness checks and the verdict.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Evaluation {
    pub summary: Summary,
    pub non_losing_year_fraction: f64,
    pub r_total: f64,
    pub r_without_top3: f64,
    pub r_without_best_symbol: f64,
    pub passes: bool,
}

/// Return of each calendar year, from the equity curve.
#[must_use]
pub fn yearly_returns(equity: &[EquityPoint], initial_capital: f64) -> Vec<(i32, f64)> {
    let mut out = Vec::new();
    let mut year_start = initial_capital;
    let mut last = initial_capital;
    let mut current: Option<i32> = None;
    for point in equity {
        let year = point.date.year();
        if current != Some(year) {
            if let Some(previous) = current {
                out.push((previous, last / year_start - 1.0));
                year_start = last;
            }
            current = Some(year);
        }
        last = point.equity;
    }
    if let Some(year) = current {
        out.push((year, last / year_start - 1.0));
    }
    out
}

/// Applies the acceptance criteria to a run.
#[must_use]
pub fn evaluate(result: &BacktestResult, initial_capital: f64, criteria: &Criteria) -> Evaluation {
    let summary = summarize(result, initial_capital);
    let years = yearly_returns(&result.equity, initial_capital);
    let non_losing = years.iter().filter(|(_, r)| *r >= 0.0).count();
    let non_losing_year_fraction = if years.is_empty() {
        0.0
    } else {
        as_f64(non_losing) / as_f64(years.len())
    };
    let mut rs: Vec<f64> = result.trades.iter().map(|t| t.r_multiple).collect();
    let r_total: f64 = rs.iter().sum();
    rs.sort_by(|a, b| b.total_cmp(a));
    let r_without_top3: f64 = rs.iter().skip(3).sum();
    let mut by_symbol: BTreeMap<&str, f64> = BTreeMap::new();
    for t in &result.trades {
        *by_symbol.entry(t.symbol.as_str()).or_default() += t.r_multiple;
    }
    let best_symbol = by_symbol.values().copied().fold(0.0_f64, f64::max);
    let r_without_best_symbol = r_total - best_symbol;
    let passes = summary.trades >= criteria.min_trades
        && summary.average_r >= criteria.min_average_r
        && summary.profit_factor >= criteria.min_profit_factor
        && summary.max_drawdown <= criteria.max_drawdown
        && non_losing_year_fraction >= criteria.min_non_losing_years
        && r_without_top3 > 0.0
        && r_without_best_symbol > 0.0;
    Evaluation {
        summary,
        non_losing_year_fraction,
        r_total,
        r_without_top3,
        r_without_best_symbol,
        passes,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use chrono::Days;
    use tp_core::market::{Candle, CandleFields};

    fn day(offset: usize) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .checked_add_days(Days::new(u64::try_from(offset).unwrap()))
            .unwrap()
    }

    fn params() -> StrategyParams {
        StrategyParams {
            stop_low_lookback: 3,
            ..StrategyParams::crypto()
        }
    }

    /// Bars are (open, high, low, close), one per day from day 0.
    fn frame(symbol: &str, bars: &[(f64, f64, f64, f64)]) -> Frame {
        let candles = bars
            .iter()
            .enumerate()
            .map(|(i, &(open, high, low, close))| {
                Candle::try_new(CandleFields {
                    date: day(i),
                    open,
                    high,
                    low,
                    close,
                    volume: 1.0,
                    quote_volume: 1.0,
                })
                .unwrap()
            })
            .collect();
        Frame::new(symbol, candles, &params())
    }

    fn order(symbol: &str, limit: f64, stop: f64, target: f64) -> Setup {
        Setup {
            symbol: symbol.to_owned(),
            date: day(0),
            close: limit,
            limit,
            stop,
            target,
            atr: 1.0,
            momentum: 0.0,
        }
    }

    fn no_costs() -> BacktestConfig {
        BacktestConfig {
            fee_rate: 0.0,
            slippage: 0.0,
            ..BacktestConfig::crypto()
        }
    }

    fn frames(list: Vec<Frame>) -> BTreeMap<String, Frame> {
        list.into_iter().map(|f| (f.symbol.clone(), f)).collect()
    }

    /// Steps day 0, places `orders` for day 1, then steps days 1..days.
    fn simulate(
        config: BacktestConfig,
        f: &BTreeMap<String, Frame>,
        orders: Vec<Setup>,
        days: usize,
    ) -> Engine {
        let mut engine = Engine::new(config);
        engine.step(day(0), f);
        engine.set_orders(orders);
        for d in 1..days {
            engine.step(day(d), f);
        }
        engine
    }

    const START: (f64, f64, f64, f64) = (100.0, 101.0, 99.0, 100.0);

    fn trade(symbol: &str, r: f64) -> Trade {
        Trade {
            symbol: symbol.to_owned(),
            signal_date: day(0),
            entry_date: day(1),
            exit_date: day(2),
            entry_price: 100.0,
            initial_stop: 90.0,
            target: 120.0,
            quantity: 10.0,
            partial_taken: false,
            pnl: r * 100.0,
            r_multiple: r,
            reason: ExitReason::Time,
            signal_close: 101.0,
            limit: 100.0,
        }
    }

    #[test]
    fn evaluation_applies_robustness_checks() {
        let point = |y: i32, equity: f64| EquityPoint {
            date: NaiveDate::from_ymd_opt(y, 12, 31).unwrap(),
            equity,
            cash: equity,
            positions: 0,
        };
        let result = BacktestResult {
            trades: vec![
                trade("A", 3.0),
                trade("A", 1.0),
                trade("B", 1.0),
                trade("B", -1.0),
                trade("C", -1.0),
                trade("C", 0.5),
            ],
            equity: vec![point(2024, 10_500.0), point(2025, 10_200.0)],
            final_cash: 10_200.0,
        };
        let e = evaluate(&result, 10_000.0, &Criteria::default());
        assert!((e.r_total - 3.5).abs() < 1e-12);
        assert!((e.r_without_top3 + 1.5).abs() < 1e-12);
        assert!((e.r_without_best_symbol + 0.5).abs() < 1e-12);
        assert!((e.non_losing_year_fraction - 0.5).abs() < 1e-12);
        assert!(!e.passes);
        let years = yearly_returns(&result.equity, 10_000.0);
        assert!((years[0].1 - 0.05).abs() < 1e-12);
        assert!((years[1].1 - (10_200.0 / 10_500.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn without_partial_the_whole_position_trails_from_entry() {
        let f = frames(vec![frame(
            "A",
            &[
                START,
                (100.0, 101.0, 99.0, 100.0),
                (113.0, 121.0, 112.0, 118.0),
                (118.0, 119.0, 115.0, 116.0),
                (116.0, 117.0, 113.0, 114.0),
                (114.0, 115.0, 108.0, 109.0),
                (110.0, 111.0, 109.0, 110.0),
            ],
        )]);
        let config = BacktestConfig {
            take_partial: false,
            ..no_costs()
        };
        let engine = simulate(config, &f, vec![order("A", 100.0, 90.0, 120.0)], 7);
        let t = &engine.trades()[0];
        assert!(!t.partial_taken);
        assert_eq!(t.reason, ExitReason::Trail);
        assert!((t.pnl - 200.0).abs() < 1e-9);
        assert!((t.r_multiple - 1.0).abs() < 1e-9);
    }

    #[test]
    fn stop_loss_costs_one_r() {
        let f = frames(vec![frame(
            "A",
            &[START, (100.0, 102.0, 95.0, 97.0), (97.0, 98.0, 85.0, 86.0)],
        )]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 3);
        let t = &engine.trades()[0];
        assert_eq!(t.quantity, 20.0);
        assert!((t.pnl + 200.0).abs() < 1e-9);
        assert!((t.r_multiple + 1.0).abs() < 1e-9);
        assert_eq!(t.reason, ExitReason::Stop);
    }

    #[test]
    fn gap_below_stop_exits_at_open() {
        let f = frames(vec![frame(
            "A",
            &[START, (100.0, 102.0, 95.0, 97.0), (80.0, 82.0, 78.0, 79.0)],
        )]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 3);
        let t = &engine.trades()[0];
        assert!((t.pnl + 400.0).abs() < 1e-9);
        assert!((t.r_multiple + 2.0).abs() < 1e-9);
    }

    #[test]
    fn same_day_stop_and_target_counts_the_stop() {
        let f = frames(vec![frame(
            "A",
            &[
                START,
                (100.0, 102.0, 95.0, 97.0),
                (100.0, 125.0, 85.0, 100.0),
            ],
        )]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 3);
        assert!((engine.trades()[0].pnl + 200.0).abs() < 1e-9);
    }

    #[test]
    fn target_then_trailing_exit() {
        let f = frames(vec![frame(
            "A",
            &[
                START,
                (100.0, 101.0, 99.0, 100.0),
                (113.0, 121.0, 112.0, 118.0),
                (118.0, 119.0, 115.0, 116.0),
                (116.0, 117.0, 113.0, 114.0),
                (114.0, 115.0, 108.0, 109.0),
                (110.0, 111.0, 109.0, 110.0),
            ],
        )]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 7);
        let t = &engine.trades()[0];
        assert!(t.partial_taken);
        assert_eq!(t.reason, ExitReason::Trail);
        assert_eq!(t.exit_date, day(6));
        assert!(
            (t.pnl - 300.0).abs() < 1e-9,
            "10 x 20 at target + 10 x 10 at the open"
        );
        assert!((t.r_multiple - 1.5).abs() < 1e-9);
    }

    #[test]
    fn open_below_limit_fills_at_open() {
        let f = frames(vec![frame(
            "A",
            &[START, (97.0, 99.0, 96.0, 98.0), (98.0, 99.0, 97.0, 98.0)],
        )]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 3);
        assert_eq!(engine.open_positions(), 1);
        engine.clone().finish(day(2));
        let mut done = engine;
        done.finish(day(2));
        assert_eq!(done.trades()[0].entry_price, 97.0);
        assert!((done.trades()[0].quantity - 200.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn costs_are_charged_on_both_sides() {
        let f = frames(vec![frame(
            "A",
            &[START, (100.0, 102.0, 95.0, 97.0), (97.0, 98.0, 85.0, 86.0)],
        )]);
        let config = BacktestConfig::crypto();
        let engine = simulate(config, &f, vec![order("A", 100.0, 90.0, 120.0)], 3);
        let t = &engine.trades()[0];
        let entry = 100.0 * 1.001;
        let quantity = 200.0 / (entry - 90.0);
        let paid = quantity * entry * 1.001;
        let received = quantity * 90.0 * 0.999 * 0.999;
        assert!((t.quantity - quantity).abs() < 1e-9);
        assert!((t.pnl - (received - paid)).abs() < 1e-9);
        assert!((engine.cash() - (10_000.0 + received - paid)).abs() < 1e-9);
    }

    #[test]
    fn position_limits_are_respected() {
        let symbols = ["A", "B", "C", "D", "E", "F"];
        let list = symbols
            .iter()
            .map(|s| frame(s, &[START, (100.0, 101.0, 99.0, 100.0)]))
            .collect();
        let f = frames(list);
        let orders = symbols
            .iter()
            .map(|s| order(s, 100.0, 95.0, 120.0))
            .collect();
        let config = BacktestConfig {
            max_position_fraction: 0.15,
            ..no_costs()
        };
        let engine = simulate(config, &f, orders, 2);
        assert_eq!(engine.open_positions(), 5);
        let point = engine.equity_curve().last().unwrap();
        assert!(point.cash >= 0.0);
        assert!((point.equity - 10_000.0).abs() < 1e-9);
    }

    #[test]
    fn time_limit_exits_at_next_open() {
        let f = frames(vec![frame(
            "A",
            &[
                START,
                (100.0, 101.0, 99.0, 100.0),
                (100.0, 101.0, 99.0, 100.0),
                (101.0, 102.0, 100.0, 101.0),
            ],
        )]);
        let config = BacktestConfig {
            max_hold_days: 2,
            ..no_costs()
        };
        let engine = simulate(config, &f, vec![order("A", 100.0, 90.0, 120.0)], 4);
        let t = &engine.trades()[0];
        assert_eq!(t.reason, ExitReason::Time);
        assert_eq!(t.exit_date, day(3));
        assert!((t.pnl - 20.0).abs() < 1e-9);
    }

    #[test]
    fn delisted_symbol_closes_at_last_close() {
        let a = frame(
            "A",
            &[
                START,
                (100.0, 101.0, 99.0, 100.0),
                (100.0, 104.0, 99.0, 103.0),
            ],
        );
        let bench = frame("BENCH", &[START, START, START, START]);
        let f = frames(vec![a, bench]);
        let engine = simulate(no_costs(), &f, vec![order("A", 100.0, 90.0, 120.0)], 4);
        let t = &engine.trades()[0];
        assert_eq!(t.reason, ExitReason::Delisted);
        assert!((t.pnl - 60.0).abs() < 1e-9);
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
                let open = price;
                price *= 1.0 + 0.002 + (unit - 0.5) * 0.06;
                Candle::try_new(CandleFields {
                    date: day(i),
                    open,
                    high: open.max(price) * 1.01,
                    low: open.min(price) * 0.99,
                    close: price,
                    volume: 1.0,
                    quote_volume: 1.0,
                })
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn accounting_reconciles_to_the_cent() {
        let p = StrategyParams {
            sma_fast: 10,
            sma_slow: 30,
            ema_entry: 5,
            atr_period: 5,
            momentum_days: 10,
            momentum_top_fraction: 1.0,
            min_pullback_atr: 0.5,
            ..StrategyParams::crypto()
        };
        let mut f = BTreeMap::new();
        for seed in 1..=6 {
            let name = format!("S{seed}USDT");
            f.insert(name.clone(), Frame::new(&name, random_walk(seed, 400), &p));
        }
        let benchmark = f["S1USDT"].clone();
        let members: Vec<String> = f.keys().cloned().collect();
        let mut universe = BTreeMap::new();
        for c in &benchmark.candles {
            universe.insert((c.date().year(), c.date().month()), members.clone());
        }
        let market = Market {
            benchmark: &benchmark,
            frames: &f,
            universe: &universe,
        };
        let config = BacktestConfig::crypto();
        let result = run(&market, &p, &config, day(0), day(399));
        assert!(
            !result.trades.is_empty(),
            "the scenario must trade to mean anything"
        );
        let pnl: f64 = result.trades.iter().map(|t| t.pnl).sum();
        assert!((result.final_cash - (config.initial_capital + pnl)).abs() < 1e-6);
        let last = result.equity.last().unwrap();
        assert_eq!(last.positions, 0);
        assert!((last.equity - result.final_cash).abs() < 1e-9);
        assert!(result.equity.iter().all(|e| e.cash >= -1e-9));
    }
}
