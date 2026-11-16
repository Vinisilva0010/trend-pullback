//! `tp sp200-study`: buying when the price comes back to its 200-day
//! average while the trend is up, the way it is usually said about the
//! S&P 500 ("when it touches the 200-day average, buy").
//!
//! Three rules, each known at the close of the day, buying at that close:
//! - holds: the 200-day average is rising, the price closed above it on each
//!   of the 20 days before, today's low comes to 1% of it or below and the
//!   close stays above it;
//! - breaks: the same situation, but today closes below the average;
//! - reclaims: the average is rising, the 5 closes before were below it and
//!   today closes above it.
//!
//! Part 1 is the S&P 500 index itself since 1950 (Yahoo `^GSPC`); before
//! 1962 the index has only closes, so the touch there is by the close.
//! Part 2 is every US stock and ETF of the trade universe on its own
//! average, ten years each.
//!
//! Every buy is measured 5, 20, 60 and 120 trading days later, with 0.1% of
//! costs, and compared with buying the same asset on any day of the same
//! period (the median). One case per asset every 20 trading days.

use std::path::Path;

use chrono::{DateTime, NaiveDate};
use tp_core::intraday::Bar;
use tp_data::yahoo::fetch_daily_max;

use super::{CliResult, count_f64, held_out_start, markets};

const AVERAGE: usize = 200;
const HORIZONS: [usize; 4] = [5, 20, 60, 120];
const COOLDOWN: usize = 20;
const COST: f64 = 0.001;
/// How close the low has to come to the average to count as a touch.
const TOUCH: f64 = 0.01;

/// The 200-day simple average of the closes ending at each day.
fn averages(bars: &[Bar]) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    let mut sum = 0.0;
    for (i, b) in bars.iter().enumerate() {
        sum += b.close;
        if i >= AVERAGE {
            sum -= bars[i - AVERAGE].close;
        }
        if i + 1 >= AVERAGE {
            out[i] = Some(sum / count_f64(AVERAGE));
        }
    }
    out
}

/// The rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Holds,
    Breaks,
    Reclaims,
}

impl Rule {
    fn label(self) -> &'static str {
        match self {
            Self::Holds => {
                "Toca e segura: média de 200 dias subindo, preço fechou acima dela nos 20 dias antes, a mínima do dia chega a 1% da média ou abaixo e o fechamento fica acima; compra no fechamento"
            }
            Self::Breaks => {
                "Rompe para baixo: a mesma situação, mas o dia fecha abaixo da média; compra no fechamento"
            }
            Self::Reclaims => {
                "Volta para cima: média subindo, os 5 fechamentos antes abaixo dela e o dia fecha acima; compra no fechamento"
            }
        }
    }

    /// Whether day `t` completes the rule.
    fn fires(self, bars: &[Bar], ma: &[Option<f64>], t: usize) -> bool {
        let (Some(Some(now)), Some(Some(before))) =
            (ma.get(t).copied(), t.checked_sub(20).map(|i| ma[i]))
        else {
            return false;
        };
        let rising = now > before;
        let side = |k: usize, above: bool| {
            (t - k..t).all(|i| ma[i].is_some_and(|m| (bars[i].close > m) == above))
        };
        let b = &bars[t];
        match self {
            Self::Holds => {
                rising && side(20, true) && b.low <= now * (1.0 + TOUCH) && b.close >= now
            }
            Self::Breaks => rising && side(20, true) && b.close < now,
            Self::Reclaims => rising && side(5, false) && b.close > now,
        }
    }
}

/// Return in percent from the close of `t` to the close `h` days later.
fn forward(bars: &[Bar], t: usize, h: usize) -> Option<f64> {
    let entry = bars.get(t)?.close;
    let exit = bars.get(t + h)?.close;
    (entry > 0.0).then(|| (exit / entry - 1.0 - COST) * 100.0)
}

/// The middle value (median), or 0 for none.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Median return of buying any day from `first` on, per horizon, `(dev, final)`.
fn base(bars: &[Bar], first: usize, split_ms: i64) -> Vec<(f64, f64)> {
    HORIZONS
        .iter()
        .map(|h| {
            let (mut dev, mut fin) = (Vec::new(), Vec::new());
            for i in first..bars.len() {
                if let Some(r) = forward(bars, i, *h) {
                    if bars[i].open_time_ms <= split_ms {
                        dev.push(r);
                    } else {
                        fin.push(r);
                    }
                }
            }
            (median(&dev), median(&fin))
        })
        .collect()
}

