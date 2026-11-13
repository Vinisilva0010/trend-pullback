//! `tp support-study`: buying at support on the way down, the way Vi
//! trades it, turned into fixed rules and tested.
//!
//! Levels are the prices the daily candles touched most in the last year
//! (clusters of turning points, the same levels as the daily report), seen
//! only with the data up to the day before. Three plans:
//! - quick bounce: a limit buy just above the support, stop 3% below, a
//!   small target, out by the end of the next day;
//! - pullback in an uptrend: a limit buy at the support, stop one ATR
//!   below, stop to break-even after the bounce, half out at 2R and the
//!   rest following the price;
//! - two entries in a fall: after a drop of 20% or more from the 90-day
//!   high, half at the support and half lower, in the liquidity zone, stop
//!   below the second entry; tested with a quick break-even (the stop goes
//!   to the average price after a 3% bounce) and with holding (only after
//!   10%).
//!
//! Each plan is compared with a base that buys at the open of any day in
//! the same situation with the same stop and exits, so the test shows
//! whether aiming at the level matters. BTC, ETH and SOL run on hourly
//! candles (the order of prices inside the day is known); the other coins
//! of the trade universe run on daily candles, assuming the worst order
//! inside a day, as an independent check.

use std::path::Path;

use chrono::{DateTime, NaiveDate};
use tp_core::intraday::Bar;
use tp_core::liquidity::simple_atr;
use tp_core::market::Candle;
use tp_core::stormer::ema;
use tp_core::support::{horizontal_levels, support_below};
use tp_data::history::{processed_dir, read_series};
use tp_data::intraday::parse_bars_csv;

use super::crypto_study::Universe;
use super::{CliResult, count_f64, daily_bars, held_out_start, r_mean_t};

const BIG: [&str; 3] = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];
const DAY_MS: i64 = 24 * 3600 * 1000;
/// Round-trip cost per unit of money traded.
const COST: f64 = 0.002;
/// Levels: one year of daily candles, swing points with 5 candles on each
/// side, joined within half an ATR, at least 2 touches.
const LOOKBACK: usize = 365;
const WING: usize = 5;
const TOLERANCE_ATR: f64 = 0.5;
const MIN_TOUCHES: usize = 2;
/// The order goes this much above the level ("near it, not on it").
const ABOVE_LEVEL: f64 = 0.005;
/// A support further than this below the last close is not used.
const MAX_DISTANCE: f64 = 0.15;

/// How a plan exits.
#[derive(Debug, Clone, Copy)]
enum Exit {
    /// Target above the average price, out at the end of the day after the
    /// fill at the latest.
    Quick { target: f64 },
    /// Stop to the average price after a bounce of `breakeven`, half out at
    /// `half_at` above the average, the rest under a stop `trail` below the
    /// highest price, for at most `max_days`.
    Runner {
        breakeven: f64,
        half_at: f64,
        trail: f64,
        max_days: usize,
    },
}

/// Where the stop is.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// A price.
    At(f64),
    /// This share below the average price.
    Below(f64),
}

/// Orders placed on one day.
#[derive(Debug, Clone)]
struct Plan {
    day: usize,
    /// Limit price and share of the money; `f64::INFINITY` buys at the open.
    entries: Vec<(f64, f64)>,
    stop: Stop,
    exit: Exit,
    /// Days the entry orders stay open.
    valid_days: usize,
    /// The plan is dropped if the price reaches this before any fill.
    cancel_above: f64,
}

/// What happened to a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Outcome {
    /// Result as a share of the money set aside for the plan, with costs.
    result: f64,
    /// Share of the money that was bought.
    filled: f64,
    /// Entries filled.
    fills: usize,
    /// Day of the first fill, and the day the plan ended.
    first_day: usize,
    last_day: usize,
}

/// The candles a plan is followed on: hourly or daily, grouped by day.
struct Track {
    bars: Vec<Bar>,
    /// Range of `bars` of each daily candle (empty when missing).
    days: Vec<(usize, usize)>,
}

