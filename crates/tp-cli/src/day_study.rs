//! `tp day-study`: one trade a day on a short chart (15 minutes or 1 hour)
//! with the setup that held up on the longer charts, the trap (price trades
//! past a low or high and closes back), with Stormer's entry. Each variant
//! takes only the first trade of each UTC day and is compared with entering
//! on any candle with the same stop distance, target and filter.

use std::collections::BTreeMap;
use std::path::Path;

use tp_core::intraday::Bar;
use tp_core::stormer::{Exit, Params, Pattern, Side, ema, follow, mirror, scan};
use tp_data::intraday::parse_bars_csv;

use super::{CliResult, count_f64, held_out_start};
use crate::stormer_study::{Base, Row, line};

const DAY_MS: i64 = 24 * 3600 * 1000;
const MINUTE_MS: i64 = 60 * 1000;
/// Futures taker fee 0.05% per side plus slippage, as a share of the entry.
const COST: f64 = 0.0012;
pub const SYMBOLS: [&str; 6] = [
    "BTCUSDT", "ETHUSDT", "SOLUSDT", "XRPUSDT", "BNBUSDT", "DOGEUSDT",
];

/// Candles of `step_ms` built from smaller candles of `from_ms`; a candle is
/// kept only when every smaller one is present.
#[must_use]
pub fn regroup(bars: &[Bar], from_ms: i64, step_ms: i64) -> Vec<Bar> {
    let per = usize::try_from(step_ms / from_ms).unwrap_or(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bars.len() {
        let key = bars[i].open_time_ms.div_euclid(step_ms);
        let mut j = i;
        while j < bars.len() && bars[j].open_time_ms.div_euclid(step_ms) == key {
            j += 1;
        }
        let group = &bars[i..j];
        if group.len() == per {
            out.push(Bar {
                open_time_ms: key * step_ms,
                open: group[0].open,
                high: group.iter().map(|b| b.high).fold(f64::MIN, f64::max),
                low: group.iter().map(|b| b.low).fold(f64::MAX, f64::min),
                close: group[group.len() - 1].close,
                volume: group.iter().map(|b| b.volume).sum(),
            });
        }
        i = j;
    }
    out
}

/// What the previous complete day says, for each UTC day.
#[derive(Debug, Clone, Copy)]
struct DayInfo {
    prev_low: f64,
    prev_high: f64,
    /// +1 up, -1 down, 0 neither (8 and 80 day averages at the previous close).
    trend: i8,
}

fn day_infos(daily: &[Bar]) -> BTreeMap<i64, DayInfo> {
    let fast = ema(daily, 8);
    let slow = ema(daily, 80);
    let mut out = BTreeMap::new();
    for i in 1..daily.len() {
        let p = i - 1;
        let trend = match (fast[p], slow[p]) {
            (Some(f), Some(s)) if daily[p].close > f && f > s => 1,
            (Some(f), Some(s)) if daily[p].close < f && f < s => -1,
            _ => 0,
        };
        out.insert(
            daily[i].open_time_ms.div_euclid(DAY_MS),
            DayInfo {
                prev_low: daily[p].low,
                prev_high: daily[p].high,
                trend,
            },
        );
    }
    out
}

/// A triggered trade before the one-a-day choice.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    entry_index: usize,
    side: Side,
    r: f64,
    how: Exit,
    risk: f64,
}

fn day_of(bars: &[Bar], i: usize) -> i64 {
    bars[i].open_time_ms.div_euclid(DAY_MS)
}

/// Sweeps of the previous day's low (buys) or high (sells): a candle trades
/// past it and closes back; entry on the break of that candle.
fn previous_day_sweeps(bars: &[Bar], days: &BTreeMap<i64, DayInfo>, p: Params) -> Vec<Candidate> {
    let mut out = Vec::new();
    for side in [Side::Long, Side::Short] {
        let view = match side {
            Side::Long => bars.to_vec(),
            Side::Short => mirror(bars),
        };
        let mut free_from = 0;
        for i in 0..view.len().saturating_sub(1) {
            if i < free_from {
                continue;
            }
            let Some(info) = days.get(&day_of(bars, i)) else {
                continue;
            };
            let level = match side {
                Side::Long => info.prev_low,
                Side::Short => -info.prev_high,
            };
            let b = view[i];
            if !(b.low < level && b.close > level) {
                continue;
            }
            let (trigger, stop) = (b.high, b.low);
            let k = i + 1;
            if view[k].high <= trigger {
                continue;
            }
            let entry = trigger.max(view[k].open);
            let risk = entry - stop;
            if risk <= 0.0 {
                continue;
            }
            let target = entry + p.reward * risk;
            let (exit_index, exit, how) = follow(&view, k, stop, target, p.max_hold);
            if how == Exit::Open {
                continue;
            }
            out.push(Candidate {
                entry_index: k,
                side,
                r: (exit - entry) / risk - p.cost * entry.abs() / risk,
                how,
                risk: risk / entry.abs(),
            });
            free_from = exit_index + 1;
        }
    }
    out
}

