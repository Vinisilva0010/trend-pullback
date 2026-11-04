//! Paper account that follows the daily report day by day.
//!
//! It trades exactly what the report measures: when an entry rose first more
//! often than chance, it buys with the whole account (spot, no leverage), aims
//! one daily ATR above the entry, stops one ATR below, and gives up after
//! seven days at the close. One position at a time.
//!
//! Event order inside a day: an open position is checked first (an open below
//! the stop exits at the open; a touched stop exits at the stop; an open above
//! the target exits at the open; a touched target exits at the target; when
//! both are touched the stop is assumed first; on the seventh day it exits at
//! the close). Then a limit order placed the day before fills if the low
//! reaches it, at the order price or the open if lower; on that day only the
//! stop is checked. Finally, the signals of the day's close are read: a
//! breakout buys at the close, a limit entry is placed for the next day. Every
//! buy and sell pays the spot fee.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::NaiveDate;

/// Binance spot fee per side (0.1%).
pub const SPOT_FEE: f64 = 0.001;
/// Days a position may stay open before it is closed at the close.
pub const MAX_DAYS: usize = 7;

/// Prices of one asset on one day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayBar {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// How a signal enters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Order {
    /// Buy at this day's close.
    AtClose,
    /// Place a limit buy at this price for the next day.
    Limit(f64),
}

/// An entry the report found with an edge at a day's close.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub asset: String,
    /// Short Portuguese name of the entry, for the journal.
    pub label: String,
    pub order: Order,
    /// Daily ATR at the signal; target and stop are one ATR away.
    pub atr: f64,
    /// Strength of the historical edge; the strongest signal wins.
    pub z: f64,
}

/// Open position.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub asset: String,
    pub label: String,
    pub entry_date: NaiveDate,
    pub entry_price: f64,
    pub quantity: f64,
    pub target: f64,
    pub stop: f64,
    /// Account cash spent on the entry, fee included.
    pub cost: f64,
    /// Days checked after the entry day.
    pub days: usize,
}

/// Limit order waiting for the next day.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub asset: String,
    pub label: String,
    pub price: f64,
    pub atr: f64,
    pub placed: NaiveDate,
}

/// Why a position closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Target,
    Stop,
    Time,
}

impl ExitReason {
    #[must_use]
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::Target => "alvo",
            Self::Stop => "stop",
            Self::Time => "7 dias sem alvo nem stop",
        }
    }
}

/// A finished trade.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub asset: String,
    pub label: String,
    pub entry_date: NaiveDate,
    pub entry_price: f64,
    pub exit_date: NaiveDate,
    pub exit_price: f64,
    pub quantity: f64,
    /// Profit in account currency, fees included.
    pub pnl: f64,
    pub reason: ExitReason,
}

/// Something that happened on a day.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Bought(Position),
    Placed(Pending),
    Expired(Pending),
    Sold(Trade),
}

/// Paper account state.
#[derive(Debug, Clone, PartialEq)]
pub struct Paper {
    pub start: NaiveDate,
    pub capital: f64,
    pub cash: f64,
    /// Last day already processed.
    pub last: NaiveDate,
    pub position: Option<Position>,
    pub pending: Option<Pending>,
}

/// Errors reading the saved paper state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaperError {
    #[error("paper state, line {line}: {reason}")]
    Parse { line: usize, reason: String },
    #[error("paper state: missing {0}")]
    Missing(&'static str),
}

impl Paper {
    /// New account that will process days after `last`.
    #[must_use]
    pub fn new(capital: f64, last: NaiveDate) -> Self {
        Self {
            start: last,
            capital,
            cash: capital,
            last,
            position: None,
            pending: None,
        }
    }

    /// Account value at the given close prices.
    #[must_use]
    pub fn equity(&self, closes: &BTreeMap<String, f64>) -> f64 {
        self.cash
            + self.position.as_ref().map_or(0.0, |p| {
                closes.get(&p.asset).copied().unwrap_or(p.entry_price)
                    * p.quantity
                    * (1.0 - SPOT_FEE)
            })
    }