impl Track {
    fn daily(bars: &[Bar]) -> Self {
        Self {
            bars: bars.to_vec(),
            days: (0..bars.len()).map(|i| (i, i + 1)).collect(),
        }
    }

    /// Hourly bars grouped by the daily candles' days.
    fn hourly(daily: &[Bar], hourly: Vec<Bar>) -> Self {
        let mut days = Vec::with_capacity(daily.len());
        let mut j = 0;
        for d in daily {
            while j < hourly.len() && hourly[j].open_time_ms < d.open_time_ms {
                j += 1;
            }
            let start = j;
            while j < hourly.len() && hourly[j].open_time_ms < d.open_time_ms + DAY_MS {
                j += 1;
            }
            days.push((start, j));
        }
        Self { bars: hourly, days }
    }

    fn has_day(&self, day: usize) -> bool {
        self.days.get(day).is_some_and(|(a, b)| b > a)
    }
}

/// Follows a plan bar by bar. Inside one bar the worst order is assumed:
/// fills first, then the stop, and targets only from the next bar.
#[allow(clippy::too_many_lines)] // one pass over the bars with every rule in order
fn follow(plan: &Plan, path: &Track, atr: f64) -> Option<Outcome> {
    let (start, _) = *path.days.get(plan.day)?;
    let last_order_day = plan.day + plan.valid_days - 1;
    let mut pending: Vec<(f64, f64)> = plan.entries.clone();
    let mut cost_basis = 0.0; // money paid for what is held
    let mut units = 0.0;
    let mut result = 0.0;
    let mut filled = 0.0;
    let mut fills = 0;
    let mut first_day = None;
    let mut stop_price: Option<f64> = None;
    let mut breakeven_done = false;
    let mut half_done = false;
    let mut highest = f64::MIN;
    for day in plan.day..path.days.len() {
        let (a, b) = path.days[day];
        if a == b {
            if units > 0.0 || day <= last_order_day {
                continue;
            }
            break;
        }
        for bar in path.bars.iter().take(b).skip(a.max(start)) {
            // Orders.
            let mut filled_now = false;
            if day <= last_order_day && !breakeven_done {
                if fills == 0
                    && plan.entries.iter().all(|(p, _)| bar.low > *p)
                    && bar.high >= plan.cancel_above
                {
                    return Some(Outcome {
                        result: 0.0,
                        filled: 0.0,
                        fills: 0,
                        first_day: day,
                        last_day: day,
                    });
                }
                pending.retain(|(price, share)| {
                    if bar.low <= *price || price.is_infinite() {
                        let fill = price.min(bar.open);
                        units += share / fill;
                        cost_basis += share;
                        filled += share;
                        fills += 1;
                        result -= share * COST;
                        filled_now = true;
                        false
                    } else {
                        true
                    }
                });
                if filled_now && first_day.is_none() {
                    first_day = Some(day);
                }
            }
            if units == 0.0 {
                continue;
            }
            let avg = cost_basis / units;
            let stop = match (stop_price, plan.stop) {
                (Some(s), _) | (None, Stop::At(s)) => s,
                (None, Stop::Below(share)) => avg * (1.0 - share),
            };
            // Stop (also on the bar of the fill).
            if bar.low <= stop {
                let exit = stop.min(bar.open);
                result += units * exit - cost_basis;
                return Some(Outcome {
                    result,
                    filled,
                    fills,
                    first_day: first_day.unwrap_or(day),
                    last_day: day,
                });
            }
            highest = highest.max(bar.high);
            match plan.exit {
                Exit::Quick { target } => {
                    let goal = avg * (1.0 + target);
                    if !filled_now && bar.high >= goal {
                        let exit = goal.max(bar.open);
                        result += units * exit - cost_basis;
                        return Some(Outcome {
                            result,
                            filled,
                            fills,
                            first_day: first_day.unwrap_or(day),
                            last_day: day,
                        });
                    }
                }
                Exit::Runner {
                    breakeven, half_at, ..
                } => {
                    if !filled_now && !half_done && bar.high >= avg * (1.0 + half_at) {
                        let exit = (avg * (1.0 + half_at)).max(bar.open);
                        let sold = units / 2.0;
                        result += sold * exit - cost_basis / 2.0;
                        units -= sold;
                        cost_basis /= 2.0;
                        half_done = true;
                    }
                    if !filled_now && !breakeven_done && bar.high >= avg * (1.0 + breakeven) {
                        breakeven_done = true;
                        pending.clear();
                    }
                }
            }
            // The stop for the next bar.
            if let Exit::Runner { trail, .. } = plan.exit
                && breakeven_done
            {
                let mut s = stop.max(avg * (1.0 + COST));
                if half_done {
                    s = s.max(highest - trail * atr);
                }
                stop_price = Some(s);
            }
        }
        // End of day: time exits.
        if units > 0.0 {
            let since = day - first_day.unwrap_or(day);
            let out = match plan.exit {
                Exit::Quick { .. } => since >= 1,
                Exit::Runner { max_days, .. } => since >= max_days,
            };
            if out {
                let close = path.bars[b - 1].close;
                result += units * close - cost_basis;
                return Some(Outcome {
                    result,
                    filled,
                    fills,
                    first_day: first_day.unwrap_or(day),
                    last_day: day,
                });
            }
        } else if day >= last_order_day {
            // The orders were never reached.
            return Some(Outcome {
                result: 0.0,
                filled: 0.0,
                fills: 0,
                first_day: day,
                last_day: day,
            });
        }
    }
    None
}

