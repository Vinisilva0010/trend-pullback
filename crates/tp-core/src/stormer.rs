//! Swing-trade setups in the style taught by Alexandre Wolwacz (Stormer),
//! as fixed rules so they can be tested on history:
//!
//! - every entry is a stop order one tick above the high of the signal
//!   candle (below the low for shorts), valid only on the next candle;
//! - the stop is below the low that defines the pattern;
//! - the target is twice the risk, except "candle fechado", whose target is
//!   the open of the wide bar it tries to recover;
//! - the trend filter is the one of his published 1-2-3: 8 and 80 period
//!   exponential averages (fast above slow and close above fast for longs).
//!
//! Shorts are the same rules on a mirrored price series.

use crate::intraday::Bar;

/// One millisecond day.
const DAY_MS: i64 = 24 * 3600 * 1000;

/// The patterns tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pattern {
    /// Three candles, the middle one with the lowest low; trigger above the third.
    OneTwoThree,
    /// The same 1-2-3 when the third candle is inside the second.
    OneTwoThreeInside,
    /// A candle that trades below the last confirmed swing low and closes back
    /// above it (a failed breakdown, the liquidity sweep of SMC).
    Trap,
    /// A candle inside the previous one.
    InsideBar,
    /// A wide candle against the trend, a narrow candle, then a break of the narrow one.
    ClosedCandle,
    /// Opens below the previous candle's low and trades back above it.
    GapTrap,
}

impl Pattern {
    pub const ALL: [Self; 6] = [
        Self::OneTwoThree,
        Self::OneTwoThreeInside,
        Self::Trap,
        Self::InsideBar,
        Self::ClosedCandle,
        Self::GapTrap,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// How a trade ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Target,
    Stop,
    Time,
    /// Still open at the end of the data.
    Open,
}

/// Rules shared by every pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Candles a trade may stay open.
    pub max_hold: usize,
    /// Target in multiples of the risk.
    pub reward: f64,
    /// Round-trip cost as a share of the entry price.
    pub cost: f64,
    pub fast: usize,
    pub slow: usize,
    /// Candles each side of a swing low.
    pub wing: usize,
}

impl Params {
    #[must_use]
    pub fn with_max_hold(max_hold: usize) -> Self {
        Self {
            max_hold,
            reward: 2.0,
            cost: 0.002,
            fast: 8,
            slow: 80,
            wing: 3,
        }
    }
}

/// A signal that was triggered, with its result.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub pattern: Pattern,
    pub side: Side,
    /// Candle whose high (low for shorts) is the trigger.
    pub signal_index: usize,
    pub entry_index: usize,
    pub exit_index: usize,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub exit: f64,
    /// Result in multiples of the risk, after costs.
    pub r: f64,
    pub how: Exit,
    /// Fast average above slow and close above fast (longs), at the signal.
    pub with_trend: bool,
}

/// A signal on the last candle, waiting for its trigger on the next one.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub pattern: Pattern,
    pub side: Side,
    pub signal_index: usize,
    pub trigger: f64,
    pub stop: f64,
    pub target: f64,
    pub with_trend: bool,
}

/// Exponential average of closes (None until `n` candles exist).
#[must_use]
pub fn ema(bars: &[Bar], n: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    if n == 0 || bars.len() < n {
        return out;
    }
    let k = 2.0 / (count(n) + 1.0);
    let mut value = bars[..n].iter().map(|b| b.close).sum::<f64>() / count(n);
    out[n - 1] = Some(value);
    for i in n..bars.len() {
        value += k * (bars[i].close - value);
        out[i] = Some(value);
    }
    out
}

fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Weekly candles (Monday 00:00 UTC to Sunday) from daily ones; a week is
/// kept only when it has all seven days, so the last open week is dropped.
#[must_use]
pub fn weekly(daily: &[Bar]) -> Vec<Bar> {
    // 1970-01-01 was a Thursday; the first Monday is four days later.
    let week_of = |ms: i64| (ms - 4 * DAY_MS).div_euclid(7 * DAY_MS);
    let mut out: Vec<(i64, usize, Bar)> = Vec::new();
    for b in daily {
        let w = week_of(b.open_time_ms);
        match out.last_mut() {
            Some((week, days, bar)) if *week == w => {
                bar.high = bar.high.max(b.high);
                bar.low = bar.low.min(b.low);
                bar.close = b.close;
                bar.volume += b.volume;
                *days += 1;
            }
            _ => out.push((
                w,
                1,
                Bar {
                    open_time_ms: w * 7 * DAY_MS + 4 * DAY_MS,
                    ..*b
                },
            )),
        }
    }
    out.into_iter()
        .filter(|(_, days, _)| *days == 7)
        .map(|(_, _, b)| b)
        .collect()
}

/// The price series upside down, so short rules can reuse the long ones.
#[must_use]
pub fn mirror(bars: &[Bar]) -> Vec<Bar> {
    bars.iter()
        .map(|b| Bar {
            open_time_ms: b.open_time_ms,
            open: -b.open,
            high: -b.low,
            low: -b.high,
            close: -b.close,
            volume: b.volume,
        })
        .collect()
}

/// Average range of the `n` candles before `i`.
fn mean_range(bars: &[Bar], i: usize, n: usize) -> Option<f64> {
    if i < n {
        return None;
    }
    Some(bars[i - n..i].iter().map(|b| b.high - b.low).sum::<f64>() / count(n))
}

/// Most recent swing low confirmed before candle `i` (its right wing closed
/// before `i`), as (index, low).
fn last_swing_low(bars: &[Bar], i: usize, wing: usize) -> Option<(usize, f64)> {
    let last = i.checked_sub(wing + 1)?;
    (wing..=last).rev().find_map(|j| {
        let low = bars[j].low;
        let left = bars[j - wing..j].iter().all(|b| b.low > low);
        let right = bars[j + 1..=j + wing].iter().all(|b| b.low >= low);
        (left && right).then_some((j, low))
    })
}

/// Stop and target of `pattern` if candle `i` is its signal candle (on the
/// long-side view of the prices).
#[allow(clippy::many_single_char_names)] // candle names follow the pattern drawings
fn signal_at(bars: &[Bar], i: usize, pattern: Pattern, p: Params) -> Option<(f64, f64)> {
    let b = bars[i];
    let trigger = b.high;
    let (stop, target) = match pattern {
        Pattern::OneTwoThree | Pattern::OneTwoThreeInside => {
            let (a, m) = (bars[i.checked_sub(2)?], bars[i - 1]);
            let shape = m.low < a.low && m.low < b.low;
            let inside = b.high <= m.high && b.low >= m.low;
            if !shape || (pattern == Pattern::OneTwoThreeInside && !inside) {
                return None;
            }
            (m.low, None)
        }
        Pattern::Trap => {
            let (_, level) = last_swing_low(bars, i, p.wing)?;
            if !(b.low < level && b.close > level) {
                return None;
            }
            (b.low, None)
        }
        Pattern::InsideBar => {
            let prev = bars[i.checked_sub(1)?];
            if !(b.high <= prev.high && b.low >= prev.low) {
                return None;
            }
            (b.low, None)
        }
        Pattern::ClosedCandle => {
            let wide = bars[i.checked_sub(1)?];
            let range = wide.high - wide.low;
            let usual = mean_range(bars, i - 1, 20)?;
            let falls = wide.close < wide.open && range >= 2.0 * usual;
            let narrow = b.high - b.low <= 0.5 * range;
            if !(falls && narrow) {
                return None;
            }
            (b.low, Some(wide.open))
        }
        Pattern::GapTrap => {
            let prev = bars[i.checked_sub(1)?];
            if !(b.open < prev.low && b.close > prev.low) {
                return None;
            }
            (b.low, None)
        }
    };
    let risk = trigger - stop;
    if risk <= 0.0 {
        return None;
    }
    let target = target.unwrap_or(trigger + p.reward * risk);
    (target > trigger).then_some((stop, target))
}