fn swing_traps(bars: &[Bar], p: Params) -> Vec<Candidate> {
    let (trades, _) = scan(bars, p);
    trades
        .into_iter()
        .filter(|t| t.pattern == Pattern::Trap && t.how != Exit::Open)
        .map(|t| Candidate {
            entry_index: t.entry_index,
            side: t.side,
            r: t.r,
            how: t.how,
            risk: if t.entry == 0.0 {
                0.0
            } else {
                ((t.entry - t.stop) / t.entry).abs()
            },
        })
        .collect()
}

fn allowed(side: Side, info: Option<&DayInfo>, with_trend: bool) -> bool {
    if !with_trend {
        return true;
    }
    matches!(
        (side, info.map(|d| d.trend)),
        (Side::Long, Some(1)) | (Side::Short, Some(-1))
    )
}

/// The first allowed trade of each day.
fn one_a_day(
    bars: &[Bar],
    mut all: Vec<Candidate>,
    days: &BTreeMap<i64, DayInfo>,
    with_trend: bool,
) -> Vec<Candidate> {
    all.sort_by_key(|c| c.entry_index);
    let mut taken_day = None;
    let mut out = Vec::new();
    for c in all {
        let day = day_of(bars, c.entry_index);
        if taken_day == Some(day) || !allowed(c.side, days.get(&day), with_trend) {
            continue;
        }
        taken_day = Some(day);
        out.push(c);
    }
    out
}

fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    Some(xs[xs.len() / 2])
}

/// Mean result of entering at candle opens allowed for `side` (every
/// `stride`-th candle, which keeps the average and the run time short), with
/// a stop `risk` away and the same target, hold and cost; per period.
#[allow(clippy::too_many_arguments)] // the base mirrors every rule of the chosen trades
fn base_means(
    bars: &[Bar],
    side: Side,
    risk: f64,
    days: &BTreeMap<i64, DayInfo>,
    with_trend: bool,
    p: Params,
    split_ms: i64,
    stride: usize,
) -> (Option<f64>, Option<f64>) {
    let view = match side {
        Side::Long => bars.to_vec(),
        Side::Short => mirror(bars),
    };
    let (mut dev, mut fin) = (Vec::new(), Vec::new());
    for k in (1..view.len()).step_by(stride.max(1)) {
        if !allowed(side, days.get(&day_of(bars, k)), with_trend) {
            continue;
        }
        let entry = view[k].open;
        let distance = risk * entry.abs();
        let (_, exit, how) = follow(
            &view,
            k,
            entry - distance,
            entry + p.reward * distance,
            p.max_hold,
        );
        if how == Exit::Open {
            continue;
        }
        let r = (exit - entry) / distance - p.cost * entry.abs() / distance;
        if bars[k].open_time_ms <= split_ms {
            dev.push(r);
        } else {
            fin.push(r);
        }
    }
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / count_f64(v.len()));
    (mean(&dev), mean(&fin))
}

/// Trades chosen for one asset, one chart and one variant, and their base.
struct Chosen {
    rows: Vec<Row>,
    /// Per period: (sum of base mean x trades, trades).
    dev: (f64, usize),
    fin: (f64, usize),
}