/// Supports below the close of day `t`, nearest first.
fn supports_at(candles: &[Candle], bars: &[Bar], atr: Option<f64>, t: usize) -> Vec<f64> {
    let Some(a) = atr.filter(|a| *a > 0.0) else {
        return Vec::new();
    };
    let levels = horizontal_levels(candles, t, LOOKBACK, WING, TOLERANCE_ATR * a);
    let mut out = Vec::new();
    let mut below = bars[t].close;
    while let Some(s) = support_below(&levels, below, MIN_TOUCHES) {
        out.push(s.price);
        below = s.price * 0.995;
    }
    out
}

/// One coin ready for the plans.
struct Coin {
    symbol: String,
    bars: Vec<Bar>,
    atr: Vec<Option<f64>>,
    fast: Vec<Option<f64>>,
    slow: Vec<Option<f64>>,
    path: Track,
    /// Supports below each day's close, nearest first (from day 200 on).
    supports: Vec<Vec<f64>>,
}

impl Coin {
    fn new(symbol: &str, candles: &[Candle], hourly: Option<Vec<Bar>>) -> Option<Self> {
        let bars = daily_bars(candles);
        if bars.len() != candles.len() || bars.len() < 200 {
            return None;
        }
        let path = hourly.map_or_else(|| Track::daily(&bars), |h| Track::hourly(&bars, h));
        let atr = simple_atr(candles, 14);
        let supports = (0..bars.len())
            .map(|t| {
                if t < 200 {
                    Vec::new()
                } else {
                    supports_at(candles, &bars, atr[t], t)
                }
            })
            .collect();
        Some(Self {
            symbol: symbol.to_owned(),
            fast: ema(&bars, 8),
            slow: ema(&bars, 80),
            atr,
            bars,
            path,
            supports,
        })
    }

    fn uptrend(&self, t: usize) -> Option<bool> {
        Some(self.fast[t]? > self.slow[t]?)
    }

    fn drop_from_high(&self, t: usize) -> f64 {
        let from = t.saturating_sub(89);
        let high = self.bars[from..=t]
            .iter()
            .map(|b| b.high)
            .fold(f64::MIN, f64::max);
        1.0 - self.bars[t].close / high
    }
}

/// The plans tested, each with its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    QuickUp(u8),
    QuickDown(u8),
    Pullback,
    TwoNextLevel { hold: bool },
    TwoFixed { hold: bool },
}

