//! Smart Money / Wyckoff setup, written as fixed rules so it can be tested.
//!
//! Long setup (the short one is the mirror image):
//! 1. **Liquidity sweep**: a candle trades below the most recent confirmed
//!    swing low and closes back above it (within [`SmcParams::reclaim`]
//!    candles).
//! 2. **Change of character (`CHoCH`)**: within [`SmcParams::choch_window`]
//!    candles, a close above the last swing high that existed before the
//!    sweep, without trading below the sweep low first.
//! 3. **Order block entry**: a limit buy at the top of the body of the last
//!    down candle between the sweep and the `CHoCH`, valid for
//!    [`SmcParams::order_valid`] candles.
//! 4. **Stop** a tenth of an ATR below the sweep low; **target** at the
//!    nearest swing high above the entry that pays at least
//!    [`SmcParams::min_reward`] times the risk (the next liquidity).
//!
//! Swing points need [`SmcParams::wing`] candles on each side and are only
//! used once those candles have closed, so nothing looks ahead. Each setup is
//! also tagged with its Wyckoff context: a sweep of the floor (or ceiling) of
//! a sideways range is a spring (or UTAD), and a sweep candle with at least
//! twice the recent average volume is a climax.

use crate::indicators::as_f64;
use crate::intraday::Bar;

/// Rules of the setup for one timeframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmcParams {
    /// Candles on each side of a swing point.
    pub wing: usize,
    /// Candles after the sweep candle in which the close may come back.
    pub reclaim: usize,
    /// Candles after the sweep in which the `CHoCH` must happen.
    pub choch_window: usize,
    /// Candles after the `CHoCH` during which the limit order stays open.
    pub order_valid: usize,
    /// Candles a filled trade may stay open before it is closed.
    pub max_hold: usize,
    /// Minimum reward to risk for the target.
    pub min_reward: f64,
    /// Stop buffer beyond the sweep, in ATRs.
    pub stop_buffer_atr: f64,
    /// Cost of a round trip, as a share of the price (fees and slippage).
    pub round_trip_cost: f64,
}

impl SmcParams {
    /// Shared rules with a given holding limit.
    #[must_use]
    pub fn with_max_hold(max_hold: usize) -> Self {
        Self {
            wing: 3,
            reclaim: 2,
            choch_window: 20,
            order_valid: 20,
            max_hold,
            min_reward: 2.0,
            stop_buffer_atr: 0.1,
            round_trip_cost: 0.002,
        }
    }
}

/// Direction of a setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// Where a setup stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Limit order waiting for the retest.
    Waiting,
    /// Filled and still open.
    Open,
    /// Hit the target.
    Target,
    /// Hit the stop.
    Stop,
    /// Closed at the holding limit.
    Time,
    /// The order was never filled: the price ran to the target or the stop
    /// first, or the order expired.
    Cancelled,
}

/// One setup found by the scanner.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub side: Side,
    /// Candle that swept the liquidity.
    pub sweep_index: usize,
    /// Swing level that was swept.
    pub swept_level: f64,
    /// Lowest low (long) or highest high (short) of the sweep.
    pub sweep_extreme: f64,
    /// Candle that broke the structure.
    pub choch_index: usize,
    /// Level the `CHoCH` closed beyond.
    pub choch_level: f64,
    /// Candle used as the order block.
    pub order_block_index: usize,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    /// Sweep of the floor (long) or ceiling (short) of a sideways range.
    pub wyckoff_range: bool,
    /// Sweep candle volume at least twice the recent average.
    pub climax_volume: bool,
    pub status: Status,
    pub fill_index: Option<usize>,
    pub exit_index: Option<usize>,
    pub exit_price: Option<f64>,
    /// Result in units of risk, after costs (closed trades only).
    pub r: Option<f64>,
}

impl Setup {
    /// Reward to risk of the plan.
    #[must_use]
    pub fn reward_to_risk(&self) -> f64 {
        (self.target - self.entry).abs() / (self.entry - self.stop).abs()
    }
}

