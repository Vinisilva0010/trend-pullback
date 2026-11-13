//! `tp crypto-study`: the Stormer setups on the daily and weekly charts of
//! the crypto trade universe (the most traded USDT pairs of each month,
//! built by `tp build-universe`), instead of BTC, ETH and SOL only.
//!
//! A coin's signal counts only in the months it was in the universe, so
//! coins that were big once and died are in the test and today's winners
//! are not picked with hindsight. A setup passes only when it:
//! - wins with proof before Nov 2024 and stays positive after it;
//! - beats the base (entering on any candle with the same stop and target)
//!   in both periods;
//! - does the same without BTC, ETH and SOL (the coins of the earlier test);
//! - stays positive in both periods with twice the costs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Datelike};
use tp_core::intraday::Bar;
use tp_core::stormer::{Exit, Params, Pattern, Side, Trade, baseline, median_risk, scan, weekly};
use tp_data::history::{processed_dir, read_series};
use tp_data::month::YearMonth;

use super::stormer_study::{Base, Row, line, pattern_name};
use super::{CliResult, count_f64, daily_bars, held_out_start, load_universe, r_mean_t, r_pt};

/// The coins of the earlier test, left out in the independent check.
const EARLIER: [&str; 3] = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];
/// Cost added per trade in the cost check (0.2% more, 0.4% in all).
const EXTRA_COST: f64 = 0.002;

struct Frame {
    name: &'static str,
    max_hold: usize,
    weekly: bool,
}

const FRAMES: [Frame; 2] = [
    Frame {
        name: "diário (até 30 dias)",
        max_hold: 30,
        weekly: false,
    },
    Frame {
        name: "semanal (até 12 semanas)",
        max_hold: 12,
        weekly: true,
    },
];

/// Months of the trade universe and who was in each.
pub(crate) struct Universe {
    months: BTreeMap<YearMonth, BTreeSet<String>>,
}

impl Universe {
    /// Every coin that was ever in the universe.
    pub(crate) fn symbols(&self) -> impl Iterator<Item = &String> {
        self.months.values().flatten()
    }

    /// In the universe of the month of `ms` (the last known month when the
    /// universe file ends before it).
    pub(crate) fn has(&self, symbol: &str, ms: i64) -> bool {
        let Some(t) = DateTime::from_timestamp_millis(ms) else {
            return false;
        };
        let Ok(month) = YearMonth::new(t.year(), t.month()) else {
            return false;
        };
        self.months
            .range(..=month)
            .next_back()
            .is_some_and(|(_, members)| members.contains(symbol))
    }
}

/// One coin on one chart: its trades whose signal was in the universe.
struct Chart {
    symbol: String,
    bars: Vec<Bar>,
    trades: Vec<Trade>,
}

type Keep<'a> = &'a dyn Fn(&Trade) -> bool;

fn closed<'a>(c: &'a Chart, keep: Keep<'a>) -> impl Iterator<Item = &'a Trade> {
    c.trades
        .iter()
        .filter(move |t| t.how != Exit::Open && keep(t))
}

/// Extra cost of a trade, in R.
fn cost_r(t: &Trade, extra: f64) -> f64 {
    let risk = (t.entry - t.stop).abs();
    if risk > 0.0 {
        extra * t.entry.abs() / risk
    } else {
        0.0
    }
}

fn rows(charts: &[&Chart], keep: Keep, extra: f64) -> Vec<Row> {
    let mut out: Vec<Row> = charts
        .iter()
        .flat_map(|c| {
            closed(c, keep).map(|t| {
                (
                    c.bars[t.entry_index].open_time_ms,
                    t.r - cost_r(t, extra),
                    t.how,
                )
            })
        })
        .collect();
    out.sort_by_key(|r| r.0);
    out
}