fn trend_up(fast: &[Option<f64>], slow: &[Option<f64>], bars: &[Bar], i: usize) -> bool {
    match (fast[i], slow[i]) {
        (Some(f), Some(s)) => f > s && bars[i].close > f,
        _ => false,
    }
}

fn sign(side: Side) -> f64 {
    match side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    }
}

/// Every triggered trade of every pattern, both sides, plus the signals on
/// the last candle that wait for tomorrow's trigger. Trades of the same
/// pattern and side never overlap.
#[must_use]
pub fn scan(bars: &[Bar], p: Params) -> (Vec<Trade>, Vec<Signal>) {
    let mut trades = Vec::new();
    let mut signals = Vec::new();
    for side in [Side::Long, Side::Short] {
        let view = match side {
            Side::Long => bars.to_vec(),
            Side::Short => mirror(bars),
        };
        let fast = ema(&view, p.fast);
        let slow = ema(&view, p.slow);
        for pattern in Pattern::ALL {
            scan_pattern(
                &view,
                pattern,
                side,
                p,
                &fast,
                &slow,
                &mut trades,
                &mut signals,
            );
        }
    }
    trades.sort_by_key(|t| (t.entry_index, t.pattern));
    (trades, signals)
}

#[allow(clippy::too_many_arguments, clippy::many_single_char_names)] // one pass over one pattern and side
fn scan_pattern(
    m: &[Bar],
    pattern: Pattern,
    side: Side,
    p: Params,
    fast: &[Option<f64>],
    slow: &[Option<f64>],
    trades: &mut Vec<Trade>,
    signals: &mut Vec<Signal>,
) {
    let n = m.len();
    let s = sign(side);
    let mut free_from = 0;
    for i in 0..n {
        if i < free_from {
            continue;
        }
        let Some((stop, target)) = signal_at(m, i, pattern, p) else {
            continue;
        };
        let with_trend = trend_up(fast, slow, m, i);
        let trigger = m[i].high;
        let k = i + 1;
        if k >= n {
            signals.push(Signal {
                pattern,
                side,
                signal_index: i,
                trigger: trigger * s,
                stop: stop * s,
                target: target * s,
                with_trend,
            });
            continue;
        }
        if m[k].high <= trigger {
            continue;
        }
        // A gap above the trigger fills at the open.
        let entry = trigger.max(m[k].open);
        let risk = entry - stop;
        if risk <= 0.0 || target <= entry {
            continue;
        }
        let (exit_index, exit, how) = follow(m, k, stop, target, p.max_hold);
        let r = (exit - entry) / risk - p.cost * entry.abs() / risk;
        trades.push(Trade {
            pattern,
            side,
            signal_index: i,
            entry_index: k,
            exit_index,
            entry: entry * s,
            stop: stop * s,
            target: target * s,
            exit: exit * s,
            r,
            how,
            with_trend,
        });
        free_from = exit_index + 1;
    }
}

/// The comparison base for a pattern: enter at the open of every candle
/// (optionally only after candles with the trend), with a stop `risk` away
/// as a share of the entry and the same target, hold and cost rules.
/// Returns (entry index, result in R) for every closed trade.
#[must_use]
pub fn baseline(
    bars: &[Bar],
    side: Side,
    risk: f64,
    trend_only: bool,
    p: Params,
) -> Vec<(usize, f64)> {
    if !(risk > 0.0 && risk < 1.0) {
        return Vec::new();
    }
    let view = match side {
        Side::Long => bars.to_vec(),
        Side::Short => mirror(bars),
    };
    let fast = ema(&view, p.fast);
    let slow = ema(&view, p.slow);
    let mut out = Vec::new();
    for i in 0..view.len().saturating_sub(1) {
        if trend_only && !trend_up(&fast, &slow, &view, i) {
            continue;
        }
        let k = i + 1;
        let entry = view[k].open;
        let distance = risk * entry.abs();
        let stop = entry - distance;
        let target = entry + p.reward * distance;
        let (_, exit, how) = follow(&view, k, stop, target, p.max_hold);
        if how == Exit::Open {
            continue;
        }
        out.push((
            k,
            (exit - entry) / distance - p.cost * entry.abs() / distance,
        ));
    }
    out
}