/// One buy.
struct Case {
    asset: String,
    ms: i64,
    returns: Vec<Option<f64>>,
    excess: Vec<Option<f64>>,
    /// Lowest low of the next 20 days below the purchase, in percent (≤ 0).
    worst: f64,
    /// A close more than 5% below the average within 60 days.
    fell_through: bool,
}

/// The cases of a rule on one asset, from day `first` on.
fn cases(asset: &str, bars: &[Bar], first: usize, rule: Rule, split_ms: i64) -> Vec<Case> {
    let ma = averages(bars);
    let base = base(bars, first, split_ms);
    let mut out = Vec::new();
    let start = first.max(AVERAGE + 20);
    let mut next_free = start;
    for t in start..bars.len() {
        if t < next_free || !rule.fires(bars, &ma, t) {
            continue;
        }
        next_free = t + COOLDOWN;
        let ms = bars[t].open_time_ms;
        let returns: Vec<Option<f64>> = HORIZONS.iter().map(|h| forward(bars, t, *h)).collect();
        let excess = returns
            .iter()
            .zip(&base)
            .map(|(r, (d, f))| r.map(|r| r - if ms <= split_ms { d } else { f }))
            .collect();
        let after = &bars[t + 1..(t + 21).min(bars.len())];
        let worst = after
            .iter()
            .map(|b| (b.low / bars[t].close - 1.0) * 100.0)
            .fold(0.0, f64::min);
        let fell_through = (t + 1..(t + 61).min(bars.len()))
            .any(|i| ma[i].is_some_and(|m| bars[i].close < m * 0.95));
        out.push(Case {
            asset: asset.to_owned(),
            ms,
            returns,
            excess,
            worst,
            fell_through,
        });
    }
    out
}

fn pct(v: f64) -> String {
    format!("{v:+.1}%").replace('.', ",")
}

fn fell(worst: f64) -> String {
    format!("{:.1}%", (-worst).max(0.0)).replace('.', ",")
}

fn date(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).map_or(String::new(), |d| d.format("%d/%m/%Y").to_string())
}

fn horizon_pt(h: usize) -> &'static str {
    match h {
        5 => "1 semana (5 pregões)",
        20 => "1 mês (20 pregões)",
        60 => "3 meses (60 pregões)",
        _ => "6 meses (120 pregões)",
    }
}

/// Whether a set of excess returns proves the rule: the development period
/// with 30 or more cases and a sign test of 2 or more, the final period
/// with 10 or more cases and most of them better than the typical day.
fn proof(dev: (usize, usize), fin: (usize, usize)) -> bool {
    let (dev_k, dev_n) = dev;
    let (fin_k, fin_n) = fin;
    let z = if dev_n > 0 {
        (count_f64(dev_k) - count_f64(dev_n) / 2.0) / (count_f64(dev_n) / 4.0).sqrt()
    } else {
        0.0
    };
    dev_n >= 30 && z >= 2.0 && fin_n >= 10 && fin_k * 2 > fin_n
}