fn choose(
    bars: &[Bar],
    candidates: &[Candidate],
    days: &BTreeMap<i64, DayInfo>,
    with_trend: bool,
    only: Option<Side>,
    p: Params,
    split_ms: i64,
) -> Chosen {
    let pool: Vec<Candidate> = candidates
        .iter()
        .copied()
        .filter(|c| only.is_none_or(|s| c.side == s))
        .collect();
    let picked = one_a_day(bars, pool, days, with_trend);
    let rows: Vec<Row> = picked
        .iter()
        .map(|c| (bars[c.entry_index].open_time_ms, c.r, c.how))
        .collect();
    let mut chosen = Chosen {
        rows,
        dev: (0.0, 0),
        fin: (0.0, 0),
    };
    for side in [Side::Long, Side::Short] {
        let mine: Vec<&Candidate> = picked.iter().filter(|c| c.side == side).collect();
        let Some(risk) = median(mine.iter().map(|c| c.risk).collect()) else {
            continue;
        };
        // About one base entry every two hours on either chart.
        let stride = if p.max_hold > 24 { 8 } else { 2 };
        let (b_dev, b_fin) = base_means(bars, side, risk, days, with_trend, p, split_ms, stride);
        let in_dev = mine
            .iter()
            .filter(|c| bars[c.entry_index].open_time_ms <= split_ms)
            .count();
        let in_fin = mine.len() - in_dev;
        if let Some(m) = b_dev {
            chosen.dev.0 += m * count_f64(in_dev);
            chosen.dev.1 += in_dev;
        }
        if let Some(m) = b_fin {
            chosen.fin.0 += m * count_f64(in_fin);
            chosen.fin.1 += in_fin;
        }
    }
    chosen
}

fn merge(parts: Vec<Chosen>) -> Chosen {
    let mut all = Chosen {
        rows: Vec::new(),
        dev: (0.0, 0),
        fin: (0.0, 0),
    };
    for c in parts {
        all.rows.extend(c.rows);
        all.dev.0 += c.dev.0;
        all.dev.1 += c.dev.1;
        all.fin.0 += c.fin.0;
        all.fin.1 += c.fin.1;
    }
    all.rows.sort_by_key(|r| r.0);
    all
}

fn print_line(label: &str, c: &Chosen, split_ms: i64, good: &mut Vec<String>, title: &str) {
    let base = (c.dev.1 > 0 && c.fin.1 > 0).then(|| Base {
        dev: c.dev.0 / count_f64(c.dev.1),
        fin: c.fin.0 / count_f64(c.fin.1),
    });
    let (text, ok) = line(label, &c.rows, split_ms, base);
    println!(" {text}");
    if ok {
        good.push(format!("{title} · {}", text.trim()));
    }
}