impl Kind {
    const ALL: [Self; 9] = [
        Self::QuickUp(3),
        Self::QuickUp(5),
        Self::QuickDown(3),
        Self::QuickDown(5),
        Self::Pullback,
        Self::TwoNextLevel { hold: false },
        Self::TwoNextLevel { hold: true },
        Self::TwoFixed { hold: false },
        Self::TwoFixed { hold: true },
    ];

    fn label(self) -> String {
        match self {
            Self::QuickUp(p) => format!(
                "Toque no suporte com o mercado em alta (médias 8 acima da 80): compra 0,5% acima do suporte, stop 3% abaixo da compra, alvo de {p}%, sai no fim do dia seguinte"
            ),
            Self::QuickDown(p) => format!(
                "Toque no suporte com o mercado em queda (média 8 abaixo da 80): compra 0,5% acima do suporte, stop 3% abaixo da compra, alvo de {p}%, sai no fim do dia seguinte"
            ),
            Self::Pullback => "Recuo na alta: compra no primeiro suporte com o mercado em alta, stop 1 ATR abaixo do suporte, stop no zero a zero depois de subir 3%, metade vendida a 2 vezes o risco e o resto com stop seguindo o preço (até 60 dias)".to_owned(),
            Self::TwoNextLevel { hold } => format!(
                "Duas entradas na queda (preço 20% ou mais abaixo da máxima de 90 dias): metade no suporte e metade no suporte seguinte abaixo, stop 3% abaixo da segunda entrada; {}",
                if hold { "segura: stop no zero a zero só depois de subir 10%" } else { "zero a zero rápido: stop no preço médio depois de subir 3%" }
            ),
            Self::TwoFixed { hold } => format!(
                "Duas entradas na queda (preço 20% ou mais abaixo da máxima de 90 dias): metade no suporte e metade 8% abaixo dele, stop 3% abaixo da segunda entrada; {}",
                if hold { "segura: stop no zero a zero só depois de subir 10%" } else { "zero a zero rápido: stop no preço médio depois de subir 3%" }
            ),
        }
    }

    /// The plan and the base for day `t + 1`, decided at the close of `t`.
    #[allow(clippy::too_many_lines)] // one block per kind of plan
    fn plans(self, coin: &Coin, t: usize) -> Option<(Plan, Plan)> {
        let close = coin.bars[t].close;
        let atr = coin.atr[t]?;
        let supports = &coin.supports[t];
        let first = *supports
            .first()
            .filter(|s| **s >= close * (1.0 - MAX_DISTANCE))?;
        let day = t + 1;
        let runner = |breakeven: f64, half_at: f64| Exit::Runner {
            breakeven,
            half_at,
            trail: 3.0,
            max_days: 60,
        };
        match self {
            Self::QuickUp(p) | Self::QuickDown(p) => {
                let up = coin.uptrend(t)?;
                if up != matches!(self, Self::QuickUp(_)) {
                    return None;
                }
                let exit = Exit::Quick {
                    target: f64::from(p) / 100.0,
                };
                let entry = first * (1.0 + ABOVE_LEVEL);
                Some((
                    Plan {
                        day,
                        entries: vec![(entry, 1.0)],
                        stop: Stop::Below(0.03),
                        exit,
                        valid_days: 1,
                        cancel_above: f64::INFINITY,
                    },
                    Plan {
                        day,
                        entries: vec![(f64::INFINITY, 1.0)],
                        stop: Stop::Below(0.03),
                        exit,
                        valid_days: 1,
                        cancel_above: f64::INFINITY,
                    },
                ))
            }
            Self::Pullback => {
                if !coin.uptrend(t)? {
                    return None;
                }
                let entry = first * (1.0 + ABOVE_LEVEL);
                let stop = first - atr;
                let risk = 1.0 - stop / entry;
                if !(risk > 0.0 && risk < 0.5) {
                    return None;
                }
                let exit = runner(0.03, 2.0 * risk);
                Some((
                    Plan {
                        day,
                        entries: vec![(entry, 1.0)],
                        stop: Stop::At(stop),
                        exit,
                        valid_days: 1,
                        cancel_above: f64::INFINITY,
                    },
                    Plan {
                        day,
                        entries: vec![(f64::INFINITY, 1.0)],
                        stop: Stop::Below(risk),
                        exit,
                        valid_days: 1,
                        cancel_above: f64::INFINITY,
                    },
                ))
            }
            Self::TwoNextLevel { hold } | Self::TwoFixed { hold } => {
                if coin.drop_from_high(t) < 0.20 {
                    return None;
                }
                let second = match self {
                    Self::TwoNextLevel { .. } => *supports.get(1)?,
                    _ => first * 0.92,
                };
                let (e1, e2) = (first * (1.0 + ABOVE_LEVEL), second * (1.0 + ABOVE_LEVEL));
                let stop = second * 0.97;
                let avg = 2.0 / (1.0 / e1 + 1.0 / e2);
                let risk = 1.0 - stop / avg;
                if !(risk > 0.0 && risk < 0.6) {
                    return None;
                }
                let exit = runner(if hold { 0.10 } else { 0.03 }, 2.0 * risk);
                Some((
                    Plan {
                        day,
                        entries: vec![(e1, 0.5), (e2, 0.5)],
                        stop: Stop::At(stop),
                        exit,
                        valid_days: 10,
                        cancel_above: e1 * 1.10,
                    },
                    Plan {
                        day,
                        entries: vec![(f64::INFINITY, 1.0)],
                        stop: Stop::Below(risk),
                        exit,
                        valid_days: 1,
                        cancel_above: f64::INFINITY,
                    },
                ))
            }
        }
    }
}