/// Prints a rule. Returns (passed with proof in both periods, proven in the
/// development period at 1 and 3 months).
fn report(label: &str, cases: &[Case], split_ms: i64, list_final: bool) -> (bool, bool) {
    println!();
    println!(" {label}");
    if cases.is_empty() {
        println!("   Nenhum caso.");
        return (false, false);
    }
    let mut worst: Vec<f64> = cases.iter().map(|c| c.worst).collect();
    worst.sort_by(f64::total_cmp);
    let through = cases.iter().filter(|c| c.fell_through).count();
    let late = cases.iter().filter(|c| c.ms > split_ms).count();
    println!(
        "   {} casos ({} até out/2024, {late} depois). Nos 20 pregões depois da compra o preço ainda caiu tipicamente {} abaixo da compra; em 1 de cada 10 casos, mais de {}. Em {:.0}% dos casos o preço fechou mais de 5% abaixo da média nos 3 meses seguintes (a média não segurou).",
        cases.len(),
        cases.len() - late,
        fell(worst[worst.len() / 2]),
        fell(worst[worst.len() / 10]),
        count_f64(through) / count_f64(cases.len()) * 100.0,
    );
    let mut passed = true;
    let mut dev_proven = true;
    for (k, h) in HORIZONS.iter().enumerate() {
        let all: Vec<f64> = cases.iter().filter_map(|c| c.returns[k]).collect();
        if all.is_empty() {
            continue;
        }
        let beat = |late: bool| -> (usize, usize) {
            let v: Vec<f64> = cases
                .iter()
                .filter(|c| (c.ms > split_ms) == late)
                .filter_map(|c| c.excess[k])
                .collect();
            (v.iter().filter(|x| **x > 0.0).count(), v.len())
        };
        let (dev, fin) = (beat(false), beat(true));
        let share = |(k, n): (usize, usize)| count_f64(k) / count_f64(n.max(1)) * 100.0;
        let proven = proof(dev, fin);
        let dev_only = proof(dev, (10, 10));
        if *h == 20 || *h == 60 {
            passed &= proven;
            dev_proven &= dev_only;
        }
        let wins = all.iter().filter(|r| **r > 0.0).count();
        println!(
            "   {}: caso típico {}, subiu em {:.0}% dos casos. Ganhou do dia típico em {:.0}% dos casos até out/2024 ({} casos) e em {:.0}% depois ({} casos).{}",
            horizon_pt(*h),
            pct(median(&all)),
            count_f64(wins) / count_f64(all.len()) * 100.0,
            share(dev),
            dev.1,
            share(fin),
            fin.1,
            if proven {
                " GANHOU COM PROVA."
            } else if dev_only {
                " Ganhou com prova até out/2024."
            } else {
                ""
            }
        );
    }
    if list_final {
        let recent: Vec<&Case> = cases.iter().filter(|c| c.ms > split_ms).collect();
        let shown: Vec<&Case> = if recent.is_empty() {
            cases.iter().rev().take(5).rev().collect()
        } else {
            recent
        };
        println!(
            "   {}:",
            if late == 0 {
                "Nenhum caso depois de out/2024; os 5 últimos"
            } else {
                "Os casos depois de out/2024"
            }
        );
        for c in shown {
            let r = |k: usize| c.returns[k].map_or("ainda sem dado".to_owned(), pct);
            println!(
                "     {}: 1 mês {}, 3 meses {}; nos 20 pregões seguintes caiu até {} abaixo da compra{}",
                date(c.ms),
                r(1),
                r(2),
                fell(c.worst),
                if c.fell_through {
                    "; depois fechou mais de 5% abaixo da média"
                } else {
                    ""
                }
            );
        }
    }
    let verdict = if passed {
        "PASSOU em 1 e 3 meses, nos dois períodos"
    } else if dev_proven {
        "ganhou com prova até out/2024 em 1 e 3 meses, mas o período depois não tem casos suficientes ou não confirmou"
    } else {
        "não passou (precisa ganhar do dia típico na maioria dos casos, com prova, em 1 e 3 meses)"
    };
    println!("   {verdict}.");
    (passed, dev_proven)
}