struct Loaded {
    name: &'static str,
    days: BTreeMap<i64, DayInfo>,
    charts: Vec<(&'static str, Vec<Bar>)>,
}

const VARIANTS: [(&str, bool, bool); 4] = [
    ("Trap (fundo/topo recente), 1º do dia", false, false),
    (
        "Trap, 1º do dia, só a favor da tendência do diário",
        false,
        true,
    ),
    (
        "Varrida da mínima/máxima do dia anterior, 1ª do dia",
        true,
        false,
    ),
    (
        "Varrida do dia anterior, 1ª do dia, só a favor da tendência do diário",
        true,
        true,
    ),
];

fn run_chart(
    l: &Loaded,
    chart: &str,
    bars: &[Bar],
    variant: (bool, bool),
    only: Option<Side>,
    split_ms: i64,
) -> Chosen {
    let (previous_day, with_trend) = variant;
    let hold = if chart == "15m" { 96 } else { 24 };
    let p = Params {
        cost: COST,
        ..Params::with_max_hold(hold)
    };
    let candidates = if previous_day {
        previous_day_sweeps(bars, &l.days, p)
    } else {
        swing_traps(bars, p)
    };
    choose(bars, &candidates, &l.days, with_trend, only, p, split_ms)
}

/// Runs the study and prints it.
///
/// # Errors
///
/// Returns an error when no 15-minute history exists.
pub fn cmd_day_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    let dir = data_dir
        .join("processed")
        .join("binance")
        .join("futures_15m");
    let mut loaded = Vec::new();
    for sym in SYMBOLS {
        let path = dir.join(format!("{sym}.csv"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("warning: {} not found", path.display());
            continue;
        };
        let m15 = parse_bars_csv(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let daily = regroup(&m15, 15 * MINUTE_MS, DAY_MS);
        let h1 = regroup(&m15, 15 * MINUTE_MS, 60 * MINUTE_MS);
        loaded.push(Loaded {
            name: sym.strip_suffix("USDT").unwrap_or(sym),
            days: day_infos(&daily),
            charts: vec![("15m", m15), ("1h", h1)],
        });
    }
    if loaded.is_empty() {
        return Err("no 15-minute history: run python3 scripts/futures_klines.py 15m first".into());
    }
    println!(
        "Um trade por dia (o 1º sinal do dia em UTC, 21h de Brasília), no gráfico de 15 minutos ou de 1 hora. Entrada no rompimento do candle de sinal, stop na mínima (máxima, nas vendas) dele, alvo em 2 vezes o risco, fecha em até 24 horas. Custos de 0,12% por trade (taxa de futuros 0,05% por lado mais deslizamento). Base = entrar em qualquer candle com o mesmo stop, alvo e filtro."
    );
    let mut good = Vec::new();
    for chart in ["15m", "1h"] {
        for (label, previous_day, with_trend) in VARIANTS {
            println!();
            println!("{chart} · {label}");
            for (side_label, only) in [
                ("os dois lados", None),
                ("só compras", Some(Side::Long)),
                ("só vendas", Some(Side::Short)),
            ] {
                let mut parts = Vec::new();
                for l in &loaded {
                    let bars = &l
                        .charts
                        .iter()
                        .find(|(c, _)| *c == chart)
                        .map(|(_, b)| b)
                        .ok_or("chart")?;
                    let c = run_chart(l, chart, bars, (previous_day, with_trend), only, split_ms);
                    if only.is_none() {
                        print_line(
                            &format!("{} ({side_label})", l.name),
                            &c,
                            split_ms,
                            &mut good,
                            &format!("{chart} · {label}"),
                        );
                    }
                    parts.push(c);
                }
                print_line(
                    &format!("TODAS JUNTAS ({side_label})"),
                    &merge(parts),
                    split_ms,
                    &mut good,
                    &format!("{chart} · {label}"),
                );
            }
        }
    }
    println!();
    println!("RESUMO: o que ficou positivo e acima da base nos dois períodos");
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

    fn bar(t: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: t,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn regroup_needs_every_small_candle() {
        let q = 15 * MINUTE_MS;
        let bars: Vec<Bar> = (0..7)
            .map(|i| {
                bar(
                    i * q,
                    1.0,
                    2.0 + f64::from(u8::try_from(i).unwrap()),
                    0.5,
                    1.5,
                )
            })
            .collect();
        let h = regroup(&bars, q, 4 * q);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].high, 5.0);
    }

    #[test]
    fn only_the_first_trade_of_a_day_is_taken() {
        let bars: Vec<Bar> = (0..10)
            .map(|i| bar(i * 3600 * 1000, 1.0, 1.0, 1.0, 1.0))
            .collect();
        let c = |i: usize, side: Side| Candidate {
            entry_index: i,
            side,
            r: 1.0,
            how: Exit::Target,
            risk: 0.01,
        };
        let mut days = BTreeMap::new();
        days.insert(
            0,
            DayInfo {
                prev_low: 0.0,
                prev_high: 2.0,
                trend: 1,
            },
        );
        let picked = one_a_day(
            &bars,
            vec![c(5, Side::Short), c(2, Side::Long), c(7, Side::Long)],
            &days,
            false,
        );
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].entry_index, 2);
        // With the trend filter the short is skipped; the first long stays.
        let picked = one_a_day(
            &bars,
            vec![c(1, Side::Short), c(4, Side::Long)],
            &days,
            true,
        );
        assert_eq!(picked[0].entry_index, 4);
    }

    #[test]
    fn a_sweep_of_yesterdays_low_buys_the_break() {
        let h = 3600 * 1000;
        let mut daily = vec![bar(0, 10.0, 11.0, 9.0, 10.0)];
        daily.push(bar(DAY_MS, 10.0, 11.0, 9.0, 10.0));
        let days = day_infos(&daily);
        // Day 1: an hour trades to 8.8 (below 9.0) and closes at 9.2; the next breaks 9.6.
        let bars = vec![
            bar(DAY_MS, 9.5, 9.6, 8.8, 9.2),
            bar(DAY_MS + h, 9.3, 9.8, 9.2, 9.7),
            bar(DAY_MS + 2 * h, 9.7, 11.3, 9.6, 11.2),
        ];
        let p = Params {
            cost: 0.0,
            ..Params::with_max_hold(24)
        };
        let c = previous_day_sweeps(&bars, &days, p);
        let long = c.iter().find(|c| c.side == Side::Long).unwrap();
        assert_eq!(long.entry_index, 1);
        assert_eq!(long.how, Exit::Target);
        assert!((long.r - 2.0).abs() < 1e-9);
    }
}