    /// Processes one day: open position, yesterday's limit order, then the
    /// signals of today's close. Days at or before `last` are ignored.
    pub fn step(
        &mut self,
        date: NaiveDate,
        bars: &BTreeMap<String, DayBar>,
        signals: &[Signal],
    ) -> Vec<Event> {
        let mut events = Vec::new();
        if date <= self.last {
            return events;
        }
        self.last = date;
        self.check_position(date, bars, &mut events);
        self.fill_pending(date, bars, &mut events);
        if self.position.is_some() {
            return events;
        }
        let best = signals
            .iter()
            .filter(|s| s.atr > 0.0 && bars.contains_key(&s.asset))
            .max_by(|a, b| a.z.total_cmp(&b.z));
        if let Some(s) = best {
            match s.order {
                Order::AtClose => {
                    if let Some(bar) = bars.get(&s.asset) {
                        let p = self.open(date, &s.asset, &s.label, bar.close, s.atr);
                        events.push(Event::Bought(p));
                    }
                }
                Order::Limit(price) => {
                    let pending = Pending {
                        asset: s.asset.clone(),
                        label: s.label.clone(),
                        price,
                        atr: s.atr,
                        placed: date,
                    };
                    self.pending = Some(pending.clone());
                    events.push(Event::Placed(pending));
                }
            }
        }
        events
    }

    fn open(
        &mut self,
        date: NaiveDate,
        asset: &str,
        label: &str,
        price: f64,
        atr: f64,
    ) -> Position {
        let cost = self.cash;
        let position = Position {
            asset: asset.to_owned(),
            label: label.to_owned(),
            entry_date: date,
            entry_price: price,
            quantity: cost * (1.0 - SPOT_FEE) / price,
            target: price + atr,
            stop: price - atr,
            cost,
            days: 0,
        };
        self.cash = 0.0;
        self.position = Some(position.clone());
        position
    }

    fn close(&mut self, date: NaiveDate, price: f64, reason: ExitReason) -> Option<Trade> {
        let p = self.position.take()?;
        let proceeds = p.quantity * price * (1.0 - SPOT_FEE);
        self.cash += proceeds;
        Some(Trade {
            asset: p.asset,
            label: p.label,
            entry_date: p.entry_date,
            entry_price: p.entry_price,
            exit_date: date,
            exit_price: price,
            quantity: p.quantity,
            pnl: proceeds - p.cost,
            reason,
        })
    }

    fn check_position(
        &mut self,
        date: NaiveDate,
        bars: &BTreeMap<String, DayBar>,
        events: &mut Vec<Event>,
    ) {
        let Some(p) = self.position.as_mut() else {
            return;
        };
        let Some(bar) = bars.get(&p.asset).copied() else {
            return;
        };
        p.days += 1;
        let (stop, target, days) = (p.stop, p.target, p.days);
        let exit = if bar.open <= stop {
            Some((bar.open, ExitReason::Stop))
        } else if bar.low <= stop {
            Some((stop, ExitReason::Stop))
        } else if bar.open >= target {
            Some((bar.open, ExitReason::Target))
        } else if bar.high >= target {
            Some((target, ExitReason::Target))
        } else if days >= MAX_DAYS {
            Some((bar.close, ExitReason::Time))
        } else {
            None
        };
        if let Some((price, reason)) = exit
            && let Some(trade) = self.close(date, price, reason)
        {
            events.push(Event::Sold(trade));
        }
    }

    fn fill_pending(
        &mut self,
        date: NaiveDate,
        bars: &BTreeMap<String, DayBar>,
        events: &mut Vec<Event>,
    ) {
        let Some(order) = self.pending.take() else {
            return;
        };
        let bar = bars.get(&order.asset).copied();
        let filled = self.position.is_none() && bar.is_some_and(|b| b.low <= order.price);
        let Some(bar) = bar.filter(|_| filled) else {
            events.push(Event::Expired(order));
            return;
        };
        let price = order.price.min(bar.open);
        let position = self.open(date, &order.asset, &order.label, price, order.atr);
        events.push(Event::Bought(position.clone()));
        if bar.low <= position.stop
            && let Some(trade) = self.close(date, position.stop.min(bar.open), ExitReason::Stop)
        {
            events.push(Event::Sold(trade));
        }
    }