/// Base: per coin, enter on every candle of its universe months with the
/// median stop distance of its trades; coins weigh as many trades as the
/// setup made there in each period.
#[allow(clippy::too_many_arguments)] // the charts, the variant, the split and the cost
fn base(
    charts: &[&Chart],
    universe: &Universe,
    keep: Keep,
    side: Side,
    trend_only: bool,
    max_hold: usize,
    split_ms: i64,
    extra: f64,
) -> Option<Base> {
    let (mut dev_sum, mut dev_n, mut fin_sum, mut fin_n) = (0.0, 0_usize, 0.0, 0_usize);
    for c in charts {
        let mine: Vec<&Trade> = closed(c, keep).collect();
        let Some(risk) = median_risk(&mine) else {
            continue;
        };
        let entries: Vec<(i64, f64)> = baseline(
            &c.bars,
            side,
            risk,
            trend_only,
            Params::with_max_hold(max_hold),
        )
        .into_iter()
        .filter(|(k, _)| universe.has(&c.symbol, c.bars[k - 1].open_time_ms))
        .map(|(k, r)| (c.bars[k].open_time_ms, r - extra / risk))
        .collect();
        let mean_of = |dev: bool| {
            let xs: Vec<f64> = entries
                .iter()
                .filter(|(ms, _)| (*ms <= split_ms) == dev)
                .map(|(_, r)| *r)
                .collect();
            (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / count_f64(xs.len()))
        };
        let in_dev = mine
            .iter()
            .filter(|t| c.bars[t.entry_index].open_time_ms <= split_ms)
            .count();
        let in_fin = mine.len() - in_dev;
        if let Some(m) = mean_of(true) {
            dev_sum += m * count_f64(in_dev);
            dev_n += in_dev;
        }
        if let Some(m) = mean_of(false) {
            fin_sum += m * count_f64(in_fin);
            fin_n += in_fin;
        }
    }
    (dev_n > 0 && fin_n > 0).then(|| Base {
        dev: dev_sum / count_f64(dev_n),
        fin: fin_sum / count_f64(fin_n),
    })
}

/// The two periods of some rows: (dev mean, dev t, dev n, final mean, final n).
fn periods(rows: &[Row], split_ms: i64) -> (f64, f64, usize, f64, usize) {
    let dev: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 <= split_ms)
        .map(|r| r.1)
        .collect();
    let fin: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 > split_ms)
        .map(|r| r.1)
        .collect();
    let (dev_mean, dev_t) = r_mean_t(&dev);
    let (fin_mean, _) = r_mean_t(&fin);
    (dev_mean, dev_t, dev.len(), fin_mean, fin.len())
}

/// Wins with proof before the split, positive after it and above the base
/// in both periods.
fn passes(rows: &[Row], base: Option<Base>, split_ms: i64) -> bool {
    let (dev_mean, dev_t, dev_n, fin_mean, fin_n) = periods(rows, split_ms);
    dev_n >= 30
        && dev_t >= 2.0
        && fin_n >= 10
        && fin_mean > 0.0
        && base.is_some_and(|b| dev_mean > b.dev && fin_mean > b.fin)
}

/// Mean per calendar year, as "2020 +0,21R (34)".
fn by_year(rows: &[Row]) -> (String, usize, usize) {
    let mut years: BTreeMap<i32, Vec<f64>> = BTreeMap::new();
    for r in rows {
        if let Some(t) = DateTime::from_timestamp_millis(r.0) {
            years.entry(t.year()).or_default().push(r.1);
        }
    }
    let positive = years.values().filter(|v| r_mean_t(v).0 > 0.0).count();
    let text = years
        .iter()
        .map(|(y, v)| format!("{y} {} ({})", r_pt(r_mean_t(v).0), v.len()))
        .collect::<Vec<_>>()
        .join(", ");
    (text, positive, years.len())
}

/// How many coins traded and the share of trades of the busiest one.
fn spread(charts: &[&Chart], keep: Keep) -> (usize, String, f64) {
    let counts: Vec<(&str, usize)> = charts
        .iter()
        .map(|c| (c.symbol.as_str(), closed(c, keep).count()))
        .filter(|(_, n)| *n > 0)
        .collect();
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    let top = counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .copied()
        .unwrap_or(("", 0));
    let name = top.0.strip_suffix("USDT").unwrap_or(top.0).to_owned();
    (
        counts.len(),
        name,
        count_f64(top.1) / count_f64(total.max(1)) * 100.0,
    )
}