/// Typical stop distance of some trades, as a share of the entry (median).
#[must_use]
pub fn median_risk(trades: &[&Trade]) -> Option<f64> {
    let mut risks: Vec<f64> = trades
        .iter()
        .filter(|t| t.entry != 0.0)
        .map(|t| ((t.entry - t.stop) / t.entry).abs())
        .collect();
    if risks.is_empty() {
        return None;
    }
    risks.sort_by(f64::total_cmp);
    Some(risks[risks.len() / 2])
}

/// Runs a filled trade forward from its entry candle `k`. The stop is
/// checked first on any candle that touches both levels, and the target is
/// not taken on the entry candle (its order inside the candle is unknown).
#[must_use]
pub fn follow(m: &[Bar], k: usize, stop: f64, target: f64, max_hold: usize) -> (usize, f64, Exit) {
    let n = m.len();
    if m[k].low <= stop {
        return (k, stop, Exit::Stop);
    }
    let last = k + max_hold;
    for (j, b) in m.iter().enumerate().take(n.min(last + 1)).skip(k + 1) {
        if b.low <= stop {
            return (j, stop.min(b.open), Exit::Stop);
        }
        if b.high >= target {
            return (j, target.max(b.open), Exit::Target);
        }
    }
    if last < n {
        (last, m[last].close, Exit::Time)
    } else {
        (n - 1, m[n - 1].close, Exit::Open)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

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

    fn quiet() -> Params {
        Params {
            cost: 0.0,
            ..Params::with_max_hold(10)
        }
    }

    #[test]
    fn one_two_three_buys_above_the_third_candle_and_takes_twice_the_risk() {
        let bars = vec![
            bar(0, 10.0, 10.5, 9.5, 10.0),
            bar(1, 10.0, 10.2, 9.0, 9.4),   // 1
            bar(2, 9.4, 9.6, 8.5, 9.2),     // 2: lowest low
            bar(3, 9.2, 10.0, 9.1, 9.9),    // 3: signal, trigger 10.0
            bar(4, 9.9, 10.4, 9.8, 10.3),   // fills at 10.0, risk 1.5
            bar(5, 10.3, 13.2, 10.2, 13.0), // target 13.0
        ];
        let (trades, _) = scan(&bars, quiet());
        let t = trades
            .iter()
            .find(|t| t.pattern == Pattern::OneTwoThree && t.side == Side::Long)
            .unwrap();
        assert_eq!((t.signal_index, t.entry_index, t.exit_index), (3, 4, 5));
        assert_eq!((t.entry, t.stop, t.target), (10.0, 8.5, 13.0));
        assert_eq!(t.how, Exit::Target);
        assert!((t.r - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_trap_needs_a_close_back_above_the_swing_low() {
        let mut bars: Vec<Bar> = (0..10)
            .map(|i| bar(i, 10.0, 10.5, if i == 4 { 9.0 } else { 9.6 }, 10.0))
            .collect();
        // Candle 10 trades below the 9.0 swing low and closes back above it.
        bars.push(bar(10, 10.0, 10.2, 8.8, 9.8));
        bars.push(bar(11, 9.8, 10.5, 9.7, 10.4));
        let (trades, signals) = scan(&bars, quiet());
        assert!(
            trades
                .iter()
                .any(|t| t.pattern == Pattern::Trap && t.signal_index == 10 && t.stop == 8.8)
        );
        assert!(signals.iter().all(|s| s.pattern != Pattern::Trap));
        // Without the close back above, no trap.
        bars[10].close = 8.9;
        let (trades, _) = scan(&bars, quiet());
        assert!(
            !trades
                .iter()
                .any(|t| t.pattern == Pattern::Trap && t.signal_index == 10)
        );
    }

    #[test]
    fn stop_wins_on_a_candle_that_touches_both_levels_and_shorts_mirror_longs() {
        let bars = vec![
            bar(0, 10.0, 10.5, 9.5, 10.0),
            bar(1, 10.0, 10.2, 9.0, 9.4),
            bar(2, 9.4, 9.6, 8.5, 9.2),
            bar(3, 9.2, 10.0, 9.1, 9.9),
            bar(4, 9.9, 10.4, 9.8, 10.3),
            bar(5, 10.3, 13.5, 8.0, 12.0),
        ];
        let (trades, _) = scan(&bars, quiet());
        let t = trades
            .iter()
            .find(|t| t.pattern == Pattern::OneTwoThree)
            .unwrap();
        assert_eq!(t.how, Exit::Stop);
        assert!((t.r + 1.0).abs() < 1e-9);
        let flipped: Vec<Bar> = bars
            .iter()
            .map(|b| Bar {
                open: 100.0 - b.open,
                high: 100.0 - b.low,
                low: 100.0 - b.high,
                close: 100.0 - b.close,
                ..*b
            })
            .collect();
        let (shorts, _) = scan(&flipped, quiet());
        let s = shorts
            .iter()
            .find(|t| t.pattern == Pattern::OneTwoThree && t.side == Side::Short)
            .unwrap();
        assert_eq!((s.entry, s.stop), (90.0, 91.5));
        assert_eq!(s.how, Exit::Stop);
    }

    #[test]
    fn a_signal_on_the_last_candle_waits_for_its_trigger() {
        let bars = vec![
            bar(0, 10.0, 10.5, 9.5, 10.0),
            bar(1, 10.0, 10.2, 9.0, 9.4),
            bar(2, 9.4, 9.6, 8.5, 9.2),
            bar(3, 9.2, 10.0, 9.1, 9.9),
        ];
        let (_, signals) = scan(&bars, quiet());
        let s = signals
            .iter()
            .find(|s| s.pattern == Pattern::OneTwoThree && s.side == Side::Long)
            .unwrap();
        assert_eq!((s.trigger, s.stop, s.target), (10.0, 8.5, 13.0));
    }

    #[test]
    fn weeks_start_on_monday_and_need_seven_days() {
        // 1970-01-05 was a Monday: day index 4.
        let daily: Vec<Bar> = (4..20)
            .map(|d| bar(d, 1.0, 2.0 + f64::from(u32::try_from(d).unwrap()), 0.5, 1.5))
            .collect();
        let w = weekly(&daily);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].open_time_ms, 4 * DAY_MS);
        assert_eq!(w[0].high, 12.0);
        assert_eq!(w[1].open_time_ms, 11 * DAY_MS);
    }

    #[test]
    fn the_baseline_enters_every_candle_with_the_given_stop() {
        // Steady rise of 1 a candle from 100: with a 5% stop the target (+10%)
        // is reached and the stop never is.
        let bars: Vec<Bar> = (0..40)
            .map(|i| {
                let p = 100.0 + f64::from(u32::try_from(i).unwrap());
                bar(i, p, p + 0.9, p - 0.1, p + 0.8)
            })
            .collect();
        let base = baseline(&bars, Side::Long, 0.05, false, quiet());
        assert!(!base.is_empty());
        assert!(
            base.iter()
                .all(|(_, r)| (*r - 2.0).abs() < 1e-9 || *r > 0.0)
        );
        let short = baseline(&bars, Side::Short, 0.05, false, quiet());
        assert!(short.iter().all(|(_, r)| *r < 0.0));
    }

    #[test]
    fn ema_starts_at_the_simple_average() {
        let bars: Vec<Bar> = (0..5)
            .map(|i| bar(i, 1.0, 1.0, 1.0, f64::from(u32::try_from(i).unwrap())))
            .collect();
        let e = ema(&bars, 3);
        assert_eq!(e[1], None);
        assert_eq!(e[2], Some(1.0));
        assert_eq!(e[3], Some(2.0));
    }
}