    /// Saves the state as simple comma-separated lines.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::from("# trend-pullback paper account\n");
        let _ = writeln!(out, "start,{},{}", self.start, self.capital);
        let _ = writeln!(out, "cash,{}", self.cash);
        let _ = writeln!(out, "last,{}", self.last);
        if let Some(p) = &self.position {
            let _ = writeln!(
                out,
                "position,{},{},{},{},{},{},{},{},{}",
                p.asset,
                p.label.replace(',', ";"),
                p.entry_date,
                p.entry_price,
                p.quantity,
                p.target,
                p.stop,
                p.cost,
                p.days
            );
        }
        if let Some(o) = &self.pending {
            let _ = writeln!(
                out,
                "pending,{},{},{},{},{}",
                o.asset,
                o.label.replace(',', ";"),
                o.price,
                o.atr,
                o.placed
            );
        }
        out
    }

    /// Reads the state written by [`Paper::to_text`].
    ///
    /// # Errors
    ///
    /// Returns [`PaperError`] when a line is malformed or a required line is missing.
    pub fn from_text(text: &str) -> Result<Self, PaperError> {
        let mut start = None;
        let mut cash = None;
        let mut last = None;
        let mut position = None;
        let mut pending = None;
        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = raw.split(',').collect();
            let bad = |reason: &str| PaperError::Parse {
                line,
                reason: reason.to_owned(),
            };
            let num = |s: &str| s.parse::<f64>().map_err(|_| bad("bad number"));
            let day = |s: &str| s.parse::<NaiveDate>().map_err(|_| bad("bad date"));
            match (f[0], f.len()) {
                ("start", 3) => start = Some((day(f[1])?, num(f[2])?)),
                ("cash", 2) => cash = Some(num(f[1])?),
                ("last", 2) => last = Some(day(f[1])?),
                ("position", 10) => {
                    position = Some(Position {
                        asset: f[1].to_owned(),
                        label: f[2].to_owned(),
                        entry_date: day(f[3])?,
                        entry_price: num(f[4])?,
                        quantity: num(f[5])?,
                        target: num(f[6])?,
                        stop: num(f[7])?,
                        cost: num(f[8])?,
                        days: f[9].parse().map_err(|_| bad("bad day count"))?,
                    });
                }
                ("pending", 6) => {
                    pending = Some(Pending {
                        asset: f[1].to_owned(),
                        label: f[2].to_owned(),
                        price: num(f[3])?,
                        atr: num(f[4])?,
                        placed: day(f[5])?,
                    });
                }
                _ => return Err(bad("unknown line")),
            }
        }
        let (start, capital) = start.ok_or(PaperError::Missing("start"))?;
        Ok(Self {
            start,
            capital,
            cash: cash.ok_or(PaperError::Missing("cash"))?,
            last: last.ok_or(PaperError::Missing("last"))?,
            position,
            pending,
        })
    }
}

