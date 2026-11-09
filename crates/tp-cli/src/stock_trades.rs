//! Paper trades on US stocks with the two Stormer setups that beat the
//! any-day base on 84 US stocks and ETFs over ten years (`tp market-study
//! --universe`): the daily trap buy with the trend (8 and 80 day averages)
//! and the weekly trap buy. About 3.6 signals a week together.
//!
//! Every stage goes to Telegram and the phone. The paper book takes at most
//! one new trade a day (the daily setup first, then the weekly one, then the
//! list order) and holds up to three at once, each position at most a third
//! of the balance.

use std::collections::{BTreeMap, BTreeSet};

use tp_core::stormer::{Exit, Pattern, Side};

use crate::smc_live::{Books, TradeRow};
use crate::stormer_live::{Chart, Follow, Pick, Role, StageAlert, key, stage_alerts_with};

/// Paper book of the stock trades.
pub const BOOK: &str = "acoes";
pub const BOOK_LABEL: &str = "Ações EUA (trap)";
/// Sent stage keys, one per line.
pub const SENT_FILE: &str = "stock_sent.txt";
/// Open trades the book holds at most.
pub const MAX_OPEN: usize = 3;
/// Largest position as a share of the balance.
pub const POSITION_CAP: f64 = 1.0 / 3.0;
const DAY_MS: i64 = 24 * 3600 * 1000;
/// Marks a trade that went into the book, after its stage key.
const IN_BOOK: &str = "|conta";

/// The setups followed on US stocks.
#[must_use]
pub fn role(tf: &str, pattern: Pattern, side: Side, with_trend: bool) -> Option<Role> {
    match (tf, pattern, side) {
        ("1d", Pattern::Trap, Side::Long) if with_trend => Some(Role::Candidate),
        ("1w", Pattern::Trap, Side::Long) => Some(Role::Candidate),
        _ => None,
    }
}

/// One stock with its daily and weekly charts.
pub struct Stock<'a> {
    pub symbol: &'a str,
    pub name: &'a str,
    pub charts: &'a [Chart],
}

/// A triggered trade of a followed setup.
struct Candidate {
    key: String,
    entry_ms: i64,
    exit_ms: Option<i64>,
    /// Daily setup before weekly, then the list order.
    rank: (u8, usize),
}

fn candidates(stocks: &[Stock], start_ms: i64) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (n, s) in stocks.iter().enumerate() {
        for c in s.charts {
            for t in &c.trades {
                if role(c.tf, t.pattern, t.side, t.with_trend).is_none() {
                    continue;
                }
                let entry_ms = c.close_ms(t.entry_index);
                if entry_ms <= start_ms {
                    continue;
                }
                out.push(Candidate {
                    key: key(s.symbol, c, t.pattern, t.side, t.signal_index),
                    entry_ms,
                    exit_ms: (t.how != Exit::Open).then(|| c.close_ms(t.exit_index)),
                    rank: (u8::from(c.tf != "1d"), n),
                });
            }
        }
    }
    out
}

/// Which triggered trades go into the book. Trades already in it (closed
/// rows, or marked when they entered) stay; the others are decided in
/// order of entry day, at most one a day and three open at once.
fn picks(stocks: &[Stock], books: &Books, sent: &BTreeSet<String>) -> BTreeMap<String, Pick> {
    let start = books.starts.get(BOOK).copied().unwrap_or(i64::MAX);
    let mut all = candidates(stocks, start);
    all.sort_by_key(|c| (c.entry_ms.div_euclid(DAY_MS), c.rank, c.entry_ms));
    let recorded: BTreeMap<&str, &TradeRow> = books
        .trades
        .iter()
        .filter(|t| t.tf == BOOK)
        .map(|t| (t.key.as_str(), t))
        .collect();
    let marked: BTreeSet<&str> = sent
        .iter()
        .filter_map(|k| k.strip_suffix(IN_BOOK))
        .collect();
    // (entry day, entry, exit) of the trades in the book.
    let mut taken: Vec<(Option<i64>, i64, Option<i64>)> = Vec::new();
    let mut out = BTreeMap::new();
    for key in marked.iter().copied().chain(recorded.keys().copied()) {
        if out.contains_key(key) {
            continue;
        }
        let found = all.iter().find(|c| c.key == key);
        let slot = match (found, recorded.get(key)) {
            (_, Some(row)) => (
                Some(row.fill_ms.div_euclid(DAY_MS)),
                row.fill_ms,
                Some(row.exit_ms),
            ),
            (Some(c), None) => (Some(c.entry_ms.div_euclid(DAY_MS)), c.entry_ms, c.exit_ms),
            // Its stock did not load this time: still open as far as we know.
            (None, None) => (None, i64::MIN, None),
        };
        taken.push(slot);
        out.insert(key.to_owned(), Pick::In);
    }
    for c in &all {
        if out.contains_key(&c.key) {
            continue;
        }
        let day = c.entry_ms.div_euclid(DAY_MS);
        let pick = if taken.iter().any(|(d, _, _)| *d == Some(day)) {
            Pick::Out("já entrou outro trade de ação neste dia (a conta pega um por dia)")
        } else if taken
            .iter()
            .filter(|(_, entry, exit)| *entry < c.entry_ms && exit.is_none_or(|x| x > c.entry_ms))
            .count()
            >= MAX_OPEN
        {
            Pick::Out("a conta já tinha três trades abertos")
        } else {
            taken.push((Some(day), c.entry_ms, c.exit_ms));
            Pick::In
        };
        out.insert(c.key.clone(), pick);
    }
    out
}