/// Average true range over `period` candles (None until enough candles).
#[must_use]
pub fn bar_atr(bars: &[Bar], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    if period == 0 {
        return out;
    }
    let tr: Vec<f64> = bars
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let range = b.high - b.low;
            match i.checked_sub(1).map(|p| bars[p].close) {
                Some(prev) => range.max((b.high - prev).abs()).max((b.low - prev).abs()),
                None => range,
            }
        })
        .collect();
    for i in period..bars.len() {
        out[i] = Some(tr[i + 1 - period..=i].iter().sum::<f64>() / as_f64(period));
    }
    out
}

/// Swing highs and lows as (index, price), each usable from `index + wing`.
/// Swing points as (index, price).
type Swings = Vec<(usize, f64)>;

fn swings(bars: &[Bar], wing: usize) -> (Swings, Swings) {
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    for i in wing..bars.len().saturating_sub(wing) {
        let around = &bars[i - wing..=i + wing];
        if around.iter().all(|b| b.high <= bars[i].high) {
            highs.push((i, bars[i].high));
        }
        if around.iter().all(|b| b.low >= bars[i].low) {
            lows.push((i, bars[i].low));
        }
    }
    (highs, lows)
}

/// Prices are mirrored for shorts so one routine handles both sides.
fn view(b: &Bar, side: Side) -> (f64, f64, f64, f64) {
    match side {
        Side::Long => (b.open, b.high, b.low, b.close),
        Side::Short => (-b.open, -b.low, -b.high, -b.close),
    }
}

fn sign(side: Side) -> f64 {
    match side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    }
}

/// A sweep still waiting for its change of character at the end of the data.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub side: Side,
    /// Candle that swept the liquidity.
    pub sweep_index: usize,
    /// Swing level that was swept.
    pub swept_level: f64,
    /// Lowest low (long) or highest high (short) of the sweep.
    pub sweep_extreme: f64,
    /// Close needed beyond this level to confirm the setup.
    pub choch_level: f64,
    /// Last candle on which the confirmation may still come.
    pub deadline_index: usize,
    pub wyckoff_range: bool,
    pub climax_volume: bool,
}

/// Every setup in the history, in time order, with its outcome as far as
/// the data goes. At most one setup per side is active at a time.
#[must_use]
pub fn scan(bars: &[Bar], p: SmcParams) -> Vec<Setup> {
    scan_with_pending(bars, p).0
}

/// Like [`scan`], plus the sweeps whose confirmation window is still open at
/// the last candle (not yet confirmed and not invalidated).
#[must_use]
pub fn scan_with_pending(bars: &[Bar], p: SmcParams) -> (Vec<Setup>, Vec<Sweep>) {
    let (mut out, mut pending) = scan_side(bars, p, Side::Long);
    let (shorts, short_pending) = scan_side(bars, p, Side::Short);
    out.extend(shorts);
    pending.extend(short_pending);
    out.sort_by_key(|s| s.choch_index);
    (out, pending)
}