/// CSV line of a finished trade for the journal file.
#[must_use]
pub fn trade_csv(t: &Trade) -> String {
    format!(
        "{},{},{},{},{},{},{},{:.2},{}",
        t.asset,
        t.label.replace(',', ";"),
        t.entry_date,
        t.entry_price,
        t.exit_date,
        t.exit_price,
        t.quantity,
        t.pnl,
        t.reason.label_pt()
    )
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn bars(open: f64, high: f64, low: f64, close: f64) -> BTreeMap<String, DayBar> {
        BTreeMap::from([(
            "SOLUSDT".to_owned(),
            DayBar {
                open,
                high,
                low,
                close,
            },
        )])
    }

    fn signal(order: Order, z: f64) -> Signal {
        Signal {
            asset: "SOLUSDT".to_owned(),
            label: "rompimento".to_owned(),
            order,
            atr: 4.0,
            z,
        }
    }

    #[test]
    fn breakout_buys_at_close_and_hits_target() {
        let mut p = Paper::new(1000.0, d(1));
        let e = p.step(
            d(2),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        assert!(
            matches!(&e[0], Event::Bought(pos) if pos.entry_price == 100.0 && pos.target == 104.0 && pos.stop == 96.0)
        );
        assert_eq!(p.cash, 0.0);
        let e = p.step(d(3), &bars(101.0, 105.0, 99.0, 103.0), &[]);
        let Event::Sold(t) = &e[0] else {
            panic!("expected a sale")
        };
        assert_eq!(t.reason, ExitReason::Target);
        assert_eq!(t.exit_price, 104.0);
        let expected = 1000.0 * 0.999 / 100.0 * 104.0 * 0.999;
        assert!((p.cash - expected).abs() < 1e-9);
        assert!((t.pnl - (expected - 1000.0)).abs() < 1e-9);
    }

    #[test]
    fn two_sided_day_counts_as_stop_and_gap_down_exits_at_open() {
        let mut p = Paper::new(1000.0, d(1));
        p.step(
            d(2),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        let e = p.step(d(3), &bars(100.0, 105.0, 95.0, 100.0), &[]);
        assert!(
            matches!(&e[0], Event::Sold(t) if t.reason == ExitReason::Stop && t.exit_price == 96.0)
        );
        p.step(
            d(4),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        let e = p.step(d(5), &bars(90.0, 92.0, 89.0, 91.0), &[]);
        assert!(matches!(&e[0], Event::Sold(t) if t.exit_price == 90.0));
    }

    #[test]
    fn position_closes_at_the_close_after_seven_days() {
        let mut p = Paper::new(1000.0, d(1));
        p.step(
            d(2),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        for day in 3..9 {
            assert!(
                p.step(d(day), &bars(100.0, 101.0, 99.0, 100.5), &[])
                    .is_empty()
            );
        }
        let e = p.step(d(9), &bars(100.0, 101.0, 99.0, 100.5), &[]);
        assert!(
            matches!(&e[0], Event::Sold(t) if t.reason == ExitReason::Time && t.exit_price == 100.5)
        );
    }

    #[test]
    fn limit_order_fills_next_day_or_expires() {
        let mut p = Paper::new(1000.0, d(1));
        let e = p.step(
            d(2),
            &bars(95.0, 96.0, 93.0, 94.0),
            &[signal(Order::Limit(90.0), 2.5)],
        );
        assert!(matches!(&e[0], Event::Placed(o) if o.price == 90.0));
        let e = p.step(d(3), &bars(93.0, 94.0, 91.0, 92.0), &[]);
        assert!(matches!(&e[0], Event::Expired(_)));
        assert!(p.position.is_none() && p.pending.is_none());
        p.step(
            d(4),
            &bars(95.0, 96.0, 93.0, 94.0),
            &[signal(Order::Limit(90.0), 2.5)],
        );
        // Opens below the limit: fills at the open.
        let e = p.step(d(5), &bars(89.0, 93.0, 88.0, 92.0), &[]);
        assert!(matches!(&e[0], Event::Bought(pos) if pos.entry_price == 89.0));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn limit_fill_day_can_only_stop_out() {
        let mut p = Paper::new(1000.0, d(1));
        p.step(
            d(2),
            &bars(95.0, 96.0, 93.0, 94.0),
            &[signal(Order::Limit(90.0), 2.5)],
        );
        let e = p.step(d(3), &bars(92.0, 99.0, 85.0, 88.0), &[]);
        assert_eq!(e.len(), 2);
        assert!(
            matches!(&e[1], Event::Sold(t) if t.reason == ExitReason::Stop && t.exit_price == 86.0)
        );
    }

    #[test]
    fn strongest_signal_wins_and_old_days_are_ignored() {
        let mut p = Paper::new(1000.0, d(5));
        assert!(
            p.step(
                d(5),
                &bars(98.0, 101.0, 97.0, 100.0),
                &[signal(Order::AtClose, 3.0)]
            )
            .is_empty()
        );
        let weak = Signal {
            label: "fraco".to_owned(),
            ..signal(Order::Limit(90.0), 2.1)
        };
        let e = p.step(
            d(6),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[weak, signal(Order::AtClose, 3.0)],
        );
        assert!(matches!(&e[0], Event::Bought(pos) if pos.label == "rompimento"));
    }

    #[test]
    fn state_round_trips_through_text() {
        let mut p = Paper::new(2200.0, d(4));
        p.step(
            d(5),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        let back = Paper::from_text(&p.to_text()).unwrap();
        assert_eq!(back, p);
        let mut q = Paper::new(2200.0, d(4));
        q.step(
            d(5),
            &bars(95.0, 96.0, 93.0, 94.0),
            &[signal(Order::Limit(90.0), 2.5)],
        );
        assert_eq!(Paper::from_text(&q.to_text()).unwrap(), q);
        assert!(Paper::from_text("cash,1\nlast,2026-11-01\n").is_err());
        assert!(Paper::from_text("start,2026-11-01,1\ncash,x\nlast,2026-11-01\n").is_err());
    }

    #[test]
    fn equity_marks_the_position_at_the_close() {
        let mut p = Paper::new(1000.0, d(1));
        p.step(
            d(2),
            &bars(98.0, 101.0, 97.0, 100.0),
            &[signal(Order::AtClose, 3.0)],
        );
        let closes = BTreeMap::from([("SOLUSDT".to_owned(), 102.0)]);
        let expected = 1000.0 * 0.999 / 100.0 * 102.0 * 0.999;
        assert!((p.equity(&closes) - expected).abs() < 1e-9);
    }
}