/// Everything one run produces.
pub struct Run {
    /// Stock name and its new stage.
    pub stages: Vec<(String, StageAlert)>,
    /// Closed book trades, already added to `books`.
    pub rows: Vec<TradeRow>,
    /// New lines for [`SENT_FILE`]: stage keys and book marks.
    pub keys: Vec<String>,
}

/// New stages for every stock and the book trades that closed.
#[must_use]
pub fn run(stocks: &[Stock], now_ms: i64, sent: &BTreeSet<String>, books: &mut Books) -> Run {
    let chosen = picks(stocks, books, sent);
    let pick = |_: &Chart, _: &tp_core::stormer::Trade, key: &str| {
        chosen.get(key).copied().unwrap_or(Pick::None)
    };
    let follow = Follow {
        role_of: &role,
        book: BOOK,
        book_label: BOOK_LABEL,
        position_cap: POSITION_CAP,
        book_charts: &["1d", "1w"],
        pick: &pick,
    };
    let mut out = Run {
        stages: Vec::new(),
        rows: Vec::new(),
        keys: Vec::new(),
    };
    for s in stocks {
        let (stages, rows) =
            stage_alerts_with(s.symbol, s.name, s.charts, now_ms, sent, books, &follow);
        out.keys.extend(stages.iter().map(|a| a.key.clone()));
        out.stages
            .extend(stages.into_iter().map(|a| (s.name.to_owned(), a)));
        out.rows.extend(rows);
    }
    for (key, pick) in &chosen {
        let mark = format!("{key}{IN_BOOK}");
        if *pick == Pick::In && !sent.contains(&mark) {
            out.keys.push(mark);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tp_core::intraday::Bar;
    use tp_core::stormer::{Params, scan};

    fn bar(i: usize, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: i64::try_from(i).unwrap() * DAY_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    /// 120 rising days, a swing low on day 121, then day 130 trades below it
    /// and closes back above: a daily trap buy with the trend.
    fn trap(shift: f64) -> Vec<Bar> {
        let mut bars: Vec<Bar> = (0..130)
            .map(|i| {
                let p = shift
                    + if i < 120 {
                        f64::from(u32::try_from(i).unwrap())
                    } else {
                        120.0
                    };
                let low = if i == 121 { p - 5.0 } else { p - 2.0 };
                bar(i, p, p + 1.0, low, p)
            })
            .collect();
        let p = shift + 120.0;
        bars.push(bar(130, p - 1.0, p + 1.0, p - 6.0, p + 0.5));
        bars
    }

    fn chart(bars: Vec<Bar>) -> Vec<Chart> {
        let d = scan(&bars, Params::with_max_hold(30));
        vec![Chart {
            weekdays_only: true,
            ..Chart::round_the_clock("1d", DAY_MS, bars, d)
        }]
    }

    /// [`trap`] moved `k` days later, with the entry candle after it.
    fn entered(shift: f64, k: usize) -> Vec<Bar> {
        let mut bars = trap(shift);
        let p = shift + 120.0;
        bars.push(bar(131, p, p + 1.5, p - 2.0, p + 1.0));
        for b in &mut bars {
            b.open_time_ms += i64::try_from(k).unwrap() * DAY_MS;
        }
        bars
    }

    fn stock<'a>(symbol: &'a str, charts: &'a [Chart]) -> Stock<'a> {
        Stock {
            symbol,
            name: symbol,
            charts,
        }
    }

    fn books() -> Books {
        let mut b = Books::default();
        b.starts.insert(BOOK.to_owned(), 0);
        b
    }

    #[test]
    fn follows_only_the_trap_buys_that_beat_the_base() {
        assert_eq!(
            role("1d", Pattern::Trap, Side::Long, true),
            Some(Role::Candidate)
        );
        assert_eq!(role("1d", Pattern::Trap, Side::Long, false), None);
        assert_eq!(
            role("1w", Pattern::Trap, Side::Long, false),
            Some(Role::Candidate)
        );
        assert_eq!(role("1w", Pattern::Trap, Side::Short, true), None);
        assert_eq!(role("1d", Pattern::OneTwoThree, Side::Long, true), None);
    }

    #[test]
    fn signal_entry_and_one_book_trade_a_day() {
        let a = trap(0.0);
        let b = trap(1000.0);
        let now = a.last().unwrap().open_time_ms + DAY_MS + 60_000;
        let mut books = books();
        let (ca, cb) = (chart(a.clone()), chart(b.clone()));
        let first = run(
            &[stock("AAA", &ca), stock("BBB", &cb)],
            now,
            &BTreeSet::new(),
            &mut books,
        );
        let signals: Vec<&StageAlert> = first
            .stages
            .iter()
            .map(|(_, s)| s)
            .filter(|s| s.key.ends_with("|sinal"))
            .collect();
        assert_eq!(signals.len(), 2, "{:?}", first.keys);
        assert!(
            signals[0].message.contains("CANDIDATO"),
            "{}",
            signals[0].message
        );
        assert!(
            signals[0].message.contains("Ações EUA (trap)"),
            "{}",
            signals[0].message
        );
        let sent: BTreeSet<String> = first.keys.iter().cloned().collect();
        // Both trigger on the same day: only the first of the list goes in.
        let mut a2 = a;
        let mut b2 = b;
        a2.push(bar(131, 120.0, 121.5, 118.0, 121.0));
        b2.push(bar(131, 1120.0, 1121.5, 1118.0, 1121.0));
        let now = now + DAY_MS;
        let (ca, cb) = (chart(a2), chart(b2));
        let second = run(
            &[stock("AAA", &ca), stock("BBB", &cb)],
            now,
            &sent,
            &mut books,
        );
        let entries: Vec<&StageAlert> = second
            .stages
            .iter()
            .map(|(_, s)| s)
            .filter(|s| s.key.ends_with("|entrou"))
            .collect();
        assert_eq!(entries.len(), 2);
        assert!(
            entries[0].message.contains("posição de"),
            "{}",
            entries[0].message
        );
        assert!(
            entries[1].message.contains("um por dia"),
            "{}",
            entries[1].message
        );
        assert!(
            second
                .keys
                .iter()
                .any(|k| k.starts_with("AAA|") && k.ends_with("|conta"))
        );
        assert!(
            !second
                .keys
                .iter()
                .any(|k| k.starts_with("BBB|") && k.ends_with("|conta"))
        );
    }

    #[test]
    fn a_marked_trade_stays_in_the_book_when_its_stock_is_missing() {
        let mut books = books();
        let sent: BTreeSet<String> = ["ZZZ|1d|trap|long|0|conta".to_owned()].into();
        let a = trap(0.0);
        let mut a2 = a;
        a2.push(bar(131, 120.0, 121.5, 118.0, 121.0));
        let now = a2.last().unwrap().open_time_ms + DAY_MS + 60_000;
        let ca = chart(a2);
        let r = run(&[stock("AAA", &ca)], now, &sent, &mut books);
        // ZZZ still counts as open, but one open trade leaves room.
        let entry = r
            .stages
            .iter()
            .map(|(_, s)| s)
            .find(|s| s.key.ends_with("|entrou"))
            .unwrap();
        assert!(entry.message.contains("posição de"), "{}", entry.message);
    }

    #[test]
    fn the_book_holds_three_trades_at_most() {
        let charts: Vec<Vec<Chart>> = (0..4)
            .map(|k| chart(entered(100.0 * f64::from(u32::try_from(k).unwrap()), k)))
            .collect();
        let names = ["AAA", "BBB", "CCC", "DDD"];
        let stocks: Vec<Stock> = names
            .iter()
            .zip(&charts)
            .map(|(n, c)| stock(n, c))
            .collect();
        let now = 136 * DAY_MS;
        let r = run(&stocks, now, &BTreeSet::new(), &mut books());
        let marks: Vec<&String> = r.keys.iter().filter(|k| k.ends_with("|conta")).collect();
        assert_eq!(marks.len(), 3, "{marks:?}");
        let last = r
            .stages
            .iter()
            .find(|(n, s)| n == "DDD" && s.key.ends_with("|entrou"))
            .unwrap();
        assert!(
            last.1.message.contains("três trades abertos"),
            "{}",
            last.1.message
        );
    }
}