#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::needless_range_loop
)] // one pass: sweep, CHoCH, order, fill, exit
fn scan_side(bars: &[Bar], p: SmcParams, side: Side) -> (Vec<Setup>, Vec<Sweep>) {
    let n = bars.len();
    let atr = bar_atr(bars, 14);
    let s = sign(side);
    // In the mirrored view the long logic works for both sides: "lows" are
    // the side's liquidity to sweep and "highs" its structure to break.
    let mirrored: Vec<Bar> = bars
        .iter()
        .map(|b| {
            let (open, high, low, close) = view(b, side);
            Bar {
                open_time_ms: b.open_time_ms,
                open,
                high,
                low,
                close,
                volume: b.volume,
            }
        })
        .collect();
    let (highs, lows) = swings(&mirrored, p.wing);
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let mut t = 0usize;
    let mut last_swept: Option<usize> = None;
    while t < n {
        // Most recent confirmed swing low at t that has not been swept yet.
        let known = lows.partition_point(|(i, _)| i + p.wing < t);
        let candidate = lows[..known]
            .iter()
            .rev()
            .take(2)
            .find(|(i, _)| Some(*i) != last_swept)
            .copied();
        let Some((low_index, level)) = candidate else {
            t += 1;
            continue;
        };
        let b = mirrored[t];
        if b.low >= level {
            t += 1;
            continue;
        }
        // Sweep: back above the level within `reclaim` candles.
        let mut extreme = b.low;
        let mut reclaimed = None;
        for k in t..=(t + p.reclaim).min(n - 1) {
            extreme = extreme.min(mirrored[k].low);
            if mirrored[k].close > level {
                reclaimed = Some(k);
                break;
            }
        }
        last_swept = Some(low_index);
        let Some(reclaim_at) = reclaimed else {
            t += 1;
            continue;
        };
        // Last swing high confirmed before the sweep: the structure to break.
        let known_highs = highs.partition_point(|(i, _)| i + p.wing < t);
        let Some(&(_, choch_level)) = known_highs.checked_sub(1).and_then(|k| highs.get(k)) else {
            t = reclaim_at + 1;
            continue;
        };
        let mut choch = None;
        let mut invalidated = false;
        for k in reclaim_at..=(t + p.choch_window).min(n - 1) {
            if mirrored[k].low < extreme {
                invalidated = true;
                break;
            }
            if mirrored[k].close > choch_level {
                choch = Some(k);
                break;
            }
        }
        let Some(choch_at) = choch else {
            if !invalidated && t + p.choch_window > n - 1 {
                pending.push(Sweep {
                    side,
                    sweep_index: t,
                    swept_level: level * s,
                    sweep_extreme: extreme * s,
                    choch_level: choch_level * s,
                    deadline_index: t + p.choch_window,
                    wyckoff_range: is_range_floor(&mirrored, &atr, t, level),
                    climax_volume: climax(bars, t),
                });
            }
            t = reclaim_at + 1;
            continue;
        };
        let Some(a) = atr[choch_at].filter(|a| *a > 0.0) else {
            t = choch_at + 1;
            continue;
        };
        // Order block: last down candle from the sweep to the `CHoCH`.
        let ob = (t..=choch_at)
            .rev()
            .find(|&k| mirrored[k].close < mirrored[k].open)
            .unwrap_or(t);
        let entry = mirrored[ob].open.max(mirrored[ob].close);
        let stop = extreme - p.stop_buffer_atr * a;
        let risk = entry - stop;
        if risk <= 0.0 || entry >= mirrored[choch_at].close + risk * 10.0 {
            t = choch_at + 1;
            continue;
        }
        // Target: nearest confirmed swing high above the entry with enough reward.
        let usable = highs.partition_point(|(i, _)| i + p.wing <= choch_at);
        let mut targets: Vec<f64> = highs[..usable]
            .iter()
            .filter(|(_, h)| *h > entry)
            .map(|(_, h)| *h)
            .collect();
        targets.sort_by(f64::total_cmp);
        let Some(target) = targets
            .into_iter()
            .find(|h| (h - entry) / risk >= p.min_reward)
        else {
            t = choch_at + 1;
            continue;
        };
        let wyckoff_range = is_range_floor(&mirrored, &atr, t, level);
        let climax_volume = climax(bars, t);
        let mut setup = Setup {
            side,
            sweep_index: t,
            swept_level: level * s,
            sweep_extreme: extreme * s,
            choch_index: choch_at,
            choch_level: choch_level * s,
            order_block_index: ob,
            entry: entry * s,
            stop: stop * s,
            target: target * s,
            wyckoff_range,
            climax_volume,
            status: Status::Waiting,
            fill_index: None,
            exit_index: None,
            exit_price: None,
            r: None,
        };
        let next = follow(&mirrored, &mut setup, (entry, stop, target), p, s);
        out.push(setup);
        t = next.max(t + 1);
    }
    (out, pending)
}