#[allow(clippy::too_many_arguments)] // the charts, the variant and the split
fn variant(
    title: &str,
    label: &str,
    charts: &[&Chart],
    universe: &Universe,
    keep: Keep,
    side: Side,
    trend_only: bool,
    max_hold: usize,
    split_ms: i64,
    good: &mut Vec<String>,
    close: &mut Vec<String>,
) {
    let all = rows(charts, keep, 0.0);
    let all_base = base(
        charts, universe, keep, side, trend_only, max_hold, split_ms, 0.0,
    );
    let (text, positive_and_above) = line(label, &all, split_ms, all_base);
    println!(" {text}");
    if !positive_and_above {
        return;
    }
    // Checks only for what is positive and above the base.
    let others: Vec<&Chart> = charts
        .iter()
        .copied()
        .filter(|c| !EARLIER.contains(&c.symbol.as_str()))
        .collect();
    let other_rows = rows(&others, keep, 0.0);
    let other_base = base(
        &others, universe, keep, side, trend_only, max_hold, split_ms, 0.0,
    );
    let (other_text, _) = line("sem BTC, ETH e SOL", &other_rows, split_ms, other_base);
    println!("      Conferência 1, {}", other_text.trim());
    let costly = rows(charts, keep, EXTRA_COST);
    let costly_base = base(
        charts, universe, keep, side, trend_only, max_hold, split_ms, EXTRA_COST,
    );
    let (c_dev, _, _, c_fin, _) = periods(&costly, split_ms);
    println!(
        "      Conferência 2, com o dobro de custo (0,4% por trade): {} até out/2024 e {} depois.",
        r_pt(c_dev),
        r_pt(c_fin)
    );
    let (years, positive_years, n_years) = by_year(&all);
    println!(
        "      Conferência 3, por ano: {years}. Positivo em {positive_years} de {n_years} anos."
    );
    let (coins, top, share) = spread(charts, keep);
    println!(
        "      Conferência 4: trades em {coins} moedas; a que mais teve trades ({top}) responde por {share:.0}% deles."
    );
    let checks = [
        passes(&all, all_base, split_ms),
        passes(&other_rows, other_base, split_ms) || {
            // Without the three big coins there are fewer trades: positive
            // and above the base in both periods is enough here.
            let (d, _, dn, f, fn_) = periods(&other_rows, split_ms);
            dn >= 30
                && fn_ >= 10
                && d > 0.0
                && f > 0.0
                && other_base.is_some_and(|b| d > b.dev && f > b.fin)
        },
        c_dev > 0.0 && c_fin > 0.0 && costly_base.is_some(),
        share <= 50.0,
    ];
    let name = format!("{title} · {label}");
    if checks.iter().all(|c| *c) {
        println!("      PASSOU EM TUDO.");
        good.push(format!("{name}: {}", text.trim()));
    } else {
        let failed: Vec<&str> = [
            "prova nos dois períodos contra a base",
            "sem BTC, ETH e SOL",
            "dobro de custo",
            "trades espalhados entre as moedas",
        ]
        .iter()
        .zip(checks)
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect();
        println!("      Não passou em: {}.", failed.join("; "));
        close.push(format!("{name} (faltou: {})", failed.join("; ")));
    }
}

fn report(
    title: &str,
    charts: &[&Chart],
    universe: &Universe,
    max_hold: usize,
    split_ms: i64,
    good: &mut Vec<String>,
    close: &mut Vec<String>,
) {
    println!();
    println!("{title}");
    for pattern in Pattern::ALL {
        println!("   {}", pattern_name(pattern));
        let variants: [(&str, Side, bool); 4] = [
            ("compras", Side::Long, false),
            ("vendas", Side::Short, false),
            (
                "compras a favor da tendência (médias 8 e 80)",
                Side::Long,
                true,
            ),
            ("vendas a favor da tendência", Side::Short, true),
        ];
        for (label, side, trend_only) in variants {
            let keep =
                |t: &Trade| t.pattern == pattern && t.side == side && (!trend_only || t.with_trend);
            variant(
                &format!("{title} · {}", pattern_name(pattern)),
                label,
                charts,
                universe,
                &keep,
                side,
                trend_only,
                max_hold,
                split_ms,
                good,
                close,
            );
        }
    }
}

/// The trade universe of `universe.csv`.
///
/// # Errors
///
/// Returns an error when the file is missing or unreadable.
pub(crate) fn load(data_dir: &Path) -> Result<Universe, String> {
    Ok(Universe {
        months: load_universe(data_dir)
            .map_err(|e| {
                format!(
                    "{e}\nThe universe file is built by `tp build-series` and `tp build-universe` after `tp download-binance`."
                )
            })?
            .into_iter()
            .map(|(m, members)| (m, members.into_iter().collect()))
            .collect(),
    })
}