/// One result: day of the first fill (ms), result, share bought, fills.
type Row = (i64, f64, f64, usize);

#[derive(Default)]
struct Rows {
    plan: Vec<Row>,
    base: Vec<Row>,
    /// Plans whose orders were never reached.
    missed: usize,
}

/// Runs one kind of plan over some coins. A coin holds one plan at a time;
/// the next one starts the day after the previous one ended.
fn run(kind: Kind, coins: &[Coin], universe: Option<&Universe>) -> Rows {
    let mut rows = Rows::default();
    for coin in coins {
        let mut t = 200;
        let mut base_free = 200;
        while t + 1 < coin.bars.len() {
            let in_universe =
                universe.is_none_or(|u| u.has(&coin.symbol, coin.bars[t].open_time_ms));
            if !in_universe || !coin.path.has_day(t + 1) {
                t += 1;
                continue;
            }
            let Some((plan, base)) = kind.plans(coin, t) else {
                t += 1;
                continue;
            };
            let atr = coin.atr[t].unwrap_or(0.0);
            if t >= base_free
                && let Some(o) = follow(&base, &coin.path, atr)
            {
                rows.base.push((
                    coin.bars[o.first_day].open_time_ms,
                    o.result,
                    o.filled,
                    o.fills,
                ));
                base_free = o.last_day + 1;
            }
            match follow(&plan, &coin.path, atr) {
                Some(o) if o.fills > 0 => {
                    rows.plan.push((
                        coin.bars[o.first_day].open_time_ms,
                        o.result,
                        o.filled,
                        o.fills,
                    ));
                    t = o.last_day.max(t + 1);
                }
                Some(o) => {
                    rows.missed += 1;
                    t = o.last_day.max(t + 1);
                }
                None => t += 1,
            }
        }
    }
    rows.plan.sort_by_key(|r| r.0);
    rows.base.sort_by_key(|r| r.0);
    rows
}

fn pct(v: f64) -> String {
    let s = format!("{:+.2}%", v * 100.0).replace('.', ",");
    s.replace("+-", "-")
}

fn split(rows: &[Row], split_ms: i64, late: bool) -> Vec<f64> {
    rows.iter()
        .filter(|r| (r.0 > split_ms) == late)
        .map(|r| r.1)
        .collect()
}