/// Runs the order and the trade forward; returns the next candle to scan from.
#[allow(clippy::many_single_char_names, clippy::needless_range_loop)] // short names for prices in the mirrored view
fn follow(
    m: &[Bar],
    setup: &mut Setup,
    (entry, stop, target): (f64, f64, f64),
    p: SmcParams,
    s: f64,
) -> usize {
    let n = m.len();
    let start = setup.choch_index + 1;
    let last_order = (setup.choch_index + p.order_valid).min(n.saturating_sub(1));
    let mut fill = None;
    for k in start..=last_order.max(start).min(n.saturating_sub(1)) {
        if k >= n {
            break;
        }
        let b = m[k];
        if b.high >= target && b.low > entry {
            setup.status = Status::Cancelled;
            setup.exit_index = Some(k);
            return k + 1;
        }
        if b.low <= entry {
            fill = Some((k, entry.min(b.open)));
            break;
        }
    }
    let Some((k, price)) = fill else {
        if start + p.order_valid < n {
            setup.status = Status::Cancelled;
            setup.exit_index = Some(start + p.order_valid - 1);
            return start + p.order_valid;
        }
        return n;
    };
    setup.fill_index = Some(k);
    setup.entry = price * s;
    setup.status = Status::Open;
    let risk = price - stop;
    let close_trade = |setup: &mut Setup, at: usize, exit: f64, status: Status| {
        setup.status = status;
        setup.exit_index = Some(at);
        setup.exit_price = Some(exit * s);
        let cost_r = p.round_trip_cost * price.abs() / risk;
        setup.r = Some((exit - price) / risk - cost_r);
    };
    if m[k].low <= stop {
        close_trade(setup, k, stop.min(m[k].open), Status::Stop);
        return k + 1;
    }
    let last = (k + p.max_hold).min(n - 1);
    for j in k + 1..=last {
        let b = m[j];
        if b.open <= stop || b.low <= stop {
            close_trade(setup, j, stop.min(b.open), Status::Stop);
            return j + 1;
        }
        if b.open >= target || b.high >= target {
            close_trade(setup, j, target.max(b.open), Status::Target);
            return j + 1;
        }
        if j == k + p.max_hold {
            close_trade(setup, j, b.close, Status::Time);
            return j + 1;
        }
    }
    n
}

/// True when `level` sits at the floor of a sideways range: the 40 candles
/// before `t` stayed within eight ATRs and `level` is within half an ATR of
/// their lowest low.
fn is_range_floor(m: &[Bar], atr: &[Option<f64>], t: usize, level: f64) -> bool {
    let Some(a) = atr.get(t).copied().flatten().filter(|a| *a > 0.0) else {
        return false;
    };
    if t < 40 {
        return false;
    }
    let w = &m[t - 40..t];
    let high = w.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let low = w.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    high - low <= 8.0 * a && (level - low).abs() <= 0.5 * a
}

/// True when the candle at `t` traded at least twice the average volume of
/// the 20 candles before it.
fn climax(bars: &[Bar], t: usize) -> bool {
    if t < 20 {
        return false;
    }
    let avg = bars[t - 20..t].iter().map(|b| b.volume).sum::<f64>() / 20.0;
    avg > 0.0 && bars[t].volume >= 2.0 * avg
}

/// Nearest untouched liquidity around the last close: the closest confirmed
/// swing high above it (stops of sellers) and swing low below it (stops of
/// buyers) that no later candle has traded through.
#[must_use]
pub fn nearest_liquidity(bars: &[Bar], wing: usize) -> (Option<f64>, Option<f64>) {
    let Some(last) = bars.last() else {
        return (None, None);
    };
    let (highs, lows) = swings(bars, wing);
    let n = bars.len();
    let confirmed = |i: usize| i + wing < n;
    let above = highs
        .iter()
        .filter(|(i, h)| {
            confirmed(*i) && *h > last.close && bars[i + 1..].iter().all(|b| b.high < *h)
        })
        .map(|(_, h)| *h)
        .min_by(f64::total_cmp);
    let below = lows
        .iter()
        .filter(|(i, l)| {
            confirmed(*i) && *l < last.close && bars[i + 1..].iter().all(|b| b.low > *l)
        })
        .map(|(_, l)| *l)
        .max_by(f64::total_cmp);
    (above, below)
}