/// Runs the study over the trade universe and prints it.
///
/// # Errors
///
/// Returns an error when the universe file is missing or unreadable.
pub fn cmd_crypto_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    let universe = load(data_dir)?;
    let symbols: BTreeSet<&String> = universe.months.values().flatten().collect();
    let (first, last) = (
        universe.months.keys().next().ok_or("empty universe")?,
        universe.months.keys().next_back().ok_or("empty universe")?,
    );
    println!(
        "Setups do Stormer nas criptos do universo de trade: as moedas mais negociadas contra USDT em cada mês, de {first} a {last} ({} moedas diferentes). O sinal de uma moeda só conta nos meses em que ela estava nesse grupo; moedas que foram grandes e morreram entram, e as vencedoras de hoje não são escolhidas olhando para trás.",
        symbols.len()
    );
    println!(
        "Mesmas regras do teste anterior: entrada no rompimento da máxima do candle de sinal (mínima, nas vendas) no candle seguinte, stop na mínima do padrão, alvo de 2 vezes o risco, custo de 0,2% por trade. 1R = o que se perde no stop."
    );
    println!(
        "Para passar: prova antes de nov/2024, positivo depois, acima da base nos dois períodos e, nas conferências, o mesmo sem BTC, ETH e SOL, positivo com o dobro de custo e com os trades espalhados entre as moedas (nenhuma com mais da metade)."
    );
    let mut charts: [Vec<Chart>; 2] = [Vec::new(), Vec::new()];
    let mut missing = Vec::new();
    for symbol in &symbols {
        let path = processed_dir(data_dir).join(format!("{symbol}.csv"));
        let Ok(candles) = read_series(&path) else {
            missing.push(symbol.as_str());
            continue;
        };
        let daily = daily_bars(&candles);
        for (slot, frame) in charts.iter_mut().zip(&FRAMES) {
            let bars = if frame.weekly {
                weekly(&daily)
            } else {
                daily.clone()
            };
            let (trades, _) = scan(&bars, Params::with_max_hold(frame.max_hold));
            let trades = trades
                .into_iter()
                .filter(|t| universe.has(symbol, bars[t.signal_index].open_time_ms))
                .collect();
            slot.push(Chart {
                symbol: (*symbol).clone(),
                bars,
                trades,
            });
        }
    }
    if !missing.is_empty() {
        println!("Sem histórico (ficaram de fora): {}.", missing.join(", "));
    }
    let mut good = Vec::new();
    let mut close = Vec::new();
    for (frame, set) in FRAMES.iter().zip(&charts) {
        let refs: Vec<&Chart> = set.iter().collect();
        report(
            &format!("Criptos do universo, {}", frame.name),
            &refs,
            &universe,
            frame.max_hold,
            split_ms,
            &mut good,
            &mut close,
        );
    }
    println!();
    println!("RESUMO: o que passou em tudo");
    if good.is_empty() {
        println!(" Nada. Os setups do Stormer continuam só em observação em cripto.");
    }
    for g in &good {
        println!(" {g}");
    }
    if !close.is_empty() {
        println!();
        println!("Positivos e acima da base, mas que falharam em alguma conferência:");
        for c in &close {
            println!(" {c}");
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn month_ms(y: i32, m: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn membership_follows_the_month_and_the_last_known_one() {
        let mut months = BTreeMap::new();
        months.insert(
            YearMonth::new(2021, 1).unwrap(),
            ["AAAUSDT".to_owned()].into(),
        );
        months.insert(
            YearMonth::new(2021, 2).unwrap(),
            ["BBBUSDT".to_owned()].into(),
        );
        let u = Universe { months };
        assert!(u.has("AAAUSDT", month_ms(2021, 1)));
        assert!(!u.has("AAAUSDT", month_ms(2021, 2)));
        // Before the first month: nobody; after the last one: the last month.
        assert!(!u.has("AAAUSDT", month_ms(2020, 12)));
        assert!(u.has("BBBUSDT", month_ms(2021, 6)));
    }

    #[test]
    fn extra_cost_is_charged_on_the_risk() {
        let t = Trade {
            pattern: Pattern::Trap,
            side: Side::Long,
            signal_index: 0,
            entry_index: 1,
            exit_index: 2,
            entry: 100.0,
            stop: 95.0,
            target: 110.0,
            exit: 110.0,
            r: 2.0,
            how: Exit::Target,
            with_trend: true,
        };
        // 0.2% of 100 is 0.2, a risk of 5: 0.04R.
        assert!((cost_r(&t, 0.002) - 0.04).abs() < 1e-12);
    }

    #[test]
    fn years_are_counted_and_the_busiest_coin_is_found() {
        let rows: Vec<Row> = vec![
            (month_ms(2021, 3), 1.0, Exit::Target),
            (month_ms(2021, 5), -1.0, Exit::Stop),
            (month_ms(2021, 6), 2.0, Exit::Target),
            (month_ms(2022, 1), -1.0, Exit::Stop),
        ];
        let (text, positive, n) = by_year(&rows);
        assert_eq!((positive, n), (1, 2));
        assert!(text.starts_with("2021 +0,67R (3)"), "{text}");
    }
}