/// Prints one kind and says whether it passed.
fn report(kind: Kind, rows: &Rows, split_ms: i64) -> bool {
    println!();
    println!(" {}", kind.label());
    if rows.plan.is_empty() {
        println!("   Nenhuma operação.");
        return false;
    }
    let all: Vec<f64> = rows.plan.iter().map(|r| r.1).collect();
    let (dev, fin) = (
        split(&rows.plan, split_ms, false),
        split(&rows.plan, split_ms, true),
    );
    let (bdev, bfin) = (
        split(&rows.base, split_ms, false),
        split(&rows.base, split_ms, true),
    );
    let (mean, _) = r_mean_t(&all);
    let (dev_mean, dev_t) = r_mean_t(&dev);
    let (fin_mean, _) = r_mean_t(&fin);
    let (bdev_mean, _) = r_mean_t(&bdev);
    let (bfin_mean, _) = r_mean_t(&bfin);
    let wins = all.iter().filter(|r| **r > 0.0).count();
    let mut sorted = all.clone();
    sorted.sort_by(f64::total_cmp);
    let both = rows.plan.iter().filter(|r| r.3 >= 2).count();
    let fill_text = if matches!(kind, Kind::TwoNextLevel { .. } | Kind::TwoFixed { .. }) {
        format!(
            " Pegou só a primeira ordem em {} e as duas em {both}; em {} vezes o preço não chegou na ordem e subiu (passou do lado).",
            rows.plan.len() - both,
            rows.missed
        )
    } else {
        String::new()
    };
    println!(
        "   {} operações. Resultado médio {} do dinheiro separado por operação, caso típico {}, deram lucro {:.0}%. Pior operação {}; 1 em cada 10 foi pior que {}.{fill_text}",
        all.len(),
        pct(mean),
        pct(sorted[sorted.len() / 2]),
        count_f64(wins) / count_f64(all.len()) * 100.0,
        pct(sorted[0]),
        pct(sorted[sorted.len() / 10]),
    );
    println!(
        "   Até out/2024: {} em {} operações; de nov/2024 até hoje: {} em {}. Base (comprar na abertura de qualquer dia na mesma situação, com o mesmo stop e a mesma saída): {} e {}.",
        pct(dev_mean),
        dev.len(),
        pct(fin_mean),
        fin.len(),
        pct(bdev_mean),
        pct(bfin_mean),
    );
    let proven = dev.len() >= 30 && dev_t >= 2.0 && fin.len() >= 10 && fin_mean > 0.0;
    let beats =
        !bdev.is_empty() && !bfin.is_empty() && dev_mean > bdev_mean && fin_mean > bfin_mean;
    let verdict = match (proven, beats, dev_mean > 0.0 && fin_mean > 0.0) {
        (true, true, _) => "GANHOU NOS DOIS PERÍODOS, COM PROVA, E GANHOU DA BASE",
        (true, false, _) => {
            "ganhou com prova, mas NÃO ganhou de comprar em qualquer dia na mesma situação"
        }
        (false, true, true) => {
            "positivo e acima da base nos dois períodos, mas sem prova (pode ser sorte)"
        }
        (false, false, true) => "positivo nos dois períodos, mas não ganhou da base",
        _ => "PERDEU",
    };
    println!("   {verdict}.");
    proven && beats
}

/// Falls of 15% or more from a top to a bottom (a bottom counts after a
/// 15% rise from it), and the rise that followed.
fn legs(bars: &[Bar]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut peak = bars.first().map_or(0.0, |b| b.high);
    let mut trough = f64::MAX;
    let mut falling = false;
    let mut pending: Option<f64> = None;
    for b in bars {
        if falling {
            trough = trough.min(b.low);
            if b.high >= trough * 1.15 {
                pending = Some(1.0 - trough / peak);
                falling = false;
                peak = b.high;
            }
        } else {
            if b.high > peak {
                peak = b.high;
            }
            if b.low <= peak * 0.85 {
                if let Some(depth) = pending.take() {
                    out.push((depth, peak / trough - 1.0));
                }
                falling = true;
                trough = b.low;
            }
        }
    }
    out
}