/// Runs the study and prints it.
///
/// # Errors
///
/// Returns an error when the S&P 500 history cannot be fetched.
pub async fn cmd_sp200_study(_data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    let start_ms = NaiveDate::from_ymd_opt(1950, 1, 1)
        .ok_or("invalid date")?
        .and_hms_opt(0, 0, 0)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Compra quando o preço volta para a média de 200 dias com a tendência de alta. Cada compra é no fechamento do dia do sinal, medida 1 semana, 1, 3 e 6 meses depois, já com 0,1% de custo, e comparada com o dia típico do mesmo ativo no mesmo período (a mediana de comprar em qualquer dia). Um caso por ativo a cada 20 pregões."
    );
    let now = markets::now_ms()?;
    let index = fetch_daily_max("^GSPC", now).await?;
    let first = index
        .iter()
        .position(|b| b.open_time_ms >= start_ms)
        .unwrap_or(0);
    println!(
        "S&P 500 (^GSPC, Yahoo): {} pregões, de {} a {}. Antes de 1962 o índice só tem o fechamento, então ali o toque é medido pelo fechamento.",
        index.len() - first,
        index
            .get(first)
            .map_or(String::new(), |b| date(b.open_time_ms)),
        index.last().map_or(String::new(), |b| date(b.open_time_ms)),
    );

    println!();
    println!("PARTE 1: o próprio S&P 500, desde 1950.");
    let mut summary = Vec::new();
    for rule in [Rule::Holds, Rule::Breaks, Rule::Reclaims] {
        let c = cases("S&P 500", &index, first, rule, split_ms);
        let (passed, dev) = report(rule.label(), &c, split_ms, true);
        if passed || dev {
            summary.push(format!(
                "S&P 500, {}: {}",
                rule.label().split(':').next().unwrap_or_default(),
                if passed {
                    "passou nos dois períodos"
                } else {
                    "ganhou com prova até out/2024; depois, poucos casos"
                }
            ));
        }
    }

    println!();
    println!(
        "PARTE 2: cada ação e ETF dos EUA da lista de trade, na sua própria média de 200 dias, 10 anos."
    );
    let items: Vec<markets::Item> = markets::read_list(markets::TRADE_UNIVERSE)?
        .into_iter()
        .filter(|i| i.group == "EUA")
        .collect();
    let loaded = markets::load_all(&items, now).await;
    for rule in [Rule::Holds, Rule::Breaks, Rule::Reclaims] {
        let mut all = Vec::new();
        for l in &loaded {
            all.extend(cases(&l.item.symbol, &l.daily, 0, rule, split_ms));
        }
        let mut days: Vec<i64> = all.iter().map(|c| c.ms / 86_400_000).collect();
        days.sort_unstable();
        days.dedup();
        let assets: std::collections::BTreeSet<&str> =
            all.iter().map(|c| c.asset.as_str()).collect();
        let (passed, _) = report(rule.label(), &all, split_ms, false);
        println!(
            "   Os casos são de {} ativos e caíram em {} dias diferentes; ações no mesmo dia costumam ser o mesmo movimento do mercado.",
            assets.len(),
            days.len()
        );
        if passed {
            summary.push(format!(
                "Ações da lista, {}: passou nos dois períodos",
                rule.label().split(':').next().unwrap_or_default()
            ));
        }
    }

    println!();
    println!("RESUMO");
    if summary.is_empty() {
        println!(" Nenhuma regra ganhou do dia típico com prova.");
    }
    for s in summary {
        println!(" {s}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn bar(day: usize, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: i64::try_from(day).unwrap() * DAY,
            open: close,
            high: close.max(low),
            low,
            close,
            volume: 1.0,
        }
    }

    /// 260 days climbing 0.1 a day from 100, then `tail`.
    fn climb(tail: &[(f64, f64)]) -> Vec<Bar> {
        let mut bars: Vec<Bar> = (0..260)
            .map(|d| {
                let c = 100.0 + 0.1 * count_f64(d);
                bar(d, c - 0.2, c)
            })
            .collect();
        for (low, close) in tail {
            bars.push(bar(bars.len(), *low, *close));
        }
        bars
    }

    #[test]
    fn the_average_is_the_mean_of_the_last_200_closes() {
        let bars = climb(&[]);
        let ma = averages(&bars);
        assert_eq!(ma[198], None);
        // Closes 100.0 .. 119.9: mean 109.95.
        assert!((ma[199].unwrap() - 109.95).abs() < 1e-9);
        assert!((ma[259].unwrap() - 115.95).abs() < 1e-9);
    }

    #[test]
    fn the_three_rules_are_recognized() {
        // The average at day 260 is about 116.05; the low touches it and the close holds.
        let held = climb(&[(116.3, 117.0)]);
        let ma = averages(&held);
        assert!(Rule::Holds.fires(&held, &ma, 260));
        assert!(!Rule::Breaks.fires(&held, &ma, 260));
        let broke = climb(&[(114.0, 115.0)]);
        let ma = averages(&broke);
        assert!(Rule::Breaks.fires(&broke, &ma, 260));
        assert!(!Rule::Holds.fires(&broke, &ma, 260));
        // A low far from the average is not a touch.
        let far = climb(&[(124.0, 125.0)]);
        assert!(!Rule::Holds.fires(&far, &averages(&far), 260));
        // Five closes below, then one above.
        let back = climb(&[
            (114.0, 115.0),
            (114.0, 115.0),
            (114.0, 115.0),
            (114.0, 115.0),
            (114.0, 115.0),
            (116.0, 118.0),
        ]);
        let ma = averages(&back);
        assert!(Rule::Reclaims.fires(&back, &ma, 265));
        assert!(!Rule::Reclaims.fires(&back, &ma, 264));
    }

    #[test]
    fn cases_measure_the_returns_and_keep_one_per_20_days() {
        let mut tail = vec![(116.3, 117.0)];
        tail.extend((0..30).map(|i| {
            let c = 117.0 + count_f64(i);
            (c - 0.5, c)
        }));
        let bars = climb(&tail);
        let c = cases("X", &bars, 0, Rule::Holds, i64::MAX);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].ms, 260 * DAY);
        // 5 days later the close is 121 (117 + 4).
        let r5 = c[0].returns[0].unwrap();
        assert!((r5 - ((121.0 / 117.0 - 1.0 - COST) * 100.0)).abs() < 1e-9);
        assert_eq!(c[0].returns[2], None);
        assert!(!c[0].fell_through);
        assert!(c[0].worst <= 0.0);
    }

    #[test]
    fn the_proof_needs_both_periods() {
        assert!(proof((25, 36), (7, 12)));
        assert!(!proof((25, 36), (5, 10)));
        assert!(!proof((25, 36), (6, 9)));
        assert!(!proof((20, 36), (9, 10)));
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    }
}