/// Summary of closed trades.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmcStats {
    pub trades: usize,
    pub targets: usize,
    pub stops: usize,
    pub timeouts: usize,
    /// Mean result per trade, in units of risk.
    pub mean_r: f64,
    /// Sum of results, in units of risk.
    pub total_r: f64,
}

/// Stats of the closed trades among `setups`.
#[must_use]
pub fn stats<'a>(setups: impl IntoIterator<Item = &'a Setup>) -> SmcStats {
    let mut s = SmcStats::default();
    for setup in setups {
        let Some(r) = setup.r else { continue };
        s.trades += 1;
        s.total_r += r;
        match setup.status {
            Status::Target => s.targets += 1,
            Status::Stop => s.stops += 1,
            Status::Time => s.timeouts += 1,
            _ => {}
        }
    }
    if s.trades > 0 {
        s.mean_r = s.total_r / as_f64(s.trades);
    }
    s
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn bar(i: usize, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: i64::try_from(i).unwrap() * 3_600_000,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    /// A drop to a swing low at 90, a bounce to a swing high at 100, a lower
    /// high at 97, a sweep under 90 that closes back above it, a `CHoCH` above
    /// 97, a retest of the order block and a run to 110.
    fn long_story() -> Vec<Bar> {
        let mut v = Vec::new();
        let path = [
            105.0, 104.0, 103.0, 102.0, 101.0, 100.0, 99.0, 98.0, 97.0, 96.0, 95.0, 94.0, 93.0,
            92.0, 91.0, 90.0, 92.0, 94.0, 96.0, 98.0, 100.0, 98.0, 96.0, 95.0, 94.0, 95.0, 96.0,
            97.0, 96.0, 95.0, 94.0, 93.0, 92.0, 91.5,
        ];
        for (i, c) in path.iter().enumerate() {
            let o = if i == 0 { *c } else { path[i - 1] };
            v.push(bar(i, o, o.max(*c) + 0.2, o.min(*c) - 0.2, *c));
        }
        let n = v.len();
        // Sweep: wick to 88, close back at 91 (a down candle: the order block).
        v.push(bar(n, 91.5, 91.8, 88.0, 91.0));
        // Up move that closes above the lower high (~97.2) at 98.5: the `CHoCH`.
        v.push(bar(n + 1, 91.0, 95.0, 90.8, 94.5));
        v.push(bar(n + 2, 94.5, 98.8, 94.3, 98.5));
        // Retest of the order block top (91.5) and the run up past 105.
        v.push(bar(n + 3, 98.5, 98.6, 91.2, 93.0));
        for k in 0..8 {
            let o = 93.0 + 2.0 * f64::from(u32::try_from(k).unwrap());
            v.push(bar(n + 4 + k, o, o + 2.2, o - 0.2, o + 2.0));
        }
        v
    }

    #[test]
    fn finds_sweep_choch_order_block_and_hits_the_target() {
        let bars = long_story();
        let p = SmcParams::with_max_hold(48);
        let setups: Vec<Setup> = scan(&bars, p)
            .into_iter()
            .filter(|s| s.side == Side::Long)
            .collect();
        let s = setups.last().unwrap();
        // The most recent unswept swing low (the lower low near 94) is the one taken.
        assert!(s.swept_level < 94.0 && s.sweep_extreme == 88.0, "{s:?}");
        assert!((s.target - 100.2).abs() < 1e-9, "{s:?}");
        assert!(s.choch_level > 96.0 && s.choch_level < 98.0, "{s:?}");
        assert_eq!(s.entry, 91.5);
        assert!(s.stop < 88.0);
        assert!(s.reward_to_risk() >= 2.0);
        assert_eq!(s.status, Status::Target, "{s:?}");
        assert!(s.r.unwrap() > 1.9);
    }

    #[test]
    fn short_setup_is_the_mirror_image() {
        let bars: Vec<Bar> = long_story()
            .iter()
            .map(|b| Bar {
                open_time_ms: b.open_time_ms,
                open: 200.0 - b.open,
                high: 200.0 - b.low,
                low: 200.0 - b.high,
                close: 200.0 - b.close,
                volume: b.volume,
            })
            .collect();
        let setups: Vec<Setup> = scan(&bars, SmcParams::with_max_hold(48))
            .into_iter()
            .filter(|s| s.side == Side::Short)
            .collect();
        let s = setups.last().unwrap();
        assert_eq!(s.sweep_extreme, 112.0);
        assert_eq!(s.entry, 108.5);
        assert!(s.stop > 112.0);
        assert_eq!(s.status, Status::Target, "{s:?}");
    }

    #[test]
    fn a_stop_before_the_target_loses_one_r_plus_costs() {
        let mut bars = long_story();
        let n = bars.len();
        bars.truncate(n - 8);
        let m = bars.len();
        bars.push(bar(m, 93.0, 93.5, 85.0, 86.0));
        let s = scan(&bars, SmcParams::with_max_hold(48))
            .into_iter()
            .rfind(|s| s.side == Side::Long)
            .unwrap();
        assert_eq!(s.status, Status::Stop);
        assert!(s.r.unwrap() < -1.0 && s.r.unwrap() > -1.2, "{s:?}");
        let st = stats([&s]);
        assert_eq!((st.trades, st.stops), (1, 1));
    }

    #[test]
    fn nearest_liquidity_skips_levels_already_taken() {
        // Highs: 12 at 3 (later taken by 13 at 8), 13 at 8; lows: 8 at 5, 10 at 11.
        let prices = [
            10.0, 10.5, 11.0, 12.0, 10.0, 8.0, 10.0, 11.0, 13.0, 11.0, 10.5, 10.0, 10.2, 10.1,
        ];
        let bars: Vec<Bar> = prices
            .iter()
            .enumerate()
            .map(|(i, p)| bar(i, *p, *p, *p, *p))
            .collect();
        let (above, below) = nearest_liquidity(&bars, 2);
        assert_eq!(above, Some(13.0));
        assert_eq!(below, Some(10.0));
        assert_eq!(nearest_liquidity(&[], 2), (None, None));
    }

    #[test]
    fn atr_averages_true_ranges() {
        let bars: Vec<Bar> = (0..5).map(|i| bar(i, 10.0, 11.0, 9.0, 10.0)).collect();
        let atr = bar_atr(&bars, 2);
        assert_eq!(atr[1], None);
        assert_eq!(atr[2], Some(2.0));
    }

    #[test]
    fn scans_a_long_history_quickly() {
        let mut price = 100.0_f64;
        let bars: Vec<Bar> = (0..60_000usize)
            .map(|i| {
                let step = f64::from(u32::try_from((i * 7919 + i / 13) % 17).unwrap()) - 8.0;
                let open = price;
                price = (price * (1.0 + step / 400.0)).max(1.0);
                bar(
                    i,
                    open,
                    open.max(price) * 1.002,
                    open.min(price) * 0.998,
                    price,
                )
            })
            .collect();
        let start = std::time::Instant::now();
        let setups = scan(&bars, SmcParams::with_max_hold(48));
        assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
        assert!(!setups.is_empty());
    }

    #[test]
    fn a_fresh_sweep_waits_for_its_choch() {
        let mut bars = long_story();
        // Keep the story up to the sweep candle and one more candle without the break.
        let cut = bars
            .iter()
            .position(|b| (b.low - 88.0).abs() < 1e-9)
            .unwrap()
            + 2;
        bars.truncate(cut);
        let (setups, pending) = scan_with_pending(&bars, SmcParams::with_max_hold(48));
        assert!(
            setups
                .iter()
                .all(|s| s.side != Side::Long || s.sweep_extreme != 88.0)
        );
        let sweep = pending.iter().find(|s| s.side == Side::Long).unwrap();
        assert_eq!(sweep.sweep_extreme, 88.0);
        assert!(sweep.choch_level > 96.0 && sweep.choch_level < 98.0);
        assert_eq!(sweep.deadline_index, sweep.sweep_index + 20);
    }
}