fn print_legs(coin: &Coin) {
    let l = legs(&coin.bars);
    if l.is_empty() {
        return;
    }
    let buckets = [
        (0.15, 0.20),
        (0.20, 0.30),
        (0.30, 0.40),
        (0.40, 0.50),
        (0.50, 1.0),
    ];
    let parts: Vec<String> = buckets
        .iter()
        .map(|(lo, hi)| {
            let n = l.iter().filter(|(d, _)| *d >= *lo && *d < *hi).count();
            if *hi >= 1.0 {
                format!("{n} de 50% ou mais")
            } else {
                format!("{n} de {:.0}% a {:.0}%", lo * 100.0, hi * 100.0)
            }
        })
        .collect();
    let mut rises: Vec<f64> = l.iter().map(|(_, r)| *r).collect();
    rises.sort_by(f64::total_cmp);
    println!(
        "  {}: {} quedas de 15% ou mais desde {}: {}. Depois do fundo de cada queda, a alta típica foi de {:.0}%.",
        coin.symbol.trim_end_matches("USDT"),
        l.len(),
        DateTime::from_timestamp_millis(coin.bars[0].open_time_ms)
            .map_or(String::new(), |d| d.format("%Y").to_string()),
        parts.join(", "),
        rises[rises.len() / 2] * 100.0
    );
}

fn load(data_dir: &Path, symbol: &str, hourly: bool) -> Option<Coin> {
    let candles = read_series(&processed_dir(data_dir).join(format!("{symbol}.csv"))).ok()?;
    let hourly = if hourly {
        let path = data_dir
            .join("processed")
            .join("binance")
            .join("futures_1h")
            .join(format!("{symbol}.csv"));
        Some(parse_bars_csv(&std::fs::read_to_string(path).ok()?).ok()?)
    } else {
        None
    };
    Coin::new(symbol, &candles, hourly)
}

