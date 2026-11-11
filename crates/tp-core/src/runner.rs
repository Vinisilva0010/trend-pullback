//! Trades that let the winner run: no fixed target, a stop that only moves
//! up (the highest high since entry minus a few ATRs, the "chandelier"
//! stop). This is how trades of 5, 10 or more times the risk happen: most
//! trades lose a little, a few run for months.
//!
//! Also the breakout entries tested with it: a new 52-week high and a new
//! all-time high, both on closed weekly candles.

use crate::intraday::Bar;

/// One trade ridden with the trailing stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ride {
    pub entry_index: usize,
    pub exit_index: usize,
    pub entry: f64,
    pub stop: f64,
    pub exit: f64,
    /// Result in multiples of the first risk (entry minus stop), with costs.
    pub r: f64,
    /// Still running at the last candle (left out of the results).
    pub open: bool,
}

/// Average true range, Wilder's smoothing; `None` until `n` candles.
#[must_use]
pub fn atr(bars: &[Bar], n: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    if n == 0 || bars.len() < n {
        return out;
    }
    let tr = |i: usize| {
        let b = &bars[i];
        if i == 0 {
            b.high - b.low
        } else {
            let pc = bars[i - 1].close;
            (b.high - b.low)
                .max((b.high - pc).abs())
                .max((b.low - pc).abs())
        }
    };
    let nf = f64::from(u32::try_from(n).unwrap_or(u32::MAX));
    let mut value = (0..n).map(tr).sum::<f64>() / nf;
    out[n - 1] = Some(value);
    for (i, slot) in out.iter_mut().enumerate().skip(n) {
        value = (value * (nf - 1.0) + tr(i)) / nf;
        *slot = Some(value);
    }
    out
}

/// Rides a long trade entered on candle `k` at `entry` with `stop`. The stop
/// is checked first on every candle (on the entry candle too); after each
/// candle it moves up to the highest high since entry minus `mult` ATRs,
/// never down. A gap below the stop exits at the open. After `max_hold`
/// candles the trade closes at the close.
#[must_use]
#[allow(clippy::too_many_arguments)] // bars, the ATR, the entry and the exit rule
pub fn ride(
    bars: &[Bar],
    atr: &[Option<f64>],
    k: usize,
    entry: f64,
    stop: f64,
    mult: f64,
    max_hold: usize,
    cost: f64,
) -> Option<Ride> {
    let risk = entry - stop;
    if k >= bars.len() || risk.is_nan() || risk <= 0.0 || !entry.is_finite() {
        return None;
    }
    let r_of = |exit: f64| (exit - entry) / risk - cost * entry / risk;
    let mut trail = stop;
    let mut highest = entry;
    let last = (k + max_hold).min(bars.len() - 1);
    for (j, b) in bars.iter().enumerate().take(last + 1).skip(k) {
        if b.low <= trail {
            let exit = if j > k { trail.min(b.open) } else { trail };
            return Some(Ride {
                entry_index: k,
                exit_index: j,
                entry,
                stop,
                exit,
                r: r_of(exit),
                open: false,
            });
        }
        highest = highest.max(b.high);
        if let Some(a) = atr.get(j).copied().flatten() {
            trail = trail.max(highest - mult * a);
        }
    }
    let exit = bars[last].close;
    Some(Ride {
        entry_index: k,
        exit_index: last,
        entry,
        stop,
        exit,
        r: r_of(exit),
        open: last + 1 >= bars.len() && last < k + max_hold,
    })
}

/// Closed candles that make a new high of the last `lookback` candles (all
/// earlier candles with `lookback == 0`, the all-time high, from `min_history`
/// candles on). Entry is the next candle's open.
#[must_use]
pub fn breakouts(bars: &[Bar], lookback: usize, min_history: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut running_high = f64::MIN;
    for i in 0..bars.len() {
        let start = if lookback == 0 {
            0
        } else {
            i.saturating_sub(lookback)
        };
        let prior = if lookback == 0 {
            running_high
        } else {
            bars[start..i]
                .iter()
                .map(|b| b.high)
                .fold(f64::MIN, f64::max)
        };
        if i >= min_history.max(1) && i + 1 < bars.len() && bars[i].close > prior {
            out.push(i);
        }
        running_high = running_high.max(bars[i].high);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bar(open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: 0,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn atr_averages_the_true_range() {
        let bars = vec![bar(10.0, 11.0, 9.0, 10.0); 5];
        let a = atr(&bars, 3);
        assert_eq!(a[1], None);
        assert_eq!(a[2], Some(2.0));
        assert_eq!(a[4], Some(2.0));
    }

    #[test]
    fn the_stop_follows_a_rally_and_lets_it_run() {
        // Steady rise of 1 a candle with a range of 1: ATR 1, trail = high - 3.
        let mut bars: Vec<Bar> = (0..30)
            .map(|i| {
                let p = 100.0 + f64::from(i);
                bar(p, p + 0.5, p - 0.5, p)
            })
            .collect();
        // Then a drop through the trail.
        bars.push(bar(125.0, 125.0, 110.0, 111.0));
        let a = atr(&bars, 3);
        let r = ride(&bars, &a, 5, 105.0, 100.0, 3.0, 500, 0.0).unwrap();
        assert!(!r.open);
        assert_eq!(r.exit_index, 30);
        // Highest high 129.5 at candle 29, trail 126.5; open 125 gaps below.
        assert_eq!(r.exit, 125.0);
        assert!((r.r - 4.0).abs() < 1e-9, "{}", r.r);
    }

    #[test]
    fn the_first_stop_exits_with_minus_one() {
        let bars = vec![bar(10.0, 10.5, 9.5, 10.0), bar(10.0, 10.2, 8.0, 8.5)];
        let a = atr(&bars, 14);
        let r = ride(&bars, &a, 0, 10.0, 9.0, 3.0, 500, 0.0).unwrap();
        assert_eq!(r.exit_index, 1);
        assert_eq!(r.r, -1.0);
    }

    #[test]
    fn breakouts_need_a_close_above_the_prior_high() {
        let closes = [10.0, 11.0, 10.5, 12.0, 11.0, 11.5];
        let bars: Vec<Bar> = closes
            .iter()
            .map(|c| bar(*c, c + 0.2, c - 0.2, *c))
            .collect();
        // Over the two previous candles: 1 (11 > 10.2), 3 (12 > 11.2).
        assert_eq!(breakouts(&bars, 2, 1), vec![1, 3]);
        // All-time high from candle 2 on: only 3; 5 is not (12.2 earlier).
        assert_eq!(breakouts(&bars, 0, 2), vec![3]);
    }
}