/// Runs the study and prints it.
///
/// # Errors
///
/// Returns an error when the BTC, ETH or SOL history is missing.
pub fn cmd_support_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Compra no suporte, do jeito descrito pelo Vi, em regras fixas. Suporte = preço onde os candles diários mais bateram no último ano (pontos de virada agrupados, pelo menos 2 toques), visto só com os dados até o dia anterior. A ordem fica 0,5% acima do suporte. Sem alavancagem. Custo de 0,2% por compra e venda."
    );
    println!(
        "Resultado em % do dinheiro separado para a operação (nas duas entradas, metade em cada ordem). Para passar: prova antes de nov/2024, positivo depois e acima da base nos dois períodos."
    );
    let mut big = Vec::new();
    for s in BIG {
        let coin = load(data_dir, s, true).ok_or_else(|| {
            format!(
                "{s}: daily or hourly history missing (processed/binance/spot/1d and futures_1h)"
            )
        })?;
        big.push(coin);
    }
    println!();
    println!("Como foram as quedas (candles diários, do topo ao fundo):");
    for c in &big {
        print_legs(c);
    }
    let universe = super::crypto_study::load(data_dir).ok();
    let mut others = Vec::new();
    if let Some(u) = &universe {
        let symbols: std::collections::BTreeSet<&String> = u.symbols().collect();
        for s in symbols {
            if BIG.contains(&s.as_str()) {
                continue;
            }
            if let Some(c) = load(data_dir, s, false) {
                others.push(c);
            }
        }
    }
    let first_hour = big
        .iter()
        .filter_map(|c| c.path.bars.first())
        .map(|b| b.open_time_ms)
        .max()
        .and_then(DateTime::from_timestamp_millis)
        .map_or(NaiveDate::MIN, |d| d.date_naive());
    println!();
    println!("BTC, ETH e SOL, seguindo hora a hora (dados de hora em hora desde {first_hour}):");
    let mut good = Vec::new();
    for kind in Kind::ALL {
        let rows = run(kind, &big, None);
        if report(kind, &rows, split_ms) {
            good.push(format!("BTC, ETH e SOL: {}", kind.label()));
        }
    }
    if !others.is_empty() {
        println!();
        println!(
            "As outras {} moedas do universo, só nos meses em que estavam nele, com candles diários (dentro do dia, supõe a pior ordem: stop antes do alvo). Teste independente:",
            others.len()
        );
        for kind in Kind::ALL {
            let rows = run(kind, &others, universe.as_ref());
            if report(kind, &rows, split_ms) {
                good.push(format!("Outras moedas: {}", kind.label()));
            }
        }
    }
    println!();
    println!("RESUMO: o que passou");
    if good.is_empty() {
        println!(" Nada.");
    }
    for g in good {
        println!(" {g}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bar(day: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: day * DAY_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    fn plan(entries: Vec<(f64, f64)>, stop: Stop, exit: Exit) -> Plan {
        Plan {
            day: 0,
            entries,
            stop,
            exit,
            valid_days: 3,
            cancel_above: f64::INFINITY,
        }
    }

    #[test]
    fn a_quick_trade_takes_the_target_from_the_next_bar() {
        let path = Track::daily(&[
            bar(0, 105.0, 106.0, 99.0, 104.0),
            bar(1, 104.0, 106.0, 103.0, 105.0),
        ]);
        let o = follow(
            &plan(
                vec![(100.0, 1.0)],
                Stop::Below(0.03),
                Exit::Quick { target: 0.05 },
            ),
            &path,
            1.0,
        )
        .unwrap();
        // Bought at 100 on day 0 (the high of 106 that day does not count), sold at 105.
        assert!((o.result - (0.05 - COST)).abs() < 1e-9, "{o:?}");
        assert_eq!((o.fills, o.first_day, o.last_day), (1, 0, 1));
    }

    #[test]
    fn the_stop_on_the_fill_bar_counts_and_a_gap_fills_at_the_open() {
        let path = Track::daily(&[bar(0, 98.0, 99.0, 90.0, 92.0)]);
        let o = follow(
            &plan(
                vec![(100.0, 1.0)],
                Stop::Below(0.03),
                Exit::Quick { target: 0.05 },
            ),
            &path,
            1.0,
        )
        .unwrap();
        // Filled at the open, 98; stop 3% below, at 95.06.
        assert!((o.result - (0.97 - 1.0 - COST)).abs() < 1e-9, "{o:?}");
    }

    #[test]
    fn two_entries_average_down_and_break_even_cancels_the_rest() {
        let exit = Exit::Runner {
            breakeven: 0.03,
            half_at: 0.5,
            trail: 3.0,
            max_days: 60,
        };
        // Day 0 fills the first half at 100; day 1 bounces 4% and falls back
        // to 100: out at break-even; the second order (90) never fills.
        let path = Track::daily(&[
            bar(0, 101.0, 101.0, 99.5, 100.0),
            bar(1, 100.0, 104.0, 100.5, 103.0),
            bar(2, 103.0, 103.0, 85.0, 86.0),
        ]);
        let o = follow(
            &plan(vec![(100.0, 0.5), (90.0, 0.5)], Stop::At(80.0), exit),
            &path,
            1.0,
        )
        .unwrap();
        assert_eq!(o.fills, 1);
        assert_eq!(o.last_day, 2);
        // Half the money, out at the average plus costs: about zero.
        assert!(o.result.abs() < 0.002, "{o:?}");
    }

    #[test]
    fn falls_and_the_rises_after_them_are_measured() {
        let bars = vec![
            bar(0, 100.0, 100.0, 99.0, 100.0),
            bar(1, 99.0, 99.0, 70.0, 72.0),
            bar(2, 72.0, 90.0, 72.0, 88.0),
            bar(3, 88.0, 95.0, 80.0, 82.0),
            bar(4, 82.0, 82.0, 75.0, 76.0),
        ];
        let l = legs(&bars);
        assert_eq!(l.len(), 1);
        assert!((l[0].0 - 0.30).abs() < 1e-9);
        // From 70 to the next top, 95.
        assert!((l[0].1 - (95.0 / 70.0 - 1.0)).abs() < 1e-9);
    }
}
